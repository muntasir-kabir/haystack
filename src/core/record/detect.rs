use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::core::time::{CustomTimeFormat, TimeFormatKind, TIME_FORMATS};

use super::{CompiledProfile, TimestampSelection};

pub const DISCOVERY_MAX_LINES: usize = 8_192;
pub const DISCOVERY_MAX_BYTES: usize = 2 * 1024 * 1024;
pub const DISCOVERY_MAX_LINE_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionConfidence {
    Explicit,
    High,
    Low,
    Ambiguous,
    Unresolved,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectionDiagnostics {
    pub profile_id: Option<String>,
    pub time_format: Option<String>,
    /// Fixed calendar year used for a yearless timestamp family, when any.
    #[serde(default)]
    pub yearless_reference_year: Option<i32>,
    pub confidence: DetectionConfidence,
    pub supporting_headers: usize,
    pub conflicting_candidates: Vec<String>,
    pub sampled_lines: usize,
    pub sampled_bytes: usize,
    pub line_limit_reached: bool,
    pub byte_limit_reached: bool,
    pub truncated_lines: usize,
}

impl Default for DetectionDiagnostics {
    fn default() -> Self {
        Self {
            profile_id: None,
            time_format: None,
            yearless_reference_year: None,
            confidence: DetectionConfidence::Unresolved,
            supporting_headers: 0,
            conflicting_candidates: Vec::new(),
            sampled_lines: 0,
            sampled_bytes: 0,
            line_limit_reached: false,
            byte_limit_reached: false,
            truncated_lines: 0,
        }
    }
}

/// Resolve the timestamp parser for one explicitly selected profile. Named
/// choices bypass popularity thresholds; Auto compares only matches in the
/// profile's designated `{time}` slot and reports equal evidence as ambiguous.
pub fn resolve_profile_time<'a>(
    profile: &CompiledProfile,
    sample: impl IntoIterator<Item = &'a str>,
    custom: &[CustomTimeFormat],
) -> (Option<TimeFormatKind>, DetectionDiagnostics) {
    let lines = sample.into_iter().collect::<Vec<_>>();
    let mut diagnostics = DetectionDiagnostics {
        profile_id: Some(profile.profile.id.clone()),
        sampled_lines: lines.len(),
        sampled_bytes: lines.iter().map(|line| line.len()).sum(),
        ..DetectionDiagnostics::default()
    };
    if profile.field_index("time").is_none()
        || matches!(profile.profile.timestamp, TimestampSelection::None)
    {
        diagnostics.confidence = DetectionConfidence::Explicit;
        return (None, diagnostics);
    }

    let named = match &profile.profile.timestamp {
        TimestampSelection::BuiltIn(name) => TIME_FORMATS
            .iter()
            .find(|format| format.name() == name)
            .map(|format| TimeFormatKind::BuiltIn(*format)),
        TimestampSelection::Custom(name) => custom
            .iter()
            .find(|format| format.name == *name)
            .cloned()
            .map(|format| TimeFormatKind::Custom(Arc::new(format))),
        TimestampSelection::Auto | TimestampSelection::None => None,
    };
    if !matches!(profile.profile.timestamp, TimestampSelection::Auto) {
        diagnostics.time_format = named.as_ref().map(TimeFormatKind::name);
        diagnostics.confidence = if named.is_some() {
            DetectionConfidence::Explicit
        } else {
            DetectionConfidence::Unresolved
        };
        return (named, diagnostics);
    }

    let mut candidates = TIME_FORMATS
        .iter()
        .map(|format| TimeFormatKind::BuiltIn(*format))
        .chain(
            custom
                .iter()
                .cloned()
                .map(|format| TimeFormatKind::Custom(Arc::new(format))),
        )
        .map(|candidate| {
            let mut hits = 0usize;
            let mut covered_bytes = 0usize;
            for line in &lines {
                if let Some(matched) = profile.match_text(line, false, Some(&candidate)) {
                    hits += 1;
                    covered_bytes += matched.timestamp_span.map_or(0, |span| span.len());
                }
            }
            (candidate, hits, covered_bytes)
        })
        .filter(|(_, hits, _)| *hits > 0)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| (right.1, right.2).cmp(&(left.1, left.2)));
    let Some((winner, hits, covered_bytes)) = candidates.first().cloned() else {
        return (None, diagnostics);
    };
    diagnostics.supporting_headers = hits;
    diagnostics.conflicting_candidates = candidates
        .iter()
        .skip(1)
        .filter(|(_, other_hits, other_bytes)| *other_hits == hits && *other_bytes == covered_bytes)
        .map(|(candidate, _, _)| candidate.name())
        .collect();
    if !diagnostics.conflicting_candidates.is_empty() {
        diagnostics.confidence = DetectionConfidence::Ambiguous;
        return (None, diagnostics);
    }
    diagnostics.time_format = Some(winner.name());
    diagnostics.confidence = if hits >= 2 {
        DetectionConfidence::High
    } else if lines.len() <= 4 {
        DetectionConfidence::Low
    } else {
        DetectionConfidence::Unresolved
    };
    if diagnostics.confidence == DetectionConfidence::Unresolved {
        (None, diagnostics)
    } else {
        (Some(winner), diagnostics)
    }
}

/// Distributed contiguous-window sample indexes. The leading 512 lines are
/// always considered, followed by 32 evenly spaced 128-line windows. The
/// caller enforces the independent byte budget while decoding.
pub fn discovery_line_indices(total_lines: usize) -> Vec<usize> {
    let mut indexes = (0..total_lines.min(512)).collect::<Vec<_>>();
    if total_lines > 512 {
        const WINDOWS: usize = 32;
        const WINDOW_LINES: usize = 128;
        for window in 0..WINDOWS {
            let center = window.saturating_mul(total_lines.saturating_sub(1)) / (WINDOWS - 1);
            let start = center.saturating_sub(WINDOW_LINES / 2);
            indexes.extend(start..(start + WINDOW_LINES).min(total_lines));
        }
    }
    indexes.sort_unstable();
    indexes.dedup();
    indexes.truncate(DISCOVERY_MAX_LINES);
    indexes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::record::RecordProfile;

    #[test]
    fn distributed_sampling_is_bounded_and_reaches_the_tail() {
        let indexes = discovery_line_indices(1_000_000);
        assert!(indexes.len() <= DISCOVERY_MAX_LINES);
        assert_eq!(indexes[0], 0);
        assert_eq!(indexes.last(), Some(&999_999));
        assert!(indexes.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn sparse_profile_headers_resolve_without_a_physical_line_percentage() {
        let profile = CompiledProfile::compile(RecordProfile::default_text()).unwrap();
        let mut lines = vec!["continuation"; 1_000];
        lines[0] = "2026-09-11 10:00:00.000 first";
        lines[999] = "2026-09-11 10:00:01.000 second";
        let (format, diagnostics) = resolve_profile_time(&profile, lines, &[]);
        assert_eq!(format.unwrap().name(), "ISO-8601");
        assert_eq!(diagnostics.supporting_headers, 2);
        assert_eq!(diagnostics.confidence, DetectionConfidence::High);
    }

    #[test]
    fn named_timestamp_selection_has_explicit_precedence() {
        let mut profile = RecordProfile::default_text();
        profile.timestamp = TimestampSelection::BuiltIn("ISO-8601".to_string());
        let profile = CompiledProfile::compile(profile).unwrap();
        let (format, diagnostics) = resolve_profile_time(&profile, std::iter::empty::<&str>(), &[]);
        assert_eq!(format.unwrap().name(), "ISO-8601");
        assert_eq!(diagnostics.confidence, DetectionConfidence::Explicit);
    }
}
