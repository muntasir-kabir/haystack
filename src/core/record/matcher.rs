use std::ops::Range;

use crate::core::time::TimeFormatKind;

use super::template::{CompiledField, CompiledProfile, FastPath, FieldKind, Part};

/// Allocation-free capture storage for up to 32 compiled header fields.
#[derive(Clone, Debug)]
pub struct FieldCaptures {
    spans: [Option<Range<usize>>; 32],
    len: usize,
}

impl FieldCaptures {
    fn new(len: usize) -> Self {
        Self {
            spans: std::array::from_fn(|_| None),
            len,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn span_at(&self, index: usize) -> Option<Range<usize>> {
        self.spans.get(index).and_then(Clone::clone)
    }
}

#[derive(Clone, Debug)]
pub struct TemplateMatch {
    pub captures: FieldCaptures,
    pub timestamp: Option<(i64, Range<usize>)>,
    /// Exact timestamp-shaped token, even when value conversion failed.
    pub timestamp_span: Option<Range<usize>>,
    pub header_span: Range<usize>,
    pub message_span: Range<usize>,
}

impl CompiledProfile {
    /// Match a physical line from byte zero after the explicitly permitted
    /// BOM/color/indent prefix. The returned spans always address the original
    /// bytes, including when the message contains invalid UTF-8.
    pub fn match_line(
        &self,
        line: &[u8],
        is_first_line: bool,
        time_format: Option<&TimeFormatKind>,
    ) -> Option<TemplateMatch> {
        let base = permitted_prefix(self, line, is_first_line)?;
        let limit = line
            .len()
            .min(base.saturating_add(self.profile.limits.max_header_bytes));

        if let Some(regex) = &self.advanced {
            return match_advanced(self, regex, line, base, limit, time_format);
        }
        match self.fast_path {
            FastPath::TimeMessage => match_time_message(self, line, base, limit, time_format),
            FastPath::MessageOnly => {
                let mut captures = FieldCaptures::new(self.field_count());
                captures.spans[0] = Some(base..line.len());
                Some(TemplateMatch {
                    captures,
                    timestamp: None,
                    timestamp_span: None,
                    header_span: base..base,
                    message_span: base..line.len(),
                })
            }
            FastPath::None => match_general(self, line, base, limit, time_format),
        }
    }

    pub fn match_text(
        &self,
        line: &str,
        is_first_line: bool,
        time_format: Option<&TimeFormatKind>,
    ) -> Option<TemplateMatch> {
        self.match_line(line.as_bytes(), is_first_line, time_format)
    }

    pub fn captured<'a>(&self, matched: &TemplateMatch, name: &str) -> Option<Range<usize>> {
        matched.captures.span_at(self.field_index(name)?)
    }
}

fn permitted_prefix(profile: &CompiledProfile, line: &[u8], is_first_line: bool) -> Option<usize> {
    let mut position = 0;
    if is_first_line && profile.profile.prefix.allow_file_bom && line.starts_with(b"\xef\xbb\xbf") {
        position = 3;
    }
    if profile.profile.prefix.allow_terminal_color {
        let color_limit = line.len().min(position + 128);
        while position + 3 <= color_limit && line.get(position..position + 2) == Some(b"\x1b[") {
            let mut end = position + 2;
            while end < color_limit && (line[end].is_ascii_digit() || line[end] == b';') {
                end += 1;
            }
            if end >= color_limit || line[end] != b'm' {
                break;
            }
            position = end + 1;
        }
    }
    let indentation = profile.profile.prefix.fixed_indentation.as_bytes();
    if !line.get(position..)?.starts_with(indentation) {
        return None;
    }
    Some(position + indentation.len())
}

fn match_time_message(
    profile: &CompiledProfile,
    line: &[u8],
    base: usize,
    limit: usize,
    time_format: Option<&TimeFormatKind>,
) -> Option<TemplateMatch> {
    let parser = time_format?;
    let suffix = valid_utf8_prefix(&line[base..limit])?;
    let relative = parser.recognize(suffix)?;
    if relative.start != 0 {
        return None;
    }
    let timestamp = base + relative.start..base + relative.end;
    let value = parser
        .extract(suffix)
        .filter(|(_, extracted)| *extracted == relative)
        .map(|(value, _)| value);
    let mut message_start = timestamp.end;
    if !line
        .get(message_start)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        return None;
    }
    while line
        .get(message_start)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        message_start += 1;
    }
    let mut captures = FieldCaptures::new(profile.field_count());
    captures.spans[0] = Some(timestamp.clone());
    captures.spans[1] = Some(message_start..line.len());
    Some(TemplateMatch {
        captures,
        timestamp: value.map(|value| (value, timestamp.clone())),
        timestamp_span: Some(timestamp),
        header_span: base..message_start,
        message_span: message_start..line.len(),
    })
}

fn match_general(
    profile: &CompiledProfile,
    line: &[u8],
    base: usize,
    limit: usize,
    time_format: Option<&TimeFormatKind>,
) -> Option<TemplateMatch> {
    let mut state = MatchState {
        captures: FieldCaptures::new(profile.field_count()),
        timestamp: None,
        timestamp_span: None,
    };
    let mut first = None;
    let mut matches = 0u8;
    let mut work_remaining = 65_536usize;
    walk(
        profile,
        line,
        0,
        base,
        base,
        limit,
        time_format,
        &mut state,
        &mut first,
        &mut matches,
        &mut work_remaining,
    );
    if matches == 1 && work_remaining > 0 {
        first
    } else {
        None
    }
}

#[derive(Clone)]
struct MatchState {
    captures: FieldCaptures,
    timestamp: Option<(i64, Range<usize>)>,
    timestamp_span: Option<Range<usize>>,
}

#[allow(clippy::too_many_arguments)]
fn walk(
    profile: &CompiledProfile,
    line: &[u8],
    part_index: usize,
    position: usize,
    base: usize,
    limit: usize,
    time_format: Option<&TimeFormatKind>,
    state: &mut MatchState,
    first: &mut Option<TemplateMatch>,
    matches: &mut u8,
    work_remaining: &mut usize,
) {
    if *matches > 1 || *work_remaining == 0 {
        return;
    }
    *work_remaining -= 1;
    let Some(part) = profile.parts.get(part_index) else {
        return;
    };
    match part {
        Part::Literal(literal) => {
            if let Some(next) = match_literal(line, position, limit, literal) {
                walk(
                    profile,
                    line,
                    part_index + 1,
                    next,
                    base,
                    limit,
                    time_format,
                    state,
                    first,
                    matches,
                    work_remaining,
                );
            }
        }
        Part::Field(field_index) => {
            let field = &profile.fields[*field_index];
            match field.kind {
                FieldKind::Log => {
                    if let Some(index) = field.capture_index {
                        state.captures.spans[index] = Some(position..line.len());
                    }
                    *matches = matches.saturating_add(1);
                    if first.is_none() {
                        *first = Some(TemplateMatch {
                            captures: state.captures.clone(),
                            timestamp: state.timestamp.clone(),
                            timestamp_span: state.timestamp_span.clone(),
                            header_span: base..position,
                            message_span: position..line.len(),
                        });
                    }
                    if let Some(index) = field.capture_index {
                        state.captures.spans[index] = None;
                    }
                }
                FieldKind::Time => {
                    let Some(parser) = time_format else { return };
                    let Some(suffix) = valid_utf8_prefix(&line[position..limit]) else {
                        return;
                    };
                    let Some(relative) = parser.recognize(suffix) else {
                        return;
                    };
                    if relative.start != 0 {
                        return;
                    }
                    let span = position + relative.start..position + relative.end;
                    if let Some(index) = field.capture_index {
                        state.captures.spans[index] = Some(span.clone());
                    }
                    let previous_span = state.timestamp_span.replace(span.clone());
                    let parsed = parser
                        .extract(suffix)
                        .filter(|(_, extracted)| *extracted == relative)
                        .map(|(value, _)| (value, span));
                    let previous = std::mem::replace(&mut state.timestamp, parsed);
                    walk(
                        profile,
                        line,
                        part_index + 1,
                        position + relative.end,
                        base,
                        limit,
                        time_format,
                        state,
                        first,
                        matches,
                        work_remaining,
                    );
                    state.timestamp = previous;
                    state.timestamp_span = previous_span;
                    if let Some(index) = field.capture_index {
                        state.captures.spans[index] = None;
                    }
                }
                _ => {
                    let bounded_path_with_spaces = field.kind == FieldKind::Path
                        && matches!(profile.parts.get(part_index + 1), Some(Part::Literal(literal))
                            if literal.first() == Some(&b'"')
                                || (literal.first().is_some_and(u8::is_ascii_whitespace)
                                    && literal.iter().any(|byte| !byte.is_ascii_whitespace())));
                    for end in
                        candidate_ends(line, position, limit, field, bounded_path_with_spaces)
                    {
                        if *work_remaining == 0 {
                            return;
                        }
                        *work_remaining -= 1;
                        if !valid_field(field, &line[position..end]) {
                            continue;
                        }
                        if let Some(index) = field.capture_index {
                            state.captures.spans[index] = Some(position..end);
                        }
                        walk(
                            profile,
                            line,
                            part_index + 1,
                            end,
                            base,
                            limit,
                            time_format,
                            state,
                            first,
                            matches,
                            work_remaining,
                        );
                        if let Some(index) = field.capture_index {
                            state.captures.spans[index] = None;
                        }
                        if *matches > 1 {
                            return;
                        }
                    }
                }
            }
        }
    }
}

fn candidate_ends<'a>(
    line: &'a [u8],
    start: usize,
    limit: usize,
    field: &'a CompiledField,
    bounded_path_with_spaces: bool,
) -> impl Iterator<Item = usize> + 'a {
    let natural_end = match field.kind {
        FieldKind::Token | FieldKind::Number => line[start..limit]
            .iter()
            .position(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
            .map_or(limit, |offset| start + offset),
        FieldKind::Path if !bounded_path_with_spaces => line[start..limit]
            .iter()
            .position(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
            .map_or(limit, |offset| start + offset),
        _ => limit,
    };
    (start + 1..=natural_end).rev()
}

fn valid_field(field: &CompiledField, value: &[u8]) -> bool {
    if value.is_empty() {
        return false;
    }
    let kind_valid = match field.kind {
        FieldKind::Token => !value
            .iter()
            .any(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n')),
        FieldKind::Number => valid_number(value),
        FieldKind::Path => valid_path(value),
        FieldKind::Text => {
            std::str::from_utf8(value).is_ok()
                && !value.iter().any(|byte| matches!(byte, b'\r' | b'\n' | 0))
        }
        FieldKind::DelimitedText => true,
        FieldKind::Time | FieldKind::Log => unreachable!(),
    };
    kind_valid
        && field.validator.as_ref().is_none_or(|validator| {
            std::str::from_utf8(value)
                .ok()
                .is_some_and(|text| validator.is_match(text))
        })
}

fn valid_number(value: &[u8]) -> bool {
    let mut cursor = 0;
    if value
        .first()
        .is_some_and(|byte| matches!(byte, b'+' | b'-'))
    {
        cursor += 1;
    }
    let integral_start = cursor;
    while value.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
    }
    let mut digits = cursor - integral_start;
    if value.get(cursor) == Some(&b'.') {
        cursor += 1;
        let decimal_start = cursor;
        while value.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        digits += cursor - decimal_start;
    }
    if digits == 0 {
        return false;
    }
    if value
        .get(cursor)
        .is_some_and(|byte| matches!(byte, b'e' | b'E'))
    {
        cursor += 1;
        if value
            .get(cursor)
            .is_some_and(|byte| matches!(byte, b'+' | b'-'))
        {
            cursor += 1;
        }
        let exponent_start = cursor;
        while value.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if cursor == exponent_start {
            return false;
        }
    }
    cursor == value.len()
}

fn valid_path(value: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(value) else {
        return false;
    };
    !text.chars().any(char::is_control)
        && !text.contains('"')
        && (!(text.starts_with("https://") || text.starts_with("http://"))
            || !text.chars().any(char::is_whitespace))
        && (text.starts_with('/')
            || text.starts_with(".\\")
            || text.starts_with("./")
            || text.starts_with("..\\")
            || text.starts_with("../")
            || text.starts_with("https://")
            || text.starts_with("http://")
            || text.contains('/')
            || text.contains('\\')
            || text.contains('.'))
}

fn match_literal(line: &[u8], mut position: usize, limit: usize, literal: &[u8]) -> Option<usize> {
    let mut literal_position = 0;
    while literal_position < literal.len() {
        if matches!(literal[literal_position], b' ' | b'\t') {
            while literal_position < literal.len()
                && matches!(literal[literal_position], b' ' | b'\t')
            {
                literal_position += 1;
            }
            if position >= limit || !matches!(line[position], b' ' | b'\t') {
                return None;
            }
            while position < limit && matches!(line[position], b' ' | b'\t') {
                position += 1;
            }
        } else {
            if position >= limit || line[position] != literal[literal_position] {
                return None;
            }
            position += 1;
            literal_position += 1;
        }
    }
    Some(position)
}

fn valid_utf8_prefix(bytes: &[u8]) -> Option<&str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(error) if error.valid_up_to() > 0 => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).ok()
        }
        Err(_) => None,
    }
}

fn match_advanced(
    profile: &CompiledProfile,
    regex: &regex::Regex,
    line: &[u8],
    base: usize,
    limit: usize,
    time_format: Option<&TimeFormatKind>,
) -> Option<TemplateMatch> {
    let text = valid_utf8_prefix(&line[base..limit])?;
    let captures_match = regex.captures(text)?;
    let whole = captures_match.get(0)?;
    if whole.start() != 0 {
        return None;
    }
    let mut captures = FieldCaptures::new(profile.field_count());
    for field in &profile.fields {
        if let (Some(name), Some(capture_index)) = (&field.name, field.capture_index) {
            if let Some(value) = captures_match.name(name) {
                captures.spans[capture_index] = Some(base + value.start()..base + value.end());
            }
        }
    }
    let timestamp_capture = captures_match.name("timestamp");
    let timestamp_span = timestamp_capture
        .as_ref()
        .map(|value| base + value.start()..base + value.end());
    let timestamp = timestamp_capture.and_then(|value| {
        let parser = time_format?;
        let captured = value.as_str();
        let (timestamp, relative) = parser.extract(captured)?;
        (relative.start == 0 && relative.end == captured.len())
            .then_some((timestamp, base + value.start()..base + value.end()))
    });
    let message_span = captures_match
        .name("log")
        .map(|value| base + value.start()..base + value.end())
        .unwrap_or(base + whole.end()..line.len());
    Some(TemplateMatch {
        captures,
        timestamp,
        timestamp_span,
        header_span: base..message_span.start,
        message_span,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::record::{PrefixPolicy, ProfileSource, RecordProfile};
    use crate::core::time::Iso;

    fn iso() -> TimeFormatKind {
        TimeFormatKind::BuiltIn(&Iso)
    }

    #[test]
    fn default_match_is_anchored_and_does_not_search_payload() {
        let profile = CompiledProfile::compile(RecordProfile::default_text()).unwrap();
        let parser = iso();
        let matched = profile
            .match_text("2026-09-11 10:00:00.100 ERROR failed", false, Some(&parser))
            .unwrap();
        assert_eq!(
            &"2026-09-11 10:00:00.100 ERROR failed"[matched.timestamp.unwrap().1],
            "2026-09-11 10:00:00.100"
        );
        assert_eq!(
            &"2026-09-11 10:00:00.100 ERROR failed"[matched.message_span],
            "ERROR failed"
        );
        assert!(profile
            .match_text(
                "    payload retry_at=2035-01-01T00:00:00Z",
                false,
                Some(&parser)
            )
            .is_none());
    }

    #[test]
    fn detailed_template_captures_windows_path_and_fields() {
        let profile = CompiledProfile::compile(RecordProfile::text(
            "custom:detailed",
            "Detailed",
            "{time} - [{log_level}] - {thread_id} {file:path}:{line:number} {log}",
        ))
        .unwrap();
        let parser = iso();
        let line =
            "2026-09-11 10:00:00.100 - [ERROR] - worker-7 C:\\src\\Handler.rs:42 request failed";
        let matched = profile.match_text(line, false, Some(&parser)).unwrap();
        assert_eq!(
            &line[profile.captured(&matched, "log_level").unwrap()],
            "ERROR"
        );
        assert_eq!(
            &line[profile.captured(&matched, "thread_id").unwrap()],
            "worker-7"
        );
        assert_eq!(
            &line[profile.captured(&matched, "file").unwrap()],
            "C:\\src\\Handler.rs"
        );
        assert_eq!(&line[profile.captured(&matched, "line").unwrap()], "42");
        assert_eq!(&line[matched.message_span], "request failed");
    }

    #[test]
    fn supports_reordered_fields_empty_messages_and_flexible_inner_space() {
        let mut definition = RecordProfile::text(
            "custom:reordered",
            "Reordered",
            "[{log_level}] {time} {log}",
        );
        definition.log_levels.values.push("VERBOSE".into());
        definition.log_levels.case_sensitive = false;
        let profile = CompiledProfile::compile(definition).unwrap();
        let parser = iso();
        let line = "[verbose]\t\t2026-09-11T10:00:00Z   ";
        let matched = profile.match_text(line, false, Some(&parser)).unwrap();
        assert_eq!(matched.message_span, line.len()..line.len());
    }

    #[test]
    fn inline_number_field_rejects_nonmatching_tokens() {
        let definition = RecordProfile::text(
            "custom:req",
            "Request",
            "{time} {request_id:number} | {log}",
        );
        let profile = CompiledProfile::compile(definition).unwrap();
        let parser = iso();
        assert!(profile
            .match_text("2026-09-11T10:00:00Z 42 | accepted", false, Some(&parser))
            .is_some());
        assert!(profile
            .match_text(
                "2026-09-11T10:00:00Z user-42 | rejected",
                false,
                Some(&parser)
            )
            .is_none());
    }

    #[test]
    fn permitted_prefix_retains_original_byte_offsets_and_bounds_work() {
        let mut definition = RecordProfile::default_text();
        definition.prefix = PrefixPolicy {
            fixed_indentation: "  ".into(),
            ..PrefixPolicy::default()
        };
        let profile = CompiledProfile::compile(definition).unwrap();
        let parser = iso();
        let mut line = b"\xef\xbb\xbf\x1b[31m  2026-09-11T10:00:00Z hello".to_vec();
        line.extend_from_slice(&[0xff, 0xfe]);
        let matched = profile.match_line(&line, true, Some(&parser)).unwrap();
        let timestamp = matched.timestamp.unwrap().1;
        assert_eq!(&line[timestamp], b"2026-09-11T10:00:00Z");

        let mut too_short = RecordProfile::default_text();
        too_short.limits.max_header_bytes = 10;
        let too_short = CompiledProfile::compile(too_short).unwrap();
        assert!(too_short.match_line(&line, true, Some(&parser)).is_none());
    }

    #[test]
    fn message_only_is_rejected_by_the_current_grammar() {
        let error =
            CompiledProfile::compile(RecordProfile::text("custom:single", "Single line", "{log}"))
                .err()
                .unwrap();
        assert!(error.to_string().contains("add {time}"));
    }

    #[test]
    fn advanced_match_is_anchored_and_uses_named_timestamp() {
        let mut definition = RecordProfile::default_text();
        definition.source = ProfileSource::AdvancedRegex {
            pattern: r"\A\[worker-\d+\] INFO (?P<timestamp>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z) "
                .into(),
        };
        let profile = CompiledProfile::compile(definition).unwrap();
        let parser = iso();
        let line = "[worker-7] INFO 2026-09-11T10:00:00Z started";
        let matched = profile.match_text(line, false, Some(&parser)).unwrap();
        assert_eq!(&line[matched.timestamp.unwrap().1], "2026-09-11T10:00:00Z");
        assert_eq!(&line[matched.message_span], "started");
        assert!(profile
            .match_text(
                "prefix [worker-7] INFO 2026-09-11T10:00:00Z started",
                false,
                Some(&parser)
            )
            .is_none());
    }
}
