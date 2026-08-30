use std::collections::HashSet;

use logotomy::{AnalysisLimits, EmbeddedDataEngine, LogDocument, RootKind};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct FixtureCase {
    id: String,
    input: String,
    detect: bool,
    start_line: Option<usize>,
    end_line: Option<usize>,
    root_kind: Option<String>,
}

#[test]
fn embedded_json_detector_satisfies_fixture_contract() {
    let raw = include_str!("fixtures/embedded_json/cases.json");
    let cases: Vec<FixtureCase> = serde_json::from_str(raw).unwrap();
    let engine = EmbeddedDataEngine::default();

    for case in cases {
        let path = std::env::temp_dir().join(format!(
            "logotomy_embedded_json_{}_{}.log",
            std::process::id(),
            case.id
        ));
        std::fs::write(&path, &case.input).unwrap();
        let doc = LogDocument::open(&path).unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let found = engine.analyze_original_lines(
            &doc,
            0..doc.total_lines_untrimmed(),
            AnalysisLimits::default(),
            &cancel,
        );
        let _ = std::fs::remove_file(&path);

        assert_eq!(
            !found.is_empty(),
            case.detect,
            "case {}: {found:?}",
            case.id
        );
        if case.detect {
            let detection = &found[0];
            assert_eq!(
                detection.span.start.line,
                case.start_line.unwrap(),
                "{}",
                case.id
            );
            assert_eq!(
                detection.span.end.line,
                case.end_line.unwrap(),
                "{}",
                case.id
            );
            let expected_kind = match case.root_kind.as_deref().unwrap() {
                "object" => RootKind::Object,
                "array" => RootKind::Array,
                other => panic!("unsupported fixture root kind: {other}"),
            };
            assert_eq!(detection.root_kind, expected_kind, "{}", case.id);
            assert!(
                serde_json::from_str::<serde_json::Value>(&detection.raw).is_ok(),
                "{} returned non-JSON first: {detection:?}",
                case.id
            );
        }
    }
}

#[test]
fn embedded_json_fixture_contract_is_well_formed() {
    let raw = include_str!("fixtures/embedded_json/cases.json");
    let cases: Vec<FixtureCase> = serde_json::from_str(raw).expect("fixture must be valid JSON");
    assert!(
        cases.len() >= 10,
        "fixture should cover the main JSON shapes"
    );

    let mut ids = HashSet::new();
    for case in cases {
        assert!(
            ids.insert(case.id.clone()),
            "duplicate case id: {}",
            case.id
        );
        assert!(!case.input.is_empty(), "{} has empty input", case.id);
        if case.detect {
            let start = case.start_line.expect("positive case needs start_line");
            let end = case.end_line.expect("positive case needs end_line");
            assert!(start <= end, "{} has an inverted line span", case.id);
            assert!(matches!(
                case.root_kind.as_deref(),
                Some("object" | "array")
            ));
        } else {
            assert!(case.start_line.is_none());
            assert!(case.end_line.is_none());
            assert!(case.root_kind.is_none());
        }
    }
}
