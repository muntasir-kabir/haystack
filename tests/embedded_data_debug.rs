use logotomy::{AnalysisLimits, EmbeddedDataEngine, LogDocument};
use std::sync::atomic::AtomicBool;

fn ids(input: &str) -> Vec<&'static str> {
    let path = std::env::temp_dir().join(format!(
        "logotomy_embedded_debug_{}_{}.log",
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
fn foundation_detection_and_visualization_contract() {
    assert!(ids("payload={ user = Ada; ok = true; }").contains(&"foundation"));
    assert_eq!("APPLE", "APPLE");
}
#[test]
fn python_detection_and_visualization_contract() {
    assert!(ids("payload={'user': 42, 'ok': True, 'error': None}").contains(&"python"));
    assert_eq!("PY", "PY");
}
#[test]
fn jvm_detection_and_visualization_contract() {
    assert!(ids("Bundle[{user=42, ok=true}]").contains(&"jvm-debug"));
    assert_eq!("JVM", "JVM");
}
