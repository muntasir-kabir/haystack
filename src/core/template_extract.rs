//! Standalone, persistent template extraction for shell workflows.
//!
//! The extractor deliberately uses the same record-profile, masking, and
//! native Drain pipeline as the GUI.  Its JSON store restores Drain clusters
//! before each run, which makes an existing template retain its ID while new
//! shapes are appended.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::document::{LogDocument, ParsingConfig};
use crate::core::drain::{Drain, LogCluster};
use crate::core::record::{CompiledProfile, RecordProfile};

pub const TEMPLATE_STORE_SCHEMA_VERSION: u32 = 1;
pub const TEMPLATE_OUTPUT_SCHEMA_VERSION: u32 = 1;

const MAX_CLUSTERS: usize = 20_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTemplate {
    pub id: u32,
    pub pattern: String,
    pub count: usize,
    pub example_line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateStore {
    pub schema_version: u32,
    pub templates: Vec<StoredTemplate>,
}

impl Default for TemplateStore {
    fn default() -> Self {
        Self {
            schema_version: TEMPLATE_STORE_SCHEMA_VERSION,
            templates: vec![StoredTemplate {
                id: 0,
                pattern: "<EMPTY>".to_string(),
                count: 0,
                example_line: 0,
            }],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateSequence {
    pub schema_version: u32,
    pub source_file: String,
    pub format: String,
    pub line_count: usize,
    pub template_ids: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExtractionSummary {
    pub schema_version: u32,
    pub file: String,
    pub store: String,
    pub output: String,
    pub line_count: usize,
    pub template_count: usize,
    pub new_template_count: usize,
}

/// Mine `file`, update the JSON `store`, and write one template ID for every
/// physical source line to `output`.  The store and output are separately
/// replaced atomically, so either file is always valid JSON.
pub fn extract_templates(
    file: &Path,
    format: &str,
    store_path: &Path,
    output_path: &Path,
) -> Result<ExtractionSummary, String> {
    if format.trim().is_empty() {
        return Err("format must not be empty".to_string());
    }
    if file == store_path || file == output_path || store_path == output_path {
        return Err("file, store, and output paths must be distinct".to_string());
    }

    let store = load_store(store_path)?;
    let old_template_count = store.templates.len();
    let drain = Drain::from_clusters(
        ParsingConfig::default().drain_depth,
        ParsingConfig::default().sim_threshold,
        100,
        MAX_CLUSTERS,
        store_to_clusters(&store)?,
    )?;
    let profile = CompiledProfile::compile(RecordProfile::inline(
        "cli:extract-template",
        "Standalone template extraction",
        format,
    ))
    .map_err(|error| format!("invalid format: {error}"))?;
    let document = LogDocument::open_with_record_profile_and_drain(
        file,
        ParsingConfig::default(),
        &[],
        profile,
        drain,
    )?;

    let updated_store = TemplateStore {
        schema_version: TEMPLATE_STORE_SCHEMA_VERSION,
        templates: document
            .templates
            .iter()
            .map(|template| StoredTemplate {
                id: template.id,
                pattern: template.pattern.clone(),
                count: template.count,
                example_line: template.example_line,
            })
            .collect(),
    };
    let sequence = TemplateSequence {
        schema_version: TEMPLATE_OUTPUT_SCHEMA_VERSION,
        source_file: file.to_string_lossy().into_owned(),
        format: format.to_string(),
        line_count: document.total_lines(),
        template_ids: (0..document.total_lines())
            .map(|line| document.template_at(line))
            .collect(),
    };

    write_json(store_path, &updated_store)?;
    write_json(output_path, &sequence)?;

    Ok(ExtractionSummary {
        schema_version: TEMPLATE_OUTPUT_SCHEMA_VERSION,
        file: file.to_string_lossy().into_owned(),
        store: store_path.to_string_lossy().into_owned(),
        output: output_path.to_string_lossy().into_owned(),
        line_count: sequence.line_count,
        template_count: updated_store.templates.len(),
        new_template_count: updated_store
            .templates
            .len()
            .saturating_sub(old_template_count),
    })
}

fn load_store(path: &Path) -> Result<TemplateStore, String> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let store: TemplateStore = serde_json::from_str(&text).map_err(|error| {
                format!("failed to parse template store {}: {error}", path.display())
            })?;
            if store.schema_version != TEMPLATE_STORE_SCHEMA_VERSION {
                return Err(format!(
                    "unsupported template store schema {}; expected {TEMPLATE_STORE_SCHEMA_VERSION}",
                    store.schema_version
                ));
            }
            Ok(store)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(TemplateStore::default()),
        Err(error) => Err(format!(
            "failed to read template store {}: {error}",
            path.display()
        )),
    }
}

fn store_to_clusters(store: &TemplateStore) -> Result<Vec<LogCluster>, String> {
    store
        .templates
        .iter()
        .map(|template| {
            let tokens: Vec<String> = template
                .pattern
                .split_whitespace()
                .map(str::to_string)
                .collect();
            if tokens.is_empty() {
                return Err(format!("template {} has an empty pattern", template.id));
            }
            Ok(LogCluster {
                id: template.id,
                template: tokens,
                size: template.count,
                example_line: template.example_line,
            })
        })
        .collect()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("failed to encode JSON for {}: {error}", path.display()))?;
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    fs::write(&temporary, bytes)
        .map_err(|error| format!("failed to write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("failed to replace {}: {error}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_paths(label: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "haystack-template-extract-{label}-{}",
            std::process::id()
        ));
        (
            base.with_extension("log"),
            base.with_extension("store.json"),
            base.with_extension("output.json"),
        )
    }

    #[test]
    fn persists_templates_and_reuses_ids_across_runs() {
        let (source, store, output) = test_paths("persistent");
        fs::write(
            &source,
            "2026-09-19T10:00:00Z INFO user alice connected\n2026-09-19T10:00:01Z INFO user bob connected\n2026-09-19T10:00:02Z ERROR database unavailable\n",
        )
        .unwrap();

        let first = extract_templates(&source, "{time} {log}", &store, &output).unwrap();
        assert_eq!(first.line_count, 3);
        assert_eq!(first.new_template_count, 2);
        let first_sequence: TemplateSequence =
            serde_json::from_str(&fs::read_to_string(&output).unwrap()).unwrap();
        assert_eq!(first_sequence.template_ids.len(), 3);
        assert_eq!(
            first_sequence.template_ids[0],
            first_sequence.template_ids[1]
        );

        fs::write(
            &source,
            "2026-09-19T10:01:00Z INFO user carol connected\n2026-09-19T10:01:01Z ERROR database unavailable\n",
        )
        .unwrap();
        let second = extract_templates(&source, "{time} {log}", &store, &output).unwrap();
        let second_sequence: TemplateSequence =
            serde_json::from_str(&fs::read_to_string(&output).unwrap()).unwrap();
        assert_eq!(second.new_template_count, 0);
        assert_eq!(
            second_sequence.template_ids[0],
            first_sequence.template_ids[0]
        );
        assert_eq!(
            second_sequence.template_ids[1],
            first_sequence.template_ids[2]
        );

        let stored: TemplateStore =
            serde_json::from_str(&fs::read_to_string(&store).unwrap()).unwrap();
        assert_eq!(
            stored.templates[first_sequence.template_ids[0] as usize].count,
            3
        );
        assert_eq!(
            stored.templates[first_sequence.template_ids[2] as usize].count,
            2
        );
        let _ = fs::remove_file(source);
        let _ = fs::remove_file(store);
        let _ = fs::remove_file(output);
    }

    #[test]
    fn accepts_a_custom_record_format() {
        let (source, store, output) = test_paths("custom-format");
        fs::write(
            &source,
            "2026-09-19T10:00:00Z [api] INFO request 42 complete\n2026-09-19T10:00:01Z [api] INFO request 84 complete\n",
        )
        .unwrap();

        let summary =
            extract_templates(&source, "{time} [{component}] {log}", &store, &output).unwrap();
        let sequence: TemplateSequence =
            serde_json::from_str(&fs::read_to_string(&output).unwrap()).unwrap();
        assert_eq!(summary.new_template_count, 1);
        assert_eq!(sequence.template_ids, vec![1, 1]);
        let _ = fs::remove_file(source);
        let _ = fs::remove_file(store);
        let _ = fs::remove_file(output);
    }
}
