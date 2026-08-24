use std::sync::atomic::AtomicBool;

pub const JSON_DETECTOR_ID: &str = "json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourcePos {
    pub line: usize,
    pub byte: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceSpan {
    pub start: SourcePos,
    pub end: SourcePos,
}

impl SourceSpan {
    pub fn includes_line(self, line: usize) -> bool {
        self.start.line <= line && line <= self.end.line
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootKind {
    Object,
    Array,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DataNode {
    Object(Vec<(String, DataNode)>),
    Array(Vec<DataNode>),
    String(String),
    Number(String),
    Bool(bool),
    Null,
}

impl DataNode {
    pub fn root_kind(&self) -> Option<RootKind> {
        match self {
            Self::Object(_) => Some(RootKind::Object),
            Self::Array(_) => Some(RootKind::Array),
            _ => None,
        }
    }
    pub fn summary(&self) -> String {
        match self {
            Self::Object(entries) => format!("object · {} fields", entries.len()),
            Self::Array(items) => format!("array · {} items", items.len()),
            Self::String(value) => format!("string · {} bytes", value.len()),
            Self::Number(_) => "number".to_string(),
            Self::Bool(_) => "boolean".to_string(),
            Self::Null => "null".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Detection {
    pub detector_id: &'static str,
    pub span: SourceSpan,
    pub root_kind: RootKind,
    pub raw: String,
    pub pretty: String,
    pub data: DataNode,
    pub source_lines: usize,
}

impl Detection {
    pub fn structured(
        detector_id: &'static str,
        span: SourceSpan,
        raw: impl Into<String>,
        data: DataNode,
    ) -> Self {
        let raw = raw.into();
        let root_kind = data.root_kind().unwrap_or(RootKind::Object);
        let pretty = pretty_node(&data, 0);
        Self {
            detector_id,
            span,
            root_kind,
            raw,
            pretty,
            data,
            source_lines: span.end.line.saturating_sub(span.start.line) + 1,
        }
    }
    pub fn summary(&self) -> String {
        format!(
            "{} · {} lines · {} bytes",
            self.data.summary(),
            self.source_lines,
            self.raw.len()
        )
    }
}

pub(crate) fn pretty_node(node: &DataNode, depth: usize) -> String {
    let pad = "  ".repeat(depth);
    match node {
        DataNode::Object(values) => {
            if values.is_empty() {
                return "{}".to_owned();
            }
            let body = values
                .iter()
                .map(|(key, value)| format!("{}  {}: {}", pad, key, pretty_node(value, depth + 1)))
                .collect::<Vec<_>>()
                .join(",\n");
            format!("{{\n{body}\n{pad}}}")
        }
        DataNode::Array(values) => {
            if values.is_empty() {
                return "[]".to_owned();
            }
            let body = values
                .iter()
                .map(|value| format!("{}  {}", pad, pretty_node(value, depth + 1)))
                .collect::<Vec<_>>()
                .join(",\n");
            format!("[\n{body}\n{pad}]")
        }
        DataNode::String(value) => serde_json::to_string(value).unwrap_or_default(),
        DataNode::Number(value) => value.clone(),
        DataNode::Bool(value) => value.to_string(),
        DataNode::Null => "null".to_owned(),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AnalysisLimits {
    pub max_bytes: usize,
    pub max_lines: usize,
    pub max_results: usize,
    pub max_nodes: usize,
    pub max_depth: usize,
    pub max_token_bytes: usize,
    pub max_frames: usize,
    pub max_decoded_bytes: usize,
}

impl Default for AnalysisLimits {
    fn default() -> Self {
        Self {
            max_bytes: 512 * 1024,
            max_lines: 1_000,
            max_results: 64,
            max_nodes: 20_000,
            max_depth: 128,
            max_token_bytes: 64 * 1024,
            max_frames: 4_000,
            max_decoded_bytes: 512 * 1024,
        }
    }
}

pub(crate) fn node_allowed(nodes: &mut usize, depth: usize, limits: &AnalysisLimits) -> bool {
    if depth > limits.max_depth || *nodes >= limits.max_nodes {
        return false;
    }
    *nodes += 1;
    true
}

#[allow(dead_code)]
pub(crate) fn cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(std::sync::atomic::Ordering::Relaxed)
}
