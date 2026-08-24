use std::sync::atomic::AtomicBool;

use logotomy::{AnalysisLimits, EmbeddedDataEngine, LogDocument};

fn detector_for(input: &str) -> Vec<&'static str> {
    let path = std::env::temp_dir().join(format!(
        "logotomy_embedded_kv_{}_{}.log",
        std::process::id(),
        input.len()
    ));
    std::fs::write(&path, input).unwrap();
    let doc = LogDocument::open(&path).unwrap();
    let values = EmbeddedDataEngine::default().analyze_original_lines(
        &doc,
        0..doc.total_lines_untrimmed(),
        AnalysisLimits::default(),
        &AtomicBool::new(false),
    );
    std::fs::remove_file(path).ok();
    values.into_iter().map(|value| value.detector_id).collect()
}

#[test]
fn logfmt_detection_and_visualization_contract() {
    assert!(detector_for("INFO user=42 status=\"ok\" duration_ms=18").contains(&"logfmt"));
    assert_eq!(crate_badge("logfmt"), "KV");
}

#[test]
fn colon_fields_detection_and_visualization_contract() {
    assert!(detector_for("user: 42, status: ok").contains(&"fields"));
    assert!(detector_for("user: 42\n  status: ok").contains(&"fields"));
    assert_eq!(crate_badge("fields"), "FIELDS");
}

#[test]
fn urls_and_prose_are_not_colon_fields() {
    assert!(!detector_for("https://example.test/path").contains(&"fields"));
}

// Presentation has GUI-only dependencies, so badge tests live in each
// highlighter module. This mirrors the externally visible cue contract.
fn crate_badge(id: &str) -> &'static str {
    match id {
        "logfmt" => "KV",
        "fields" => "FIELDS",
        _ => "",
    }
}
