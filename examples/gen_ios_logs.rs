//! Generate iOS-style dummy app logs for testing logotomy.
//!
//! Deterministic: the same `--seed` always produces byte-identical output —
//! the generator is pure Rust (PCG64) with zero platform-dependent behavior,
//! so tests & benchmarks are reproducible on every OS/arch, forever.
//!
//! Usage:
//!   cargo run --release --example gen_ios_logs -- [SIZES...] [--all] [--seed N]
//!
//!   cargo run --release --example gen_ios_logs --            -> iOS-1K.log, iOS-10K.log (default)
//!   cargo run --release --example gen_ios_logs -- 100K 1M    -> iOS-100K.log, iOS-1M.log
//!   cargo run --release --example gen_ios_logs -- --all      -> iOS-1K.log, iOS-10K.log, iOS-100K.log, iOS-1M.log
//!   cargo run --release --example gen_ios_logs -- 1M --seed 7 -> custom RNG seed
//!
//! Every size starts from a fresh PCG64 seeded identically, so smaller files
//! are exact byte-prefixes of larger ones (1K ⊂ 10K ⊂ 100K ⊂ 1M) — handy for
//! comparing benchmark scaling on identical data.

use std::io::{self, BufWriter, Write};
use std::path::Path;

use rand::Rng;
use rand::SeedableRng;
use rand_pcg::Pcg64;

// ---------------------------------------------------------------------------
// Seeded generator
// ---------------------------------------------------------------------------

/// Thin wrapper around a seeded PCG64. PCG64 is a fully-specified algorithm,
/// so identical seeds yield identical streams on every platform.
struct Gen {
    rng: Pcg64,
}

impl Gen {
    fn new(seed: u64) -> Self {
        Self {
            rng: Pcg64::seed_from_u64(seed),
        }
    }

    /// Inclusive integer range [lo, hi].
    fn int(&mut self, lo: i64, hi: i64) -> i64 {
        self.rng.gen_range(lo..=hi)
    }

    /// Inclusive u64 range [lo, hi].
    fn un(&mut self, lo: u64, hi: u64) -> u64 {
        self.rng.gen_range(lo..=hi)
    }

    /// Float in [lo, hi).
    fn float(&mut self, lo: f64, hi: f64) -> f64 {
        self.rng.gen_range(lo..hi)
    }

    /// True with probability `p`.
    fn chance(&mut self, p: f64) -> bool {
        self.rng.gen_range(0.0..1.0) < p
    }

    fn choice<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.rng.gen_range(0..items.len())]
    }

    /// Weighted pick: item with cumulative-weight index `r`.
    fn weighted<'a, T>(&mut self, items: &'a [T], weights: &[u32]) -> &'a T {
        let total: u32 = weights.iter().sum();
        let mut r: u32 = self.rng.gen_range(0..total);
        for (i, w) in weights.iter().enumerate() {
            if r < *w {
                return &items[i];
            }
            r -= *w;
        }
        unreachable!("weights sum > 0 and r < total")
    }

    fn u32(&mut self) -> u32 {
        self.rng.gen()
    }

    fn u16(&mut self) -> u16 {
        self.rng.gen()
    }

    fn u64(&mut self) -> u64 {
        self.rng.gen()
    }
}

// ---------------------------------------------------------------------------
// Log level distribution
// ---------------------------------------------------------------------------
// ERROR 5%, FAULT 1%, WARNING 10%, NOTICE 14%, INFO 45%, DEBUG 25%
const LEVELS: [&str; 6] = ["ERROR", "FAULT", "WARNING", "NOTICE", "INFO", "DEBUG"];
const LEVEL_WEIGHTS: [u32; 6] = [5, 1, 10, 14, 45, 25];

// ---------------------------------------------------------------------------
// Source files
// ---------------------------------------------------------------------------
const FILES: [&str; 20] = [
    "AppDelegate.swift",
    "ViewController.swift",
    "NetworkManager.swift",
    "CoreDataStack.swift",
    "NotificationService.swift",
    "SceneDelegate.swift",
    "UserDefaultsManager.swift",
    "PushHandler.swift",
    "LocationManager.swift",
    "AuthService.swift",
    "SyncEngine.swift",
    "AnalyticsTracker.swift",
    "WatchConnectivityManager.swift",
    "StoreKitManager.swift",
    "HealthKitManager.swift",
    "CameraService.swift",
    "DeepLinkRouter.swift",
    "ConflictResolver.swift",
    "MediaCacheManager.swift",
    "SecureStore.swift",
];

// ---------------------------------------------------------------------------
// Messages per level. `{token}` placeholders are filled from the value
// generator; all other text (including literal braces like {{0,0},{375,812}})
// is written to the log exactly as typed, mirroring real iOS Console output.
// ---------------------------------------------------------------------------
const DEBUG_MSGS: [&str; 34] = [
    "viewDidLoad called",
    "viewDidLoad called frame=0x{addr}",
    "viewWillAppear animated=YES",
    "viewDidAppear animated=YES",
    "viewWillDisappear animated=NO",
    "viewDidDisappear animated=NO",
    "applicationDidBecomeActive",
    "applicationWillResignActive",
    "applicationDidEnterBackground",
    "applicationWillEnterForeground",
    "sceneWillConnectTo session role=foreground",
    "sceneDidDisconnect",
    "prepareForSegue sender=LoginButton",
    "didReceiveMemoryWarning",
    "layoutSubviews bounds={{0,0},{375,812}}",
    "updateConstraints",
    "dealloc",
    "tableView:cellForRowAtIndexPath: section=2 row=14",
    "collectionView:didSelectItemAtIndexPath: item=7",
    "scrollViewDidScroll offset.y=142.5",
    "willTransitionToTraitCollection userInterfaceStyle=dark",
    "application:supportedInterfaceOrientationsForWindow: mask=portrait",
    "touchesBegan tapCount=1 location={{120,340}}",
    "observeValueForKeyPath keyPath=contentOffset",
    "presentViewController animated=YES completion=nil",
    "dismissViewControllerAnimated=YES",
    "renderer:didFinishFrame frame={{0,0},{1179,2556}} presentTime={ms}ms",
    "queue drained itemCount={count} mode=main",
    "state machine transitioned from=idle to=loading event={event}",
    "AVPlayerItem status changed to readyToPlay asset=video_{entity_id}.mp4",
    "sqlite3 prepare step rc=SQLITE_OK stmt_id={request_id}",
    "image decoded dimensions={{1179,2556}} bytes={bytes}",
    "haptic feedback played pattern=success intensity={percent}%",
    "CNContactStore access granted for contact_id={entity_id}",
];

const INFO_MSGS: [&str; 41] = [
    "UserDefaults sync completed in 0.042s",
    "CoreData save context took 0.018s",
    "Network request GET /api/v2/users status=200 length=12402",
    "Network request POST /api/v2/login status=201 length=512",
    "Network request GET /api/v2/feed status=304 length=0",
    "Push token registered: <APNS_TOKEN>",
    "Background fetch completed with result=UIBackgroundFetchResultNewData",
    "Cache cleanup removed 47 expired entries",
    "Keychain write succeeded for key=com.app.auth.token",
    "Session renewed expiry=2026-07-20T10:00:00Z",
    "Location update lat=40.7128 lon=-74.0060 accuracy=12.3",
    "Battery state changed to charging level=0.78",
    "Deep link handled url=myapp://profile/42",
    "Remote config fetched keys=42",
    "In-app purchase restored product_id=premium_monthly",
    "Push notification received aps={alert=Hello, badge=3}",
    "CloudKit record pushed recordID=user_42",
    "Watch connectivity message sent payload={command=sync}",
    "Handoff activity type=com.app.browsing updated",
    "URLSession task completed identifier=upload_42 bytesSent=8192",
    "CoreData fetch request completed entity=Message count=127",
    "Image cache hit for URL /images/avatar_42.png",
    "Local notification scheduled fireDate=2026-07-20T08:00:00Z",
    "App state restored from encoded archive version=3",
    "Shortcut item performed type=com.app.search",
    "Network request GET /api/v2/users?id={user_id} status={status} length={bytes}",
    "Network request PUT /api/v2/profile/{user_id} status=200 length={bytes}",
    "Push token registered: {entity_uuid}",
    "Session renewed expiry={expiry}",
    "Location update lat={lat} lon={lon} accuracy={accuracy}",
    "CoreData fetch request completed entity=Message count={count} duration={ms}ms",
    "Watch connectivity message sent payload={command={event}, id={entity_id}}",
    "Handoff activity type=com.app.{route} updated",
    "Image cache stored for URL /images/avatar_{user_id}.png size={bytes}",
    "Local notification scheduled fireDate={expiry}",
    "CloudKit record pushed recordID=user_{user_id}",
    "Deep link handled url=myapp://{route}/{entity_id}",
    "Background fetch completed with result=UIBackgroundFetchResultNewData items={count}",
    "User profile loaded user_id={user_id} role=premium",
    "Login audit user_id={user_id} method=password",
    "Session token refreshed user_id={user_id} ttl=3600",
];

const WARNING_MSGS: [&str; 24] = [
    "Network request GET /api/v2/users slow took 3.2s",
    "CoreData fetch exceeded threshold entity=LogEntry count=15000",
    "Memory pressure warning received pressureLevel=critical",
    "Disk cache eviction rate high evicted=2048 entries in 60s",
    "Thread performance checker: -[UIView layoutSubviews] took 42ms",
    "Keychain write returned duplicate item for key=com.app.auth.token",
    "Push notification payload too large size=5120 bytes",
    "Rate limit approaching: 85/100 requests used this window",
    "Background task will expire soon identifier=com.app.sync.42",
    "Certificate expiration warning daysRemaining=14",
    "Network request GET /api/v2/users?id={user_id} slow took {secs}s",
    "Retry {count} of {percent} for request {session_id}",
    "CoreData fetch exceeded threshold entity=Message count={count} duration={ms_int}ms",
    "Disk cache eviction rate high evicted={count} entries in {secs}s",
    "Thread performance checker: -[UIView layoutSubviews] took {ms_int}ms",
    "Keychain write returned duplicate item for key=com.app.{key_name}",
    "Push notification payload too large size={bytes} bytes",
    "Rate limit approaching: {percent}/100 requests used this window",
    "Background task will expire soon identifier=com.app.sync.{entity_id}",
    "Certificate expiration warning daysRemaining={count}",
    "Memory pressure warning received pressureLevel={event}",
    "Watchdog preemption risk: main thread busy for {secs}s",
    "Rate limit exceeded user_id={user_id} endpoint=/api/v2/feed",
    "Duplicate sync request detected user_id={user_id} job=sync-photos",
];

const NOTICE_MSGS: [&str; 24] = [
    "User login detected method=biometric",
    "User logout initiated",
    "App version 3.14.2 build 2074",
    "First launch after update detected",
    "Watch connectivity session activated",
    "Handoff activity type=com.app.browsing started",
    "CloudKit sync pushed 12 records",
    "CoreData migration completed from v3 to v4",
    "Certificate pinning validation passed",
    "Rate limit warning: 85/100 requests used",
    "iCloud account status changed to available",
    "Sandbox receipt validation succeeded",
    "StoreKit payment queue updated transactions=3",
    "Sign in with Apple credential revoked",
    "Background URL session events drained",
    "User login detected method=biometric username={name}",
    "App version 3.14.2 build {count}",
    "CloudKit sync pushed {count} records",
    "StoreKit payment queue updated transactions={count} product_id=premium_{event}",
    "Watch connectivity session activated device={device}",
    "Handoff activity type=com.app.{route} started user={name}",
    "Remote config fetched keys={count} namespace={event}",
    "Background URL session events drained count={count}",
    "Deep link handled url=myapp://{route}/{entity_id} route_known=YES",
];

const ERROR_MSGS: [&str; 42] = [
    "Network request GET /api/v2/users failed status=500 error='Internal server error'",
    "Network request POST /api/v2/sync failed status=503 error='Service unavailable'",
    "CoreData save failed error='NSValidationErrorKey' entity=User key=email",
    "Keychain read failed for key=com.app.auth.refresh error='errSecItemNotFound'",
    "Decoding JSON failed error='typeMismatch' key=expectedDeliveryDate",
    "URLSession task failed with error='The network connection was lost'",
    "Background task expired before completion identifier=com.app.upload.42",
    "Push notification delivery failed error='BadDeviceToken'",
    "File write failed at path=/tmp/crashreport.plist error='No space left on device'",
    "Assertion failure in -[AppDelegate application:didFinishLaunchingWithOptions:]",
    "Network request PUT /api/v2/profile failed status=401 error='Unauthorized'",
    "CoreData fetch failed error='NSInternalInconsistencyError' entity=Session",
    "Image decoding failed for URL /images/avatar_99.png error='Invalid data'",
    "AVPlayer playback failed error='Cannot decode' asset=video_42.mp4",
    "PDF generation failed error='Invalid page layout' pageSize=A4",
    "StoreKit payment failed error='SKErrorPaymentCancelled' product_id=premium",
    "WebSocket connection closed with code=1006 reason='Abnormal closure'",
    "MapKit reverse geocode failed error='kCLErrorDomain error 2'",
    "Biometric authentication failed error='LAErrorUserFallback'",
    "Audio session activation failed error='AVAudioSessionErrorCodeResourceNotAvailable'",
    "Camera capture failed error='AVErrorMediaServicesWereReset'",
    "HealthKit query failed error='HKErrorAuthorizationDenied'",
    "ARKit world tracking failed error='ARErrorSensorUnavailable'",
    "SiriKit intent resolution failed error='INIntentErrorRequestTimedOut'",
    "CoreBluetooth connection failed peripheral=0xABCD error='CBErrorConnectionTimeout'",
    "Network request GET /api/v2/users?id={user_id} failed status={status} error='Internal server error'",
    "Network request POST /api/v2/sync/{user_id} failed status={status} error='Service unavailable'",
    "Network request PUT /api/v2/profile/{user_id} failed status=401 error='Unauthorized'",
    "WebSocket connection closed with code=1006 reason='Abnormal closure' peer={ip}:{port}",
    "URLSession task failed with error='{error_code}' request={session_id}",
    "CoreData save failed error='NSValidationErrorKey' entity=User key=email value=<private>",
    "Keychain read failed for key=<private> error='errSecItemNotFound'",
    "Push notification delivery failed error='BadDeviceToken' token={entity_uuid}",
    "Decoding JSON failed error='typeMismatch' key=expectedDeliveryDate payload=<private>",
    "Biometric authentication failed error='LAErrorUserFallback' user={name}",
    "Network request POST /api/v2/login failed status=429 error='Too Many Requests' retry_after={secs}s",
    "File write failed at path=/tmp/crash_{user_id}.plist error='No space left on device'",
    "CloudKit record push failed recordID=user_{user_id} error='CKErrorNetworkFailure'",
    "MapKit reverse geocode failed error='kCLErrorDomain error {count}'",
    "CoreBluetooth connection failed peripheral=0x{addr_short} error='CBErrorConnectionTimeout'",
    "Authentication failed user_id={user_id} error='Invalid credentials'",
    "Session expired mid-request user_id={user_id} error='TokenExpired'",
];

const FAULT_MSGS: [&str; 12] = [
    "SIGABRT — uncaught exception 'NSInvalidArgumentException' reason='-[NSNull length]: unrecognized selector'",
    "EXC_BAD_ACCESS — dangling pointer reference in dealloc of UserModel",
    "SIGSEGV — null pointer dereference at NetworkManager.swift:142",
    "Watchdog timeout — main thread blocked for 12.3s by sync operation",
    "OOM — process killed by jetsam memory limit reached (512MB)",
    "EXC_CRASH (SIGABRT) — uncaught exception 'NSRangeException' reason='*** -[__NSArrayM objectAtIndex:]: index {count} beyond bounds [0 .. {percent}]'",
    "SIGPIPE — write to closed socket fd={count} in NetworkManager.swift:{line}",
    "EXC_BAD_INSTRUCTION (SIGILL) — assertion failed at ViewController.swift:{line}",
    "SIGBUS — misaligned memory access in CoreDataStack.swift:{line} address=0x{addr_short}",
    "Thread 1: EXC_BREAKPOINT (SIGTRAP) — Swift runtime trap: force unwrapped nil Optional at {route}:{line}",
    "NSInternalInconsistencyException — 'Invalid parameter not satisfying: count > 0' in -[SyncEngine startSync]",
    "EXC_CRASH (SIGKILL) — jetsam killed process after memory limit exceeded ({count}MB)",
];

/// Crash-time stack frames appended after most FAULT lines (raw frames, no
/// timestamp — exactly like real crash reports).
const FAULT_STACK_FRAMES: [&str; 8] = [
    "{frame_n}   MyApp                               0x{addr} 0x1029f8000 + {offset}",
    "{frame_n}   libsystem_kernel.dylib              0x{addr} __pthread_kill + {offset}",
    "{frame_n}   libsystem_pthread.dylib             0x{addr} _pthread_start + {offset}",
    "{frame_n}   CoreFoundation                      0x{addr} CFRunLoopRunSpecific + {offset}",
    "{frame_n}   MyApp                               0x{addr} -[NetworkManager sendRequest:] + {offset}",
    "{frame_n}   MyApp                               0x{addr} -[ViewController viewDidLoad] + {offset}",
    "{frame_n}   UIKitCore                           0x{addr} -[UIApplication _run] + {offset}",
    "{frame_n}   libobjc.A.dylib                     0x{addr} objc_exception_throw + {offset}",
];

fn msgs_for(level: &str) -> &'static [&'static str] {
    match level {
        "DEBUG" => &DEBUG_MSGS,
        "INFO" => &INFO_MSGS,
        "WARNING" => &WARNING_MSGS,
        "NOTICE" => &NOTICE_MSGS,
        "ERROR" => &ERROR_MSGS,
        "FAULT" => &FAULT_MSGS,
        _ => unreachable!("unknown level {level:?}"),
    }
}

// ---------------------------------------------------------------------------
// Template substitution
// ---------------------------------------------------------------------------

/// Substitute `{token}` placeholders in a message template.
///
/// Matches Python's scanner semantics: `{` followed by `[A-Za-z_]` then
/// `[A-Za-z0-9_]*` then `}` is a placeholder; everything else (including
/// literal `{{0,0},{375,812}}` and `payload={command=sync}` where the closing
/// brace doesn't immediately follow an identifier) passes through untouched.
fn make_message(rng: &mut Gen, template: &str) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let bytes = template.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        if bytes[i] == b'{' {
            let mut j = i + 1;
            if j < n && (bytes[j].is_ascii_alphabetic() || bytes[j] == b'_') {
                let start = j;
                while j < n && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                if j < n && bytes[j] == b'}' {
                    out.push_str(&gen_value(rng, &template[start..j]));
                    i = j + 1;
                    continue;
                }
            }
        }
        // Copy the full UTF-8 char at byte offset i (templates may contain
        // em-dashes in FAULT messages).
        let ch = template[i..].chars().next().expect("non-empty tail");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Seeded value generator for `{token}` placeholders.
fn gen_value(rng: &mut Gen, token: &str) -> String {
    match token {
        "user_id" => rng.int(1, 99_999).to_string(),
        "entity_id" => rng.int(1, 9_999).to_string(),
        "request_id" => rng.int(100_000, 999_999).to_string(),
        "session_id" => format!("sess-{}", rng.int(100_000, 999_999)),
        "status" => {
            const S: [i64; 14] = [
                200, 201, 204, 301, 304, 400, 401, 403, 404, 429, 500, 502, 503, 504,
            ];
            rng.choice(&S).to_string()
        }
        "bytes" => rng.int(1, 65_535).to_string(),
        "ms" => format!("{:.3}", rng.float(0.5, 9_500.0)),
        "ms_int" => rng.int(1, 9_500).to_string(),
        "secs" => format!("{:.2}", rng.float(0.1, 45.0)),
        "percent" => rng.int(1, 100).to_string(),
        "count" => rng.int(1, 99_999).to_string(),
        "port" => rng.int(1_024, 65_535).to_string(),
        "ip" => format!(
            "{}.{}.{}.{}",
            rng.int(1, 255),
            rng.int(0, 255),
            rng.int(0, 255),
            rng.int(1, 254)
        ),
        "device" => rng
            .choice(&[
                "iPhone16,2",
                "iPhone15,3",
                "iPhone14,2",
                "iPad14,5",
                "iPad13,4",
                "AppleTV14,1",
                "Watch7,4",
            ])
            .to_string(),
        "key_name" => rng
            .choice(&[
                "auth.token",
                "auth.refresh",
                "launch.count",
                "theme",
                "locale",
                "push.token",
                "onboarding.done",
            ])
            .to_string(),
        "route" => rng
            .choice(&[
                "profile",
                "settings",
                "feed",
                "messages",
                "search",
                "notifications",
                "billing",
            ])
            .to_string(),
        "entity_uuid" => format!(
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            rng.u32(),
            rng.u16(),
            rng.u16(),
            rng.u16() | 0x4000,
            (rng.u64() & 0xFFFF_FFFF_FFFF) | 0x8000_0000_0000
        ),
        "error_code" => rng
            .choice(&[
                "NSURLErrorTimedOut",
                "NSURLErrorCannotConnectToHost",
                "NSURLErrorNetworkConnectionLost",
                "kCLErrorLocationUnknown",
                "AVErrorDiskFull",
                "CBErrorConnectionTimeout",
                "CKErrorNetworkFailure",
                "SKErrorStoreProductNotAvailable",
            ])
            .to_string(),
        "lat" => format!("{:.4}", rng.float(-90.0, 90.0)),
        "lon" => format!("{:.4}", rng.float(-180.0, 180.0)),
        "accuracy" => format!("{:.1}", rng.float(3.0, 65.0)),
        "line" => rng.int(1, 300).to_string(),
        "addr" => format!("{:016x}", rng.un(0x100000, 0xFFFF_FFFF_FF)),
        "addr_short" => format!("{:x}", rng.un(1, 0xFFFFF)),
        "name" => rng
            .choice(&["alice", "bob", "carol", "dave", "erin", "frank"])
            .to_string(),
        "event" => rng
            .choice(&[
                "login",
                "logout",
                "sync_start",
                "sync_end",
                "push_register",
                "deep_link",
                "purchase",
                "upload",
            ])
            .to_string(),
        "expiry" => format!(
            "2026-07-{:02}T{:02}:{:02}:00Z",
            rng.int(1, 30),
            rng.int(0, 23),
            rng.int(0, 59)
        ),
        other => format!("{{{other}}}"), // unknown token: keep literal
    }
}

// ---------------------------------------------------------------------------
// Streaming generation
// ---------------------------------------------------------------------------
const PIDS: [i64; 4] = [12345, 91234, 45678, 78901];
const BASE_TS: i64 = 1_784_152_800; // 2026-07-15 22:00:00 UTC (displayed with +0300 offset)

fn iso_ts(epoch_sec: i64) -> String {
    chrono::DateTime::from_timestamp(epoch_sec, 0)
        .expect("timestamp in range")
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// Write exactly `count` primary lines (plus crash-stack frames after most
/// FAULT lines) to `path`, streaming in 8K-line chunks. Returns the actual
/// number of lines written (may exceed `count` by added FAULT frames).
fn generate(path: &Path, count: u64, rng: &mut Gen) -> io::Result<u64> {
    let f = std::fs::File::create(path)?;
    let mut out = BufWriter::with_capacity(1 << 20, f);
    let mut chunk: Vec<String> = Vec::with_capacity(8192);
    let mut written: u64 = 0;
    let mut sec: i64 = 0;
    let mut remaining_in_sec: i64 = 0;

    while written < count {
        if remaining_in_sec == 0 {
            remaining_in_sec = rng.int(1, 5); // bursty: 1–5 lines/sec
            sec += 1;
        }
        let us = rng.int(0, 999_999);
        remaining_in_sec -= 1;

        let pid = *rng.choice(&PIDS);
        let tid = rng.int(1, 16);
        let level = *rng.weighted(&LEVELS, &LEVEL_WEIGHTS);
        let src = *rng.choice(&FILES);
        let src_line = rng.int(1, 300);
        let tpl = *rng.choice(msgs_for(level));
        let msg = make_message(rng, tpl);

        chunk.push(format!(
            "{}.{:06}+0300 MyApp[{}:{}] <{}> {}:{} {}\n",
            iso_ts(BASE_TS + sec),
            us,
            pid,
            tid,
            level,
            src,
            src_line,
            msg
        ));
        written += 1;

        // Most faults carry a multi-frame crash stack (90% of FAULTs).
        if level == "FAULT" && rng.chance(0.9) {
            for n in 0..rng.int(2, 4) {
                if written >= count {
                    break;
                }
                let tpl = *rng.choice(&FAULT_STACK_FRAMES);
                let addr = format!("{:016x}", rng.un(0x100000, 0xFFFF_FFFF_FF));
                let offset = rng.un(16, 8192);
                chunk.push(format!(
                    "{}\n",
                    tpl.replace("{frame_n}", &n.to_string())
                        .replace("{addr}", &addr)
                        .replace("{offset}", &offset.to_string())
                ));
                written += 1;
            }
        }

        if chunk.len() >= 8192 {
            for line in chunk.drain(..) {
                out.write_all(line.as_bytes())?;
            }
        }
    }
    for line in chunk.drain(..) {
        out.write_all(line.as_bytes())?;
    }
    out.flush()?;
    Ok(written)
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

fn parse_size(s: &str) -> u64 {
    let s = s.trim().to_ascii_uppercase();
    if let Some(n) = s.strip_suffix('K') {
        n.trim().parse::<u64>().expect("invalid size") * 1_000
    } else if let Some(n) = s.strip_suffix('M') {
        n.trim().parse::<u64>().expect("invalid size") * 1_000_000
    } else {
        s.parse::<u64>().expect("invalid size")
    }
}

fn print_usage() {
    eprintln!(
        "Generate iOS-style dummy app logs for testing logotomy.\n\
         \n\
         Usage:\n\
         \x20 cargo run --release --example gen_ios_logs -- [SIZES...] [--all] [--seed N]\n\
         \n\
         \x20 cargo run --release --example gen_ios_logs --             -> iOS-1K.log, iOS-10K.log (default)\n\
         \x20 cargo run --release --example gen_ios_logs -- 100K 1M     -> iOS-100K.log, iOS-1M.log\n\
         \x20 cargo run --release --example gen_ios_logs -- --all       -> iOS-1K.log, iOS-10K.log, iOS-100K.log, iOS-1M.log\n\
         \x20 cargo run --release --example gen_ios_logs -- 1M --seed 7 -> custom RNG seed\n\
         \n\
         Deterministic: the same seed always produces byte-identical output,\n\
         and every size starts from the same seed so smaller files are exact\n\
         prefixes of larger ones (1K ⊂ 10K ⊂ 100K ⊂ 1M)."
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut seed: u64 = 42;
    let mut all = false;
    let mut sizes: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--all" => all = true,
            "--seed" => {
                i += 1;
                let raw = args.get(i).unwrap_or_else(|| {
                    eprintln!("error: --seed requires a value");
                    std::process::exit(2);
                });
                seed = raw.parse().unwrap_or_else(|_| {
                    eprintln!("error: --seed must be an integer, got {raw:?}");
                    std::process::exit(2);
                });
            }
            "-h" | "--help" => {
                print_usage();
                return;
            }
            s if s.starts_with('-') => {
                eprintln!("error: unknown flag {s:?}");
                print_usage();
                std::process::exit(2);
            }
            s => sizes.push(s.to_string()),
        }
        i += 1;
    }

    if all {
        sizes = vec!["1K".into(), "10K".into(), "100K".into(), "1M".into()];
    }
    if sizes.is_empty() {
        sizes = vec!["1K".into(), "10K".into()];
    }

    println!("[seed={seed}] sizes={}", sizes.join(" "));
    for label in &sizes {
        let upper = label.trim().to_ascii_uppercase();
        let count = parse_size(&upper);
        let path = format!("iOS-{upper}.log");
        // Each size starts from a fresh PCG64 seeded identically, so
        // generation is fully deterministic per size AND smaller files are
        // exact byte-prefixes of larger ones — convenient for benchmarking.
        let written = generate(Path::new(&path), count, &mut Gen::new(seed)).unwrap_or_else(|e| {
            eprintln!("error: failed to write {path}: {e}");
            std::process::exit(1);
        });
        println!("Generated {written} lines → {path}");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("logotomy_gen_{}_{}", std::process::id(), name))
    }

    fn cleanup(p: &std::path::Path) {
        fs::remove_file(p).ok();
    }

    #[test]
    fn same_seed_is_byte_identical() {
        let p1 = tmp("det1.log");
        let p2 = tmp("det2.log");
        generate(&p1, 10_000, &mut Gen::new(42)).unwrap();
        generate(&p2, 10_000, &mut Gen::new(42)).unwrap();
        assert_eq!(fs::read(&p1).unwrap(), fs::read(&p2).unwrap());
        cleanup(&p1);
        cleanup(&p2);
    }

    #[test]
    fn different_seeds_differ() {
        let p1 = tmp("seed1.log");
        let p2 = tmp("seed2.log");
        generate(&p1, 10_000, &mut Gen::new(1)).unwrap();
        generate(&p2, 10_000, &mut Gen::new(2)).unwrap();
        assert_ne!(fs::read(&p1).unwrap(), fs::read(&p2).unwrap());
        cleanup(&p1);
        cleanup(&p2);
    }

    #[test]
    fn smaller_is_prefix_of_larger() {
        let small = tmp("pref_small.log");
        let large = tmp("pref_large.log");
        let s = generate(&small, 1_000, &mut Gen::new(42)).unwrap();
        let l = generate(&large, 10_000, &mut Gen::new(42)).unwrap();
        assert!(l > s);
        let bytes_small = fs::read(&small).unwrap();
        let bytes_large = fs::read(&large).unwrap();
        assert!(bytes_large.len() >= bytes_small.len());
        assert_eq!(&bytes_large[..bytes_small.len()], &bytes_small[..]);
        cleanup(&small);
        cleanup(&large);
    }

    #[test]
    fn token_scanner_preserves_literals() {
        let mut rng = Gen::new(42);
        let a = make_message(&mut rng, "layoutSubviews bounds={{0,0},{375,812}}");
        assert!(a.ends_with("bounds={{0,0},{375,812}}"));

        let b = make_message(
            &mut rng,
            "Watch connectivity message sent payload={command=sync}",
        );
        assert!(b.contains("payload={command=sync}"));

        // Real substitution works and keeps the surrounding literal text.
        let c = make_message(
            &mut rng,
            "Deep link handled url=myapp://{route}/{entity_id}",
        );
        assert!(c.starts_with("Deep link handled url=myapp://"));
        assert!(!c.contains("{entity_id}"));
    }

    #[test]
    fn only_known_levels() {
        let mut rng = Gen::new(42);
        for _ in 0..5_000 {
            let lvl = *rng.weighted(&LEVELS, &LEVEL_WEIGHTS);
            assert!(LEVELS.contains(&lvl));
        }
    }

    #[test]
    fn fault_frames_can_appear() {
        // With ~10k lines at 1% FAULT, ~90 faults * 90% * 2-4 frames, the
        // file must contain at least one raw (timestamp-less) stack frame
        // line. (We assert on content, not on the written count, because the
        // last line may be a FAULT whose frames are skipped by the
        // `written >= count` guard.)
        let p = tmp("faults.log");
        generate(&p, 10_000, &mut Gen::new(42)).unwrap();
        let content = fs::read_to_string(&p).unwrap();
        assert!(content
            .lines()
            .any(|l| l.starts_with(char::is_numeric) && l.contains("0x") && !l.contains("MyApp[")));
        cleanup(&p);
    }

    #[test]
    fn exact_count_respected() {
        let p = tmp("count1K.log");
        let n = generate(&p, 1_000, &mut Gen::new(42)).unwrap();
        assert!(n >= 1_000 && n < 1_100, "written={n}");
        cleanup(&p);
    }
}
