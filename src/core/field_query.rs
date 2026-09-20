//! Typed queries over retained, record-owned profile captures.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use chrono::DateTime;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::core::document::LogDocument;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldOperator {
    Equal,
    NotEqual,
    Contains,
    Regex,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
    Exists,
    Missing,
}

impl FieldOperator {
    pub fn label(self) -> &'static str {
        match self {
            Self::Equal => "=",
            Self::NotEqual => "!=",
            Self::Contains => "contains",
            Self::Regex => "matches",
            Self::Greater => ">",
            Self::GreaterEqual => ">=",
            Self::Less => "<",
            Self::LessEqual => "<=",
            Self::Exists => "exists",
            Self::Missing => "missing",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldQuery {
    pub field: String,
    pub operator: FieldOperator,
    #[serde(default)]
    pub value: String,
    #[serde(default = "default_true")]
    pub case_sensitive: bool,
    /// Type at creation time; prevents a format switch from reinterpreting a
    /// saved numeric query as a token comparison with the same field name.
    #[serde(default)]
    pub field_type: Option<String>,
}

fn default_true() -> bool {
    true
}

impl FieldQuery {
    /// Detect a value being typed, including an incomplete quoted value.
    pub fn completion_context(input: &str) -> Option<(&str, &str)> {
        let text = input.trim_start();
        let end = text
            .bytes()
            .position(|byte| !(byte.is_ascii_alphanumeric() || byte == b'_'))?;
        let field = &text[..end];
        let rest = text[end..].trim_start();
        let tail = [">=", "<=", "!=", "=", ">", "<", "contains", "matches"]
            .into_iter()
            .find_map(|operator| rest.strip_prefix(operator))?
            .trim_start();
        let prefix = tail.strip_prefix('"').unwrap_or(tail).trim_end_matches('"');
        Some((field, prefix))
    }

    /// Parse only in explicit Field mode. Ordinary text input never reaches
    /// this parser, even when it happens to contain `=` or `contains`.
    pub fn parse(input: &str, case_sensitive: bool) -> Result<Self, String> {
        let text = input.trim();
        let name_end = text
            .bytes()
            .position(|byte| !(byte.is_ascii_alphanumeric() || byte == b'_'))
            .unwrap_or(text.len());
        let field = &text[..name_end];
        if field.is_empty()
            || !field.as_bytes()[0].is_ascii_alphabetic() && field.as_bytes()[0] != b'_'
        {
            return Err("Start with a field name, for example `b >= 9`.".into());
        }
        let rest = text[name_end..].trim_start();
        let (operator, value_text) = [
            (">=", FieldOperator::GreaterEqual),
            ("<=", FieldOperator::LessEqual),
            ("!=", FieldOperator::NotEqual),
            ("=", FieldOperator::Equal),
            (">", FieldOperator::Greater),
            ("<", FieldOperator::Less),
        ]
        .into_iter()
        .find_map(|(spelling, operator)| rest.strip_prefix(spelling).map(|tail| (operator, tail)))
        .or_else(|| {
            [
                ("contains", FieldOperator::Contains),
                ("matches", FieldOperator::Regex),
                ("exists", FieldOperator::Exists),
                ("missing", FieldOperator::Missing),
            ]
            .into_iter()
            .find_map(|(spelling, operator)| {
                rest.strip_prefix(spelling)
                    .filter(|tail| tail.is_empty() || tail.starts_with(char::is_whitespace))
                    .map(|tail| (operator, tail))
            })
        })
        .ok_or("Use =, !=, contains, matches, >, >=, <, <=, exists, or missing.")?;
        let value_text = value_text.trim();
        let unary = matches!(operator, FieldOperator::Exists | FieldOperator::Missing);
        if unary && !value_text.is_empty() {
            return Err("exists and missing do not take a value.".into());
        }
        if !unary && value_text.is_empty() {
            return Err("Enter a value after the operator.".into());
        }
        let value = if value_text.starts_with('"') {
            serde_json::from_str::<String>(value_text).map_err(|_| {
                "Use a complete quoted string, such as \"CameraService\".".to_string()
            })?
        } else {
            value_text.to_owned()
        };
        Ok(Self {
            field: field.to_owned(),
            operator,
            value,
            case_sensitive,
            field_type: None,
        })
    }

    pub fn expression(&self) -> String {
        let operator = self.operator.label();
        if matches!(
            self.operator,
            FieldOperator::Exists | FieldOperator::Missing
        ) {
            format!("{} {operator}", self.field)
        } else {
            let quoted = serde_json::to_string(&self.value).unwrap_or_default();
            format!("{} {operator} {quoted}", self.field)
        }
    }

    pub fn compile(&self, doc: &LogDocument) -> Result<CompiledFieldQuery, String> {
        let kind = doc
            .record_field_type(&self.field)
            .ok_or_else(|| format!("Field `{}` is not in the applied log format.", self.field))?;
        if self
            .field_type
            .as_deref()
            .is_some_and(|expected| expected != kind)
        {
            return Err(format!(
                "Field `{}` changed type; review this query.",
                self.field
            ));
        }
        let numeric = matches!(kind, "integer" | "number");
        let timestamp = kind == "timestamp";
        let relational = matches!(
            self.operator,
            FieldOperator::Greater
                | FieldOperator::GreaterEqual
                | FieldOperator::Less
                | FieldOperator::LessEqual
        );
        if relational && !numeric && !timestamp {
            return Err(format!(
                "{} is a {kind} field; ordering requires a number or time field.",
                self.field
            ));
        }
        if matches!(
            self.operator,
            FieldOperator::Contains | FieldOperator::Regex
        ) && (numeric || timestamp)
        {
            return Err(format!(
                "{} is a {kind} field; use a numeric/time comparison.",
                self.field
            ));
        }
        let target = if matches!(
            self.operator,
            FieldOperator::Exists | FieldOperator::Missing
        ) {
            FieldTarget::None
        } else if numeric {
            FieldTarget::Number(
                Decimal::parse(&self.value)
                    .ok_or_else(|| format!("Expected a number for {}.", self.field))?,
            )
        } else if timestamp {
            let time = parse_zoned_time(&self.value).ok_or_else(|| {
                "Use an RFC3339 timestamp with a timezone for time comparisons.".to_string()
            })?;
            FieldTarget::Time(time)
        } else if self.operator == FieldOperator::Regex {
            if self.value.len() > 4096 {
                return Err("Field regex is limited to 4096 bytes.".into());
            }
            FieldTarget::Regex(
                RegexBuilder::new(&self.value)
                    .case_insensitive(!self.case_sensitive)
                    .size_limit(1 << 20)
                    .dfa_size_limit(1 << 20)
                    .build()
                    .map_err(|error| error.to_string())?,
            )
        } else {
            FieldTarget::Text(self.value.clone())
        };
        Ok(CompiledFieldQuery {
            query: self.clone(),
            target,
            is_message: kind == "message",
        })
    }

    pub fn bind_to_doc(mut self, doc: &LogDocument) -> Result<Self, String> {
        self.compile(doc)?;
        if self.field_type.is_none() {
            self.field_type = doc.record_field_type(&self.field).map(str::to_owned);
        }
        Ok(self)
    }
}

enum FieldTarget {
    None,
    Text(String),
    Number(Decimal),
    Time((i64, u32)),
    Regex(Regex),
}

pub struct CompiledFieldQuery {
    query: FieldQuery,
    target: FieldTarget,
    is_message: bool,
}

impl CompiledFieldQuery {
    /// `original_line` may be a continuation; ownership resolves to its header.
    pub fn matches(&self, doc: &LogDocument, original_line: usize) -> Result<bool, String> {
        if doc.record_range_containing(original_line).is_none() {
            return Ok(false);
        }
        let span = doc.record_field_span(original_line, &self.query.field);
        if self.query.operator == FieldOperator::Exists {
            return Ok(span.is_some());
        }
        if self.query.operator == FieldOperator::Missing {
            return Ok(span.is_none());
        }
        let Some(span) = span else {
            return Ok(false);
        };
        let relation = match &self.target {
            FieldTarget::Number(target) => {
                let Some(value) = doc.source_bytes(span) else {
                    return Ok(false);
                };
                let Some(value) = std::str::from_utf8(value).ok().and_then(Decimal::parse) else {
                    return Ok(false);
                };
                Some(value.cmp(target))
            }
            FieldTarget::Time(target) => {
                let precise = doc
                    .source_bytes(span)
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    .and_then(parse_zoned_time);
                let time = precise.or_else(|| {
                    doc.explicit_time_untrimmed(original_line).map(|millis| {
                        (
                            millis.div_euclid(1000),
                            millis.rem_euclid(1000) as u32 * 1_000_000,
                        )
                    })
                });
                time.map(|time| time.cmp(target))
            }
            FieldTarget::Text(target) => {
                let bytes = self.field_bytes(doc, original_line, span)?;
                let value = String::from_utf8_lossy(&bytes);
                let (value, target) = if self.query.case_sensitive {
                    (value.into_owned(), target.clone())
                } else {
                    (value.to_lowercase(), target.to_lowercase())
                };
                match self.query.operator {
                    FieldOperator::Contains => return Ok(value.contains(&target)),
                    _ => Some(value.cmp(&target)),
                }
            }
            FieldTarget::Regex(regex) => {
                let bytes = self.field_bytes(doc, original_line, span)?;
                return Ok(regex.is_match(&String::from_utf8_lossy(&bytes)));
            }
            FieldTarget::None => return Ok(false),
        };
        Ok(relation.is_some_and(|order| match self.query.operator {
            FieldOperator::Equal => order == Ordering::Equal,
            FieldOperator::NotEqual => order != Ordering::Equal,
            FieldOperator::Greater => order == Ordering::Greater,
            FieldOperator::GreaterEqual => order != Ordering::Less,
            FieldOperator::Less => order == Ordering::Less,
            FieldOperator::LessEqual => order != Ordering::Greater,
            _ => false,
        }))
    }

    fn field_bytes(
        &self,
        doc: &LogDocument,
        original_line: usize,
        span: std::ops::Range<u64>,
    ) -> Result<Vec<u8>, String> {
        if !self.is_message {
            return Ok(doc.source_bytes(span).unwrap_or_default().to_vec());
        }
        let segments = doc
            .record_field_segments(original_line, &self.query.field)
            .unwrap_or_default();
        let bytes = segments
            .iter()
            .map(|range| range.end.saturating_sub(range.start) as usize)
            .sum::<usize>()
            .saturating_add(segments.len().saturating_sub(1));
        if bytes > 8 * 1024 * 1024 {
            return Err("A single log field exceeds the 8 MiB query limit; narrow the log or use text search.".into());
        }
        let mut value = Vec::with_capacity(bytes);
        for (index, segment) in segments.into_iter().enumerate() {
            if index > 0 {
                value.push(b'\n');
            }
            value.extend_from_slice(doc.source_bytes(segment).unwrap_or_default());
        }
        Ok(value)
    }
}

/// Use the full fractional precision present in a zoned capture. Other
/// supported timestamp families fall back to the document's millisecond index.
fn parse_zoned_time(value: &str) -> Option<(i64, u32)> {
    let time = DateTime::parse_from_rfc3339(value)
        .or_else(|_| DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f%z"))
        .ok()?;
    Some((time.timestamp(), time.timestamp_subsec_nanos()))
}

/// Decimal order without f64 conversion, preserving large integers and exponents.
#[derive(Clone, Debug)]
struct Decimal {
    sign: i8,
    digits: Vec<u8>,
    order: BigSigned,
}

impl PartialEq for Decimal {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Decimal {}

impl Decimal {
    fn parse(input: &str) -> Option<Self> {
        let raw = input.as_bytes();
        let (sign, raw) = match raw.first()? {
            b'-' => (-1, &raw[1..]),
            b'+' => (1, &raw[1..]),
            _ => (1, raw),
        };
        let exponent_at = raw.iter().position(|byte| matches!(byte, b'e' | b'E'));
        let (mantissa, exponent) =
            exponent_at.map_or((raw, &b"0"[..]), |at| (&raw[..at], &raw[at + 1..]));
        let dot = mantissa.iter().position(|byte| *byte == b'.');
        let fractional = dot.map_or(0, |at| mantissa.len() - at - 1);
        let mut digits = Vec::with_capacity(mantissa.len());
        for (index, &byte) in mantissa.iter().enumerate() {
            if Some(index) == dot {
                continue;
            }
            if !byte.is_ascii_digit() {
                return None;
            }
            digits.push(byte);
        }
        if digits.is_empty() {
            return None;
        }
        let mut order = BigSigned::parse(exponent)?;
        let first = digits.iter().position(|byte| *byte != b'0');
        let Some(first) = first else {
            return Some(Self {
                sign: 0,
                digits: vec![b'0'],
                order: BigSigned::zero(),
            });
        };
        digits.drain(..first);
        order.add_small(digits.len() as i64 - fractional as i64);
        Some(Self {
            sign,
            digits,
            order,
        })
    }
}

impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.sign.cmp(&other.sign) {
            Ordering::Equal if self.sign == 0 => Ordering::Equal,
            Ordering::Equal => {
                let abs = self.order.cmp(&other.order).then_with(|| {
                    (0..self.digits.len().max(other.digits.len()))
                        .map(|index| {
                            self.digits
                                .get(index)
                                .copied()
                                .unwrap_or(b'0')
                                .cmp(&other.digits.get(index).copied().unwrap_or(b'0'))
                        })
                        .find(|order| *order != Ordering::Equal)
                        .unwrap_or(Ordering::Equal)
                });
                if self.sign < 0 {
                    abs.reverse()
                } else {
                    abs
                }
            }
            order => order,
        }
    }
}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BigSigned {
    sign: i8,
    digits: Vec<u8>,
}

impl BigSigned {
    fn zero() -> Self {
        Self {
            sign: 0,
            digits: vec![0],
        }
    }

    fn parse(input: &[u8]) -> Option<Self> {
        let (sign, input) = match input.first()? {
            b'-' => (-1, &input[1..]),
            b'+' => (1, &input[1..]),
            _ => (1, input),
        };
        if input.is_empty() || !input.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let first = input.iter().position(|byte| *byte != b'0');
        Some(first.map_or_else(Self::zero, |at| Self {
            sign,
            digits: input[at..].iter().map(|byte| byte - b'0').collect(),
        }))
    }

    fn add_small(&mut self, value: i64) {
        if value == 0 {
            return;
        }
        let other = Self::parse(value.to_string().as_bytes()).unwrap();
        if self.sign == 0 {
            *self = other;
            return;
        }
        if self.sign == other.sign {
            self.digits = add_magnitude(&self.digits, &other.digits);
        } else {
            match cmp_magnitude(&self.digits, &other.digits) {
                Ordering::Greater => self.digits = sub_magnitude(&self.digits, &other.digits),
                Ordering::Less => {
                    self.digits = sub_magnitude(&other.digits, &self.digits);
                    self.sign = other.sign;
                }
                Ordering::Equal => *self = Self::zero(),
            }
        }
    }
}

impl Ord for BigSigned {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sign.cmp(&other.sign).then_with(|| {
            let magnitude = cmp_magnitude(&self.digits, &other.digits);
            if self.sign < 0 {
                magnitude.reverse()
            } else {
                magnitude
            }
        })
    }
}
impl PartialOrd for BigSigned {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn cmp_magnitude(left: &[u8], right: &[u8]) -> Ordering {
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

fn add_magnitude(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(left.len().max(right.len()) + 1);
    let mut carry = 0;
    for index in 0..left.len().max(right.len()) {
        let digit = left
            .get(left.len().wrapping_sub(index + 1))
            .copied()
            .unwrap_or(0)
            + right
                .get(right.len().wrapping_sub(index + 1))
                .copied()
                .unwrap_or(0)
            + carry;
        out.push(digit % 10);
        carry = digit / 10;
    }
    if carry > 0 {
        out.push(carry);
    }
    out.reverse();
    out
}

fn sub_magnitude(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(left.len());
    let mut borrow = 0i8;
    for index in 0..left.len() {
        let mut digit = left[left.len() - index - 1] as i8
            - right
                .get(right.len().wrapping_sub(index + 1))
                .copied()
                .unwrap_or(0) as i8
            - borrow;
        borrow = if digit < 0 { 1 } else { 0 };
        if digit < 0 {
            digit += 10;
        }
        out.push(digit as u8);
    }
    while out.len() > 1 && out.last() == Some(&0) {
        out.pop();
    }
    out.reverse();
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldValueSuggestion {
    pub value: String,
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldValueSuggestions {
    pub values: Vec<FieldValueSuggestion>,
    pub scanned_records: usize,
    pub complete: bool,
}

/// Bounded value inventory from the active trim, intentionally evaluated
/// before filter composition. This never caps exact query results.
pub fn suggest_field_values(
    doc: &LogDocument,
    field: &str,
    prefix: &str,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<FieldValueSuggestions, String> {
    let kind = doc
        .record_field_type(field)
        .ok_or_else(|| format!("Field `{field}` is not in the applied format."))?;
    if kind == "message" {
        return Ok(FieldValueSuggestions {
            values: Vec::new(),
            scanned_records: 0,
            complete: false,
        });
    }
    let mut counts: HashMap<String, u32> = HashMap::new();
    let prefix_folded = prefix.to_lowercase();
    let mut retained_bytes = 0usize;
    let mut scanned = 0usize;
    let mut complete = true;
    let total = doc.count_records(0, doc.trim_end);
    for ordinal in 0..total {
        if ordinal % 1024 == 0 && cancel.load(AtomicOrdering::Relaxed) {
            complete = false;
            break;
        }
        let Some(start) = doc.record_start_line(ordinal) else {
            break;
        };
        let end = doc
            .record_start_line(ordinal + 1)
            .unwrap_or(doc.total_lines_untrimmed());
        if end <= doc.trim_start {
            continue;
        }
        scanned += 1;
        if scanned > 100_000 {
            complete = false;
            break;
        }
        let Some(span) = doc.record_field_span(start, field) else {
            continue;
        };
        let Some(bytes) = doc.source_bytes(span) else {
            continue;
        };
        if bytes.len() > 4096 {
            complete = false;
            continue;
        }
        let value = String::from_utf8_lossy(bytes);
        if !value.to_lowercase().starts_with(&prefix_folded) {
            continue;
        }
        if let Some(count) = counts.get_mut(value.as_ref()) {
            *count = count.saturating_add(1);
        } else if counts.len() < 8192 && retained_bytes + value.len() + 64 <= 2 * 1024 * 1024 {
            retained_bytes += value.len() + 64;
            counts.insert(value.into_owned(), 1);
        } else {
            complete = false;
        }
    }
    let mut values: Vec<_> = counts
        .into_iter()
        .map(|(value, count)| FieldValueSuggestion { value, count })
        .collect();
    values.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.value.cmp(&right.value))
    });
    values.truncate(limit.min(50));
    Ok(FieldValueSuggestions {
        values,
        scanned_records: scanned.min(100_000),
        complete,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::document::ParsingConfig;
    use crate::core::record::{CompiledProfile, RecordProfile};
    use crate::core::search::{self, FilterJoin, FilterPolarity, FilterSpec};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    #[test]
    fn parses_field_expressions_without_reinterpreting_ordinary_text() {
        let query = FieldQuery::parse("source contains \"Camera Service\"", true).unwrap();
        assert_eq!(query.field, "source");
        assert_eq!(query.value, "Camera Service");
        assert_eq!(FieldQuery::parse(&query.expression(), true).unwrap(), query);
        assert!(FieldQuery::parse("a exists something", true).is_err());
        assert!(FieldQuery::parse("b >=", true).is_err());
    }

    #[test]
    fn exact_decimal_comparison_keeps_large_numbers_and_exponents() {
        let number = |text| Decimal::parse(text).unwrap();
        assert_eq!(number("9007199254740993"), number("9.007199254740993e15"));
        assert!(number("9007199254740993") > number("9007199254740992"));
        assert_eq!(number("1.20"), number("1.2"));
        assert_eq!(number("-0e999999999999999999999"), number("0"));
        assert!(number("1e999999999999999999999") > number("1e999999999999999999998"));
        assert!(number("-1e999999999999999999999") < number("-1e999999999999999999998"));
        assert!(Decimal::parse("0efoo").is_none());
    }

    #[test]
    fn field_search_and_filter_select_record_rows_and_suggest_real_values() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "haystack_field_query_{}_{}.log",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, "preamble\n2026-07-15 22:26:39.907481+0300 pid=9007199254740993 severity=FAULT file=CameraService.swift crash\n detail\n2026-07-15 22:26:40.907481+0300 pid=2 severity=INFO file=/tmp/app.log recovered\n").unwrap();
        let profile = CompiledProfile::compile(RecordProfile::inline(
            "query:test",
            "Query",
            "{time} pid={process:number} severity={loglevel} file={source:path} {log}",
        ))
        .unwrap();
        let mut doc = LogDocument::open_with_record_profile(
            &path,
            ParsingConfig::default(),
            &[],
            Arc::clone(&profile),
            Some(crate::core::time::TimeFormatKind::BuiltIn(
                &crate::core::time::Iso,
            )),
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let check = |doc: &LogDocument, expression: &str, expected: Vec<u32>| {
            let query = FieldQuery::parse(expression, true)
                .unwrap()
                .bind_to_doc(doc)
                .unwrap();
            let spec = FilterSpec {
                text: query.expression(),
                case_sensitive: true,
                polarity: FilterPolarity::Include,
                regex: false,
                template_id: None,
                field_query: Some(query),
            };
            assert_eq!(
                search::scan_advanced(doc, &[spec.clone()], &cancel, None).unwrap()[0],
                expected
            );
            assert_eq!(
                search::find_advanced_u32(doc, None, &spec, &cancel).unwrap(),
                expected
            );
        };
        check(&doc, "process >= 9007199254740993", vec![1, 2]);
        check(&doc, "loglevel = \"FAULT\"", vec![1, 2]);
        check(&doc, "source contains \"CameraService\"", vec![1, 2]);
        check(&doc, "source matches \"Camera.*\\\\.swift\"", vec![1, 2]);
        check(&doc, "log contains \"detail\"", vec![1, 2]);
        check(&doc, "time >= \"2026-07-15T19:26:40Z\"", vec![3]);
        check(&doc, "time < \"2026-07-15T19:26:39.907482Z\"", vec![1, 2]);
        check(&doc, "process missing", vec![]);
        check(&doc, "process exists", vec![1, 2, 3]);
        check(&doc, "loglevel != \"INFO\"", vec![1, 2]);
        let field = FilterSpec {
            text: "process >= 9007199254740993".into(),
            case_sensitive: true,
            polarity: FilterPolarity::Include,
            regex: false,
            template_id: None,
            field_query: Some(
                FieldQuery::parse("process >= 9007199254740993", true)
                    .unwrap()
                    .bind_to_doc(&doc)
                    .unwrap(),
            ),
        };
        let text = FilterSpec::phrase("detail");
        let lanes =
            search::scan_advanced(&doc, &[field.clone(), text.clone()], &cancel, None).unwrap();
        assert_eq!(lanes, vec![vec![1, 2], vec![2]]);
        assert_eq!(
            search::combine_filter_matches(
                doc.total_lines(),
                &[field.clone(), text.clone()],
                &lanes,
                FilterJoin::All
            ),
            vec![2]
        );
        assert_eq!(
            search::combine_filter_matches(
                doc.total_lines(),
                &[field.clone(), text.clone()],
                &lanes,
                FilterJoin::Any
            ),
            vec![1, 2]
        );
        let mut excluded_text = text;
        excluded_text.polarity = FilterPolarity::Exclude;
        assert_eq!(
            search::combine_filter_matches(
                doc.total_lines(),
                &[field, excluded_text],
                &lanes,
                FilterJoin::Any
            ),
            vec![1]
        );
        let values = suggest_field_values(&doc, "process", "9", 10, &cancel).unwrap();
        assert_eq!(
            values.values,
            vec![FieldValueSuggestion {
                value: "9007199254740993".into(),
                count: 1
            }]
        );
        assert!(values.complete);
        doc.trim_left(2);
        check(&doc, "process >= 9007199254740993", vec![0]);
        std::fs::remove_file(path).ok();
    }
}
