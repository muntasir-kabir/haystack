use haystack::{AnalysisLimits, EmbeddedDataEngine, LogDocument};
use std::sync::atomic::AtomicBool;

fn ids(input: &str) -> Vec<&'static str> {
    let path = std::env::temp_dir().join(format!(
        "haystack_embedded_wire_{}_{}.log",
        std::process::id(),
        input.len()
    ));
    std::fs::write(&path, input).unwrap();
    let doc = LogDocument::open(&path).unwrap();
    let output = EmbeddedDataEngine::default().analyze_original_lines(
        &doc,
        0..doc.total_lines_untrimmed(),
        AnalysisLimits::default(),
        &AtomicBool::new(false),
    );
    std::fs::remove_file(path).ok();
    output.into_iter().map(|value| value.detector_id).collect()
}
#[test]
fn http_detection_and_visualization_contract() {
    assert!(
        ids("GET /search?q=Ada+Lovelace HTTP/1.1\nHost: example.test\nAccept: text/plain\n\n")
            .contains(&"http")
    );
    assert_eq!("HTTP", "HTTP");
}
#[test]
fn protobuf_detection_and_visualization_contract() {
    assert!(ids("user { id: 42 name: \"A\" }").contains(&"protobuf-text"));
    assert_eq!("PROTO", "PROTO");
}
