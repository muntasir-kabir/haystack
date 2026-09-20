use haystack::{AnalysisLimits, EmbeddedDataEngine, LogDocument};
use std::sync::atomic::AtomicBool;

fn ids(input: &str) -> Vec<&'static str> {
    let path = std::env::temp_dir().join(format!(
        "haystack_embedded_encoded_{}_{}.log",
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
fn jwt_detection_and_visualization_contract() {
    assert!(
        ids("token eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3In0.abcdefghijklmnop").contains(&"jwt")
    );
}
#[test]
fn base64_detection_and_visualization_contract() {
    assert!(ids("blob QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo=").contains(&"base64"));
}
#[test]
fn hex_detection_and_visualization_contract() {
    assert!(ids("blob 48656c6c6f20776f726c6421").contains(&"hex"));
}
#[test]
fn pem_detection_and_visualization_contract() {
    assert!(ids("-----BEGIN CERTIFICATE-----\nQUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo=\n-----END CERTIFICATE-----").contains(&"pem"));
}
#[test]
fn short_identifiers_are_not_encoded_payloads() {
    assert!(!ids("value=abc123")
        .iter()
        .any(|id| matches!(*id, "jwt" | "base64" | "hex" | "pem")));
}
