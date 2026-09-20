use serde::{Deserialize, Serialize};

/// The single, unreleased log-format grammar. Ordinary fields default to
/// tokens and `{ignore}` matches without retaining a field.
pub const RECORD_PROFILE_SCHEMA_VERSION: u32 = 3;

/// Persisted record-profile definition. The ID identifies a preset while the
/// revision invalidates compiled/cache state after an edit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordProfile {
    pub schema_version: u32,
    pub id: String,
    pub revision: u32,
    pub name: String,
    pub source: ProfileSource,
    pub timestamp: TimestampSelection,
    /// Optional explicit calendar year for yearless timestamp families.
    /// Without it, the document pins the loading-time reference once.
    #[serde(default)]
    pub yearless_year: Option<i32>,
    #[serde(default)]
    pub prefix: PrefixPolicy,
    #[serde(default)]
    pub custom_fields: Vec<CustomFieldDefinition>,
    #[serde(default)]
    pub log_levels: LogLevelPolicy,
    #[serde(default)]
    pub limits: ProfileLimits,
}

impl RecordProfile {
    pub fn text(id: impl Into<String>, name: impl Into<String>, layout: impl Into<String>) -> Self {
        Self {
            schema_version: RECORD_PROFILE_SCHEMA_VERSION,
            id: id.into(),
            revision: 1,
            name: name.into(),
            source: ProfileSource::Template {
                layout: layout.into(),
            },
            timestamp: TimestampSelection::Auto,
            yearless_year: None,
            prefix: PrefixPolicy::default(),
            custom_fields: Vec::new(),
            log_levels: LogLevelPolicy::default(),
            limits: ProfileLimits::default(),
        }
    }

    /// Create a self-contained log format.
    pub fn inline(
        id: impl Into<String>,
        name: impl Into<String>,
        layout: impl Into<String>,
    ) -> Self {
        Self::text(id, name, layout)
    }

    /// Alias retained for internal call sites during the refinement work.
    pub fn refined_inline(
        id: impl Into<String>,
        name: impl Into<String>,
        layout: impl Into<String>,
    ) -> Self {
        Self::text(id, name, layout)
    }

    pub fn default_text() -> Self {
        Self::text(
            "builtin:plain-leading-time",
            "Timestamp + message",
            "{time} {log}",
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileSource {
    Template { layout: String },
    AdvancedRegex { pattern: String },
    BuiltIn { adapter: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum TimestampSelection {
    Auto,
    BuiltIn(String),
    Custom(String),
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrefixPolicy {
    #[serde(default = "default_true")]
    pub allow_file_bom: bool,
    #[serde(default = "default_true")]
    pub allow_terminal_color: bool,
    #[serde(default)]
    pub fixed_indentation: String,
}

impl Default for PrefixPolicy {
    fn default() -> Self {
        Self {
            allow_file_bom: true,
            allow_terminal_color: true,
            fixed_indentation: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileLimits {
    pub max_template_bytes: usize,
    pub max_fields: usize,
    pub max_header_bytes: usize,
}

impl Default for ProfileLimits {
    fn default() -> Self {
        Self {
            max_template_bytes: 4 * 1024,
            max_fields: 32,
            max_header_bytes: 4 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomFieldDefinition {
    pub name: String,
    pub kind: CustomFieldKind,
    #[serde(default)]
    pub validator: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomFieldKind {
    Token,
    Integer,
    DelimitedText,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLevelPolicy {
    #[serde(default = "default_log_levels")]
    pub values: Vec<String>,
    #[serde(default = "default_true")]
    pub case_sensitive: bool,
}

impl Default for LogLevelPolicy {
    fn default() -> Self {
        Self {
            values: default_log_levels(),
            case_sensitive: true,
        }
    }
}

fn default_log_levels() -> Vec<String> {
    [
        "TRACE", "DEBUG", "INFO", "NOTICE", "WARN", "WARNING", "ERROR", "FATAL", "FAULT",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_round_trips_and_defaults_prefix_policy() {
        let profile = RecordProfile::default_text();
        let json = serde_json::to_string(&profile).unwrap();
        assert_eq!(
            serde_json::from_str::<RecordProfile>(&json).unwrap(),
            profile
        );

        let legacy = r#"{
            "schema_version":1,
            "id":"custom:one",
            "revision":1,
            "name":"One",
            "source":{"kind":"template","layout":"{time} {log}"},
            "timestamp":{"kind":"auto"}
        }"#;
        let decoded: RecordProfile = serde_json::from_str(legacy).unwrap();
        assert_eq!(decoded.prefix, PrefixPolicy::default());
        assert_eq!(decoded.limits, ProfileLimits::default());
    }
}
