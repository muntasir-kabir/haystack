use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use crate::core::embedded_data::model::{node_allowed, JSON_DETECTOR_ID};
use crate::core::embedded_data::{
    AnalysisLimits, DataDetector, DataNode, Detection, ScanWindow, SourceSpan,
};

pub(super) struct JsonDetector;

impl DataDetector for JsonDetector {
    fn id(&self) -> &'static str {
        JSON_DETECTOR_ID
    }

    fn detect(
        &self,
        window: &ScanWindow,
        limits: &AnalysisLimits,
        cancel: &AtomicBool,
    ) -> Vec<Detection> {
        let mut out = Vec::new();
        let mut cursor = 0;
        while cursor < window.bytes.len() && out.len() < limits.max_results {
            if cursor % 4096 == 0 && cancel.load(Ordering::Relaxed) {
                break;
            }
            if !matches!(window.bytes[cursor], b'{' | b'[') {
                cursor += 1;
                continue;
            }
            let Some(end) = crate::core::embedded_data::parse::balanced_end(
                &window.bytes,
                cursor,
                limits.max_depth,
            ) else {
                cursor += 1;
                continue;
            };
            let candidate = &window.bytes[cursor..end];
            let Ok(raw) = std::str::from_utf8(candidate) else {
                cursor += 1;
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(raw) else {
                cursor += 1;
                continue;
            };
            let mut nodes = 0;
            let Some(data) = value_to_node(&value, 0, &mut nodes, limits) else {
                cursor = end;
                continue;
            };
            if data.root_kind().is_none() {
                cursor += 1;
                continue;
            }
            let span = SourceSpan {
                start: window.source_pos(cursor),
                end: window.source_pos(end),
            };
            let mut result = Detection::structured(self.id(), span, raw, data);
            result.pretty = serde_json::to_string_pretty(&value).unwrap_or_else(|_| raw.to_owned());
            out.push(result);
            cursor = end;
        }
        out
    }
}

fn value_to_node(
    value: &Value,
    depth: usize,
    nodes: &mut usize,
    limits: &AnalysisLimits,
) -> Option<DataNode> {
    if !node_allowed(nodes, depth, limits) {
        return None;
    }
    match value {
        Value::Object(object) => Some(DataNode::Object(
            object
                .iter()
                .map(|(key, value)| {
                    Some((key.clone(), value_to_node(value, depth + 1, nodes, limits)?))
                })
                .collect::<Option<Vec<_>>>()?,
        )),
        Value::Array(array) => Some(DataNode::Array(
            array
                .iter()
                .map(|value| value_to_node(value, depth + 1, nodes, limits))
                .collect::<Option<Vec<_>>>()?,
        )),
        Value::String(value) => Some(DataNode::String(value.clone())),
        Value::Number(value) => Some(DataNode::Number(value.to_string())),
        Value::Bool(value) => Some(DataNode::Bool(*value)),
        Value::Null => Some(DataNode::Null),
    }
}

#[cfg(test)]
mod tests {
    use crate::core::embedded_data::parse::balanced_end;
    #[test]
    fn balances_quoted_delimiters() {
        let input = br#"{"value":"} ] \" {", "ok":true} trailing"#;
        let end = balanced_end(input, 0, 32).unwrap();
        assert_eq!(&input[..end], br#"{"value":"} ] \" {", "ok":true}"#);
    }
}
