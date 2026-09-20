use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use regex::{Regex, RegexBuilder};

use super::profile::{ProfileSource, RecordProfile, RECORD_PROFILE_SCHEMA_VERSION};

#[derive(Clone, Debug)]
pub struct TemplateCompileError {
    message: String,
}

impl TemplateCompileError {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for TemplateCompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TemplateCompileError {}

#[derive(Clone)]
pub struct CompiledProfile {
    pub profile: Arc<RecordProfile>,
    pub(super) parts: Vec<Part>,
    pub(super) fields: Vec<CompiledField>,
    pub(super) captured_fields: Vec<usize>,
    pub(super) fast_path: FastPath,
    pub(super) advanced: Option<Regex>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FastPath {
    None,
    TimeMessage,
    MessageOnly,
}

#[derive(Clone, Debug)]
pub(super) enum Part {
    Literal(Vec<u8>),
    Field(usize),
}

#[derive(Clone)]
pub(super) struct CompiledField {
    pub name: Option<String>,
    pub kind: FieldKind,
    pub validator: Option<Regex>,
    pub capture_index: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FieldKind {
    Time,
    Log,
    Token,
    DelimitedText,
    Number,
    Path,
    Text,
}

impl CompiledProfile {
    pub fn compile(profile: RecordProfile) -> Result<Arc<Self>, TemplateCompileError> {
        validate_profile(&profile)?;
        let profile = Arc::new(profile);
        match &profile.source {
            ProfileSource::Template { layout } => compile_template(Arc::clone(&profile), layout),
            ProfileSource::AdvancedRegex { pattern } => {
                compile_advanced(Arc::clone(&profile), pattern)
            }
            ProfileSource::BuiltIn { adapter } => Err(TemplateCompileError::new(format!(
                "built-in adapter {adapter:?} is compiled by its format module"
            ))),
        }
    }

    pub fn field_count(&self) -> usize {
        self.captured_fields.len()
    }

    /// Declarations that validate a header but intentionally produce no
    /// retained field capture. Schema 3 currently uses this for `{ignore}`.
    pub fn ignored_field_count(&self) -> usize {
        self.fields
            .iter()
            .filter(|field| field.name.is_none())
            .count()
    }

    pub fn field_name(&self, index: usize) -> Option<&str> {
        self.captured_fields
            .get(index)
            .and_then(|field| self.fields.get(*field))
            .and_then(|field| field.name.as_deref())
    }

    pub fn field_type_label(&self, index: usize) -> Option<&'static str> {
        self.captured_fields
            .get(index)
            .and_then(|field| self.fields.get(*field))
            .map(|field| match field.kind {
                FieldKind::Time => "timestamp",
                FieldKind::Log => "message",
                FieldKind::Token => "token",
                FieldKind::Number => "number",
                FieldKind::Path => "path",
                FieldKind::Text | FieldKind::DelimitedText => "text",
            })
    }

    pub fn field_index(&self, name: &str) -> Option<usize> {
        self.captured_fields.iter().position(|index| {
            let field = &self.fields[*index];
            field.name.as_deref() == Some(name)
        })
    }
}

fn validate_profile(profile: &RecordProfile) -> Result<(), TemplateCompileError> {
    if profile
        .yearless_year
        .is_some_and(|year| !(1..=9999).contains(&year))
    {
        return Err(TemplateCompileError::new(
            "yearless year must be between 1 and 9999",
        ));
    }
    if profile.schema_version != RECORD_PROFILE_SCHEMA_VERSION {
        return Err(TemplateCompileError::new(format!(
            "unsupported profile schema {}; expected 3",
            profile.schema_version
        )));
    }
    if profile.id.trim().is_empty() {
        return Err(TemplateCompileError::new("profile ID cannot be empty"));
    }
    if profile.name.trim().is_empty() {
        return Err(TemplateCompileError::new("profile name cannot be empty"));
    }
    if profile.limits.max_fields == 0 || profile.limits.max_fields > 32 {
        return Err(TemplateCompileError::new(
            "max_fields must be between 1 and 32",
        ));
    }
    if profile.limits.max_header_bytes == 0 {
        return Err(TemplateCompileError::new(
            "max_header_bytes must be greater than zero",
        ));
    }
    if !profile
        .prefix
        .fixed_indentation
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t'))
    {
        return Err(TemplateCompileError::new(
            "fixed indentation may contain only spaces and tabs",
        ));
    }
    Ok(())
}

fn compile_template(
    profile: Arc<RecordProfile>,
    layout: &str,
) -> Result<Arc<CompiledProfile>, TemplateCompileError> {
    if layout.len() > profile.limits.max_template_bytes {
        return Err(TemplateCompileError::new(format!(
            "template exceeds the {} byte limit",
            profile.limits.max_template_bytes
        )));
    }
    if layout.is_empty() {
        return Err(TemplateCompileError::new("template cannot be empty"));
    }
    if layout.starts_with(' ') || layout.starts_with('\t') {
        return Err(TemplateCompileError::new(
            "leading indentation belongs in the profile prefix policy",
        ));
    }

    let mut parts = Vec::new();
    let mut fields = Vec::new();
    let mut captured_fields = Vec::new();
    let mut literal = String::new();
    let mut chars = layout.char_indices().peekable();
    let mut names = HashSet::new();

    while let Some((_, character)) = chars.next() {
        match character {
            '{' if chars.peek().is_some_and(|(_, next)| *next == '{') => {
                chars.next();
                literal.push('{');
            }
            '}' if chars.peek().is_some_and(|(_, next)| *next == '}') => {
                chars.next();
                literal.push('}');
            }
            '{' => {
                if !literal.is_empty() {
                    parts.push(Part::Literal(std::mem::take(&mut literal).into_bytes()));
                }
                let mut name = String::new();
                let mut closed = false;
                for (_, next) in chars.by_ref() {
                    if next == '}' {
                        closed = true;
                        break;
                    }
                    if next == '{' {
                        return Err(TemplateCompileError::new(
                            "nested opening brace in placeholder",
                        ));
                    }
                    name.push(next);
                }
                if !closed {
                    return Err(TemplateCompileError::new("unclosed placeholder"));
                }
                if name.is_empty() {
                    return Err(TemplateCompileError::new(
                        "placeholder name cannot be empty",
                    ));
                }
                let inline = true;
                let (field_name, type_name) = if inline {
                    name.split_once(':')
                        .map_or((name.as_str(), None), |(field, kind)| (field, Some(kind)))
                } else {
                    (name.as_str(), None)
                };
                if inline && !valid_inline_name(field_name) {
                    return Err(TemplateCompileError::new(format!(
                        "invalid field name {{{name}}}; use letters, digits, and underscores, starting with a letter or underscore"
                    )));
                }
                let identity = field_name;
                let ignored = field_name == "ignore";
                if !ignored && !names.insert(identity.to_string()) {
                    return Err(TemplateCompileError::new(format!(
                        "duplicate field {{{field_name}}}"
                    )));
                }
                if fields.len() >= profile.limits.max_fields {
                    return Err(TemplateCompileError::new(format!(
                        "template exceeds the {} field limit",
                        profile.limits.max_fields
                    )));
                }
                let (kind, validator) = compile_inline_field(field_name, type_name)?;
                let field_index = fields.len();
                let capture_index = (!ignored).then_some(captured_fields.len());
                fields.push(CompiledField {
                    name: (!ignored).then(|| field_name.to_string()),
                    kind,
                    validator,
                    capture_index,
                });
                if !ignored {
                    captured_fields.push(field_index);
                }
                parts.push(Part::Field(field_index));
            }
            '}' => {
                return Err(TemplateCompileError::new(
                    "unescaped closing brace; use }} for a literal brace",
                ));
            }
            other => literal.push(other),
        }
    }
    if !literal.is_empty() {
        parts.push(Part::Literal(literal.into_bytes()));
    }

    validate_parts(&parts, &fields, true)?;
    let fast_path = if fields.len() == 2
        && fields[0].kind == FieldKind::Time
        && fields[1].kind == FieldKind::Log
        && matches!(&parts[..], [Part::Field(0), Part::Literal(space), Part::Field(1)] if space == b" ")
    {
        FastPath::TimeMessage
    } else if fields.len() == 1
        && fields[0].kind == FieldKind::Log
        && matches!(&parts[..], [Part::Field(0)])
    {
        FastPath::MessageOnly
    } else {
        FastPath::None
    };

    Ok(Arc::new(CompiledProfile {
        profile,
        parts,
        fields,
        captured_fields,
        fast_path,
        advanced: None,
    }))
}

fn compile_advanced(
    profile: Arc<RecordProfile>,
    pattern: &str,
) -> Result<Arc<CompiledProfile>, TemplateCompileError> {
    if pattern.len() > profile.limits.max_template_bytes {
        return Err(TemplateCompileError::new(format!(
            "advanced pattern exceeds the {} byte limit",
            profile.limits.max_template_bytes
        )));
    }
    if !pattern.starts_with(r"\A") {
        return Err(TemplateCompileError::new(
            "advanced header patterns must begin with \\A",
        ));
    }
    let regex = RegexBuilder::new(pattern)
        .size_limit(2 * 1024 * 1024)
        .dfa_size_limit(2 * 1024 * 1024)
        .build()
        .map_err(|error| TemplateCompileError::new(format!("invalid advanced regex: {error}")))?;
    let fields = regex
        .capture_names()
        .flatten()
        .enumerate()
        .map(|(capture_index, name)| CompiledField {
            name: Some(name.to_string()),
            kind: match name {
                "timestamp" => FieldKind::Time,
                "log" => FieldKind::Log,
                _ => FieldKind::DelimitedText,
            },
            validator: None,
            capture_index: Some(capture_index),
        })
        .collect::<Vec<_>>();
    if fields.len() > profile.limits.max_fields {
        return Err(TemplateCompileError::new(format!(
            "advanced pattern exceeds the {} capture limit",
            profile.limits.max_fields
        )));
    }

    Ok(Arc::new(CompiledProfile {
        profile,
        parts: Vec::new(),
        captured_fields: (0..fields.len()).collect(),
        fields,
        fast_path: FastPath::None,
        advanced: Some(regex),
    }))
}

fn valid_inline_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn compile_inline_field(
    name: &str,
    type_name: Option<&str>,
) -> Result<(FieldKind, Option<Regex>), TemplateCompileError> {
    match name {
        "time" if type_name.is_none() => Ok((FieldKind::Time, None)),
        "log" if type_name.is_none() => Ok((FieldKind::Log, None)),
        "time" | "log" => Err(TemplateCompileError::new(format!(
            "{{{name}}} cannot have an inline type"
        ))),
        _ => match type_name.unwrap_or("token") {
            "token" => Ok((FieldKind::Token, None)),
            "number" => Ok((FieldKind::Number, None)),
            "path" => Ok((FieldKind::Path, None)),
            "text" => Ok((FieldKind::Text, None)),
            selected => Err(TemplateCompileError::new(format!(
                "unknown type {selected:?}; choose token, number, path, or text"
            ))),
        },
    }
}

fn validate_parts(
    parts: &[Part],
    fields: &[CompiledField],
    inline: bool,
) -> Result<(), TemplateCompileError> {
    let log_fields = fields
        .iter()
        .enumerate()
        .filter(|(_, field)| field.kind == FieldKind::Log)
        .collect::<Vec<_>>();
    if log_fields.len() != 1 {
        return Err(TemplateCompileError::new(
            "a text template must contain exactly one {log} field",
        ));
    }
    if inline && !fields.iter().any(|field| field.kind == FieldKind::Time) {
        return Err(TemplateCompileError::new(
            "add {time}; every new format needs a timestamp",
        ));
    }
    if !matches!(parts.last(), Some(Part::Field(index)) if fields[*index].kind == FieldKind::Log) {
        return Err(TemplateCompileError::new(
            "{log} must be the final template element",
        ));
    }
    if fields
        .iter()
        .filter(|field| field.kind == FieldKind::Time)
        .count()
        > 1
    {
        return Err(TemplateCompileError::new(
            "a template may contain at most one {time} field",
        ));
    }
    for window in parts.windows(2) {
        if matches!(window, [Part::Field(_), Part::Field(_)]) {
            return Err(TemplateCompileError::new(
                "adjacent variable fields are ambiguous; add a literal delimiter",
            ));
        }
    }
    let broad_fields = fields
        .iter()
        .filter(|field| matches!(field.kind, FieldKind::DelimitedText))
        .count();
    if !inline && broad_fields > 1 {
        return Err(TemplateCompileError::new(
            "multiple delimited-text fields can require unbounded segmentation; use token fields or an advanced anchored rule",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_default_and_escaped_literals() {
        let default = CompiledProfile::compile(RecordProfile::default_text()).unwrap();
        assert_eq!(default.fast_path, FastPath::TimeMessage);

        let escaped =
            RecordProfile::text("custom:escaped", "Escaped", "{time} {{{thread_id}}} {log}");
        let compiled = CompiledProfile::compile(escaped).unwrap();
        assert_eq!(compiled.field_count(), 3);
        assert!(compiled
            .parts
            .iter()
            .any(|part| matches!(part, Part::Literal(value) if value.contains(&b'{'))));
    }

    #[test]
    fn rejects_ambiguous_or_invalid_layouts() {
        for (layout, expected) in [
            ("{time}", "exactly one {log}"),
            ("{time} {log} suffix", "{log} must be the final"),
            ("{time} {thread_id}{log_level} {log}", "adjacent variable"),
            ("{time} {time} {log}", "duplicate field"),
        ] {
            let error = CompiledProfile::compile(RecordProfile::text("id", "name", layout))
                .err()
                .unwrap()
                .to_string();
            assert!(error.contains(expected), "{layout}: {error}");
        }
    }

    #[test]
    fn legacy_templates_default_undeclared_fields_to_tokens() {
        let compiled = CompiledProfile::compile(RecordProfile::text(
            "legacy:implicit-token",
            "Implicit token",
            "{time} [{component}] {log}",
        ))
        .unwrap();
        assert_eq!(compiled.field_type_label(1), Some("token"));
    }

    #[test]
    fn compiles_simple_positional_token_fields() {
        let profile = RecordProfile::text(
            "custom:ios",
            "iOS",
            "{time} MyApp[{a}:{b}] <{log_level}> {file}:{line} {log}",
        );
        let compiled = CompiledProfile::compile(profile).unwrap();
        assert_eq!(compiled.field_name(1), Some("a"));
        assert_eq!(compiled.field_name(2), Some("b"));
    }

    #[test]
    fn advanced_patterns_must_be_anchored() {
        let mut profile = RecordProfile::default_text();
        profile.source = ProfileSource::AdvancedRegex {
            pattern: r".*(?P<timestamp>\d{4}-\d{2}-\d{2})".into(),
        };
        let error = match CompiledProfile::compile(profile) {
            Ok(_) => panic!("unanchored advanced pattern compiled"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("must begin with \\A"));
    }
}
