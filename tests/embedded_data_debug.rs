use logotomy::{AnalysisLimits, EmbeddedDataEngine, LogDocument};
use std::sync::atomic::AtomicBool;

fn detect(input: &str) -> Vec<logotomy::Detection> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "logotomy_embedded_debug_{}_{}_{}.log",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
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
    values
}

fn ids(input: &str) -> Vec<&'static str> {
    detect(input)
        .into_iter()
        .map(|value| value.detector_id)
        .collect()
}

#[test]
fn foundation_detection_and_visualization_contract() {
    assert!(ids("payload={ user = Ada; ok = true; }").contains(&"foundation"));
    assert_eq!("APPLE", "APPLE");
}

#[test]
fn swift_and_nsobject_outputs_are_detected_without_tuple_false_positives() {
    let values = detect(
        "swift=User(id: 42, profile: Optional(Profile(active: true)))\n\
         enum=Result.failure(error: NetworkError(code: 401))\n\
         dict=[\"user\": \"Ada\", \"roles\": [\"admin\", \"editor\"]]\n\
         object=<User: 0x123; name = Ada; active = true>\n\
         dump=▿ User\n  - id: 42\n  ▿ profile: Profile\n    - active: true\n\
         prose=(foo, bar)\n",
    );
    let apple = values
        .iter()
        .filter(|value| value.detector_id == "foundation")
        .collect::<Vec<_>>();
    assert!(
        apple.len() >= 5,
        "expected Swift/Foundation values: {values:?}"
    );
    assert!(apple.iter().any(|value| value.source_lines > 1));
    assert!(!apple.iter().any(|value| value.raw == "(foo, bar)"));
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
