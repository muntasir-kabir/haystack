use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Oracle {
    schema_version: u32,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    profile: String,
    source: String,
    lines: Vec<ExpectedLine>,
}

#[derive(Debug, Deserialize)]
struct ExpectedLine {
    class: String,
    time: String,
    owner: Option<usize>,
    timestamp: Option<String>,
    span: Option<[usize; 2]>,
}

fn oracle() -> Oracle {
    serde_json::from_str(include_str!("fixtures/record_parsing/oracle.json"))
        .expect("record parsing oracle must be valid JSON")
}

#[test]
fn record_parsing_oracle_is_self_consistent() {
    let oracle = oracle();
    assert_eq!(oracle.schema_version, 1);
    assert!(!oracle.cases.is_empty());

    for case in oracle.cases {
        assert!(!case.id.is_empty());
        assert!(!case.profile.is_empty());
        let source_lines: Vec<&str> = case.source.lines().collect();
        assert_eq!(
            source_lines.len(),
            case.lines.len(),
            "{} must specify one expectation per physical line",
            case.id
        );

        let mut active_owner = None;
        for (index, (source, expected)) in source_lines.iter().zip(&case.lines).enumerate() {
            let line_number = index + 1;
            assert!(
                matches!(
                    expected.class.as_str(),
                    "header" | "continuation" | "unassigned"
                ),
                "{} L{} has an unknown classification",
                case.id,
                line_number
            );
            assert!(
                matches!(
                    expected.time.as_str(),
                    "explicit" | "inherited" | "missing" | "invalid" | "unknown"
                ),
                "{} L{} has an unknown time state",
                case.id,
                line_number
            );

            match expected.class.as_str() {
                "header" => {
                    assert_eq!(expected.owner, Some(line_number));
                    active_owner = Some(line_number);
                }
                "continuation" => assert_eq!(expected.owner, active_owner),
                "unassigned" => {
                    assert!(active_owner.is_none());
                    assert!(expected.owner.is_none());
                }
                _ => unreachable!(),
            }

            match (&expected.timestamp, expected.span) {
                (Some(timestamp), Some([start, end])) => {
                    assert_eq!(
                        source.get(start..end),
                        Some(timestamp.as_str()),
                        "{} L{} timestamp span must reference exact source bytes",
                        case.id,
                        line_number
                    );
                }
                (None, None) => {}
                _ => panic!(
                    "{} L{} must specify timestamp text and span together",
                    case.id, line_number
                ),
            }
        }
    }
}

#[test]
fn sparse_header_fixture_has_independent_expected_boundaries() {
    let continuation_count = 600usize;
    let mut source = String::from("2026-09-11 10:00:00.000 INFO first\n");
    for index in 0..continuation_count {
        source.push_str(&format!(
            "    payload row={index} retry_at=2035-01-01T00:00:00Z\n"
        ));
    }
    source.push_str("2026-09-11 10:00:01.000 INFO second\n");

    assert_eq!(source.lines().count(), continuation_count + 2);
    assert!(source.lines().next().unwrap().starts_with("2026-09-11"));
    assert!(source.lines().last().unwrap().starts_with("2026-09-11"));
    assert!(source
        .lines()
        .skip(1)
        .take(continuation_count)
        .all(|line| { line.starts_with("    ") && line.contains("2035-01-01T00:00:00Z") }));
}
