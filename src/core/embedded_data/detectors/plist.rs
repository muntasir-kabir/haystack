use super::foundation::parse_openstep_plist;
use super::{detection, line_ranges};
use crate::core::embedded_data::model::node_allowed;
use crate::core::embedded_data::parse::balanced_end;
use crate::core::embedded_data::{AnalysisLimits, DataDetector, DataNode, Detection, ScanWindow};
use std::sync::atomic::{AtomicBool, Ordering};

pub const ID: &str = "plist";
pub const BINARY_ID: &str = "binary-plist";
pub(super) struct PlistDetector;

impl DataDetector for PlistDetector {
    fn id(&self) -> &'static str {
        ID
    }

    fn detect(
        &self,
        window: &ScanWindow,
        limits: &AnalysisLimits,
        cancel: &AtomicBool,
    ) -> Vec<Detection> {
        if limits.max_results == 0 {
            return Vec::new();
        }
        let mut out = detect_xml(window, limits, cancel);
        if out.len() < limits.max_results && !cancel.load(Ordering::Relaxed) {
            detect_labeled_openstep(window, limits, cancel, &mut out);
        }
        if out.len() < limits.max_results && !cancel.load(Ordering::Relaxed) {
            detect_binary_representations(window, limits, cancel, &mut out);
        }
        out
    }
}

fn detect_xml(window: &ScanWindow, limits: &AnalysisLimits, cancel: &AtomicBool) -> Vec<Detection> {
    let bytes = &window.bytes;
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() && out.len() < limits.max_results {
        if cursor.is_multiple_of(4096) && cancel.load(Ordering::Relaxed) {
            break;
        }
        if !bytes[cursor..].starts_with(b"<plist")
            || !bytes
                .get(cursor + b"<plist".len())
                .is_some_and(|byte| byte.is_ascii_whitespace() || matches!(byte, b'>' | b'/'))
        {
            cursor += 1;
            continue;
        }
        let Some(close_offset) = find_close_tag(&bytes[cursor..], b"plist") else {
            cursor += 1;
            continue;
        };
        let end = cursor + close_offset;
        if end - cursor > limits.max_token_bytes {
            cursor = end;
            continue;
        }
        let Ok(raw) = std::str::from_utf8(&bytes[cursor..end]) else {
            cursor = end;
            continue;
        };
        let Some(data) = XmlPlistParser::new(raw, limits).parse() else {
            cursor += 1;
            continue;
        };
        if let Some(value) = detection(window, ID, cursor, end, data) {
            out.push(value);
        }
        cursor = end;
    }
    out
}

fn detect_labeled_openstep(
    window: &ScanWindow,
    limits: &AnalysisLimits,
    cancel: &AtomicBool,
    out: &mut Vec<Detection>,
) {
    let bytes = &window.bytes;
    for start in 0..bytes.len() {
        if out.len() >= limits.max_results {
            break;
        }
        if start % 4096 == 0 && cancel.load(Ordering::Relaxed) {
            break;
        }
        if !matches!(bytes[start], b'{' | b'(') || !has_plist_label(bytes, start) {
            continue;
        }
        let Some(end) = balanced_end(bytes, start, limits.max_depth) else {
            continue;
        };
        if end - start > limits.max_token_bytes {
            continue;
        }
        let Ok(raw) = std::str::from_utf8(&bytes[start..end]) else {
            continue;
        };
        let Some(data) = parse_openstep_plist(raw, limits.max_depth) else {
            continue;
        };
        if count_nodes(&data, limits.max_nodes).is_none() {
            continue;
        }
        if let Some(value) = detection(window, ID, start, end, data) {
            out.push(value);
        }
    }
}

fn has_plist_label(bytes: &[u8], start: usize) -> bool {
    let line_start = bytes[..start]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |offset| offset + 1);
    let prefix_start = start.saturating_sub(80).max(line_start);
    let Ok(prefix) = std::str::from_utf8(&bytes[prefix_start..start]) else {
        return false;
    };
    let prefix = prefix.trim_end();
    let Some(prefix) = prefix
        .strip_suffix('=')
        .or_else(|| prefix.strip_suffix(':'))
        .map(str::trim_end)
    else {
        return false;
    };
    let label = prefix
        .rsplit_once(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')))
        .map_or(prefix, |(_, label)| label);
    matches!(
        label.to_ascii_lowercase().as_str(),
        "plist" | "propertylist" | "property_list" | "property-list" | "nspropertylist"
    )
}

fn detect_binary_representations(
    window: &ScanWindow,
    limits: &AnalysisLimits,
    cancel: &AtomicBool,
    out: &mut Vec<Detection>,
) {
    for (line_index, (line_start, line_end)) in line_ranges(&window.bytes).enumerate() {
        if out.len() >= limits.max_results {
            break;
        }
        if line_index % 256 == 0 && cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(line) = std::str::from_utf8(&window.bytes[line_start..line_end]) else {
            continue;
        };
        let mut search_from = 0usize;
        for piece in line.split_whitespace() {
            if out.len() >= limits.max_results {
                break;
            }
            let cleaned = piece.trim_matches(|ch: char| {
                matches!(
                    ch,
                    '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
                )
            });
            let direct = binary_transport(cleaned);
            let token = if direct.is_some() {
                cleaned
            } else {
                let separator = cleaned.find(['=', ':']);
                separator
                    .map(|offset| &cleaned[offset + 1..])
                    .unwrap_or(cleaned)
                    .trim_matches(|ch: char| {
                        matches!(
                            ch,
                            '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
                        )
                    })
            };
            if token.is_empty() || token.len() > limits.max_token_bytes {
                search_from = search_from.saturating_add(piece.len());
                continue;
            }
            let Some(transport) = direct.or_else(|| binary_transport(token)) else {
                search_from = search_from.saturating_add(piece.len());
                continue;
            };
            let Some(relative) = line[search_from..]
                .find(token)
                .map(|value| search_from + value)
            else {
                continue;
            };
            search_from = relative + token.len();
            let data = binary_metadata(transport, token);
            if let Some(value) = detection(
                window,
                BINARY_ID,
                line_start + relative,
                line_start + relative + token.len(),
                data,
            ) {
                out.push(value);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BinaryTransport {
    Literal,
    Hex,
    Base64,
}

fn binary_transport(token: &str) -> Option<BinaryTransport> {
    if token.starts_with("bplist00") && token.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Some(BinaryTransport::Literal);
    }
    let hex = token.strip_prefix("0x").unwrap_or(token);
    if hex.len() >= 16
        && hex.len().is_multiple_of(2)
        && hex[..16].eq_ignore_ascii_case("62706c6973743030")
        && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Some(BinaryTransport::Hex);
    }
    (token.len() >= 12
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
        && decoded_base64_prefix(token).is_some_and(|bytes| bytes.starts_with(b"bplist00")))
    .then_some(BinaryTransport::Base64)
}

fn binary_metadata(transport: BinaryTransport, token: &str) -> DataNode {
    let transport_name = match transport {
        BinaryTransport::Literal => "literal",
        BinaryTransport::Hex => "hex",
        BinaryTransport::Base64 => "base64",
    };
    let decoded_bytes = match transport {
        BinaryTransport::Literal => token.len(),
        BinaryTransport::Hex => token.strip_prefix("0x").unwrap_or(token).len() / 2,
        BinaryTransport::Base64 => estimated_base64_bytes(token),
    };
    DataNode::Object(vec![
        (
            "format".to_owned(),
            DataNode::String("binary property list".to_owned()),
        ),
        ("version".to_owned(), DataNode::String("00".to_owned())),
        (
            "transport".to_owned(),
            DataNode::String(transport_name.to_owned()),
        ),
        (
            "source_bytes".to_owned(),
            DataNode::Number(token.len().to_string()),
        ),
        (
            "decoded_bytes".to_owned(),
            DataNode::Number(decoded_bytes.to_string()),
        ),
        (
            "decode".to_owned(),
            DataNode::String("explicit preview required".to_owned()),
        ),
    ])
}

fn estimated_base64_bytes(token: &str) -> usize {
    let padding = token.bytes().rev().take_while(|byte| *byte == b'=').count();
    token.len().saturating_mul(3) / 4 - padding.min(2)
}

fn decoded_base64_prefix(value: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(12);
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for byte in value.bytes().take(24).filter(|byte| *byte != b'=') {
        let six = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(six);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

fn count_nodes(root: &DataNode, max_nodes: usize) -> Option<usize> {
    let mut count = 0usize;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        count += 1;
        if count > max_nodes {
            return None;
        }
        match node {
            DataNode::Object(entries) => stack.extend(entries.iter().map(|(_, value)| value)),
            DataNode::Array(items) => stack.extend(items),
            DataNode::String(_) | DataNode::Number(_) | DataNode::Bool(_) | DataNode::Null => {}
        }
    }
    Some(count)
}

fn find_close_tag(haystack: &[u8], tag: &[u8]) -> Option<usize> {
    let mut cursor = 0usize;
    while cursor + tag.len() + 3 <= haystack.len() {
        if haystack[cursor..].starts_with(b"</") && haystack[cursor + 2..].starts_with(tag) {
            let mut end = cursor + 2 + tag.len();
            while haystack.get(end).is_some_and(u8::is_ascii_whitespace) {
                end += 1;
            }
            if haystack.get(end) == Some(&b'>') {
                return Some(end + 1);
            }
        }
        cursor += 1;
    }
    None
}

struct XmlPlistParser<'a> {
    input: &'a str,
    cursor: usize,
    nodes: usize,
    limits: &'a AnalysisLimits,
}

impl<'a> XmlPlistParser<'a> {
    fn new(input: &'a str, limits: &'a AnalysisLimits) -> Self {
        Self {
            input,
            cursor: 0,
            nodes: 0,
            limits,
        }
    }

    fn parse(mut self) -> Option<DataNode> {
        self.skip_misc()?;
        let self_closing = self.open_tag("plist")?;
        if self_closing {
            return None;
        }
        self.skip_misc()?;
        let value = self.value(0)?;
        self.skip_misc()?;
        self.close_tag("plist")?;
        self.skip_misc()?;
        (self.cursor == self.input.len()).then_some(value)
    }

    fn value(&mut self, depth: usize) -> Option<DataNode> {
        if !node_allowed(&mut self.nodes, depth, self.limits) {
            return None;
        }
        self.skip_misc()?;
        if self.starts_open_tag("dict") {
            return self.dictionary(depth);
        }
        if self.starts_open_tag("array") {
            return self.array(depth);
        }
        if self.starts_open_tag("true") {
            self.empty_value_element("true")?;
            return Some(DataNode::Bool(true));
        }
        if self.starts_open_tag("false") {
            self.empty_value_element("false")?;
            return Some(DataNode::Bool(false));
        }
        if self.starts_open_tag("string") {
            return Some(DataNode::String(self.text_element("string")?));
        }
        if self.starts_open_tag("integer") || self.starts_open_tag("uid") {
            let tag = if self.starts_open_tag("integer") {
                "integer"
            } else {
                "uid"
            };
            let value = self.text_element(tag)?.trim().to_owned();
            value.parse::<i128>().ok()?;
            return Some(DataNode::Number(value));
        }
        if self.starts_open_tag("real") {
            let value = self.text_element("real")?.trim().to_owned();
            value.parse::<f64>().ok()?;
            return Some(DataNode::Number(value));
        }
        for tag in ["date", "data"] {
            if self.starts_open_tag(tag) {
                return Some(DataNode::String(self.text_element(tag)?.trim().to_owned()));
            }
        }
        None
    }

    fn dictionary(&mut self, depth: usize) -> Option<DataNode> {
        if self.open_tag("dict")? {
            return Some(DataNode::Object(Vec::new()));
        }
        let mut entries = Vec::new();
        loop {
            self.skip_misc()?;
            if self.starts_close_tag("dict") {
                self.close_tag("dict")?;
                return Some(DataNode::Object(entries));
            }
            let key = self.text_element("key")?;
            self.skip_misc()?;
            entries.push((key, self.value(depth + 1)?));
        }
    }

    fn array(&mut self, depth: usize) -> Option<DataNode> {
        if self.open_tag("array")? {
            return Some(DataNode::Array(Vec::new()));
        }
        let mut values = Vec::new();
        loop {
            self.skip_misc()?;
            if self.starts_close_tag("array") {
                self.close_tag("array")?;
                return Some(DataNode::Array(values));
            }
            values.push(self.value(depth + 1)?);
        }
    }

    fn empty_value_element(&mut self, tag: &str) -> Option<()> {
        if self.open_tag(tag)? {
            return Some(());
        }
        self.skip_misc()?;
        self.close_tag(tag)
    }

    fn text_element(&mut self, tag: &str) -> Option<String> {
        if self.open_tag(tag)? {
            return Some(String::new());
        }
        let closing = format!("</{tag}>");
        let offset = self.input[self.cursor..].find(&closing)?;
        let end = self.cursor + offset;
        if end - self.cursor > self.limits.max_token_bytes {
            return None;
        }
        let value = decode_xml_text(&self.input[self.cursor..end])?;
        self.cursor = end;
        self.close_tag(tag)?;
        Some(value)
    }

    fn starts_open_tag(&self, tag: &str) -> bool {
        let remainder = self.input.get(self.cursor + 1..).unwrap_or_default();
        remainder.starts_with(tag)
            && remainder
                .as_bytes()
                .get(tag.len())
                .is_some_and(|byte| byte.is_ascii_whitespace() || matches!(byte, b'>' | b'/'))
    }

    fn starts_close_tag(&self, tag: &str) -> bool {
        self.input[self.cursor..].starts_with(&format!("</{tag}"))
    }

    fn open_tag(&mut self, tag: &str) -> Option<bool> {
        if !self.starts_open_tag(tag) {
            return None;
        }
        let mut quote = None;
        for (offset, byte) in self.input.as_bytes()[self.cursor..]
            .iter()
            .copied()
            .enumerate()
        {
            if let Some(current) = quote {
                if byte == current {
                    quote = None;
                }
                continue;
            }
            match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'>' => {
                    let end = self.cursor + offset;
                    let self_closing = self.input.as_bytes()[self.cursor..end]
                        .iter()
                        .rev()
                        .find(|byte| !byte.is_ascii_whitespace())
                        == Some(&b'/');
                    self.cursor = end + 1;
                    return Some(self_closing);
                }
                _ => {}
            }
        }
        None
    }

    fn close_tag(&mut self, tag: &str) -> Option<()> {
        let prefix = format!("</{tag}");
        if !self.input[self.cursor..].starts_with(&prefix) {
            return None;
        }
        let remainder = &self.input[self.cursor + prefix.len()..];
        let end = remainder.find('>')?;
        if !remainder[..end].trim().is_empty() {
            return None;
        }
        self.cursor += prefix.len() + end + 1;
        Some(())
    }

    fn skip_misc(&mut self) -> Option<()> {
        loop {
            self.cursor += self.input[self.cursor..]
                .find(|ch: char| !ch.is_whitespace())
                .unwrap_or(self.input.len() - self.cursor);
            if self.input[self.cursor..].starts_with("<!--") {
                let end = self.input[self.cursor + 4..].find("-->")?;
                self.cursor += 4 + end + 3;
                continue;
            }
            if self.input[self.cursor..].starts_with("<?") {
                let end = self.input[self.cursor + 2..].find("?>")?;
                self.cursor += 2 + end + 2;
                continue;
            }
            return Some(());
        }
    }
}

fn decode_xml_text(input: &str) -> Option<String> {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;
    while cursor < input.len() {
        let remainder = &input[cursor..];
        if let Some(cdata) = remainder.strip_prefix("<![CDATA[") {
            let end = cdata.find("]]>")?;
            out.push_str(&cdata[..end]);
            cursor += "<![CDATA[".len() + end + "]]>".len();
            continue;
        }
        if let Some(comment) = remainder.strip_prefix("<!--") {
            let end = comment.find("-->")?;
            cursor += "<!--".len() + end + "-->".len();
            continue;
        }
        if remainder.starts_with('<') {
            return None;
        }
        if let Some(entity) = remainder.strip_prefix('&') {
            let end = entity.find(';')?;
            out.push(decode_xml_entity(&entity[..end])?);
            cursor += 1 + end + 1;
            continue;
        }
        let ch = remainder.chars().next()?;
        out.push(ch);
        cursor += ch.len_utf8();
    }
    Some(out)
}

fn decode_xml_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        value if value.starts_with("#x") => {
            char::from_u32(u32::from_str_radix(&value[2..], 16).ok()?)
        }
        value if value.starts_with('#') => char::from_u32(value[1..].parse().ok()?),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> AnalysisLimits {
        AnalysisLimits::default()
    }

    #[test]
    fn parses_xml_dictionary_arrays_scalars_and_entities() {
        let raw = r#"<plist version="1.0"><dict>
            <key>name</key><string>Ada &amp; Grace</string>
            <key>roles</key><array><string>admin</string><string>editor</string></array>
            <key>enabled</key><true/>
            <key>count</key><integer>42</integer>
        </dict></plist>"#;
        let value = XmlPlistParser::new(raw, &limits()).parse().unwrap();
        let DataNode::Object(entries) = value else {
            panic!("expected plist dictionary")
        };
        assert_eq!(entries[0].1, DataNode::String("Ada & Grace".to_owned()));
        assert!(matches!(entries[1].1, DataNode::Array(_)));
        assert_eq!(entries[2].1, DataNode::Bool(true));
        assert_eq!(entries[3].1, DataNode::Number("42".to_owned()));
        assert_eq!(
            XmlPlistParser::new("<plist><dict/></plist>", &limits())
                .parse()
                .unwrap(),
            DataNode::Object(Vec::new())
        );
    }

    #[test]
    fn rejects_malformed_or_unsupported_xml() {
        assert!(
            XmlPlistParser::new("<plist><dict><key>x</key></dict></plist>", &limits())
                .parse()
                .is_none()
        );
        assert!(XmlPlistParser::new("<plist><set/></plist>", &limits())
            .parse()
            .is_none());
        let mut shallow = limits();
        shallow.max_depth = 0;
        assert!(
            XmlPlistParser::new("<plist><dict><key>x</key><array/></dict></plist>", &shallow)
                .parse()
                .is_none()
        );
    }

    #[test]
    fn recognizes_binary_plist_transports() {
        assert_eq!(
            binary_transport("bplist00payload"),
            Some(BinaryTransport::Literal)
        );
        assert_eq!(
            binary_transport("62706c6973743030deadbeef"),
            Some(BinaryTransport::Hex)
        );
        assert_eq!(
            binary_transport("YnBsaXN0MDA="),
            Some(BinaryTransport::Base64)
        );
        assert_eq!(binary_transport("ordinary-token"), None);
        assert_eq!(binary_transport("YnBsaXN0MDA=not-base64!"), None);
    }

    #[test]
    fn only_explicit_plist_labels_claim_openstep_values() {
        assert!(has_plist_label(b"propertyList = { key = value; }", 15));
        assert!(!has_plist_label(b"payload = { key = value; }", 10));
        assert!(!has_plist_label(b"received plist { key = value; }", 15));
    }
}
