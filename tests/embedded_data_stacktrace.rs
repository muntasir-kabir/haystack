use logotomy::{AnalysisLimits, EmbeddedDataEngine, LogDocument};
use std::sync::atomic::AtomicBool;

#[test]
fn java_stacktrace_detection_and_frames_visualization_contract() {
    let input = "java.lang.IllegalStateException: bad\n  at app.Main.run(Main.java:4)\n  at app.App.main(App.java:9)";
    let path = std::env::temp_dir().join(format!(
        "logotomy_embedded_trace_{}.log",
        std::process::id()
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
    let value = output
        .iter()
        .find(|value| value.detector_id == "stacktrace")
        .expect("stack trace detection");
    assert!(
        matches!(&value.data, logotomy::DataNode::Object(entries) if entries.iter().any(|(key, _)| key == "frames"))
    );
    assert_eq!("TRACE", "TRACE");
}
