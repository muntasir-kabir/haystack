//! Bounded preview of the same compiled classifier used by document loading.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::core::time::{CustomTimeFormat, TimeFormatKind, YearlessReference};

use super::{
    resolve_profile_time, CompiledProfile, DetectionDiagnostics, RecordClassification,
    RecordStateMachine, TimeProvenance,
};

pub const PREVIEW_MAX_LINES: usize = 200;
pub const PREVIEW_MAX_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewLine {
    pub source_line: usize,
    pub text: String,
    pub record_start: bool,
    pub provenance: TimeProvenance,
    pub timestamp: Option<i64>,
    pub timestamp_span: Option<Range<usize>>,
    pub header_span: Option<Range<usize>>,
    pub fields: Vec<(String, String, Range<usize>)>,
}

#[derive(Clone, Debug)]
pub struct PreviewResult {
    pub lines: Vec<PreviewLine>,
    pub field_types: Vec<(String, &'static str)>,
    pub detection: DetectionDiagnostics,
    pub truncated: bool,
    pub sampled_bytes: usize,
    /// Successful header declarations matched through schema-3 `{ignore}`.
    /// This is preview-only accounting; ignored values are never retained.
    pub ignored_matches: usize,
}

/// Preview at most 200 physical lines / 256 KiB. Cancellation is checked before
/// every line, and compilation is deliberately performed by the caller once.
pub fn preview_profile(
    profile: &CompiledProfile,
    sample: &str,
    custom: &[CustomTimeFormat],
    cancel: &AtomicBool,
) -> Result<PreviewResult, String> {
    let mut lines = Vec::new();
    let mut sampled_bytes = 0usize;
    let mut truncated = false;
    for source in sample.split_terminator('\n') {
        if cancel.load(Ordering::Relaxed) {
            return Err("preview cancelled".into());
        }
        if lines.len() == PREVIEW_MAX_LINES || sampled_bytes + source.len() > PREVIEW_MAX_BYTES {
            truncated = true;
            break;
        }
        let source = source.strip_suffix('\r').unwrap_or(source);
        sampled_bytes += source.len();
        lines.push(source);
    }
    let (time_format, mut detection) = resolve_profile_time(profile, lines.iter().copied(), custom);
    let reference = profile
        .profile
        .yearless_year
        .map_or_else(YearlessReference::now, |year| {
            YearlessReference::now().with_explicit_year(year)
        });
    let time_format = time_format.map(|format| format.pin_yearless(reference));
    detection.yearless_reference_year = time_format
        .as_ref()
        .and_then(TimeFormatKind::yearless_reference)
        .map(|reference| reference.year);
    let mut state = RecordStateMachine::default();
    let mut preview_lines = Vec::with_capacity(lines.len());
    let mut ignored_matches = 0usize;
    for (index, line) in lines.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("preview cancelled".into());
        }
        let matched = profile.match_text(line, index == 0, time_format.as_ref());
        if matched.is_some() {
            ignored_matches += profile.ignored_field_count();
        }
        let header_span = matched.as_ref().map(|value| value.header_span.clone());
        let mut fields = Vec::new();
        if let Some(matched) = matched.as_ref() {
            for field in 0..profile.field_count() {
                let Some(name) = profile.field_name(field) else {
                    continue;
                };
                let Some(span) = matched.captures.span_at(field) else {
                    continue;
                };
                if let Some(value) = line.get(span.clone()) {
                    fields.push((name.to_string(), value.to_string(), span));
                }
            }
        }
        let classification = matched.map_or(RecordClassification::Continuation, |value| {
            RecordClassification::from_template(value, profile.field_index("time").is_some())
        });
        let classified = state.classify(&classification);
        preview_lines.push(PreviewLine {
            source_line: index + 1,
            text: line.to_string(),
            record_start: classified.record_start,
            provenance: classified.provenance,
            timestamp: classified.timestamp,
            timestamp_span: classified.timestamp_span,
            header_span,
            fields,
        });
    }
    Ok(PreviewResult {
        lines: preview_lines,
        field_types: (0..profile.field_count())
            .filter_map(|index| {
                Some((
                    profile.field_name(index)?.to_string(),
                    profile.field_type_label(index)?,
                ))
            })
            .collect(),
        detection,
        truncated,
        sampled_bytes,
        ignored_matches,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::record::{RecordProfile, TimestampSelection};

    #[test]
    fn preview_shows_boundaries_fields_and_inherited_time() {
        let mut profile = RecordProfile::text(
            "test:detailed",
            "Detailed",
            "{time} - [{log_level}] - {thread_id} {file}:{line} {log}",
        );
        profile.timestamp = TimestampSelection::BuiltIn("ISO-8601".into());
        let compiled = CompiledProfile::compile(profile).unwrap();
        let result = preview_profile(
            &compiled,
            "2026-09-11 10:00:00.100 - [ERROR] - worker-7 Handler.rs:42 failed\n    retry_at=2035-01-01T00:00:00Z\n2026-09-11 09:59:59.000 - [INFO] - worker-8 Handler.rs:57 recovered\n",
            &[],
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.lines.len(), 3);
        assert!(result.lines[0].record_start);
        assert_eq!(result.lines[1].provenance, TimeProvenance::Inherited);
        assert!(!result.lines[1].record_start);
        assert_eq!(
            result.lines[0]
                .fields
                .iter()
                .find(|field| field.0 == "thread_id")
                .unwrap()
                .1,
            "worker-7"
        );
        assert!(result.lines[2].record_start);
        assert!(result.lines[2].timestamp.unwrap() < result.lines[0].timestamp.unwrap());
    }

    #[test]
    fn preview_caps_work_and_honors_cancellation() {
        let profile =
            CompiledProfile::compile(RecordProfile::text("test:lines", "Lines", "{time} {log}"))
                .unwrap();
        let sample = "2026-09-11T10:00:00Z line\n".repeat(PREVIEW_MAX_LINES + 1);
        let result = preview_profile(&profile, &sample, &[], &AtomicBool::new(false)).unwrap();
        assert_eq!(result.lines.len(), PREVIEW_MAX_LINES);
        assert!(result.truncated);

        let cancelled = AtomicBool::new(true);
        assert!(preview_profile(&profile, "2026-09-11T10:00:00Z line", &[], &cancelled).is_err());
    }

    #[test]
    fn preview_counts_ignored_header_values_without_exposing_them_as_fields() {
        let profile = CompiledProfile::compile(RecordProfile::refined_inline(
            "test:ignore-preview",
            "Ignore preview",
            "{time} App[{thread:number}:{ignore:number}] {log}",
        ))
        .unwrap();
        let result = preview_profile(
            &profile,
            "2026-07-15 22:26:39.907481+0300 App[12:4] ready",
            &[],
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.ignored_matches, 1);
        assert!(result.lines[0]
            .fields
            .iter()
            .all(|field| field.0 != "ignore"));
    }
}
