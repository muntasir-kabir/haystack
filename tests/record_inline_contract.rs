use std::sync::atomic::AtomicBool;

use haystack::core::record::{preview_profile, CompiledProfile, RecordProfile};
use serde_json::Value;

#[test]
fn inline_formats_match_the_human_authored_contract() {
    let contract: Value =
        serde_json::from_str(include_str!("fixtures/record_parsing/inline_contract.json")).unwrap();
    assert_eq!(contract["profile_schema_version"], 2);
    for case in contract["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let layout = case["template"].as_str().unwrap();
        let line = case["line"].as_str().unwrap();
        let profile = RecordProfile::inline(format!("test:{id}"), id, layout);
        let compiled = CompiledProfile::compile(profile).unwrap();
        let preview = preview_profile(&compiled, line, &[], &AtomicBool::new(false)).unwrap();
        let row = &preview.lines[0];
        if case["fields"].is_null() {
            assert!(!row.record_start, "{id}");
            continue;
        }
        assert!(row.record_start, "{id}: {:?}", preview.detection);
        let expected = case["fields"].as_object().unwrap();
        assert_eq!(row.fields.len(), expected.len(), "{id}");
        for (name, value, _) in &row.fields {
            assert_eq!(value, expected[name].as_str().unwrap(), "{id}:{name}");
        }
    }
}

#[test]
fn current_grammar_requires_time_and_log_and_defaults_names_to_tokens() {
    for layout in ["{a} {log}", "{time} {a}"] {
        assert!(CompiledProfile::compile(RecordProfile::inline("new", "New", layout)).is_err());
    }
    assert_eq!(
        RecordProfile::text("new", "New", "{time} {log}").schema_version,
        3
    );
    assert!(
        CompiledProfile::compile(RecordProfile::inline("new", "New", "{time} {a:wat} {log}"))
            .err()
            .unwrap()
            .to_string()
            .contains("unknown type")
    );
    let fields = CompiledProfile::compile(RecordProfile::inline(
        "fields",
        "Fields",
        "{time} [{thread_id}] <{log_level}> {log}",
    ))
    .unwrap();
    assert_eq!(fields.field_type_label(1), Some("token"));
    assert_eq!(fields.field_index("thread"), None);
}

#[test]
fn repeated_delimiters_stay_bounded() {
    let profile = CompiledProfile::compile(RecordProfile::inline(
        "adversarial",
        "Adversarial",
        "{time} a={a:text} b={b:text} {log}",
    ))
    .unwrap();
    let line = format!(
        "2026-07-15 22:26:39.907481+0300 {}",
        "a=x b=y ".repeat(1000)
    );
    let start = std::time::Instant::now();
    let _ = preview_profile(&profile, &line, &[], &AtomicBool::new(false)).unwrap();
    assert!(
        start.elapsed().as_secs() < 5,
        "bounded matching took too long"
    );
}

#[test]
fn schema_three_refinement_fixture_is_well_formed() {
    let contract: Value = serde_json::from_str(include_str!(
        "fixtures/record_parsing/refinement_contract.json"
    ))
    .unwrap();
    assert_eq!(contract["profile_schema_version"], 3);
    assert!(contract["cases"]
        .as_array()
        .is_some_and(|cases| cases.len() >= 7));
    assert!(contract["cases"].as_array().unwrap().iter().any(|case| {
        case["id"] == "ignored_number_rejects_invalid_value" && case["fields"].is_null()
    }));
}

#[test]
fn refined_formats_match_without_retaining_ignore_fields() {
    let contract: Value = serde_json::from_str(include_str!(
        "fixtures/record_parsing/refinement_contract.json"
    ))
    .unwrap();
    for case in contract["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let profile = RecordProfile::refined_inline(
            format!("refined:{id}"),
            id,
            case["template"].as_str().unwrap(),
        );
        let compiled = CompiledProfile::compile(profile).unwrap();
        let preview = preview_profile(
            &compiled,
            case["line"].as_str().unwrap(),
            &[],
            &AtomicBool::new(false),
        )
        .unwrap();
        let row = &preview.lines[0];
        if case["fields"].is_null() {
            assert!(!row.record_start, "{id}");
            continue;
        }
        let expected = case["fields"].as_object().unwrap();
        assert!(row.record_start, "{id}");
        assert_eq!(row.fields.len(), expected.len(), "{id}");
        assert!(compiled.field_index("ignore").is_none(), "{id}");
        assert_eq!(compiled.field_count(), expected.len(), "{id}");
        for (name, value, _) in &row.fields {
            assert_eq!(value, expected[name].as_str().unwrap(), "{id}:{name}");
        }
    }
}
