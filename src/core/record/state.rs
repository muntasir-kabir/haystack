use std::ops::Range;

use super::TemplateMatch;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderTime {
    Known { value: i64, span: Range<usize> },
    Missing,
    Invalid { span: Option<Range<usize>> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordClassification {
    Header {
        time: HeaderTime,
        header_span: Range<usize>,
        message_span: Range<usize>,
    },
    Continuation,
    Unassigned,
}

impl RecordClassification {
    pub fn from_template(matched: TemplateMatch, expects_time: bool) -> Self {
        let time = match (matched.timestamp, matched.timestamp_span) {
            (Some((value, span)), _) => HeaderTime::Known { value, span },
            (None, Some(span)) => HeaderTime::Invalid { span: Some(span) },
            (None, None) if expects_time => HeaderTime::Missing,
            (None, None) => HeaderTime::Missing,
        };
        Self::Header {
            time,
            header_span: matched.header_span,
            message_span: matched.message_span,
        }
    }
}

/// Two-bit state stored per physical line. Explicit versus inherited known
/// time is derived from the independent record-start bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LineTimeState {
    Unknown = 0,
    Known = 1,
    MissingHeader = 2,
    InvalidHeader = 3,
}

impl LineTimeState {
    pub fn from_bits(bits: u8) -> Self {
        match bits & 0b11 {
            1 => Self::Known,
            2 => Self::MissingHeader,
            3 => Self::InvalidHeader,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeProvenance {
    Explicit,
    Inherited,
    Missing,
    Invalid,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassifiedLine {
    pub timestamp: Option<i64>,
    pub timestamp_span: Option<Range<usize>>,
    pub record_start: bool,
    pub time_state: LineTimeState,
    pub provenance: TimeProvenance,
}

#[derive(Clone, Debug, Default)]
pub struct RecordStateMachine {
    has_active_record: bool,
    active_time: Option<i64>,
}

impl RecordStateMachine {
    pub fn with_prior_record(active_time: Option<i64>) -> Self {
        Self {
            has_active_record: true,
            active_time,
        }
    }

    pub fn classify(&mut self, classification: &RecordClassification) -> ClassifiedLine {
        match classification {
            RecordClassification::Header { time, .. } => {
                self.has_active_record = true;
                match time {
                    HeaderTime::Known { value, span } => {
                        self.active_time = Some(*value);
                        ClassifiedLine {
                            timestamp: Some(*value),
                            timestamp_span: Some(span.clone()),
                            record_start: true,
                            time_state: LineTimeState::Known,
                            provenance: TimeProvenance::Explicit,
                        }
                    }
                    HeaderTime::Missing => {
                        self.active_time = None;
                        ClassifiedLine {
                            timestamp: None,
                            timestamp_span: None,
                            record_start: true,
                            time_state: LineTimeState::MissingHeader,
                            provenance: TimeProvenance::Missing,
                        }
                    }
                    HeaderTime::Invalid { span } => {
                        self.active_time = None;
                        ClassifiedLine {
                            timestamp: None,
                            timestamp_span: span.clone(),
                            record_start: true,
                            time_state: LineTimeState::InvalidHeader,
                            provenance: TimeProvenance::Invalid,
                        }
                    }
                }
            }
            RecordClassification::Continuation if self.has_active_record => ClassifiedLine {
                timestamp: self.active_time,
                timestamp_span: None,
                record_start: false,
                time_state: self
                    .active_time
                    .map_or(LineTimeState::Unknown, |_| LineTimeState::Known),
                provenance: self
                    .active_time
                    .map_or(TimeProvenance::Unknown, |_| TimeProvenance::Inherited),
            },
            RecordClassification::Continuation | RecordClassification::Unassigned => {
                ClassifiedLine {
                    timestamp: None,
                    timestamp_span: None,
                    record_start: false,
                    time_state: LineTimeState::Unknown,
                    provenance: TimeProvenance::Unknown,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(time: HeaderTime) -> RecordClassification {
        RecordClassification::Header {
            time,
            header_span: 0..1,
            message_span: 1..2,
        }
    }

    #[test]
    fn missing_header_resets_inheritance() {
        let mut state = RecordStateMachine::default();
        state.classify(&header(HeaderTime::Known {
            value: 10,
            span: 0..1,
        }));
        assert_eq!(
            state
                .classify(&RecordClassification::Continuation)
                .provenance,
            TimeProvenance::Inherited
        );
        let missing = state.classify(&header(HeaderTime::Missing));
        assert_eq!(missing.time_state, LineTimeState::MissingHeader);
        let continuation = state.classify(&RecordClassification::Continuation);
        assert_eq!(continuation.timestamp, None);
        assert_eq!(continuation.provenance, TimeProvenance::Unknown);
    }

    #[test]
    fn preamble_remains_unassigned_and_invalid_header_starts_record() {
        let mut state = RecordStateMachine::default();
        let preamble = state.classify(&RecordClassification::Unassigned);
        assert!(!preamble.record_start);
        let invalid = state.classify(&header(HeaderTime::Invalid { span: Some(2..5) }));
        assert!(invalid.record_start);
        assert_eq!(invalid.provenance, TimeProvenance::Invalid);
        assert_eq!(invalid.timestamp_span, Some(2..5));
    }
}
