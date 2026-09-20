//! One end-to-end detector contract per supported syntax profile. The focused
//! unit tests live beside each detector and each highlighter; these cases prove
//! they are registered and return the correct presentation id.

use std::sync::atomic::AtomicBool;

use haystack::{AnalysisLimits, EmbeddedDataEngine, LogDocument};

fn detect(input: &str) -> Vec<haystack::Detection> {
    let path = std::env::temp_dir().join(format!(
        "haystack_embedded_format_{}_{}.log",
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
    output
}

fn asserts_detector(input: &str, detector: &str) {
    let output = detect(input);
    assert!(
        output.iter().any(|value| value.detector_id == detector),
        "{detector} did not detect {input:?}; got {output:?}"
    );
}

#[test]
fn logfmt_detection_contract() {
    asserts_detector("INFO user=42 status=\"ok\" duration_ms=18", "logfmt");
}
#[test]
fn colon_fields_detection_contract() {
    asserts_detector("user: 42, status: ok", "fields");
}
#[test]
fn foundation_detection_contract() {
    asserts_detector("payload={ user = Ada; ok = true; }", "foundation");
}
#[test]
fn python_detection_contract() {
    asserts_detector("payload={'user': 42, 'ok': True, 'error': None}", "python");
}
#[test]
fn jvm_debug_detection_contract() {
    asserts_detector("Bundle[{user=42, ok=true}]", "jvm-debug");
}
#[test]
fn http_detection_contract() {
    asserts_detector(
        "GET /search?q=Ada+Lovelace HTTP/1.1\nHost: example.test\nAccept: text/plain\n\n",
        "http",
    );
}
#[test]
fn protobuf_detection_contract() {
    asserts_detector("user { id: 42 name: \"A\" }", "protobuf-text");
}
#[test]
fn stacktrace_detection_contract() {
    asserts_detector("java.lang.IllegalStateException: bad\n  at app.Main.run(Main.java:4)\n  at app.App.main(App.java:9)", "stacktrace");
}
#[test]
fn jwt_detection_contract() {
    asserts_detector(
        "token eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3In0.abcdefghijklmnop",
        "jwt",
    );
}
#[test]
fn pem_detection_contract() {
    asserts_detector("-----BEGIN CERTIFICATE-----\nQUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo=\n-----END CERTIFICATE-----", "pem");
}

#[test]
fn ordinary_prose_does_not_become_fields() {
    assert!(
        !detect("2026-08-23 [INFO] request complete: this is ordinary prose")
            .iter()
            .any(|value| value.detector_id == "fields")
    );
}
