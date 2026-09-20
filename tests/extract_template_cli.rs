use std::fs;
use std::process::Command;

use haystack::{TemplateSequence, TemplateStore};
use serde_json::Value;

fn run_extract(
    file: &std::path::Path,
    store: &std::path::Path,
    output: &std::path::Path,
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_haystack"))
        .arg("extract_template")
        .arg("--file")
        .arg(file)
        .arg("--store")
        .arg(store)
        .arg("--output")
        .arg(output)
        .output()
        .expect("extract_template process starts")
}

#[test]
fn cli_extracts_a_sequence_and_reuses_persisted_template_ids() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("app.log");
    let store = temp.path().join("templates.json");
    let output = temp.path().join("sequence.json");
    fs::write(
        &source,
        "2026-09-19T10:00:00Z INFO request id=101 completed\n\
         2026-09-19T10:00:01Z INFO request id=202 completed\n\
         2026-09-19T10:00:02Z ERROR connection refused\n",
    )
    .unwrap();

    let first = run_extract(&source, &store, &output);
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let summary: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(summary["line_count"], 3);
    assert_eq!(summary["new_template_count"], 2);

    let first_sequence: TemplateSequence =
        serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    assert_eq!(first_sequence.template_ids.len(), 3);
    assert_eq!(
        first_sequence.template_ids[0],
        first_sequence.template_ids[1]
    );
    assert_ne!(
        first_sequence.template_ids[0],
        first_sequence.template_ids[2]
    );

    fs::write(
        &source,
        "2026-09-19T10:01:00Z INFO request id=303 completed\n\
         2026-09-19T10:01:01Z ERROR connection refused\n",
    )
    .unwrap();
    let second = run_extract(&source, &store, &output);
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    let second_sequence: TemplateSequence =
        serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    assert_eq!(
        second_sequence.template_ids,
        vec![
            first_sequence.template_ids[0],
            first_sequence.template_ids[2]
        ]
    );
    let persisted: TemplateStore = serde_json::from_slice(&fs::read(&store).unwrap()).unwrap();
    assert_eq!(
        persisted.templates[first_sequence.template_ids[0] as usize].count,
        3
    );
    assert_eq!(
        persisted.templates[first_sequence.template_ids[2] as usize].count,
        2
    );
}

#[test]
fn cli_reports_invalid_invocations_as_json() {
    let result = Command::new(env!("CARGO_BIN_EXE_haystack"))
        .arg("extract_template")
        .arg("--file")
        .arg("only-source.log")
        .output()
        .expect("extract_template process starts");

    assert_eq!(result.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(error["schema_version"], 1);
    assert_eq!(error["error"], "--store is required");
}
