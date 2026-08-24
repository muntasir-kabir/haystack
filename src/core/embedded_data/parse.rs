//! Small bounded parsers shared by the non-JSON syntax profiles. These never
//! execute input and intentionally accept less than a language runtime.

use super::DataNode;

pub(crate) fn balanced_end(bytes: &[u8], start: usize, max_depth: usize) -> Option<usize> {
    let mut stack = Vec::with_capacity(8);
    let mut quote = None;
    let mut escaped = false;
    for (offset, &byte) in bytes.get(start..)?.iter().enumerate() {
        if let Some(current) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == current {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'\"' => quote = Some(byte),
            b'{' => stack.push(b'}'),
            b'[' => stack.push(b']'),
            b'(' => stack.push(b')'),
            b'<' => stack.push(b'>'),
            b'}' | b']' | b')' | b'>' => {
                if stack.pop()? != byte {
                    return None;
                }
            }
            _ => {}
        }
        if stack.len() > max_depth {
            return None;
        }
        if stack.is_empty() {
            return Some(start + offset + 1);
        }
    }
    None
}

pub(crate) fn split_top_level<'a>(input: &'a str, delimiters: &[char]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut stack = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if let Some(current) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == current {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '\"' => quote = Some(ch),
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '(' => stack.push(')'),
            '<' => stack.push('>'),
            '}' | ']' | ')' | '>' => {
                if stack.last() == Some(&ch) {
                    stack.pop();
                }
            }
            _ if stack.is_empty() && delimiters.contains(&ch) => {
                out.push(input[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    out.push(input[start..].trim());
    out
}

pub(crate) fn find_top_level(input: &str, separators: &[char]) -> Option<(usize, char)> {
    let mut stack = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if let Some(current) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == current {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '\"' => quote = Some(ch),
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '(' => stack.push(')'),
            '<' => stack.push('>'),
            '}' | ']' | ')' | '>' => {
                if stack.last() == Some(&ch) {
                    stack.pop();
                }
            }
            _ if stack.is_empty() && separators.contains(&ch) => return Some((index, ch)),
            _ => {}
        }
    }
    None
}

pub(crate) fn scalar(input: &str) -> DataNode {
    let value = input.trim();
    if value.eq_ignore_ascii_case("true") {
        return DataNode::Bool(true);
    }
    if value.eq_ignore_ascii_case("false") {
        return DataNode::Bool(false);
    }
    if matches!(value, "null" | "None" | "nil" | "NULL" | "<null>") {
        return DataNode::Null;
    }
    if looks_number(value) {
        return DataNode::Number(value.to_owned());
    }
    if let Some(text) = unquote(value) {
        return DataNode::String(text);
    }
    DataNode::String(value.to_owned())
}

pub(crate) fn parse_collection(
    input: &str,
    key_separators: &[char],
    item_separators: &[char],
    depth: usize,
    max_depth: usize,
) -> Option<DataNode> {
    if depth > max_depth {
        return None;
    }
    let value = input.trim();
    let (open, close) = (value.chars().next()?, value.chars().last()?);
    let body = match (open, close) {
        ('{', '}') | ('[', ']') | ('(', ')') | ('<', '>') => {
            &value[open.len_utf8()..value.len() - close.len_utf8()]
        }
        _ => return None,
    };
    let pieces = split_top_level(body, item_separators)
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let object = open == '{'
        && pieces
            .iter()
            .any(|part| find_top_level(part, key_separators).is_some());
    if object {
        let mut entries = Vec::new();
        for piece in pieces {
            let (offset, delimiter) = find_top_level(piece, key_separators)?;
            let key = unquote(piece[..offset].trim())
                .unwrap_or_else(|| piece[..offset].trim().to_owned());
            let raw_value = piece[offset + delimiter.len_utf8()..].trim();
            entries.push((
                key,
                nested(
                    raw_value,
                    key_separators,
                    item_separators,
                    depth + 1,
                    max_depth,
                ),
            ));
        }
        Some(DataNode::Object(entries))
    } else {
        Some(DataNode::Array(
            pieces
                .into_iter()
                .map(|piece| nested(piece, key_separators, item_separators, depth + 1, max_depth))
                .collect(),
        ))
    }
}

pub(crate) fn nested(
    input: &str,
    key_separators: &[char],
    item_separators: &[char],
    depth: usize,
    max_depth: usize,
) -> DataNode {
    parse_collection(input, key_separators, item_separators, depth, max_depth)
        .unwrap_or_else(|| scalar(input))
}

pub(crate) fn unquote(value: &str) -> Option<String> {
    let value = value.trim();
    let quote = value.chars().next()?;
    if !matches!(quote, '\'' | '\"') || !value.ends_with(quote) || value.len() < 2 {
        return None;
    }
    let body = &value[1..value.len() - 1];
    let mut out = String::with_capacity(body.len());
    let mut escaped = false;
    for ch in body.chars() {
        if escaped {
            out.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            out.push(ch);
        }
    }
    if escaped {
        return None;
    }
    Some(out)
}

fn looks_number(value: &str) -> bool {
    let mut chars = value.chars();
    let first = chars.next();
    matches!(first, Some('-' | '+') if chars.clone().next().is_some()) || first.is_some_and(|ch| ch.is_ascii_digit())
        && value.chars().all(|ch| ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.' | '_' | 'e' | 'E' | 'x' | 'X' | 'a'..='f' | 'A'..='F'))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_keeps_nested_and_quoted_values() {
        assert_eq!(
            split_top_level("a=1, b={x: 'y,z'}", &[',']),
            ["a=1", "b={x: 'y,z'}"]
        );
    }
    #[test]
    fn collection_builds_nested_object() {
        assert!(matches!(
            parse_collection("{ user = 42; ok = true; }", &['='], &[';', ','], 0, 8),
            Some(DataNode::Object(_))
        ));
    }
}
