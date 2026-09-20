//! haystack core library — shared by the GUI and the MCP server.
//!
//! Everything here is pure Rust, no GUI dependencies: memory-mapped log
//! loading, timestamp extraction, Drain template mining, multi-filter
//! search, timeline bucketing, and the MCP stdio server.

pub mod core;
pub mod mcp;

pub use core::document::{LoadProgress, LoadStage, LogDocument, ParsingConfig, TemplateInfo};
pub use core::drain::{Drain, LogCluster};
pub use core::embedded_data::{
    AnalysisLimits, DataNode, Detection, EmbeddedDataEngine, RootKind, SourcePos, SourceSpan,
};
pub use core::format::{FormatDetector, LogFormat};
pub use core::masking::{LogMasker, MaskConfig};
pub use core::settings::Settings;
pub use core::template_extract::{
    extract_templates, ExtractionSummary, TemplateSequence, TemplateStore,
};
pub use core::time::{CustomDateFormat, TimeComponents, TimeDetector, TimeFormat, TimeFormatKind};
pub use core::timeline::{Timeline, TimelineDomain, DEFAULT_BUCKETS};
