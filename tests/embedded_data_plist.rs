use haystack::{AnalysisLimits, DataNode, Detection, EmbeddedDataEngine, LogDocument};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

fn detect(input: &str) -> Vec<Detection> {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "haystack_embedded_plist_{}_{}.log",
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

#[test]
fn xml_plist_is_detected_as_one_multiline_tree() {
    let input = r#"INFO ঢাকা config=<?xml version="1.0"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>name</key><string>Ada &amp; Grace</string>
    <key>roles</key><array><string>admin</string><string>editor</string></array>
    <key>enabled</key><true/>
  </dict>
</plist> trailing"#
        .replace('\n', "\r\n");
    let values = detect(&input);
    let plist = values
        .iter()
        .find(|value| value.detector_id == "plist")
        .unwrap_or_else(|| panic!("missing XML plist: {values:?}"));
    assert!(plist.raw.starts_with("<plist"));
    assert!(plist.raw.ends_with("</plist>"));
    assert!(plist.source_lines > 1);
    assert_eq!(plist.span.start.line, 2);
    assert_eq!(plist.span.start.byte, 0);
    assert_eq!(plist.span.end.line, 8);
    assert_eq!(plist.span.end.byte, "</plist>".len());
    let DataNode::Object(entries) = &plist.data else {
        panic!("expected plist dictionary")
    };
    assert_eq!(entries[0].1, DataNode::String("Ada & Grace".to_owned()));
}

#[test]
fn labeled_openstep_plist_owns_nested_collections() {
    let values = detect(
        "propertyList = { user = Ada; roles = (admin, editor); flags = { enabled = YES; }; }",
    );
    let plist = values
        .iter()
        .find(|value| value.detector_id == "plist")
        .unwrap_or_else(|| panic!("missing OpenStep plist: {values:?}"));
    assert_eq!(plist.span.start.byte, "propertyList = ".len());
    let DataNode::Object(entries) = &plist.data else {
        panic!("expected plist dictionary")
    };
    assert!(matches!(entries[1].1, DataNode::Array(_)));
    let DataNode::Object(flags) = &entries[2].1 else {
        panic!("expected nested flags dictionary")
    };
    assert_eq!(flags[0].1, DataNode::Bool(true));
}

#[test]
fn ordinary_foundation_description_is_not_reclassified_as_plist() {
    let values = detect("payload={ user = Ada; ok = true; }");
    assert!(values.iter().any(|value| value.detector_id == "foundation"));
    assert!(!values.iter().any(|value| value.detector_id == "plist"));
}

#[test]
fn binary_plist_magic_is_specialized_across_text_transports() {
    for token in [
        "bplist00payload",
        "62706c6973743030deadbeef",
        "YnBsaXN0MDA=",
    ] {
        let values = detect(&format!("payload={token}"));
        let binary = values
            .iter()
            .find(|value| value.detector_id == "binary-plist")
            .unwrap_or_else(|| panic!("missing binary plist for {token}: {values:?}"));
        assert_eq!(binary.span.start.byte, "payload=".len());
        assert!(binary.pretty.contains("binary property list"));
        assert!(binary.pretty.contains("explicit preview required"));
    }
}

#[test]
fn malformed_xml_plist_is_left_unhighlighted() {
    assert!(
        !detect("<plist><dict><key>missing-value</key></dict></plist>")
            .iter()
            .any(|value| value.detector_id == "plist")
    );
}
