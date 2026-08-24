use super::{detection, line_ranges};
use crate::core::embedded_data::parse::{nested, scalar, split_top_level};
use crate::core::embedded_data::{AnalysisLimits, DataDetector, DataNode, Detection, ScanWindow};
use std::sync::atomic::{AtomicBool, Ordering};

pub const ID: &str = "fields";
pub(super) struct FieldsDetector;

impl DataDetector for FieldsDetector {
    fn id(&self) -> &'static str {
        ID
    }
    fn detect(
        &self,
        window: &ScanWindow,
        limits: &AnalysisLimits,
        cancel: &AtomicBool,
    ) -> Vec<Detection> {
        let ranges = line_ranges(&window.bytes).collect::<Vec<_>>();
        let mut out = Vec::new();
        let mut i = 0;
        while i < ranges.len() && out.len() < limits.max_results {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let (start, end) = ranges[i];
            let Ok(line) = std::str::from_utf8(&window.bytes[start..end]) else {
                i += 1;
                continue;
            };
            let inline = split_top_level(line, &[','])
                .into_iter()
                .filter_map(field_line)
                .filter(|(_, _, value)| !value.is_empty())
                .collect::<Vec<_>>();
            if inline.len() >= 2 {
                let candidate_start = start + inline[0].0;
                let entries = inline
                    .into_iter()
                    .map(|(_, key, value)| {
                        (key, nested(value, &[':'], &[','], 0, limits.max_depth))
                    })
                    .collect();
                if let Some(value) = detection(
                    window,
                    self.id(),
                    candidate_start,
                    end,
                    DataNode::Object(entries),
                ) {
                    out.push(value);
                }
                i += 1;
                continue;
            }
            let Some(first) = field_line(line) else {
                i += 1;
                continue;
            };
            let mut rows = vec![first];
            let candidate_start = start + rows[0].0;
            let mut candidate_end = end;
            let mut j = i + 1;
            while j < ranges.len() {
                let (next_start, next_end) = ranges[j];
                let Ok(next) = std::str::from_utf8(&window.bytes[next_start..next_end]) else {
                    break;
                };
                let Some(next_field) = field_line(next) else {
                    break;
                };
                rows.push(next_field);
                candidate_end = next_end;
                j += 1;
            }
            if rows.len() >= 2 {
                let mut cursor = 0;
                let entries = indented_entries(&rows, rows[0].0, &mut cursor, limits.max_depth);
                if let Some(value) = detection(
                    window,
                    self.id(),
                    candidate_start,
                    candidate_end,
                    DataNode::Object(entries),
                ) {
                    out.push(value);
                }
                i = j;
            } else {
                i += 1;
            }
        }
        out
    }
}

fn field_line(line: &str) -> Option<(usize, String, &str)> {
    let indent = line.len() - line.trim_start().len();
    let trimmed = line.trim();
    let colon = trimmed.find(':')?;
    let key = trimmed[..colon].trim();
    let value = trimmed[colon + 1..].trim();
    if key.is_empty()
        || value.starts_with("//")
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return None;
    }
    Some((indent, key.to_owned(), value))
}

fn indented_entries(
    rows: &[(usize, String, &str)],
    indent: usize,
    cursor: &mut usize,
    max_depth: usize,
) -> Vec<(String, DataNode)> {
    let mut out = Vec::new();
    while *cursor < rows.len() {
        let (row_indent, key, value) = &rows[*cursor];
        if *row_indent < indent || *row_indent > indent {
            break;
        }
        let key = key.clone();
        let value = *value;
        *cursor += 1;
        let node = if *cursor < rows.len() && rows[*cursor].0 > *row_indent && max_depth > 0 {
            let children = indented_entries(rows, rows[*cursor].0, cursor, max_depth - 1);
            if value.is_empty() {
                DataNode::Object(children)
            } else {
                let mut fields = vec![("value".to_owned(), scalar(value))];
                fields.extend(children);
                DataNode::Object(fields)
            }
        } else {
            nested(value, &[':'], &[','], 0, max_depth)
        };
        out.push((key, node));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_fields() {
        assert!(field_line("  status: ok").is_some());
        assert!(field_line("http://example").is_none());
    }

    #[test]
    fn nests_indented_fields() {
        let rows = vec![
            (0, "user".to_owned(), ""),
            (2, "id".to_owned(), "42"),
            (2, "status".to_owned(), "ok"),
        ];
        let mut cursor = 0;
        let tree = indented_entries(&rows, 0, &mut cursor, 8);
        assert!(matches!(&tree[0].1, DataNode::Object(values) if values.len() == 2));
    }
}
