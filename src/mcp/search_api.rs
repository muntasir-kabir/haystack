//! Shared MCP matcher contract and GUI search handoff.
use super::*;

/// Exact search snapshot. The GUI rejects it if the served document changed.
pub struct GuiSearch {
    pub doc: Arc<LogDocument>,
    pub spec: search::FilterSpec,
    pub matches: Vec<u32>,
    pub first_page_line: Option<usize>,
}

pub(super) fn matcher_from_args(
    args: &Value,
    text_key: &str,
) -> Result<search::FilterSpec, String> {
    let case_sensitive = arg_bool(args, "case_sensitive", true)?;
    let regex = arg_bool(args, "regex", false)?;
    let template_id = match args.get("template_id") {
        None => None,
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|id| u32::try_from(id).ok())
                .ok_or_else(|| {
                    "template_id must be an integer between 0 and 4294967295".to_string()
                })?,
        ),
    };
    let text = match (args.get(text_key), template_id) {
        (None, Some(id)) => format!("T{{{id}}}"),
        (Some(Value::String(text)), None) if !text.trim().is_empty() => text.trim().to_string(),
        (Some(_), Some(_)) => {
            return Err(format!(
                "provide either {text_key} or template_id, not both"
            ))
        }
        _ => {
            return Err(format!(
                "provide a non-empty {text_key} or an integer template_id"
            ))
        }
    };
    if template_id.is_some() && regex {
        return Err("regex cannot be combined with template_id".into());
    }
    let spec = search::FilterSpec {
        text,
        case_sensitive,
        regex,
        template_id,
        field_query: None,
        polarity: search::FilterPolarity::Include,
    };
    search::validate_matcher(&spec)?;
    Ok(spec)
}

pub(super) fn matcher_properties(text_key: &str) -> Value {
    let mut properties = json!({
        "case_sensitive": {"type": "boolean", "default": true, "description": "Text matching folds ASCII case when false; regex uses Unicode case folding."},
        "regex": {"type": "boolean", "default": false, "description": "Interpret text as a Rust regex (max 4096 bytes; no lookaround or backreferences)."},
        "template_id": {"type": "integer", "minimum": 0, "maximum": 4294967295u32, "description": "Match a mined Drain template ID instead of text. Mutually exclusive with text and regex=true."}
    });
    properties[text_key] = json!({"type": "string", "minLength": 1, "description": "Text or regex pattern; omit when using template_id."});
    properties
}

pub(super) fn search_schema(with_log_id: bool) -> Value {
    let mut properties = matcher_properties("query");
    properties.as_object_mut().unwrap().extend(json!({
        "after": {"type": "string", "description": "Inclusive timestamp lower bound (RFC3339 or epoch milliseconds as a string)."},
        "before": {"type": "string", "description": "Inclusive timestamp upper bound; unknown timestamps are excluded when either bound is set."},
        "max_results": {"type": "integer", "minimum": 1, "maximum": 2000, "default": 50},
        "offset": {"type": "integer", "minimum": 0, "default": 0},
        "with_filtered_log": {"type": "boolean", "default": false, "description": "False searches the whole current document (including its trim). True uses include/exclude and Any/All filters, ignoring lane visibility; with zero filters returns a no-log hint."},
        "sync_ui": {"type": "boolean", "default": true, "description": "In GUI-attached mode, update Find and highlight the exact scoped results. Set false for an exploratory query without changing the UI."}
    }).as_object().unwrap().clone());
    if with_log_id {
        properties["log_id"] = json!({"type": "string", "description": "Required in standalone mode; omit when GUI-attached."});
    }
    json!({
        "name": "search",
        "description": "Search text (case-sensitive or insensitive), regex, or a mined template ID using the GUI matcher. Defaults to the whole current document. Returns paginated matches as [one_based_trim_relative_line, epoch_ms|null], total_matches, next_offset and scope. GUI mode also updates Find unless sync_ui=false. Use raw_log for exact text evidence. Pagination is valid while the document and filters stay unchanged.",
        "inputSchema": {"type": "object", "properties": properties, "oneOf": [
            {"required": ["query"], "not": {"required": ["template_id"]}},
            {"required": ["template_id"], "not": {"required": ["query"]}}
        ]}
    })
}

pub(super) fn tool_search(args: &Value, state: &mut ServerState) -> Result<Value, String> {
    let spec = matcher_from_args(args, "query")?;
    let filtered = arg_bool(args, "with_filtered_log", false)?;
    let sync_ui = arg_bool(args, "sync_ui", true)?;
    let after = arg_time(args, "after")?;
    let before = arg_time(args, "before")?;
    if after.zip(before).is_some_and(|(a, b)| a > b) {
        return Err("after must be at or before before".into());
    }
    let limit = arg_usize(args, "max_results", 50)?;
    if limit == 0 || limit > HARD_MAX_LINES {
        return Err(format!(
            "max_results must be between 1 and {HARD_MAX_LINES}"
        ));
    }
    let offset = arg_usize(args, "offset", 0)?;
    if args.get("format").is_some() || args.get("context").is_some() {
        return Err(
            "search returns line/timestamp tuples; use raw_log for text and context".into(),
        );
    }
    let (log_id, doc) = resolve_doc(state, args)?;
    let mut scope_args = args.clone();
    scope_args["with_filtered_log"] = json!(filtered);
    let visible = match filtered_view_or_no_log(state, &log_id, &scope_args) {
        Ok(visible) => visible,
        Err(payload) => return Ok(payload),
    };
    let all = state.matches_for_spec(&log_id, &spec)?;
    let matches: Vec<usize> = all
        .iter()
        .copied()
        .filter(|&line| {
            let ts = doc.ts_at(line);
            ((after.is_none() && before.is_none()) || ts >= 0)
                && after.map_or(true, |bound| ts >= bound)
                && before.map_or(true, |bound| ts <= bound)
                && visible
                    .as_ref()
                    .map_or(true, |lines| lines.binary_search(&line).is_ok())
        })
        .collect();
    let page: Vec<Value> = matches
        .iter()
        .skip(offset)
        .take(limit)
        .map(|&line| {
            let ts = doc.ts_at(line);
            json!([line + 1, if ts >= 0 { Some(ts) } else { None }])
        })
        .collect();
    let next = offset.saturating_add(page.len());
    let ui_sync = state.is_gui_mode() && sync_ui;
    if ui_sync {
        state.pending_search = Some(GuiSearch {
            doc: Arc::clone(&doc),
            spec,
            matches: matches.iter().map(|&line| line as u32).collect(),
            first_page_line: matches.get(offset).copied(),
        });
    }
    let mut out = json!({
        "matches": page, "total_matches": matches.len(), "returned": page.len(),
        "offset": offset, "has_more": next < matches.len(),
        "next_offset": if next < matches.len() {Some(next)} else {None},
        "scope": {"with_filtered_log": filtered, "after": after, "before": before,
            "document_lines": doc.total_lines(), "line_numbers": "1-based, relative to current trim"},
        "ui_sync": if ui_sync {"queued"} else {"disabled"}
    });
    if log_id != "_active" {
        out["log_id"] = json!(log_id);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::tests::gui_state;

    #[test]
    fn search_matches_core_in_every_mode_and_keeps_cache_keys_distinct() {
        let mut state = gui_state("INFO Error code=42\nINFO error code=7\nWARN other\n");
        let doc = state.get_active_doc().unwrap();
        for args in [
            json!({"query": "Error"}),
            json!({"query": "error", "case_sensitive": false}),
            json!({"query": "code=\\d+", "regex": true}),
            json!({"query": "code=\\d+", "regex": false}),
            json!({"template_id": doc.template_at(0)}),
        ] {
            let spec = matcher_from_args(&args, "query").unwrap();
            let expected =
                search::find_advanced_u32(&doc, None, &spec, &AtomicBool::new(false)).unwrap();
            let out = dispatch("search", &args, &mut state).unwrap();
            let actual: Vec<u32> = out["matches"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tuple| tuple[0].as_u64().unwrap() as u32 - 1)
                .collect();
            assert_eq!(actual, expected, "{args}");
            assert_eq!(state.pending_search.as_ref().unwrap().matches, expected);
            assert_eq!(out["scope"]["with_filtered_log"], false);
        }
        assert_eq!(state.match_cache.len(), 5);
    }

    #[test]
    fn advanced_filters_compose_and_preserve_modes_after_removal() {
        let mut state = gui_state("ERROR api timeout\nerror api ok\nINFO api timeout\n");
        tool_filters_add(
            &json!({"filter_text": "error", "case_sensitive": false}),
            &mut state,
        )
        .unwrap();
        tool_filters_add(
            &json!({"filter_text": "time.*", "regex": true, "join": "all"}),
            &mut state,
        )
        .unwrap();
        tool_filters_add(&json!({"filter_text": "INFO", "exclude": true}), &mut state).unwrap();
        assert_eq!(
            state
                .visible_lines_for("_active")
                .unwrap()
                .unwrap()
                .as_slice(),
            &[0]
        );
        let out = tool_filters_remove(&json!({"id": 0}), &mut state).unwrap();
        assert_eq!(out["filters"][0]["regex"], true);
        assert_eq!(out["filters"][1]["exclude"], true);
        assert_eq!(out["join"], "all");
        tool_filters_remove(&json!({"id": 0}), &mut state).unwrap();
        assert_eq!(
            state
                .visible_lines_for("_active")
                .unwrap()
                .unwrap()
                .as_slice(),
            &[0, 1]
        );
        let out = tool_search(
            &json!({"query": "api", "with_filtered_log": true}),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total_matches"], 2);
    }

    #[test]
    fn template_filters_and_case_edits_invalidate_filtered_cache() {
        let mut state =
            gui_state("INFO connected user=1\nINFO connected user=2\nERROR disk full\n");
        let id = state.get_active_doc().unwrap().template_at(0);
        let out = tool_filters_add(&json!({"template_id": id}), &mut state).unwrap();
        assert_eq!(out["filters"][0]["template_id"], id);
        assert_eq!(
            state
                .visible_lines_for("_active")
                .unwrap()
                .unwrap()
                .as_slice(),
            &[0, 1]
        );
        let mut spec = search::FilterSpec::phrase("info");
        state.set_filter_specs("_active", vec![spec.clone()], search::FilterJoin::Any);
        assert!(state
            .visible_lines_for("_active")
            .unwrap()
            .unwrap()
            .is_empty());
        spec.case_sensitive = false;
        state.set_filter_specs("_active", vec![spec], search::FilterJoin::Any);
        assert_eq!(
            state
                .visible_lines_for("_active")
                .unwrap()
                .unwrap()
                .as_slice(),
            &[0, 1]
        );
    }

    #[test]
    fn invalid_requests_do_not_mutate_filters_or_gui_search() {
        let mut state = gui_state("ERROR first\n");
        tool_search(&json!({"query": "ERROR"}), &mut state).unwrap();
        for args in [
            json!({"query": "(" , "regex": true}),
            json!({"query": " "}),
            json!({"query": "a", "template_id": 1}),
            json!({"template_id": -1}),
            json!({"template_id": 4294967296u64}),
            json!({"template_id": "1"}),
            json!({"template_id": 1, "regex": true}),
            json!({"query": "a", "case_sensitive": "false"}),
            json!({"query": "a", "max_results": 0}),
            json!({"query": "a", "offset": -1}),
            json!({"query": "a", "after": "2026-01-02", "before": "2026-01-01"}),
            json!({"query": "a", "with_filtered_log": "false"}),
            json!({"query": "x".repeat(search::MAX_REGEX_PATTERN_BYTES + 1), "regex": true}),
        ] {
            assert!(tool_search(&args, &mut state).is_err(), "{args}");
            assert_eq!(state.pending_search.as_ref().unwrap().spec.text, "ERROR");
        }
        for args in [
            json!({"filter_text": "(", "regex": true}),
            json!({"filter_text": "ERROR", "join": "xor"}),
        ] {
            assert!(tool_filters_add(&args, &mut state).is_err());
        }
        assert!(state.get_filters("_active").is_empty());
    }

    #[test]
    fn pagination_time_scope_and_gui_opt_out_are_explicit() {
        let mut state =
            gui_state("hit unknown\n2026-01-01T00:00:00Z hit one\n2026-01-01T00:00:01Z hit two\n");
        let out = tool_search(
            &json!({"query": "hit", "before": "2026-01-02", "max_results": 1}),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total_matches"], 2);
        assert_eq!(out["matches"][0][0], 2);
        assert_eq!(out["next_offset"], 1);
        let request = state.pending_search.take().unwrap();
        assert_eq!(request.matches, vec![1, 2]);
        assert_eq!(request.first_page_line, Some(1));
        let out = tool_search(
            &json!({"query": "hit", "offset": 2, "sync_ui": false}),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["matches"][0][0], 3);
        assert_eq!(out["next_offset"], Value::Null);
        assert_eq!(out["ui_sync"], "disabled");
        assert!(state.pending_search.is_none());
        let out = tool_search(&json!({"query": "hit", "offset": 99}), &mut state).unwrap();
        assert_eq!(out["returned"], 0);
        assert_eq!(out["has_more"], false);
        state.set_active_doc(state.get_active_doc().unwrap());
        assert!(state.pending_search.is_none());
    }

    #[test]
    fn standalone_search_keeps_legacy_calls_working_and_requires_log_id() {
        let gui = gui_state("ERROR one\nerror two\n");
        let mut state = ServerState::default();
        state
            .logs
            .insert("log_1".into(), gui.get_active_doc().unwrap());
        assert!(tool_search(&json!({"query": "error"}), &mut state).is_err());
        let out = dispatch(
            "search",
            &json!({"log_id": "log_1", "query": "error", "case_sensitive": false}),
            &mut state,
        )
        .unwrap();
        let old = dispatch("find_occurrences", &json!({"log_id": "log_1", "keyword": "error", "case_sensitive": false, "with_filtered_log": false}), &mut state).unwrap();
        assert_eq!(out["matches"], old["occurrences"]);
        assert_eq!(out["log_id"], "log_1");
        assert!(state.pending_search.is_none());
    }

    #[test]
    fn catalog_promotes_search_and_describes_its_gui_side_effect() {
        let catalog = tools(&ServerState::default());
        let list = catalog.as_array().unwrap();
        assert!(!list.iter().any(|tool| tool["name"] == "find_occurrences"));
        let tool = list.iter().find(|tool| tool["name"] == "search").unwrap();
        assert_eq!(tool["annotations"]["readOnlyHint"], false);
        assert_eq!(tool["annotations"]["destructiveHint"], false);
        assert_eq!(
            tool["inputSchema"]["properties"]["with_filtered_log"]["default"],
            false
        );
        assert_eq!(tool["inputSchema"]["oneOf"].as_array().unwrap().len(), 2);
    }
}
