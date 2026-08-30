use super::{detection, line_ranges};
use crate::core::embedded_data::parse::{
    balanced_end, find_top_level, scalar, split_top_level, unquote,
};
use crate::core::embedded_data::{AnalysisLimits, DataDetector, DataNode, Detection, ScanWindow};
use std::sync::atomic::{AtomicBool, Ordering};

pub const ID: &str = "foundation";
pub(super) struct FoundationDetector;

impl DataDetector for FoundationDetector {
    fn id(&self) -> &'static str {
        ID
    }

    fn detect(
        &self,
        window: &ScanWindow,
        limits: &AnalysisLimits,
        cancel: &AtomicBool,
    ) -> Vec<Detection> {
        let mut out = detect_swift_dumps(window, limits, cancel);
        let bytes = &window.bytes;
        for start in 0..bytes.len() {
            if out.len() >= limits.max_results {
                break;
            }
            if start % 4096 == 0 && cancel.load(Ordering::Relaxed) {
                break;
            }
            let Some((candidate_start, opener)) = candidate_at(bytes, start) else {
                continue;
            };
            let Some(end) = balanced_end(bytes, opener, limits.max_depth) else {
                continue;
            };
            let Ok(raw) = std::str::from_utf8(&bytes[candidate_start..end]) else {
                continue;
            };
            let Some(data) = parse_apple_value(raw, 0, limits.max_depth) else {
                continue;
            };
            if !within_node_limit(&data, limits.max_nodes) {
                continue;
            }
            if let Some(value) = detection(window, self.id(), candidate_start, end, data) {
                out.push(value);
            }
            if out.len() >= limits.max_results {
                break;
            }
        }
        out
    }
}

fn candidate_at(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    match *bytes.get(start)? {
        b'{' | b'(' | b'[' | b'<' => Some((start, start)),
        b'.' if bytes.get(start + 1).is_some_and(u8::is_ascii_alphabetic) => {
            typed_candidate(bytes, start)
        }
        byte if byte.is_ascii_alphabetic() || byte == b'_' => {
            if start > 0 && is_swift_name_byte(bytes[start - 1]) {
                return None;
            }
            typed_candidate(bytes, start)
        }
        _ => None,
    }
}

fn typed_candidate(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    let mut cursor = start;
    while bytes
        .get(cursor)
        .is_some_and(|byte| is_swift_name_byte(*byte))
    {
        cursor += 1;
    }
    (bytes.get(cursor) == Some(&b'(')).then_some((start, cursor))
}

fn is_swift_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.')
}

fn parse_apple_value(input: &str, depth: usize, max_depth: usize) -> Option<DataNode> {
    if depth > max_depth {
        return None;
    }
    let value = input.trim();
    match value.as_bytes().first().copied()? {
        b'{' => parse_foundation_dictionary(value, depth, max_depth),
        b'(' => parse_foundation_array(value, depth, max_depth),
        b'[' => parse_swift_brackets(value, depth, max_depth),
        b'<' => parse_nsobject_description(value, depth, max_depth),
        _ => parse_swift_typed(value, depth, max_depth),
    }
}

fn parse_foundation_dictionary(value: &str, depth: usize, max_depth: usize) -> Option<DataNode> {
    if !value.contains(';') {
        return None;
    }
    let body = enclosed_body(value, '{', '}')?;
    let pieces = nonempty_pieces(body, &[';', ',']);
    if pieces.is_empty()
        || !pieces
            .iter()
            .all(|piece| find_top_level(piece, &['=']).is_some())
    {
        return None;
    }
    keyed_object(pieces, '=', depth, max_depth, None)
}

fn parse_foundation_array(value: &str, depth: usize, max_depth: usize) -> Option<DataNode> {
    let body = enclosed_body(value, '(', ')')?;
    let pieces = nonempty_pieces(body, &[',']);
    // Single-line `(foo, bar)` is common prose and function syntax. The
    // multiline form is the stable NSArray/`po` description shape; a
    // single-line form needs an unmistakably structured element.
    if pieces.len() < 2
        || (!value.contains('\n') && !pieces.iter().any(|piece| has_swift_value_signal(piece)))
    {
        return None;
    }
    if pieces
        .iter()
        .all(|piece| find_top_level(piece, &[':']).is_some())
    {
        return keyed_object(pieces, ':', depth, max_depth, None);
    }
    Some(DataNode::Array(
        pieces
            .into_iter()
            .map(|piece| parse_apple_nested(piece, depth + 1, max_depth))
            .collect(),
    ))
}

pub(super) fn parse_openstep_plist(input: &str, max_depth: usize) -> Option<DataNode> {
    let value = input.trim();
    match value.as_bytes().first().copied()? {
        b'{' => parse_openstep_dictionary(value, 0, max_depth),
        b'(' => parse_openstep_array(value, 0, max_depth),
        _ => None,
    }
}

fn parse_openstep_dictionary(value: &str, depth: usize, max_depth: usize) -> Option<DataNode> {
    if depth > max_depth {
        return None;
    }
    let body = enclosed_body(value, '{', '}')?;
    if body.trim().is_empty() {
        return Some(DataNode::Object(Vec::new()));
    }
    if !body.contains(';') {
        return None;
    }
    let pieces = nonempty_pieces(body, &[';']);
    let mut entries = Vec::with_capacity(pieces.len());
    for piece in pieces {
        let (offset, separator) = find_top_level(piece, &['='])?;
        let raw_key = piece[..offset].trim();
        if raw_key.is_empty() {
            return None;
        }
        let key = unquote(raw_key).unwrap_or_else(|| raw_key.to_owned());
        entries.push((
            key,
            parse_openstep_nested(
                piece[offset + separator.len_utf8()..].trim(),
                depth + 1,
                max_depth,
            )?,
        ));
    }
    Some(DataNode::Object(entries))
}

fn parse_openstep_array(value: &str, depth: usize, max_depth: usize) -> Option<DataNode> {
    if depth > max_depth {
        return None;
    }
    let body = enclosed_body(value, '(', ')')?;
    let pieces = nonempty_pieces(body, &[',']);
    Some(DataNode::Array(
        pieces
            .into_iter()
            .map(|piece| parse_openstep_nested(piece.trim(), depth + 1, max_depth))
            .collect::<Option<Vec<_>>>()?,
    ))
}

fn parse_openstep_nested(value: &str, depth: usize, max_depth: usize) -> Option<DataNode> {
    if depth > max_depth {
        return None;
    }
    if value.eq_ignore_ascii_case("YES") {
        return Some(DataNode::Bool(true));
    }
    if value.eq_ignore_ascii_case("NO") {
        return Some(DataNode::Bool(false));
    }
    match value.as_bytes().first().copied() {
        Some(b'{') => parse_openstep_dictionary(value, depth, max_depth),
        Some(b'(') => parse_openstep_array(value, depth, max_depth),
        _ => Some(scalar(value)),
    }
}

fn parse_swift_brackets(value: &str, depth: usize, max_depth: usize) -> Option<DataNode> {
    let body = enclosed_body(value, '[', ']')?;
    let pieces = nonempty_pieces(body, &[',']);
    if pieces.is_empty() {
        return None;
    }
    let keyed = pieces
        .iter()
        .all(|piece| find_top_level(piece, &[':']).is_some());
    if keyed {
        let (first_offset, _) = find_top_level(pieces[0], &[':'])?;
        let first_key = pieces[0][..first_offset].trim();
        // `[UI:Navigation]` and similar log tags are not Swift dictionaries.
        if pieces.len() == 1 && unquote(first_key).is_none() {
            return None;
        }
        return keyed_object(pieces, ':', depth, max_depth, None);
    }
    if pieces.len() < 2
        || (!value.contains('\n') && !pieces.iter().any(|piece| has_swift_value_signal(piece)))
    {
        return None;
    }
    Some(DataNode::Array(
        pieces
            .into_iter()
            .map(|piece| parse_apple_nested(piece, depth + 1, max_depth))
            .collect(),
    ))
}

fn parse_swift_typed(value: &str, depth: usize, max_depth: usize) -> Option<DataNode> {
    let opener = value.find('(')?;
    if !value.ends_with(')') {
        return None;
    }
    let name = value[..opener].trim();
    if !valid_swift_type_or_case(name) {
        return None;
    }
    let body = &value[opener + 1..value.len() - 1];
    if name == "Optional" {
        if body.trim().is_empty() {
            return None;
        }
        return Some(DataNode::Object(vec![
            ("$type".to_owned(), DataNode::String("Optional".to_owned())),
            (
                "value".to_owned(),
                parse_apple_nested(body, depth + 1, max_depth),
            ),
        ]));
    }

    let pieces = nonempty_pieces(body, &[',']);
    if pieces.is_empty() {
        return None;
    }
    let is_case = swift_case_name(name);
    let metadata = if is_case { "$case" } else { "$type" };
    let all_keyed = pieces
        .iter()
        .all(|piece| find_top_level(piece, &[':']).is_some());
    if all_keyed {
        return keyed_object(pieces, ':', depth, max_depth, Some((metadata, name)));
    }
    // Associated values may be unlabeled (`.success(value)`). Ordinary
    // function calls and unlabeled constructors remain unclaimed.
    if is_case && pieces.len() == 1 {
        return Some(DataNode::Object(vec![
            (metadata.to_owned(), DataNode::String(name.to_owned())),
            (
                "$value".to_owned(),
                parse_apple_nested(pieces[0], depth + 1, max_depth),
            ),
        ]));
    }
    None
}

fn parse_nsobject_description(value: &str, depth: usize, max_depth: usize) -> Option<DataNode> {
    let body = enclosed_body(value, '<', '>')?;
    let pieces = nonempty_pieces(body, &[';']);
    if pieces.len() < 2 {
        return None;
    }
    let (type_name, address) = pieces[0].split_once(':')?;
    let mut entries = vec![
        (
            "$type".to_owned(),
            DataNode::String(type_name.trim().to_owned()),
        ),
        (
            "$address".to_owned(),
            DataNode::String(address.trim().to_owned()),
        ),
    ];
    for piece in pieces.into_iter().skip(1) {
        let (offset, separator) = find_top_level(piece, &['='])?;
        let key = piece[..offset].trim();
        if key.is_empty() {
            return None;
        }
        entries.push((
            key.to_owned(),
            parse_apple_nested(
                piece[offset + separator.len_utf8()..].trim(),
                depth + 1,
                max_depth,
            ),
        ));
    }
    Some(DataNode::Object(entries))
}

fn keyed_object(
    pieces: Vec<&str>,
    key_separator: char,
    depth: usize,
    max_depth: usize,
    metadata: Option<(&str, &str)>,
) -> Option<DataNode> {
    let mut entries = Vec::with_capacity(pieces.len() + usize::from(metadata.is_some()));
    if let Some((key, value)) = metadata {
        entries.push((key.to_owned(), DataNode::String(value.to_owned())));
    }
    for piece in pieces {
        let (offset, separator) = find_top_level(piece, &[key_separator])?;
        let raw_key = piece[..offset].trim();
        if raw_key.is_empty() {
            return None;
        }
        let key = unquote(raw_key).unwrap_or_else(|| raw_key.to_owned());
        entries.push((
            key,
            parse_apple_nested(
                piece[offset + separator.len_utf8()..].trim(),
                depth + 1,
                max_depth,
            ),
        ));
    }
    Some(DataNode::Object(entries))
}

fn parse_apple_nested(value: &str, depth: usize, max_depth: usize) -> DataNode {
    let value = value.trim();
    if matches!(value, "Optional.none" | ".none") {
        return DataNode::Null;
    }
    parse_apple_value(value, depth, max_depth).unwrap_or_else(|| scalar(value))
}

fn enclosed_body(value: &str, open: char, close: char) -> Option<&str> {
    (value.starts_with(open) && value.ends_with(close))
        .then(|| &value[open.len_utf8()..value.len() - close.len_utf8()])
}

fn nonempty_pieces<'a>(body: &'a str, separators: &[char]) -> Vec<&'a str> {
    split_top_level(body, separators)
        .into_iter()
        .filter(|piece| !piece.is_empty())
        .collect()
}

fn valid_swift_type_or_case(name: &str) -> bool {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
    {
        return false;
    }
    name == "Optional"
        || name.starts_with('.')
        || name.contains('.')
        || name.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
}

fn swift_case_name(name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    name.rsplit('.').next().is_some_and(|component| {
        component
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_lowercase)
    })
}

fn has_swift_value_signal(value: &str) -> bool {
    let value = value.trim();
    if let Some((offset, separator)) = find_top_level(value, &[':', '=']) {
        return has_swift_value_signal(value[offset + separator.len_utf8()..].trim());
    }
    unquote(value).is_some()
        || value.parse::<f64>().is_ok()
        || matches!(value, "true" | "false" | "nil" | "Optional.none" | ".none")
        || value.contains(['(', '[', '{'])
}

fn within_node_limit(root: &DataNode, max_nodes: usize) -> bool {
    let mut seen = 0usize;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        seen += 1;
        if seen > max_nodes {
            return false;
        }
        match node {
            DataNode::Object(entries) => {
                stack.extend(entries.iter().map(|(_, value)| value));
            }
            DataNode::Array(items) => stack.extend(items),
            DataNode::String(_) | DataNode::Number(_) | DataNode::Bool(_) | DataNode::Null => {}
        }
    }
    true
}

fn detect_swift_dumps(
    window: &ScanWindow,
    limits: &AnalysisLimits,
    cancel: &AtomicBool,
) -> Vec<Detection> {
    if limits.max_results == 0 {
        return Vec::new();
    }
    let ranges = line_ranges(&window.bytes).collect::<Vec<_>>();
    let mut out = Vec::new();
    for (index, &(line_start, line_end)) in ranges.iter().enumerate() {
        if index % 256 == 0 && cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(line) = std::str::from_utf8(&window.bytes[line_start..line_end]) else {
            continue;
        };
        let Some(marker_offset) = line.find('▿') else {
            continue;
        };
        if index > 0 {
            let (previous_start, previous_end) = ranges[index - 1];
            if std::str::from_utf8(&window.bytes[previous_start..previous_end])
                .ok()
                .is_some_and(is_swift_dump_line)
            {
                continue;
            }
        }
        let start = line_start + marker_offset;
        let mut last = index;
        while last + 1 < ranges.len() {
            let (next_start, next_end) = ranges[last + 1];
            let Ok(next) = std::str::from_utf8(&window.bytes[next_start..next_end]) else {
                break;
            };
            if !is_swift_dump_line(next) {
                break;
            }
            last += 1;
        }
        if last == index {
            continue;
        }
        let end = ranges[last].1;
        let Ok(raw) = std::str::from_utf8(&window.bytes[start..end]) else {
            continue;
        };
        let Some(data) = parse_swift_dump(raw, limits.max_depth) else {
            continue;
        };
        if !within_node_limit(&data, limits.max_nodes) {
            continue;
        }
        if let Some(value) = detection(window, ID, start, end, data) {
            out.push(value);
        }
        if out.len() >= limits.max_results {
            break;
        }
    }
    out
}

fn is_swift_dump_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("▿ ") || trimmed.starts_with("- ") || trimmed.starts_with("• ")
}

struct DumpFrame {
    indent: usize,
    key: Option<String>,
    entries: Vec<(String, DataNode)>,
    next_index: usize,
}

fn parse_swift_dump(raw: &str, max_depth: usize) -> Option<DataNode> {
    let mut lines = raw.lines();
    let root = lines.next()?.trim_start().strip_prefix('▿')?.trim();
    if root.is_empty() {
        return None;
    }
    let mut frames = vec![DumpFrame {
        indent: 0,
        key: None,
        entries: vec![("$type".to_owned(), DataNode::String(root.to_owned()))],
        next_index: 0,
    }];
    for line in lines {
        let trimmed = line.trim_start();
        let indent = line.len().saturating_sub(trimmed.len());
        let (expanded, content) = if let Some(content) = trimmed.strip_prefix("▿ ") {
            (true, content.trim())
        } else if let Some(content) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("• "))
        {
            (false, content.trim())
        } else {
            continue;
        };
        while frames.len() > 1 && indent <= frames.last()?.indent {
            finish_dump_frame(&mut frames)?;
        }
        if expanded {
            if frames.len() > max_depth {
                return None;
            }
            let (key, type_name) = dump_field(content).unwrap_or_else(|| {
                let index = frames.last().map_or(0, |frame| frame.next_index);
                (format!("${index}"), content)
            });
            if let Some(parent) = frames.last_mut() {
                parent.next_index += 1;
            }
            frames.push(DumpFrame {
                indent,
                key: Some(key),
                entries: vec![("$type".to_owned(), DataNode::String(type_name.to_owned()))],
                next_index: 0,
            });
        } else {
            let current_depth = frames.len();
            let parent = frames.last_mut()?;
            let (key, value) = dump_field(content).unwrap_or_else(|| {
                let key = format!("${}", parent.next_index);
                (key, content)
            });
            parent.next_index += 1;
            parent
                .entries
                .push((key, parse_apple_nested(value, current_depth, max_depth)));
        }
    }
    while frames.len() > 1 {
        finish_dump_frame(&mut frames)?;
    }
    Some(DataNode::Object(frames.pop()?.entries))
}

fn dump_field(content: &str) -> Option<(String, &str)> {
    let (key, value) = content.split_once(':')?;
    let key = key.trim();
    (!key.is_empty()).then(|| (key.to_owned(), value.trim()))
}

fn finish_dump_frame(frames: &mut Vec<DumpFrame>) -> Option<()> {
    let child = frames.pop()?;
    let key = child.key?;
    frames
        .last_mut()?
        .entries
        .push((key, DataNode::Object(child.entries)));
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(input: &str) -> Vec<(String, DataNode)> {
        match parse_apple_value(input, 0, 16).expect("Apple value") {
            DataNode::Object(entries) => entries,
            other => panic!("expected object, got {other:?}"),
        }
    }

    #[test]
    fn parses_foundation_dictionary() {
        assert!(matches!(
            parse_apple_value("{ user = Ada; ok = true; }", 0, 8),
            Some(DataNode::Object(_))
        ));
    }

    #[test]
    fn parses_swift_struct_and_optional_recursively() {
        let entries =
            object("User(id: 42, name: \"Ada\", profile: Optional(Profile(active: true)))");
        assert_eq!(
            entries[0],
            ("$type".to_owned(), DataNode::String("User".to_owned()))
        );
        assert!(matches!(entries[3].1, DataNode::Object(_)));
    }

    #[test]
    fn parses_swift_enum_associated_value() {
        let entries = object("Result.failure(error: NetworkError(code: 401))");
        assert_eq!(
            entries[0],
            (
                "$case".to_owned(),
                DataNode::String("Result.failure".to_owned())
            )
        );
    }

    #[test]
    fn parses_swift_dictionary_and_nsobject_description() {
        assert!(matches!(
            parse_apple_value("[\"user\": \"Ada\", \"ok\": true]", 0, 8),
            Some(DataNode::Object(_))
        ));
        let entries = object("<User: 0x123; name = Ada; active = true>");
        assert_eq!(entries[0].0, "$type");
        assert_eq!(entries[2].0, "name");
    }

    #[test]
    fn parses_labeled_swift_tuple_and_nested_dump_output() {
        assert!(matches!(
            parse_apple_value("(x: 10, y: 20)", 0, 8),
            Some(DataNode::Object(_))
        ));
        let dump = "▿ User\n  - id: 42\n  ▿ profile: Profile\n    - active: true";
        let entries = match parse_swift_dump(dump, 8).unwrap() {
            DataNode::Object(entries) => entries,
            other => panic!("expected dump object, got {other:?}"),
        };
        assert_eq!(entries[0].1, DataNode::String("User".to_owned()));
        assert!(matches!(entries[2].1, DataNode::Object(_)));
    }

    #[test]
    fn rejects_ordinary_parenthetical_prose_and_log_tags() {
        assert!(parse_apple_value("(foo, bar)", 0, 8).is_none());
        assert!(parse_apple_value("[UI:Navigation]", 0, 8).is_none());
    }
}
