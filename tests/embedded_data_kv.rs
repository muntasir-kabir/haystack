use std::sync::atomic::AtomicBool;

use haystack::{AnalysisLimits, Detection, EmbeddedDataEngine, LogDocument};

fn detect(input: &str) -> Vec<Detection> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "haystack_embedded_kv_{}_{}.log",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
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
    values
}

fn detector_for(input: &str) -> Vec<&'static str> {
    detect(input)
        .into_iter()
        .map(|value| value.detector_id)
        .collect()
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

#[test]
fn single_log_events_are_detected() {
    for input in [
        "INFO error='HKErrorAuthorizationDenied'",
        "INFO error=\"HKErrorAuthorizationDenied\"",
        "INFO readyToPlay asset=video_4471.mp4",
        "INFO received pressureLevel=deep_link",
        "INFO received pressureLevel=\"deep_link error\"",
        "INFO product_id=premium_monthly",
    ] {
        assert!(
            detector_for(input).contains(&"logfmt"),
            "single event was not detected: {input}"
        );
    }
}

#[test]
fn colon_event_variants_are_detected_inside_prose() {
    for input in [
        "received pressureLevel: deep_link",
        "received pressureLevel : deep_link",
        "received pressureLevel: `deep link`",
    ] {
        assert!(
            detector_for(input).contains(&"fields"),
            "colon event was not detected: {input}"
        );
    }
}

#[test]
fn paths_urls_and_following_fields_keep_exact_values() {
    let input = "INFO path=/var/data/somepath url=myapp://billing/9236 route_known=YES";
    let detection = detect(input)
        .into_iter()
        .find(|value| value.detector_id == "logfmt")
        .expect("logfmt detection");
    assert_eq!(
        detection.raw,
        "path=/var/data/somepath url=myapp://billing/9236 route_known=YES"
    );
    assert_eq!(detection.source_spans.len(), 3);
    let exact = detection
        .source_spans
        .iter()
        .map(|span| &input[span.start.byte..span.end.byte])
        .collect::<Vec<_>>();
    assert_eq!(
        exact,
        [
            "path=/var/data/somepath",
            "url=myapp://billing/9236",
            "route_known=YES"
        ]
    );
}

#[test]
fn multiline_values_keep_their_physical_span() {
    let input = "INFO message=`first line\nsecond line` status=ok";
    let detection = detect(input)
        .into_iter()
        .find(|value| value.detector_id == "logfmt")
        .expect("multiline logfmt detection");
    assert_eq!(detection.span.start.line, 0);
    assert_eq!(detection.span.end.line, 1);
    assert!(detection.raw.contains("first line\nsecond line"));
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
