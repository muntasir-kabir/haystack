//! Versioned record profiles and bounded, anchored header matching.
//!
//! A record profile describes the physical first line of an event. It is
//! intentionally separate from Drain templates, which cluster normalized
//! message content after record classification.

mod detect;
mod matcher;
mod presets;
mod preview;
mod profile;
mod state;
mod template;

pub use detect::{
    discovery_line_indices, resolve_profile_time, DetectionConfidence, DetectionDiagnostics,
    DISCOVERY_MAX_BYTES, DISCOVERY_MAX_LINES, DISCOVERY_MAX_LINE_BYTES,
};
pub use matcher::{FieldCaptures, TemplateMatch};
pub use presets::{load_presets, save_presets, PRESET_SCHEMA_VERSION};
pub use preview::{
    preview_profile, PreviewLine, PreviewResult, PREVIEW_MAX_BYTES, PREVIEW_MAX_LINES,
};
pub use profile::{
    CustomFieldDefinition, CustomFieldKind, LogLevelPolicy, PrefixPolicy, ProfileLimits,
    ProfileSource, RecordProfile, TimestampSelection, RECORD_PROFILE_SCHEMA_VERSION,
};
pub use state::{
    ClassifiedLine, HeaderTime, LineTimeState, RecordClassification, RecordStateMachine,
    TimeProvenance,
};
pub use template::{CompiledProfile, TemplateCompileError};
