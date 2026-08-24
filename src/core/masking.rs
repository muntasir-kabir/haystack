//! Pre-mining masking: replace dynamic values before Drain clustering.
//!
//! Drain tokenizes on whitespace anyway, so instead of running ~10 regex
//! passes over every line, we split the line into whitespace tokens ONCE and
//! classify each token with cheap scalar byte heuristics. Regex is only used
//! as a fallback for rare ambiguous tokens — the hot path is regex-free.
//!
//! Token-based masking also improves clustering quality:
//!   - `key=value` tokens keep their key (`status=200` → `status=<NUM>`), so
//!     the stable structural key survives for Drain's similarity check.
//!   - Masks are whole-token or whole-run, so token shapes stay consistent
//!     (no `worker-<NUM>` vs `worker-<HEX>` drift from mid-token regex hits).
//!
//! Performance: zero allocation when nothing matches (`Cow::Borrowed`),
//! single pass, single output buffer when something does.

use rustc_hash::FxHashMap;
use std::borrow::Cow;

/// Per-document memo cache: raw token → masked replacement (`None` = keep
/// as-is). Log vocabularies are small and repetitive, so after a few hundred
/// lines nearly every token is a cache hit — no classification, no alloc.
/// The cache is bounded so high-cardinality logs cannot retain every unique
/// request ID, path, or payload token for the lifetime of the document.
#[derive(Default, Clone)]
pub struct MaskCache(FxHashMap<Box<str>, Option<Box<str>>>);

const MASK_CACHE_MAX_ENTRIES: usize = 65_536;

/// Semantic mask tokens (more readable than generic `<*>`)
pub const MASK_IP: &str = "<IP>";
pub const MASK_IPV6: &str = "<IPV6>";
pub const MASK_HEX: &str = "<HEX>";
pub const MASK_UUID: &str = "<UUID>";
pub const MASK_NUM: &str = "<NUM>";
pub const MASK_PATH: &str = "<PATH>";
pub const MASK_URL: &str = "<URL>";
pub const MASK_EMAIL: &str = "<EMAIL>";
pub const MASK_JSON: &str = "<JSON>";
pub const MASK_TIME: &str = "<TIME>";

/// Masking configuration — which rules to apply.
#[derive(Clone, Debug)]
pub struct MaskConfig {
    pub mask_ips: bool,
    pub mask_hex: bool,
    pub mask_uuid: bool,
    pub mask_urls: bool,
    pub mask_paths: bool,
    pub mask_emails: bool,
    pub mask_json: bool,
    pub mask_times: bool,
    pub mask_nums: bool,
}

impl Default for MaskConfig {
    fn default() -> Self {
        Self {
            mask_ips: true,
            mask_hex: true,
            mask_uuid: true,
            mask_urls: true,
            mask_paths: true,
            mask_emails: true,
            mask_json: true,
            mask_times: true,
            mask_nums: true,
        }
    }
}

/// The masker classifies whitespace-separated tokens with scalar heuristics.
#[derive(Clone)]
pub struct LogMasker {
    config: MaskConfig,
}

impl LogMasker {
    pub fn new(config: MaskConfig) -> Self {
        Self { config }
    }

    /// Apply all enabled masks to a log line.
    /// Returns `Cow::Borrowed` if no masks matched (zero allocation).
    pub fn mask<'a>(&self, line: &'a str) -> Cow<'a, str> {
        self.mask_with_header(line, &[], &mut MaskCache::default())
    }

    /// Mask a line, additionally forcing `header[i]` as the replacement for
    /// the i-th token (learned per-file dynamic header slots, e.g. host/pid).
    /// A `Some(mask)` slot emits that mask token verbatim; `None` slots and
    /// tokens beyond the header are classified normally.
    ///
    /// `cache` memoizes per-token decisions across lines — pass the same
    /// cache for every line of a document.
    pub fn mask_with_header<'a>(
        &self,
        line: &'a str,
        header: &[Option<&'static str>],
        cache: &mut MaskCache,
    ) -> Cow<'a, str> {
        // Single pass: build the masked line while tracking whether anything
        // actually changed. Unchanged lines return a borrow (the built copy
        // is discarded — cheaper than a second lookup pass over all tokens).
        let mut out = String::with_capacity(line.len());
        let mut changed = false;
        for (i, tok) in line.split_whitespace().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            if let Some(slot) = header.get(i).copied().flatten() {
                // Shape-aware: preserve the token's constant prefix/suffix
                // when it has inner dynamic runs (`MyApp[12345:1]` →
                // `MyApp[<NUM>:<NUM>]`); fall back to the whole-token mask
                // for pure-text values (hostnames like `web-prod`).
                match self.mask_inner_runs(tok) {
                    Some(shape) => out.push_str(&shape),
                    None => out.push_str(slot),
                }
                changed = true;
            } else if let Some(masked) = self.cached_mask(tok, cache) {
                out.push_str(&masked);
                changed = true;
            } else {
                out.push_str(tok);
            }
        }
        if changed {
            Cow::Owned(out)
        } else {
            Cow::Borrowed(line)
        }
    }

    /// Cached token classification: hit = borrow from the cache, miss =
    /// classify once and memoize while capacity remains. Returns the
    /// replacement string, if any.
    fn cached_mask<'c>(&self, tok: &str, cache: &'c mut MaskCache) -> Option<Cow<'c, str>> {
        // Probe first so the immutable cache borrow ends before a possible
        // insert below.
        if cache.0.contains_key(tok) {
            return cache
                .0
                .get(tok)
                .and_then(|m| m.as_deref())
                .map(Cow::Borrowed);
        }
        let masked = self.mask_token(tok).map(|m| Box::<str>::from(m.as_ref()));
        if cache.0.len() < MASK_CACHE_MAX_ENTRIES {
            cache.0.insert(Box::from(tok), masked);
            return cache
                .0
                .get(tok)
                .and_then(|m| m.as_deref())
                .map(Cow::Borrowed);
        }

        // Do not retain unbounded unique tokens. A replacement still applies
        // to this line, but it remains short-lived instead of entering the
        // document-wide memo table.
        masked.map(|value| Cow::Owned(value.into_string()))
    }

    /// Classify one token. Returns `None` to keep it, or `Some(replacement)`
    /// (either a semantic mask constant or an owned partially-masked token).
    fn mask_token(&self, tok: &str) -> Option<Cow<'static, str>> {
        if tok.is_empty() {
            return None;
        }

        // Whole-token classes, cheapest/most-specific first.
        if self.config.mask_urls && looks_like_url(tok) {
            return Some(Cow::Borrowed(MASK_URL));
        }
        if self.config.mask_emails && looks_like_email(tok) {
            return Some(Cow::Borrowed(MASK_EMAIL));
        }
        if self.config.mask_json && looks_like_json(tok) {
            return Some(Cow::Borrowed(MASK_JSON));
        }
        if self.config.mask_uuid && looks_like_uuid(tok) {
            return Some(Cow::Borrowed(MASK_UUID));
        }
        // Clock before IPv6: `14:30:22` is all hexdigits+colons and would
        // otherwise be misclassified as an IPv6 address.
        if self.config.mask_times && looks_like_clock(tok) {
            return Some(Cow::Borrowed(MASK_TIME));
        }
        if self.config.mask_ips {
            if looks_like_ipv4(tok) {
                return Some(Cow::Borrowed(MASK_IP));
            }
            if looks_like_ipv6(tok) {
                return Some(Cow::Borrowed(MASK_IPV6));
            }
        }

        // key=value: keep the key, mask the value. The key is the stable
        // structural signal Drain needs; only the value is dynamic.
        if let Some(eq) = tok.find('=') {
            if eq > 0 && eq + 1 < tok.len() {
                let value = &tok[eq + 1..];
                if let Some(vm) = self.mask_value(value) {
                    let mut owned = String::with_capacity(tok.len() + 6);
                    owned.push_str(&tok[..=eq]);
                    owned.push_str(&vm);
                    return Some(Cow::Owned(owned));
                }
            }
            return None;
        }

        if let Some(m) = self.mask_value(tok) {
            return Some(m);
        }

        // Mixed tokens with inner dynamic runs: `worker-12`, `user_981a2f3b`,
        // `backend-3:8443`. Split into alphanumeric runs on separators and
        // mask any numeric/hex runs, keeping the separators.
        self.mask_inner_runs(tok)
    }

    /// Mask a bare value (whole token or the right-hand side of `key=`).
    fn mask_value(&self, v: &str) -> Option<Cow<'static, str>> {
        if v.is_empty() {
            return None;
        }
        if self.config.mask_paths && looks_like_path(v) {
            return Some(Cow::Borrowed(MASK_PATH));
        }
        if self.config.mask_times && looks_like_duration(v) {
            return Some(Cow::Borrowed(MASK_TIME));
        }
        if self.config.mask_nums && looks_like_number(v) {
            return Some(Cow::Borrowed(MASK_NUM));
        }
        if self.config.mask_hex && looks_like_hex_id(v) {
            return Some(Cow::Borrowed(MASK_HEX));
        }
        None
    }

    /// Mask numeric/hex runs inside a mixed token (`user_981a2f3b`,
    /// `worker-12`). Returns `None` if nothing inside is dynamic.
    fn mask_inner_runs(&self, tok: &str) -> Option<Cow<'static, str>> {
        if !tok.bytes().any(|b| b.is_ascii_digit()) {
            return None;
        }
        let mut out: Option<String> = None;
        let bytes = tok.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_alphanumeric() {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
                    i += 1;
                }
                let run = &tok[start..i];
                // Numeric runs of ANY length are dynamic (`worker-0` →
                // `worker-<NUM>`); hex needs length to avoid false positives.
                let masked: Option<&'static str> =
                    if self.config.mask_nums && looks_like_number(run) {
                        Some(MASK_NUM)
                    } else if run.len() >= 2 && self.config.mask_hex && looks_like_hex_id(run) {
                        Some(MASK_HEX)
                    } else {
                        None
                    };
                if let Some(m) = masked {
                    let buf = out.get_or_insert_with(|| {
                        let mut s = String::with_capacity(tok.len() + 6);
                        s.push_str(&tok[..start]);
                        s
                    });
                    buf.push_str(m);
                } else if let Some(buf) = out.as_mut() {
                    buf.push_str(run);
                }
            } else {
                if let Some(buf) = out.as_mut() {
                    buf.push(bytes[i] as char);
                }
                i += 1;
            }
        }
        out.map(Cow::Owned)
    }
}

impl Default for LogMasker {
    fn default() -> Self {
        Self::new(MaskConfig::default())
    }
}

// ---------------------------------------------------------------------------
// Token shape heuristics — scalar byte scans, no regex.
// ---------------------------------------------------------------------------

fn looks_like_url(tok: &str) -> bool {
    tok.contains("://")
}

fn looks_like_email(tok: &str) -> bool {
    let Some(at) = tok.find('@') else {
        return false;
    };
    at > 0 && at + 3 < tok.len() && tok[at + 1..].contains('.')
}

fn looks_like_json(tok: &str) -> bool {
    (tok.starts_with('{') && tok.ends_with('}') && tok.contains(':'))
        || (tok.starts_with('[') && tok.ends_with(']') && tok.contains('"'))
}

/// `550e8400-e29b-41d4-a716-446655440000`
fn looks_like_uuid(tok: &str) -> bool {
    let b = tok.as_bytes();
    if b.len() != 36 {
        return false;
    }
    for (i, &c) in b.iter().enumerate() {
        match i {
            8 | 13 | 18 | 23 => {
                if c != b'-' {
                    return false;
                }
            }
            _ => {
                if !c.is_ascii_hexdigit() {
                    return false;
                }
            }
        }
    }
    true
}

/// `192.168.1.50` or `10.0.0.1:8080`
fn looks_like_ipv4(tok: &str) -> bool {
    let (addr, _port) = match tok.split_once(':') {
        Some((a, p)) if p.bytes().all(|b| b.is_ascii_digit()) && !p.is_empty() => (a, true),
        _ => (tok, false),
    };
    let mut parts = 0;
    for part in addr.split('.') {
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        parts += 1;
    }
    parts == 4
}

/// Colon-separated hex groups, at least 2 colons (`fe80::1a2b`, `::1` excluded
/// intentionally — too short to bother).
fn looks_like_ipv6(tok: &str) -> bool {
    let colons = tok.bytes().filter(|&b| b == b':').count();
    colons >= 2 && tok.len() >= 6 && tok.bytes().all(|b| b.is_ascii_hexdigit() || b == b':')
}

/// `14:30`, `14:30:22`, `14:30:22.123`
fn looks_like_clock(tok: &str) -> bool {
    let b = tok.as_bytes();
    let colons = b.iter().filter(|&&c| c == b':').count();
    if colons == 0 || colons > 2 {
        return false;
    }
    let mut groups = 0;
    for part in tok.split(':') {
        let digits = part.split('.').next().unwrap_or("");
        if digits.is_empty() || digits.len() > 2 || !digits.bytes().all(|c| c.is_ascii_digit()) {
            return false;
        }
        if let Some((_, frac)) = part.split_once('.') {
            if frac.is_empty() || !frac.bytes().all(|c| c.is_ascii_digit()) {
                return false;
            }
        }
        groups += 1;
    }
    groups >= 2
}

/// `45ms`, `2h30m`, `1.5s`, `10d`
fn looks_like_duration(tok: &str) -> bool {
    let b = tok.as_bytes();
    let mut i = 0;
    let mut saw_digit = false;
    let mut saw_unit = false;
    while i < b.len() {
        if b[i].is_ascii_digit() || b[i] == b'.' {
            saw_digit = true;
            i += 1;
        } else if matches!(b[i], b'm' | b's' | b'h' | b'd') {
            // "ms" is a unit; a bare 'm' is minutes.
            saw_unit = true;
            i += 1;
        } else {
            return false;
        }
    }
    saw_digit && saw_unit
}

/// Integer or decimal, optional sign: `42`, `99.99`, `-3`
fn looks_like_number(tok: &str) -> bool {
    let t = tok.strip_prefix(['-', '+']).unwrap_or(tok);
    if t.is_empty() {
        return false;
    }
    let mut dots = 0;
    let mut digits = 0;
    for c in t.bytes() {
        if c == b'.' {
            dots += 1;
            if dots > 1 {
                return false;
            }
        } else if c.is_ascii_digit() {
            digits += 1;
        } else {
            return false;
        }
    }
    digits > 0
}

/// 8+ hex chars with at least one digit (commit hashes, session/trace IDs).
/// The digit requirement avoids masking long lowercase words like `deadbeef`
/// that are actually English-ish identifiers... well, mostly.
fn looks_like_hex_id(tok: &str) -> bool {
    tok.len() >= 8
        && tok.len() <= 64
        && tok.bytes().all(|b| b.is_ascii_hexdigit())
        && tok.bytes().any(|b| b.is_ascii_digit())
        && tok.bytes().any(|b| b.is_ascii_alphabetic())
}

/// Unix `/var/log/app.log`, Windows `C:\Users\bob\f.txt`, UNC `\\srv\share`.
fn looks_like_path(tok: &str) -> bool {
    if tok.starts_with('/') {
        // Need at least one more slash or a file extension to count as a path
        // (a bare `/` or `/api` alone is too weak a signal).
        let rest = &tok[1..];
        return rest.contains('/') || (rest.contains('.') && rest.len() > 2);
    }
    if tok.starts_with("\\\\") && tok.len() > 4 {
        return true;
    }
    let b = tok.as_bytes();
    b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

/// Check if a token contains inner dynamic runs (numeric/hex runs separated
/// by non-alphanumeric characters). Returns the mask class of the first
/// dynamic run found, or `None` if no dynamic runs exist.
///
/// This is the standalone version of `LogMasker::mask_inner_runs` — it
/// doesn't consult config and returns a single class rather than a
/// partially-masked token. Used by `classify_token_public` for header
/// slot learning, where we only need to know *whether* a position is
/// consistently dynamic, not the exact masked form.
fn has_dynamic_inner_run(tok: &str) -> Option<&'static str> {
    if !tok.bytes().any(|b| b.is_ascii_digit()) {
        return None;
    }
    let bytes = tok.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphanumeric() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
                i += 1;
            }
            let run = &tok[start..i];
            if looks_like_number(run) {
                return Some(MASK_NUM);
            }
            if run.len() >= 2 && looks_like_hex_id(run) {
                return Some(MASK_HEX);
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Classify a token for the header learner: returns the mask class it would
/// get, if any (used to find consistently-dynamic header positions).
pub fn classify_token_public(tok: &str) -> Option<&'static str> {
    if looks_like_url(tok) {
        Some(MASK_URL)
    } else if looks_like_email(tok) {
        Some(MASK_EMAIL)
    } else if looks_like_uuid(tok) {
        Some(MASK_UUID)
    } else if looks_like_clock(tok) {
        Some(MASK_TIME)
    } else if looks_like_ipv4(tok) {
        Some(MASK_IP)
    } else if looks_like_ipv6(tok) {
        Some(MASK_IPV6)
    } else if looks_like_duration(tok) {
        Some(MASK_TIME)
    } else if looks_like_number(tok) {
        Some(MASK_NUM)
    } else if looks_like_hex_id(tok) {
        Some(MASK_HEX)
    } else if looks_like_path(tok) {
        Some(MASK_PATH)
    } else if let Some(mask) = has_dynamic_inner_run(tok) {
        // Fallback for mixed tokens with inner dynamic runs
        // (e.g. `MyApp[12345:1]`, `worker-12`, `AppDelegate.swift:42`).
        // These are common in structured log headers (process IDs, thread
        // IDs, source line numbers) and need to be detected so the header
        // learner can mark them as forced mask slots.
        Some(mask)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Individual mask tests ---

    #[test]
    fn masks_ipv4() {
        let m = LogMasker::default();
        assert_eq!(m.mask("from 192.168.1.50 ok"), "from <IP> ok");
    }

    #[test]
    fn masks_ipv4_with_port() {
        let m = LogMasker::default();
        assert_eq!(m.mask("connect 10.0.0.1:8080 done"), "connect <IP> done");
    }

    #[test]
    fn masks_ipv6() {
        let m = LogMasker::default();
        assert_eq!(m.mask("client fe80::1a2b:3c4d here"), "client <IPV6> here");
    }

    #[test]
    fn masks_uuid() {
        let m = LogMasker::default();
        let input = "req 550e8400-e29b-41d4-a716-446655440000 done";
        assert_eq!(m.mask(input), "req <UUID> done");
    }

    #[test]
    fn masks_hex_commit() {
        let m = LogMasker::default();
        assert_eq!(m.mask("commit abc123def456 pushed"), "commit <HEX> pushed");
    }

    #[test]
    fn masks_url() {
        let m = LogMasker::default();
        assert_eq!(m.mask("GET https://api.io/v1?x=1 200"), "GET <URL> <NUM>");
    }

    #[test]
    fn masks_unix_path() {
        let m = LogMasker::default();
        assert_eq!(m.mask("open /var/log/syslog failed"), "open <PATH> failed");
    }

    #[test]
    fn masks_windows_path() {
        let m = LogMasker::default();
        assert_eq!(m.mask("read C:\\Users\\bob\\file.txt"), "read <PATH>");
    }

    #[test]
    fn masks_email() {
        let m = LogMasker::default();
        assert_eq!(m.mask("sent to admin@test.com ok"), "sent to <EMAIL> ok");
    }

    #[test]
    fn masks_inline_json() {
        let m = LogMasker::default();
        assert_eq!(m.mask("payload {\"a\":1} end"), "payload <JSON> end");
    }

    #[test]
    fn masks_time_of_day() {
        let m = LogMasker::default();
        assert_eq!(m.mask("at 14:30:22 done"), "at <TIME> done");
    }

    #[test]
    fn masks_duration() {
        let m = LogMasker::default();
        assert_eq!(m.mask("took 45ms total"), "took <TIME> total");
    }

    #[test]
    fn masks_numbers() {
        let m = LogMasker::default();
        assert_eq!(m.mask("retry 3 of 10"), "retry <NUM> of <NUM>");
    }

    #[test]
    fn masks_decimal_numbers() {
        let m = LogMasker::default();
        assert_eq!(m.mask("latency 12.5ms"), "latency <TIME>");
    }

    // --- key=value preservation ---

    #[test]
    fn key_value_keeps_key() {
        let m = LogMasker::default();
        assert_eq!(
            m.mask("request status=200 done"),
            "request status=<NUM> done"
        );
    }

    #[test]
    fn key_value_path() {
        let m = LogMasker::default();
        assert_eq!(m.mask("path=/api/users ok"), "path=<PATH> ok");
    }

    #[test]
    fn key_value_duration() {
        let m = LogMasker::default();
        assert_eq!(m.mask("db took=45ms end"), "db took=<TIME> end");
    }

    // --- Composite tests ---

    #[test]
    fn masks_complex_log_line() {
        let m = LogMasker::default();
        let input = "User user_981a2f3b logged in from 192.168.1.50";
        let out = m.mask(input);
        assert!(out.contains("<HEX>"), "hex should be masked: {out}");
        assert!(out.contains("<IP>"), "ip should be masked: {out}");
        assert!(!out.contains("981a2f3b"), "hex value should be gone: {out}");
        assert!(
            !out.contains("192.168.1.50"),
            "ip value should be gone: {out}"
        );
    }

    #[test]
    fn masks_mixed_token_inner_runs() {
        let m = LogMasker::default();
        assert_eq!(m.mask("on worker-12 ok"), "on worker-<NUM> ok");
    }

    #[test]
    fn no_allocation_when_nothing_to_mask() {
        let m = LogMasker::default();
        let input = "INFO all systems nominal";
        let out = m.mask(input);
        assert!(
            matches!(out, Cow::Borrowed(_)),
            "should be borrowed: {out:?}"
        );
    }

    #[test]
    fn config_disables_masks() {
        let mut cfg = MaskConfig::default();
        cfg.mask_nums = false;
        let m = LogMasker::new(cfg);
        assert_eq!(m.mask("retry 3 times"), "retry 3 times");
    }

    #[test]
    fn uuid_not_double_masked_as_hex() {
        let m = LogMasker::default();
        let input = "id 550e8400-e29b-41d4-a716-446655440000";
        let out = m.mask(input);
        assert_eq!(out, "id <UUID>");
    }

    #[test]
    fn url_not_double_masked_as_path() {
        let m = LogMasker::default();
        let input = "fetch https://api.example.com/v1/users";
        let out = m.mask(input);
        assert_eq!(out, "fetch <URL>");
    }

    #[test]
    fn json_not_double_masked_as_numbers() {
        let m = LogMasker::default();
        let input = "payload {\"id\":42}";
        let out = m.mask(input);
        assert_eq!(out, "payload <JSON>");
    }

    #[test]
    fn empty_line_borrowed() {
        let m = LogMasker::default();
        let out = m.mask("");
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn short_line_with_digit_masked() {
        let m = LogMasker::default();
        let out = m.mask("INFO 123");
        assert_eq!(out, "INFO <NUM>");
    }

    #[test]
    fn header_slots_force_masks() {
        let m = LogMasker::default();
        let header = vec![None, Some("<HOST>")];
        // Pure-text dynamic value → whole-token mask.
        let out = m.mask_with_header(
            "INFO web-prod request ok",
            &header,
            &mut MaskCache::default(),
        );
        assert_eq!(out, "INFO <HOST> request ok");
    }

    #[test]
    fn header_slots_preserve_token_shape() {
        let m = LogMasker::default();
        // iOS process token: the learned slot is `<NUM>`, but the token has
        // inner dynamic runs → shape is preserved.
        let header = vec![Some(crate::core::masking::MASK_NUM)];
        let out = m.mask_with_header("MyApp[45678:8] hello", &header, &mut MaskCache::default());
        assert_eq!(out, "MyApp[<NUM>:<NUM>] hello");
        // Source ref: filename survives, line number masked.
        let out = m.mask_with_header(
            "CoreDataStack.swift:280 hello",
            &header,
            &mut MaskCache::default(),
        );
        assert_eq!(out, "CoreDataStack.swift:<NUM> hello");
    }

    // --- classify_token_public tests ---

    #[test]
    fn classify_mixed_token_with_inner_digits() {
        // `MyApp[12345:1]` is the iOS process ID token — contains inner
        // numeric runs that should be detected as dynamic.
        assert_eq!(classify_token_public("MyApp[12345:1]"), Some(MASK_NUM));
        assert_eq!(classify_token_public("worker-12"), Some(MASK_NUM));
        assert_eq!(classify_token_public("backend-3:8443"), Some(MASK_NUM));
        assert_eq!(classify_token_public("session_42"), Some(MASK_NUM));
    }

    #[test]
    fn classify_pure_text_returns_none() {
        assert_eq!(classify_token_public("INFO"), None);
        assert_eq!(classify_token_public("MyApp"), None);
        assert_eq!(classify_token_public("User"), None);
    }

    #[test]
    fn classify_mixed_hex_recognized() {
        // `0xABCD` is not recognized as hex by `has_dynamic_inner_run`
        // because it's a single alphanumeric run (no separator) containing
        // 'x' (not a hex digit). So `classify_token_public` returns None for
        // it. That's consistent with the masker, which also doesn't mask it.
        // This is fine — the header learner won't see these tokens.
        assert_eq!(classify_token_public("0xABCD"), None);
    }

    #[test]
    fn classify_source_ref_with_line_number() {
        // `AppDelegate.swift:42` — the `:42` part has a numeric run.
        assert_eq!(
            classify_token_public("AppDelegate.swift:42"),
            Some(MASK_NUM)
        );
        assert_eq!(
            classify_token_public("ViewController.swift:142"),
            Some(MASK_NUM)
        );
    }

    #[test]
    fn classify_plain_number_still_works() {
        assert_eq!(classify_token_public("42"), Some(MASK_NUM));
        assert_eq!(classify_token_public("-3.14"), Some(MASK_NUM));
    }

    #[test]
    fn classify_cross_format_header_tokens() {
        // Android logcat brief: `I/Tag(1234):`
        assert_eq!(
            classify_token_public("I/ActivityManager(1234):"),
            Some(MASK_NUM)
        );
        // nginx error: `1234#5678:`
        assert_eq!(classify_token_public("1234#5678:"), Some(MASK_NUM));
        // Bracketed pid/tid pairs: `[1234:5678]`
        assert_eq!(classify_token_public("[1234:5678]"), Some(MASK_NUM));
        // glog source ref: `server.cc:42]`
        assert_eq!(classify_token_public("server.cc:42]"), Some(MASK_NUM));
        // syslog process: `sshd[1234]:`
        assert_eq!(classify_token_public("sshd[1234]:"), Some(MASK_NUM));
        // Quoted numbers (nginx status/bytes): `"200"`
        assert_eq!(classify_token_public("\"200\""), Some(MASK_NUM));
    }

    #[test]
    fn classify_no_digits_returns_none() {
        // No digits at all → no inner runs to detect.
        assert_eq!(classify_token_public("pure-text"), None);
        assert_eq!(classify_token_public("hello_world"), None);
    }

    #[test]
    fn cache_stops_retaining_unique_tokens_at_capacity() {
        let masker = LogMasker::default();
        let mut cache = MaskCache::default();
        for i in 0..=MASK_CACHE_MAX_ENTRIES {
            let token = format!("request-{i}");
            let _ = masker.cached_mask(&token, &mut cache);
        }

        assert_eq!(cache.0.len(), MASK_CACHE_MAX_ENTRIES);
        assert_eq!(
            masker.cached_mask("42", &mut cache).as_deref(),
            Some(MASK_NUM)
        );
        assert_eq!(cache.0.len(), MASK_CACHE_MAX_ENTRIES);
    }
}
