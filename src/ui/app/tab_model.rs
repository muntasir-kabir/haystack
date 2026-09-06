use super::*;

impl LogTab {
    pub(super) fn filter_specs_snapshot(&self) -> Vec<search::FilterSpec> {
        self.filters
            .iter()
            .enumerate()
            .map(|(index, filter)| search::FilterSpec {
                text: filter.text.clone(),
                case_sensitive: self
                    .filter_case_sensitive
                    .get(index)
                    .copied()
                    .unwrap_or(true),
                polarity: if self.filter_exclude.get(index).copied().unwrap_or(false) {
                    search::FilterPolarity::Exclude
                } else {
                    search::FilterPolarity::Include
                },
                regex: self.filter_regex.get(index).copied().unwrap_or(false),
                template_id: self.filter_template_ids.get(index).copied().flatten(),
            })
            .collect()
    }

    pub(super) fn apply_mcp_filters(
        &mut self,
        specs: Vec<search::FilterSpec>,
        join: FilterJoin,
        colors: &[Color32],
    ) {
        let old_specs = self.filter_specs_snapshot();
        let old_active = self.lane_active.clone();
        self.lane_active = specs
            .iter()
            .map(|spec| {
                old_specs
                    .iter()
                    .position(|old| old == spec)
                    .and_then(|index| old_active.get(index).copied())
                    .unwrap_or(true)
            })
            .collect();
        self.filters = specs
            .iter()
            .enumerate()
            .map(|(i, spec)| Filter {
                text: spec.text.clone(),
                color: colors[i % colors.len()],
            })
            .collect();
        self.filter_case_sensitive = specs.iter().map(|spec| spec.case_sensitive).collect();
        self.filter_exclude = specs
            .iter()
            .map(|spec| spec.polarity == search::FilterPolarity::Exclude)
            .collect();
        self.filter_regex = specs.iter().map(|spec| spec.regex).collect();
        self.filter_template_ids = specs.iter().map(|spec| spec.template_id).collect();
        self.filter_join = join;
        self.rescan_filters();
    }

    pub(super) fn apply_mcp_search(&mut self, request: logotomy::mcp::GuiSearch) {
        if !Arc::ptr_eq(&self.doc, &request.doc) {
            return;
        }
        self.clear_find();
        let spec = request.spec;
        self.find_case_sensitive = spec.case_sensitive;
        self.find_regex = spec.regex;
        self.find_template_id_mode = spec.template_id.is_some();
        self.find_template_id = spec.template_id;
        self.find_input = spec.text.clone();
        self.find_query = spec.text.clone();
        self.find_highlighter = search::build_filter_highlighter(&[spec.clone()])
            .ok()
            .map(Arc::new);
        self.find_active_spec = Some(spec);
        self.find_matches = request.matches;
        // An explicit full-document MCP search may find lines hidden by lane
        // visibility. Reveal those hits so navigation can actually show them;
        // the next filter/view edit rebuilds the normal visible subset.
        if let Some(visible) = &self.visible_lines {
            if self
                .find_matches
                .iter()
                .any(|line| visible.binary_search(line).is_err())
            {
                let mut revealed = visible.as_ref().clone();
                revealed.extend_from_slice(&self.find_matches);
                revealed.sort_unstable();
                revealed.dedup();
                self.visible_lines = Some(Arc::new(revealed));
            }
        }
        self.find_pos = request
            .first_page_line
            .and_then(|line| self.find_matches.binary_search(&(line as u32)).ok());
        self.search_history
            .retain(|query| query != &self.find_query);
        self.search_history.insert(0, self.find_query.clone());
        self.search_history.truncate(20);
        self.pending_recent_search = Some(self.find_query.clone());
        if let Some(pos) = self.find_pos {
            self.goto_find_match(pos);
        }
        self.trigger_search_focus();
    }

    #[cfg(test)]
    pub(crate) fn new(doc: LogDocument) -> Self {
        Self::new_with_inspector_mode(doc, EmbeddedInspectorMode::default())
    }

    pub(crate) fn new_with_inspector_mode(
        doc: LogDocument,
        embedded_inspector_mode: EmbeddedInspectorMode,
    ) -> Self {
        let doc = Arc::new(doc);
        let timeline = Timeline::build_u32(&doc, &[], DEFAULT_BUCKETS);

        // Timeline is fixed above the dock; Pinned and Templates begin below Log.
        let mut dock_state = DockState::new(vec![ViewTab::Log]);
        let [_main_surface, _bottom_surface] = dock_state.main_surface_mut().split_below(
            egui_dock::NodeIndex::root(),
            0.8,
            vec![ViewTab::Pinned, ViewTab::Templates],
        );

        LogTab {
            doc: Arc::clone(&doc),
            filters: Vec::new(),
            filter_case_sensitive: Vec::new(),
            filter_exclude: Vec::new(),
            filter_regex: Vec::new(),
            filter_template_ids: Vec::new(),
            filter_join: logotomy::core::search::FilterJoin::Any,
            filter_history: Vec::new(),
            filter_current_range: false,
            matches: Arc::new(Vec::new()),
            matched_filter_specs: Arc::new(Vec::new()),
            matched_filter_scope: None,
            matched_filter_doc: Arc::downgrade(&doc),
            matched_filter_doc_key: FilterDocumentKey::of(&doc),
            timeline,
            context_line: None,
            pending_scroll: None,
            template_browser: TemplateBrowserState::default(),
            timeline_zoom: None,
            timeline_brush_start: None,
            selected_lane: None,
            selected_pin: None,
            pending_pin_scroll: None,
            pending_pin_activation: false,
            undo_delete: None,
            pending_toast: None,
            filter_input: String::new(),
            filter_input_case_sensitive: true,
            filter_input_regex: false,
            filter_input_template_id: false,
            filter_input_regex_validate_at: None,
            filter_input_regex_error: None,
            filter_input_regex_error_dismissed: false,
            highlighter: None,
            search_rx: None,
            filter_scan_progress: None,
            visible_rx: None,
            tail_rx: None,
            lane_active: Vec::new(),
            everything_else_active: true,
            pending_filter_removal: None,
            pending_clear_filters: false,
            timeline_detached: false,
            visible_lines: None,
            log_font_size: 12.0,
            log_line_display_mode: logotomy::core::settings::LogLineDisplayMode::default(),
            wrap_layout: None,
            pins: Vec::new(),
            bottom_panel_open: false,
            pin_comment: String::new(),
            pin_modal: None,
            pin_edit_index: None,
            selection_range: None,
            pending_selection: None,
            drag_selecting: false,
            drag_start_line: None,
            drag_current_line: None,
            drag_start_pos: None,
            selection_popup_pos: None,
            analysis_popup: None,
            next_analysis_popup_id: 0,
            viewport_range: None,
            log_viewport_height: None,
            scroll_top_line: None,
            scroll_fraction: 0.0,
            pending_scroll_restore: None,
            preserve_anchor: None,
            applied_filter: None,
            find_input: String::new(),
            find_query: String::new(),
            find_case_sensitive: false,
            find_regex: false,
            find_template_id_mode: false,
            find_template_id: None,
            find_active_spec: None,
            find_validate_at: None,
            find_error: None,
            find_error_dismissed: false,
            find_matches: Vec::new(),
            find_pos: None,
            find_highlighter: None,
            find_rx: None,
            search_history: Vec::new(),
            pending_recent_search: None,
            pending_recent_filter: None,
            search_suggestions_open: false,
            filter_suggestions_open: false,
            full_line_inspector: None,
            keyword_highlight: None,
            keyword_automaton: None,
            embedded_detections: Arc::new(Vec::new()),
            embedded_rx: None,
            embedded_scan_key: None,
            embedded_pending_key: None,
            embedded_pending_at: None,
            embedded_epoch: 0,
            embedded_inspector: None,
            embedded_inspector_anchor: None,
            embedded_inspector_mode,
            annotation_hover: None,
            search_focus_requested: false,
            search_focus_anim: None,
            filter_highlight: None,
            occurrence_navigation_animation: None,
            dock_state,
            detached_views: HashSet::new(),
            detached_locations: HashMap::new(),
            just_closed_viewports: Vec::new(),
            pending_detach: None,
            saved_dock_state: None,
            mcp_serving: false,
            stale: false,
            pending_sidecar_restore: None,
            last_sidecar_snapshot: None,
        }
    }

    /// Analyze only contiguous physical source intervals around the current
    /// viewport. Filtered rows are never concatenated into artificial input.
    pub fn schedule_embedded_scan(&mut self) -> bool {
        const LOOK_BEHIND: usize = 32;
        const LOOK_AHEAD: usize = 512;

        let Some((first, last)) = self.viewport_range else {
            return false;
        };
        let total = self.doc.total_lines();
        if total == 0 {
            return false;
        }

        let visible_relative: Vec<usize> = match &self.visible_lines {
            Some(lines) => {
                let begin = lines.partition_point(|&line| (line as usize) < first);
                let end = lines.partition_point(|&line| (line as usize) <= last);
                lines[begin..end]
                    .iter()
                    .map(|&line| line as usize)
                    .collect()
            }
            None => (first..=last.min(total - 1)).collect(),
        };
        if visible_relative.is_empty() {
            return false;
        }

        let trim_start = self.doc.trim_start;
        let full_end = self.doc.trim_end;
        let interest_lines: Vec<usize> = visible_relative
            .iter()
            .map(|line| trim_start + *line)
            .collect();
        let mut intervals: Vec<(usize, usize)> = visible_relative
            .iter()
            .map(|line| {
                let original = trim_start + *line;
                self.doc
                    .record_range_containing(original)
                    .map(|record| (record.start.max(trim_start), record.end.min(full_end)))
                    .unwrap_or_else(|| {
                        (
                            original.saturating_sub(LOOK_BEHIND).max(trim_start),
                            original.saturating_add(LOOK_AHEAD + 1).min(full_end),
                        )
                    })
            })
            .collect();
        intervals.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for interval in intervals {
            if let Some(last) = merged.last_mut() {
                if interval.0 <= last.1 {
                    last.1 = last.1.max(interval.1);
                    continue;
                }
            }
            merged.push(interval);
        }

        let key = EmbeddedScanKey {
            epoch: self.embedded_epoch,
            file_size: self.doc.file_size,
            intervals: merged,
            interest_lines,
        };
        if self.embedded_scan_key.as_ref() == Some(&key) {
            self.embedded_pending_key = None;
            self.embedded_pending_at = None;
            return self.embedded_rx.is_some();
        }
        if self.embedded_pending_key.as_ref() != Some(&key) {
            self.embedded_pending_key = Some(key);
            self.embedded_pending_at = Some(Instant::now());
            return true;
        }
        if self
            .embedded_pending_at
            .is_some_and(|started| started.elapsed() < Duration::from_millis(60))
        {
            return true;
        }
        let key = self.embedded_pending_key.take().unwrap();
        self.embedded_pending_at = None;
        if let Some((_, cancel)) = &self.embedded_rx {
            cancel.store(true, Ordering::Relaxed);
        }

        let doc = Arc::clone(&self.doc);
        let worker_key = key.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        crate::ui::worker_pool::spawn(move || {
            let engine = EmbeddedDataEngine::default();
            let limits = AnalysisLimits {
                // Timestamp-delimited records can contain large pretty-printed
                // responses. This remains worker-only and node/depth bounded.
                max_bytes: 8 * 1024 * 1024,
                max_lines: 20_000,
                ..AnalysisLimits::default()
            };
            let mut detections = Vec::new();
            let mut seen = HashSet::<SourceSpan>::new();
            for &(start, end) in &worker_key.intervals {
                if worker_cancel.load(Ordering::Relaxed) {
                    return;
                }
                for detection in
                    engine.analyze_original_lines(&doc, start..end, limits, &worker_cancel)
                {
                    if worker_key
                        .interest_lines
                        .iter()
                        .any(|&line| detection.span.includes_line(line))
                        && seen.insert(detection.span)
                    {
                        detections.push(detection);
                    }
                }
            }
            detections.sort_by_key(|d| (d.span.start.line, d.span.start.byte));
            if !worker_cancel.load(Ordering::Relaxed) {
                let _ = tx.send(EmbeddedScanResult {
                    key: worker_key,
                    detections: Arc::new(detections),
                });
            }
        });
        self.embedded_scan_key = Some(key);
        self.embedded_rx = Some((rx, cancel));
        true
    }

    /// Poll viewport structured-data analysis. Returns true while work remains.
    pub fn poll_embedded_data(&mut self) -> bool {
        let Some((rx, _)) = &self.embedded_rx else {
            return self.embedded_pending_key.is_some();
        };
        match rx.try_recv() {
            Ok(result) => {
                if self.embedded_scan_key.as_ref() == Some(&result.key) {
                    self.embedded_detections = result.detections;
                }
                self.embedded_rx = None;
                false
            }
            Err(crossbeam_channel::TryRecvError::Empty) => true,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.embedded_rx = None;
                false
            }
        }
    }

    pub(super) fn invalidate_embedded_data(&mut self) {
        if let Some((_, cancel)) = &self.embedded_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        self.embedded_rx = None;
        self.embedded_scan_key = None;
        self.embedded_pending_key = None;
        self.embedded_pending_at = None;
        self.embedded_detections = Arc::new(Vec::new());
        self.embedded_inspector = None;
        self.embedded_inspector_anchor = None;
        self.annotation_hover = None;
        self.embedded_epoch = self.embedded_epoch.wrapping_add(1);
    }

    /// Rebuild the filtered visible-lines list based on active lanes.
    /// If all lanes + Everything Else are active, sets visible_lines to None (fast path).
    pub fn rebuild_visible_lines(&mut self) {
        let n = self.doc.total_lines();
        if n == 0 {
            self.visible_lines = None;
            return;
        }
        while self.lane_active.len() < self.filters.len() {
            self.lane_active.push(true);
        }
        self.lane_active.truncate(self.filters.len());

        // Capture the top-visible real line so the log viewport can be
        // preserved across a filter change.
        self.preserve_anchor = self.viewport_range.map(|(first, _)| first);

        let visible_lines = build_visible_lines(
            n,
            &self.matches,
            &self.lane_active,
            &self.filter_exclude,
            self.filter_join,
            self.everything_else_active,
            None,
        );
        self.install_visible_lines(visible_lines, self.preserve_anchor, n);

        if !self.find_query.is_empty() {
            self.start_find(self.find_query.clone());
        }
    }

    /// Rebuild the potentially multi-million-entry filtered index on a worker
    /// after a lane toggle. The last completed index remains drawable until
    /// this result arrives, keeping click/scroll input responsive.
    pub fn rebuild_visible_lines_background(&mut self) {
        if let Some((_, cancel)) = &self.visible_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        self.visible_rx = None;
        while self.lane_active.len() < self.filters.len() {
            self.lane_active.push(true);
        }
        self.lane_active.truncate(self.filters.len());
        let n = self.doc.total_lines();
        let anchor = self.viewport_range.map(|(first, _)| first);
        let matches = Arc::clone(&self.matches);
        let active = self.lane_active.clone();
        let exclude = self.filter_exclude.clone();
        let join = self.filter_join;
        let everything_else_active = self.everything_else_active;
        let (tx, rx) = crossbeam_channel::bounded(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancel);
        crate::ui::worker_pool::spawn(move || {
            let visible_lines = build_visible_lines(
                n,
                &matches,
                &active,
                &exclude,
                join,
                everything_else_active,
                Some(&cancel_worker),
            );
            if !cancel_worker.load(Ordering::Relaxed) {
                let _ = tx.send(VisibleLinesResult {
                    visible_lines,
                    preserve_anchor: anchor,
                });
            }
        });
        self.visible_rx = Some((rx, cancel));
    }

    fn nearest_visible_line(&self, line: usize, n: usize) -> Option<usize> {
        match &self.visible_lines {
            Some(visible) => {
                let insertion = visible
                    .binary_search(&(line as u32))
                    .unwrap_or_else(|index| index);
                match (
                    insertion.checked_sub(1).map(|i| visible[i] as usize),
                    visible.get(insertion).copied().map(|line| line as usize),
                ) {
                    (Some(lower), Some(upper)) => Some(if line - lower <= upper - line {
                        lower
                    } else {
                        upper
                    }),
                    (Some(lower), None) => Some(lower),
                    (None, Some(upper)) => Some(upper),
                    (None, None) => None,
                }
            }
            None => (line < n).then_some(line),
        }
    }

    /// Install a rebuilt visible-line index while keeping the current
    /// selection authoritative. A viewport anchor is used only when there is
    /// no selection that can be retained or replaced.
    fn install_visible_lines(
        &mut self,
        visible_lines: Option<Arc<Vec<u32>>>,
        preserve_anchor: Option<usize>,
        n: usize,
    ) {
        self.visible_lines = visible_lines;
        if let Some(selected) = self.context_line {
            // Selection is a real document line, rather than a virtual-row
            // index. Keep that identity exactly when the filter update still
            // renders it; only a hidden selection is allowed to move.
            let selected_is_visible = match self.visible_lines.as_deref() {
                Some(visible) => visible.binary_search(&(selected as u32)).is_ok(),
                None => selected < n,
            };
            let replacement = selected_is_visible
                .then_some(selected)
                .or_else(|| self.nearest_visible_line(selected, n));
            self.context_line = replacement;
            self.preserve_anchor = None;
            self.pending_scroll = replacement.filter(|&line| {
                line != selected
                    || !self
                        .viewport_range
                        .is_some_and(|(first, last)| first <= line && line <= last)
            });
            return;
        }

        self.preserve_anchor = preserve_anchor;
        self.resolve_preserve_anchor(n);
        self.scroll_to_preserved_anchor();
    }

    fn resolve_preserve_anchor(&mut self, n: usize) {
        if let Some(anchor) = self.preserve_anchor {
            let found = self.nearest_visible_line(anchor, n);
            if !matches!(found, Some(line) if line == anchor) {
                self.preserve_anchor = found;
            }
        }
    }

    /// The log scroll area needs an explicit one-shot target after its row
    /// count changes. A passive offset hint can be overridden by egui's saved
    /// scroll state, which made lane toggles fall back to the first row.
    fn scroll_to_preserved_anchor(&mut self) {
        if let Some(anchor) = self.preserve_anchor.take() {
            self.pending_scroll = Some(anchor);
        }
    }

    /// Remove a filter by index and rescan. If this was the last filter,
    /// "Everything Else" is re-enabled so the view never ends up blank.
    pub fn remove_filter(&mut self, idx: usize) {
        if idx >= self.filters.len() {
            return;
        }
        if self.filters.len() == 1 {
            self.everything_else_active = true;
        }
        if self.selected_lane == Some(idx) {
            self.selected_lane = None;
        } else if let Some(selected) = self.selected_lane {
            if selected > idx {
                self.selected_lane = Some(selected - 1);
            }
        }
        self.filters.remove(idx);
        if idx < self.filter_case_sensitive.len() {
            self.filter_case_sensitive.remove(idx);
        }
        if idx < self.filter_exclude.len() {
            self.filter_exclude.remove(idx);
        }
        if idx < self.filter_regex.len() {
            self.filter_regex.remove(idx);
        }
        if idx < self.filter_template_ids.len() {
            self.filter_template_ids.remove(idx);
        }
        self.rescan_filters();
    }

    /// Remove a filter while retaining enough state to restore it precisely.
    pub fn remove_filter_with_undo(&mut self, idx: usize) {
        let Some(filter) = self.filters.get(idx).cloned() else {
            return;
        };
        let active = self.lane_active.get(idx).copied().unwrap_or(true);
        let case_sensitive = self.filter_case_sensitive.get(idx).copied().unwrap_or(true);
        let exclude = self.filter_exclude.get(idx).copied().unwrap_or(false);
        let regex = self.filter_regex.get(idx).copied().unwrap_or(false);
        let template_id = self.filter_template_ids.get(idx).copied().flatten();
        self.remove_filter(idx);
        self.undo_delete = Some(UndoDelete::Filter {
            index: idx,
            filter,
            active,
            case_sensitive,
            exclude,
            regex,
            template_id,
        });
        self.pending_toast = Some("Filter removed — Cmd/Ctrl+Z to undo".to_string());
    }

    /// Toggle the selected filter lane. "Everything Else" is not selectable
    /// here, so keyboard Space can never hide it by accident.
    pub fn toggle_selected_lane(&mut self) {
        let Some(idx) = self.selected_lane else {
            return;
        };
        let active = self.lane_active.get(idx).copied().unwrap_or(true);
        if !self.set_lane_active(idx, !active) {
            self.selected_lane = None;
        }
    }

    /// Change one filter-lane visibility and rebuild the derived Log View.
    /// The completed rebuild is installed through `install_visible_lines`,
    /// which reconciles selection before requesting any viewport movement.
    pub fn set_lane_active(&mut self, idx: usize, active: bool) -> bool {
        if idx >= self.filters.len() {
            return false;
        }
        while self.lane_active.len() < self.filters.len() {
            self.lane_active.push(true);
        }
        if self.lane_active[idx] == active {
            return true;
        }
        self.lane_active[idx] = active;
        if !self.lane_active.iter().any(|&active| active) && !self.everything_else_active {
            self.everything_else_active = true;
        }
        self.rebuild_visible_lines_background();
        true
    }

    /// Change the Everything Else lane visibility and rebuild the derived Log
    /// View. This keeps timeline controls from bypassing selection handling.
    pub fn set_everything_else_active(&mut self, active: bool) {
        if self.everything_else_active == active {
            return;
        }
        self.everything_else_active = active;
        self.rebuild_visible_lines_background();
    }

    /// Remove the selected pin card and make the operation undoable.
    pub fn remove_selected_pin_with_undo(&mut self) {
        let Some(idx) = self.selected_pin.filter(|&idx| idx < self.pins.len()) else {
            self.selected_pin = None;
            return;
        };
        let pin = self.pins.remove(idx);
        self.selected_pin = None;
        if self.pins.is_empty() {
            self.bottom_panel_open = false;
        }
        self.undo_delete = Some(UndoDelete::Pin { index: idx, pin });
        self.pending_toast = Some("Pin removed — Cmd/Ctrl+Z to undo".to_string());
    }

    /// Clear every pin while retaining one reversible snapshot.
    pub fn clear_pins_with_undo(&mut self) {
        if self.pins.is_empty() {
            return;
        }
        let count = self.pins.len();
        self.undo_delete = Some(UndoDelete::Pins {
            pins: std::mem::take(&mut self.pins),
            bottom_panel_open: self.bottom_panel_open,
        });
        self.bottom_panel_open = false;
        self.selected_pin = None;
        self.pending_toast = Some(format!("{count} pins cleared — Cmd/Ctrl+Z to undo"));
    }

    /// Restore the latest deletion in this tab. Returns whether anything was
    /// restored so callers can choose whether to show a toast.
    pub fn undo_delete(&mut self) -> bool {
        let Some(undo) = self.undo_delete.take() else {
            return false;
        };
        match undo {
            UndoDelete::Filter {
                index,
                filter,
                active,
                case_sensitive,
                exclude,
                regex,
                template_id,
            } => {
                let index = index.min(self.filters.len());
                self.filters.insert(index, filter);
                self.filter_case_sensitive
                    .insert(index.min(self.filter_case_sensitive.len()), case_sensitive);
                self.filter_exclude
                    .insert(index.min(self.filter_exclude.len()), exclude);
                self.filter_regex
                    .insert(index.min(self.filter_regex.len()), regex);
                self.filter_template_ids
                    .insert(index.min(self.filter_template_ids.len()), template_id);
                self.lane_active
                    .insert(index.min(self.lane_active.len()), active);
                self.selected_lane = Some(index);
                self.rescan_filters();
            }
            UndoDelete::Pin { index, pin } => {
                let index = index.min(self.pins.len());
                self.pins.insert(index, pin);
                self.selected_pin = Some(index);
                self.bottom_panel_open = true;
            }
            UndoDelete::Pins {
                pins,
                bottom_panel_open,
            } => {
                self.pins = pins;
                self.bottom_panel_open = bottom_panel_open || !self.pins.is_empty();
            }
            UndoDelete::Trim {
                start,
                end_exclusive,
            } => {
                self.apply_trim_window(start, end_exclusive);
            }
        }
        self.pending_toast = Some("Deletion undone".to_string());
        true
    }

    /// Select a line and request the log viewport to reveal it.
    pub fn select_and_scroll_to(&mut self, line: usize) {
        if self.doc.total_lines() == 0 {
            return;
        }
        let line = line.min(self.doc.total_lines() - 1);
        self.context_line = Some(line);
        self.pending_scroll = Some(line);
        self.ensure_visible();
    }

    /// Navigate from a Timeline pin marker to its source evidence and card.
    /// The log request is installed before Pinned is activated; the app
    /// completes the activation after the dock has rendered Log for this frame.
    pub fn navigate_to_pin(&mut self, pin_index: usize) -> bool {
        let Some(pin) = self.pins.get(pin_index) else {
            return false;
        };
        let Some((start_line, _)) = pin.visible_bounds(self.doc.total_lines()) else {
            return false;
        };

        // Filter visibility can have changed since the pin was created. Keep
        // filters intact but reveal this one evidence row until the next filter
        // rebuild, matching programmatic MCP navigation behavior.
        if let Some(visible) = &self.visible_lines {
            if visible.binary_search(&(start_line as u32)).is_err() {
                let mut revealed = visible.as_ref().clone();
                revealed.push(start_line as u32);
                revealed.sort_unstable();
                revealed.dedup();
                self.visible_lines = Some(Arc::new(revealed));
            }
        }

        self.select_and_scroll_to(start_line);
        self.selected_pin = Some(pin_index);
        self.bottom_panel_open = true;
        self.pending_pin_scroll = Some(pin_index);
        self.pending_pin_activation = true;

        // When Log and Pinned occupy the same dock leaf, make Log active long
        // enough to consume the scroll request before Pinned takes focus.
        if !self.detached_views.contains(&ViewTab::Log) {
            if let Some(path) = self.dock_state.find_tab(&ViewTab::Log) {
                let _ = self.dock_state.set_active_tab(path);
            }
        }
        true
    }

    /// Called after the main dock has rendered, completing Timeline pin
    /// navigation without preventing the Log View from scrolling first.
    pub fn finish_pin_navigation(&mut self) {
        if !std::mem::take(&mut self.pending_pin_activation)
            || self.detached_views.contains(&ViewTab::Pinned)
        {
            return;
        }
        if let Some(path) = self.dock_state.find_tab(&ViewTab::Pinned) {
            let _ = self.dock_state.set_active_tab(path);
        }
    }

    /// Return the first or last logical line currently included by filter lanes.
    pub fn visible_boundary(&self, last: bool) -> Option<usize> {
        if let Some(lines) = self.visible_lines.as_deref() {
            if last {
                lines.last().copied().map(|line| line as usize)
            } else {
                lines.first().copied().map(|line| line as usize)
            }
        } else if self.doc.total_lines() > 0 {
            Some(if last { self.doc.total_lines() - 1 } else { 0 })
        } else {
            None
        }
    }

    /// Return a currently rendered endpoint, falling back to the complete
    /// visible range when the log view has not been painted yet.
    pub fn viewport_boundary(&self, last: bool) -> Option<usize> {
        let range = self.viewport_range?;
        Some(if last { range.1 } else { range.0 })
    }

    /// Clear ALL filters at once and rescan. "Everything Else" is re-enabled
    /// so the view never ends up blank.
    pub fn clear_all_filters(&mut self) {
        if self.filters.is_empty() {
            return;
        }
        self.filters.clear();
        self.everything_else_active = true;
        self.selected_lane = None;
        self.rescan_filters();
    }

    /// Toggle every filter lane on/off as a group. Never touches the
    /// "Everything Else" lane. If the resulting state would hide everything,
    /// "Everything Else" is re-enabled first so the view stays non-blank.
    pub fn toggle_all_lanes(&mut self) {
        if self.filters.is_empty() {
            return;
        }
        while self.lane_active.len() < self.filters.len() {
            self.lane_active.push(true);
        }
        self.lane_active.truncate(self.filters.len());
        let all_active = self.lane_active.iter().all(|&a| a);
        for flag in &mut self.lane_active {
            *flag = !all_active;
        }
        if !self.lane_active.iter().any(|&a| a) && !self.everything_else_active {
            self.everything_else_active = true;
        }
        self.rebuild_visible_lines_background();
    }

    /// Re-scan the document for the current filter set in the background.
    pub fn rescan_filters(&mut self) {
        if let Some((_, cancel)) = &self.search_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        self.filter_scan_progress = None;
        if let Some((_, cancel)) = &self.visible_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        self.visible_rx = None;
        while self.lane_active.len() < self.filters.len() {
            self.lane_active.push(true);
        }
        self.lane_active.truncate(self.filters.len());
        self.filter_case_sensitive.resize(self.filters.len(), true);
        self.filter_exclude.resize(self.filters.len(), false);
        self.filter_regex.resize(self.filters.len(), false);
        self.filter_template_ids.resize(self.filters.len(), None);

        let scan_zoom = self
            .filter_current_range
            .then_some(self.timeline_zoom)
            .flatten();
        let current_doc_key = FilterDocumentKey::of(&self.doc);
        let same_document = self
            .matched_filter_doc
            .upgrade()
            .is_some_and(|matched_doc| Arc::ptr_eq(&self.doc, &matched_doc))
            && self.matched_filter_doc_key == current_doc_key;
        if self.filters.is_empty() {
            self.selected_lane = None;
            self.matches = Arc::new(Vec::new());
            self.matched_filter_specs = Arc::new(Vec::new());
            self.matched_filter_scope = scan_zoom;
            self.matched_filter_doc = Arc::downgrade(&self.doc);
            self.matched_filter_doc_key = current_doc_key;
            self.timeline = if same_document {
                self.timeline.base_without_filters()
            } else {
                Timeline::build_u32(&self.doc, &[], DEFAULT_BUCKETS)
            };
            self.search_rx = None;
            self.filter_scan_progress = None;
            self.rebuild_visible_lines();
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let doc = Arc::clone(&self.doc);
        let filters: Arc<Vec<search::FilterSpec>> = Arc::new(self.filter_specs_snapshot());
        self.highlighter = search::build_filter_highlighter(&filters)
            .ok()
            .map(Arc::new);
        let scan_domain = self.timeline.domain;
        let can_reuse = same_document && scan_zoom == self.matched_filter_scope;
        let timeline_base = same_document.then(|| self.timeline.base_without_filters());
        let mut retained = vec![None; filters.len()];
        let mut old_lane_used = vec![false; self.matched_filter_specs.len()];
        let mut missing_specs = Vec::new();
        let mut missing_indexes = Vec::new();
        for (new_index, spec) in filters.iter().enumerate() {
            let old_index = can_reuse
                .then(|| {
                    self.matched_filter_specs
                        .iter()
                        .enumerate()
                        .find(|(old_index, old_spec)| {
                            !old_lane_used[*old_index] && spec.has_same_matcher(old_spec)
                        })
                        .map(|(old_index, _)| old_index)
                })
                .flatten();
            if let Some(old_index) = old_index {
                old_lane_used[old_index] = true;
                retained[new_index] = self.matches.get(old_index).cloned();
            } else {
                missing_specs.push(spec.clone());
                missing_indexes.push(new_index);
            }
        }
        let cancel_worker = Arc::clone(&cancel);
        let progress = Arc::new(search::ScanProgress::default());
        let progress_worker = Arc::clone(&progress);
        crate::ui::worker_pool::spawn(move || {
            let scanned = if missing_specs.is_empty() {
                progress_worker.total_lines.store(0, Ordering::Relaxed);
                progress_worker.scanned_lines.store(0, Ordering::Relaxed);
                Vec::new()
            } else {
                match search::scan_advanced(
                    &doc,
                    &missing_specs,
                    &cancel_worker,
                    Some(&progress_worker),
                ) {
                    Ok(matches) => matches,
                    Err(_) => return,
                }
            };
            for ((new_index, lane), spec) in
                missing_indexes.into_iter().zip(scanned).zip(missing_specs)
            {
                let lane = if let Some((start, end)) = scan_zoom {
                    lane.into_iter()
                        .filter(|&line| {
                            let value = match scan_domain {
                                logotomy::core::timeline::TimelineDomain::Time { .. } => {
                                    doc.ts_at(line as usize)
                                }
                                logotomy::core::timeline::TimelineDomain::Sequence => line as i64,
                            };
                            value >= start && value <= end
                        })
                        .collect()
                } else {
                    lane
                };
                debug_assert!(filters[new_index].has_same_matcher(&spec));
                retained[new_index] = Some(Arc::new(lane));
            }
            if cancel_worker.load(Ordering::Relaxed) {
                return;
            }
            let matches: Arc<Vec<Arc<Vec<u32>>>> = Arc::new(
                retained
                    .into_iter()
                    .map(|lane| lane.unwrap_or_else(|| Arc::new(Vec::new())))
                    .collect(),
            );
            let timeline = match timeline_base {
                Some(base) => base.with_shared_filter_matches(&doc, &matches),
                None => Timeline::build_shared_u32(&doc, &matches, DEFAULT_BUCKETS),
            };
            if !cancel_worker.load(Ordering::Relaxed) {
                let _ = tx.send(FilterScanResult {
                    matches,
                    timeline,
                    specs: filters,
                    scope: scan_zoom,
                });
            }
        });
        self.search_rx = Some((rx, cancel));
        self.filter_scan_progress = Some(progress);
    }

    /// Stage a disk append on a worker and install it atomically on completion.
    /// This avoids UI-thread copy-on-write/indexing/template-mining when the
    /// document is also shared with MCP.
    pub(super) fn start_tail_update(&mut self) {
        if self.tail_rx.is_some() {
            return;
        }
        let doc = Arc::clone(&self.doc);
        let (tx, rx) = crossbeam_channel::bounded(1);
        crate::ui::worker_pool::spawn(move || {
            let old_line_count = doc.total_lines();
            // Clone on this worker, not the UI thread. LogDocument::clone
            // deliberately detaches mutable Drain/cache state.
            let mut updated = (*doc).clone();
            let result = match updated.append_new_data() {
                Ok(true) => Ok(TailUpdateResult {
                    doc: Box::new(updated),
                    old_line_count,
                }),
                Ok(false) => Err("file no longer has appended data".to_string()),
                Err(error) => Err(error),
            };
            let _ = tx.send(result);
        });
        self.tail_rx = Some(rx);
    }

    fn can_extend_filters_after_append(&self, old_line_count: usize) -> bool {
        if self.search_rx.is_some()
            || self.matched_filter_scope.is_some()
            || old_line_count != self.doc.total_lines()
            || self.matched_filter_doc_key != FilterDocumentKey::of(&self.doc)
        {
            return false;
        }
        let same_document = self
            .matched_filter_doc
            .upgrade()
            .is_some_and(|matched_doc| Arc::ptr_eq(&self.doc, &matched_doc));
        let current = self.filter_specs_snapshot();
        same_document
            && current.len() == self.matched_filter_specs.len()
            && current
                .iter()
                .zip(self.matched_filter_specs.iter())
                .all(|(current, matched)| current.has_same_matcher(matched))
    }

    /// Extend completed filter lanes by scanning only appended, trim-relative
    /// lines. The old match vectors stay drawable until this worker installs
    /// the combined lanes and refreshed timeline atomically.
    fn extend_filters_after_append(&mut self, old_line_count: usize) {
        if let Some((_, cancel)) = &self.visible_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        self.visible_rx = None;
        let doc = Arc::clone(&self.doc);
        let filters = Arc::new(self.filter_specs_snapshot());
        self.highlighter = search::build_filter_highlighter(&filters)
            .ok()
            .map(Arc::new);
        let old_matches = Arc::clone(&self.matches);
        let timeline_base = self.timeline.base_without_filters();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancel);
        let progress = Arc::new(search::ScanProgress::default());
        let progress_worker = Arc::clone(&progress);
        crate::ui::worker_pool::spawn(move || {
            let appended = if filters.is_empty() {
                progress_worker.total_lines.store(
                    doc.total_lines().saturating_sub(old_line_count),
                    Ordering::Relaxed,
                );
                progress_worker.scanned_lines.store(
                    doc.total_lines().saturating_sub(old_line_count),
                    Ordering::Relaxed,
                );
                Vec::new()
            } else {
                match search::scan_advanced_range(
                    &doc,
                    &filters,
                    old_line_count..doc.total_lines(),
                    &cancel_worker,
                    Some(&progress_worker),
                ) {
                    Ok(matches) => matches,
                    Err(_) => return,
                }
            };
            if cancel_worker.load(Ordering::Relaxed) {
                return;
            }
            let matches: Arc<Vec<Arc<Vec<u32>>>> = Arc::new(
                old_matches
                    .iter()
                    .zip(appended)
                    .map(|(old, new)| {
                        let mut combined = Vec::with_capacity(old.len() + new.len());
                        combined.extend_from_slice(old);
                        combined.extend(new);
                        Arc::new(combined)
                    })
                    .collect(),
            );
            let timeline = timeline_base
                .extend_append(&doc, old_line_count, &matches)
                .unwrap_or_else(|| Timeline::build_shared_u32(&doc, &matches, DEFAULT_BUCKETS));
            if !cancel_worker.load(Ordering::Relaxed) {
                let _ = tx.send(FilterScanResult {
                    matches,
                    timeline,
                    specs: filters,
                    scope: None,
                });
            }
        });
        self.search_rx = Some((rx, cancel));
        self.filter_scan_progress = Some(progress);
    }

    /// Install a completed staged append, returning the number of added lines
    /// and file name for the app status message.
    pub(super) fn poll_tail_update(&mut self) -> Option<Result<(usize, String), String>> {
        let rx = self.tail_rx.as_ref()?;
        match rx.try_recv() {
            Ok(Ok(result)) => {
                let new_lines = result
                    .doc
                    .total_lines()
                    .saturating_sub(result.old_line_count);
                let file_name = result.doc.file_name.clone();
                let can_extend = self.can_extend_filters_after_append(result.old_line_count);
                self.doc = Arc::new(*result.doc);
                self.tail_rx = None;
                self.stale = false;
                self.invalidate_embedded_data();
                if can_extend {
                    self.extend_filters_after_append(result.old_line_count);
                } else {
                    self.rescan_filters();
                }
                Some(Ok((new_lines, file_name)))
            }
            Ok(Err(error)) => {
                self.tail_rx = None;
                Some(Err(error))
            }
            Err(crossbeam_channel::TryRecvError::Empty) => None,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.tail_rx = None;
                Some(Err(
                    "background append worker stopped unexpectedly".to_string()
                ))
            }
        }
    }

    /// Clamp all doc-positioned view state to the current visible window.
    /// When the MCP dirty-doc sync swaps in a mutated (typically trimmed)
    /// document, `context_line`/`viewport_range`/scroll anchors can still hold
    /// line indices from the previous, larger window — feeding those to the
    /// unchecked `ts_at()` would panic (regression: index 1060 vs len 1000).
    /// Bring everything back in range so the next frame is safe to render.
    pub fn clamp_view_state(&mut self) {
        let n = self.doc.total_lines();
        let clamp_pos = |p: &mut Option<usize>| {
            if let Some(v) = *p {
                *p = if n == 0 { None } else { Some(v.min(n - 1)) };
            }
        };
        let clamp_range = |r: &mut Option<(usize, usize)>| {
            if let Some((a, b)) = *r {
                if n == 0 {
                    *r = None;
                } else {
                    *r = Some((a.min(n - 1), b.min(n - 1)));
                }
            }
        };
        clamp_pos(&mut self.context_line);
        clamp_pos(&mut self.pending_scroll);
        clamp_pos(&mut self.preserve_anchor);
        clamp_pos(&mut self.drag_start_line);
        clamp_pos(&mut self.drag_current_line);
        clamp_range(&mut self.viewport_range);
        clamp_range(&mut self.pin_modal);
        clamp_range(&mut self.selection_range);
        clamp_range(&mut self.pending_selection);
        if let Some(state) = self.analysis_popup.as_mut() {
            if n == 0 {
                self.analysis_popup = None;
            } else {
                state.range.0 = state.range.0.min(n - 1);
                state.range.1 = state.range.1.min(n - 1);
            }
        }
    }

    /// Ensure the timeline zoom window includes the current context_line.
    /// If the line is outside the visible range, auto-pan to center on it.
    pub fn ensure_visible(&mut self) {
        let Some(line) = self.context_line else {
            return;
        };
        let v = match self.timeline.domain {
            logotomy::core::timeline::TimelineDomain::Time { .. } => {
                // context_line can hold a stale index after an MCP doc swap and
                // exceed the current window; never let the unchecked ts_at index
                // out of bounds (regression: index 1060 vs len 1000 panic).
                match self.doc.ts_at_opt(line) {
                    Some(t) if t >= 0 => t,
                    _ => return,
                }
            }
            logotomy::core::timeline::TimelineDomain::Sequence => line as i64,
        };
        if v < 0 {
            return;
        }
        let (full_start, full_end) = match self.timeline.domain {
            logotomy::core::timeline::TimelineDomain::Time { start_ms, end_ms } => {
                (start_ms, end_ms)
            }
            logotomy::core::timeline::TimelineDomain::Sequence => {
                (0, self.doc.total_lines().saturating_sub(1) as i64)
            }
        };
        let (view_start, view_end) = match self.timeline_zoom {
            Some((s, e)) => (s, e),
            None => (full_start, full_end),
        };
        if v >= view_start && v <= view_end {
            return; // already visible
        }
        // Pan to center on v
        let span = view_end - view_start;
        let half = span / 2;
        let new_start = (v - half).max(full_start);
        let new_end = (new_start + span).min(full_end);
        let new_start = (new_end - span).max(full_start);
        if new_start == full_start && new_end == full_end {
            self.timeline_zoom = None;
        } else {
            self.timeline_zoom = Some((new_start, new_end));
        }
    }

    /// Keep the timeline zoom window in sync with the log viewport. If the visible
    /// range ("window shadow") has scrolled *completely* outside the current
    /// timeline view, recenter the zoom window on the shadow's midpoint while
    /// preserving the current zoom span. A partially-visible shadow is left alone
    /// so an intentionally-zoomed window stays stable while scrolling within it.
    pub fn ensure_viewport_visible(&mut self) {
        let Some((first_line, last_line)) = self.viewport_range else {
            return;
        };
        let v0 = match self.timeline.domain {
            logotomy::core::timeline::TimelineDomain::Time { .. } => {
                // viewport_range can hold a stale index after an MCP doc swap and
                // exceed the current window — never let ts_at index out of bounds.
                match self.doc.ts_at_opt(first_line) {
                    Some(t) if t >= 0 => t,
                    _ => return,
                }
            }
            logotomy::core::timeline::TimelineDomain::Sequence => first_line as i64,
        };
        let v1 = match self.timeline.domain {
            logotomy::core::timeline::TimelineDomain::Time { .. } => {
                match self.doc.ts_at_opt(last_line) {
                    Some(t) if t >= 0 => t,
                    _ => return,
                }
            }
            logotomy::core::timeline::TimelineDomain::Sequence => last_line as i64,
        };
        // Match the shadow renderer: only act when the mapped values are valid.
        if v0 < 0 || v1 < 0 {
            return;
        }
        let (full_start, full_end) = match self.timeline.domain {
            logotomy::core::timeline::TimelineDomain::Time { start_ms, end_ms } => {
                (start_ms, end_ms)
            }
            logotomy::core::timeline::TimelineDomain::Sequence => {
                (0, self.doc.total_lines().saturating_sub(1) as i64)
            }
        };
        let (view_start, view_end) = match self.timeline_zoom {
            Some((s, e)) => (s, e),
            None => (full_start, full_end),
        };
        // Only recenter when the shadow lies entirely outside the current view.
        if v1 < view_start || v0 > view_end {
            let span = view_end - view_start;
            let center = v0 + (v1 - v0) / 2;
            let half = span / 2;
            let new_start = (center - half).max(full_start);
            let new_end = (new_start + span).min(full_end);
            let new_start = (new_end - span).max(full_start);
            if new_start == full_start && new_end == full_end {
                self.timeline_zoom = None;
            } else {
                self.timeline_zoom = Some((new_start, new_end));
            }
        }
    }

    /// Apply a trim action: mutates the document in-place, cancels any in-flight
    /// search, and rebuilds filters + timeline for the new trimmed range.
    pub fn handle_trim(&mut self, action: TrimAction) {
        // Cancel any in-flight search.
        if let Some((_, cancel)) = &self.search_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        self.search_rx = None;
        self.filter_scan_progress = None;
        if let Some((_, cancel)) = &self.visible_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        self.visible_rx = None;

        let previous_start = self.doc.trim_start;
        let previous_end = self.doc.trim_end;
        // Apply the trim to the document.
        let doc = Arc::make_mut(&mut self.doc);
        match action {
            TrimAction::TrimRight(l) => doc.trim_right(l),
            TrimAction::TrimLeft(l) => doc.trim_left(l),
        }

        self.rebase_pinned_lines(previous_start, self.doc.trim_start);
        self.undo_delete = Some(UndoDelete::Trim {
            start: previous_start,
            end_exclusive: previous_end,
        });

        // Rebuild derived state. Investigation state remains intact: pins are
        // rebased to their original log positions and out-of-window anchors
        // are hidden by the Pin panel rather than discarded.
        self.visible_lines = None;
        self.timeline_zoom = None;
        self.pending_filter_removal = None;
        self.pending_clear_filters = false;
        self.invalidate_embedded_data();
        self.clamp_view_state();
        self.pending_toast = Some("Trim applied — Cmd/Ctrl+Z to undo".to_string());

        // Rebuild filters + timeline for the new document.
        self.rescan_filters();
    }

    /// Reset the document trim to show all lines.
    pub fn handle_trim_reset(&mut self) {
        if !self.doc.is_trimmed() {
            return;
        }
        // Cancel any in-flight search.
        if let Some((_, cancel)) = &self.search_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        self.search_rx = None;
        self.filter_scan_progress = None;
        if let Some((_, cancel)) = &self.visible_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        self.visible_rx = None;

        let previous_start = self.doc.trim_start;
        let previous_end = self.doc.trim_end;
        let doc = Arc::make_mut(&mut self.doc);
        doc.reset_trim();
        self.rebase_pinned_lines(previous_start, self.doc.trim_start);
        self.undo_delete = Some(UndoDelete::Trim {
            start: previous_start,
            end_exclusive: previous_end,
        });

        // Reset state.
        self.visible_lines = None;
        self.timeline_zoom = None;
        self.pending_filter_removal = None;
        self.pending_clear_filters = false;
        self.invalidate_embedded_data();
        self.clamp_view_state();
        self.pending_toast = Some("Trim reset — Cmd/Ctrl+Z to undo".to_string());

        // Rebuild filters + timeline for the full document.
        self.rescan_filters();
    }

    fn apply_trim_window(&mut self, start: usize, end_exclusive: usize) {
        let previous_start = self.doc.trim_start;
        let total = self.doc.total_lines_untrimmed();
        let start = start.min(total);
        let end_exclusive = end_exclusive.min(total).max(start);
        let doc = Arc::make_mut(&mut self.doc);
        if start == 0 && end_exclusive == total {
            doc.reset_trim();
        } else if start < end_exclusive {
            doc.trim_range(start, end_exclusive - 1);
        }
        self.rebase_pinned_lines(previous_start, self.doc.trim_start);
        self.visible_lines = None;
        self.timeline_zoom = None;
        self.invalidate_embedded_data();
        self.clamp_view_state();
        self.rescan_filters();
    }

    /// Pin coordinates are trim-relative in memory. Rebase them whenever the
    /// visible window moves so they continue to refer to the same source rows.
    fn rebase_pinned_lines(&mut self, old_trim_start: usize, new_trim_start: usize) {
        // `usize::MAX - original_line` is an internal marker for an anchor
        // outside the current trim. It lets reset/undo restore the anchor
        // exactly without keeping a second, easily-stale pin list.
        let rebase = |line: usize| {
            let original = if line > usize::MAX / 2 {
                usize::MAX - line
            } else {
                line.saturating_add(old_trim_start)
            };
            if original >= new_trim_start
                && original < new_trim_start.saturating_add(self.doc.total_lines())
            {
                original - new_trim_start
            } else {
                usize::MAX - original
            }
        };
        for pin in &mut self.pins {
            pin.start_line = rebase(pin.start_line);
            for line in &mut pin.line_numbers {
                *line = rebase(*line);
            }
        }
    }

    /// Poll the background filter scan and install its already-built timeline.
    pub fn poll_search(&mut self) -> bool {
        let Some((rx, _)) = &self.search_rx else {
            return false;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.matches = result.matches;
                self.timeline = result.timeline;
                self.matched_filter_specs = result.specs;
                self.matched_filter_scope = result.scope;
                self.matched_filter_doc = Arc::downgrade(&self.doc);
                self.matched_filter_doc_key = FilterDocumentKey::of(&self.doc);
                self.search_rx = None;
                self.filter_scan_progress = None;
                self.rebuild_visible_lines();
                false
            }
            Err(crossbeam_channel::TryRecvError::Empty) => true,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.search_rx = None;
                self.filter_scan_progress = None;
                false
            }
        }
    }

    /// Poll a filtered-index rebuild started by a lane toggle.
    pub fn poll_visible_lines(&mut self) -> bool {
        let Some((rx, _)) = &self.visible_rx else {
            return false;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.install_visible_lines(
                    result.visible_lines,
                    result.preserve_anchor,
                    self.doc.total_lines(),
                );
                self.visible_rx = None;
                if !self.find_query.is_empty() {
                    self.start_find(self.find_query.clone());
                }
                false
            }
            Err(crossbeam_channel::TryRecvError::Empty) => true,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.visible_rx = None;
                false
            }
        }
    }

    /// Build the currently selected Log View matcher. Keeping this beside the
    /// scan entry point prevents the search dropdown from drifting from the
    /// Timeline filter representation.
    fn find_spec(&self, query: &str) -> Result<search::FilterSpec, String> {
        let text = query.trim();
        if self.find_template_id_mode {
            let id = search::parse_template_id(text)?;
            return Ok(search::FilterSpec {
                text: format!("T{{{id}}}"),
                case_sensitive: true,
                polarity: search::FilterPolarity::Include,
                regex: false,
                template_id: Some(id),
            });
        }
        let spec = search::FilterSpec {
            text: text.to_owned(),
            case_sensitive: self.find_case_sensitive,
            polarity: search::FilterPolarity::Include,
            regex: self.find_regex,
            template_id: None,
        };
        search::validate_matcher(&spec)?;
        Ok(spec)
    }

    /// Start a background find scan for `query`. Empty query clears find state.
    pub fn start_find(&mut self, query: String) {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            self.clear_find();
            self.find_input.clear();
            return;
        }
        let spec = match self.find_spec(trimmed) {
            Ok(spec) => spec,
            Err(error) => {
                self.find_error = Some(error);
                self.find_error_dismissed = false;
                return;
            }
        };
        if let Some((_, cancel)) = &self.find_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        self.find_query = trimmed.to_string();
        self.search_history.retain(|entry| entry != trimmed);
        self.search_history.insert(0, trimmed.to_string());
        self.search_history.truncate(20);
        self.pending_recent_search = Some(trimmed.to_string());
        self.find_template_id = spec.template_id;
        self.find_active_spec = Some(spec.clone());
        self.find_highlighter = search::build_filter_highlighter(&[spec.clone()])
            .ok()
            .map(Arc::new);
        self.find_error = None;
        self.find_error_dismissed = false;
        self.find_matches.clear();
        self.find_pos = None;

        let doc = Arc::clone(&self.doc);
        // Cloning the Arc is constant-time. Cloning a large filtered Vec here
        // used to create a noticeable UI hitch just before the find worker
        // began its background scan.
        let subset = self.visible_lines.as_ref().map(Arc::clone);
        let (tx, rx) = crossbeam_channel::bounded(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancel);
        crate::ui::worker_pool::spawn(move || {
            let result = search::find_advanced_u32(
                &doc,
                subset.as_deref().map(Vec::as_slice),
                &spec,
                &cancel_worker,
            )
            .unwrap_or_default();
            let _ = tx.send(result);
        });
        self.find_rx = Some((rx, cancel));
    }

    /// Poll the background find scan. Returns `true` while in flight.
    pub fn poll_find(&mut self) -> bool {
        let Some((rx, _)) = &self.find_rx else {
            return false;
        };
        match rx.try_recv() {
            Ok(matches) => {
                self.find_matches = matches;
                self.find_rx = None;
                if self.find_matches.is_empty() {
                    self.find_pos = None;
                } else {
                    // Start at the result nearest the current Log View
                    // viewport instead of always jumping to the first result
                    // in the file. The viewport midpoint gives stable
                    // behavior when several results are currently visible.
                    let anchor = self
                        .viewport_range
                        .map(|(first, last)| first + (last.saturating_sub(first) / 2))
                        .or(self.context_line)
                        .unwrap_or(self.find_matches[0] as usize);
                    let start = nearest_occurrence(self.find_matches.iter().copied(), anchor)
                        .map(|(position, _)| position)
                        .unwrap_or(0);
                    self.find_pos = Some(start);
                    self.goto_find_match(start);
                    if self.find_matches.len() > 1 {
                        self.pending_toast = Some(
                            "Up/Down Arrow to show previous/Next search occurrence".to_string(),
                        );
                    }
                }
                false
            }
            Err(crossbeam_channel::TryRecvError::Empty) => true,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.find_rx = None;
                false
            }
        }
    }

    /// Step to the next match, wrapping around.
    pub fn find_next(&mut self) {
        if self.find_matches.is_empty() {
            return;
        }
        let pos = self.find_pos.unwrap_or(0);
        let next = (pos + 1) % self.find_matches.len();
        self.goto_find_match(next);
    }

    /// Step to the previous match, wrapping around.
    pub fn find_prev(&mut self) {
        if self.find_matches.is_empty() {
            return;
        }
        let pos = self.find_pos.unwrap_or(0);
        let prev = if pos == 0 {
            self.find_matches.len() - 1
        } else {
            pos - 1
        };
        self.goto_find_match(prev);
    }

    /// Move the selected Log View line to the adjacent visible line when no
    /// search results are active. Filtered views use their visible-line index;
    /// otherwise every document line is navigable.
    pub fn select_adjacent_line(&mut self, next: bool) {
        let total = self.doc.total_lines();
        if total == 0 {
            return;
        }

        let target = if let Some(visible) = self.visible_lines.as_deref() {
            if visible.is_empty() {
                return;
            }
            match self
                .context_line
                .and_then(|line| visible.binary_search(&(line as u32)).ok())
            {
                Some(position) if next => visible[(position + 1).min(visible.len() - 1)] as usize,
                Some(position) => visible[position.saturating_sub(1)] as usize,
                None if next => visible[0] as usize,
                None => visible[visible.len() - 1] as usize,
            }
        } else {
            match self.context_line {
                Some(line) if next => line.saturating_add(1).min(total - 1),
                Some(line) => line.saturating_sub(1),
                None => 0,
            }
        };

        self.context_line = Some(target);
        self.pending_scroll = Some(target);
        self.ensure_visible();
    }

    /// Select a filter lane for left/right occurrence navigation.
    pub fn select_lane(&mut self, lane: usize) {
        if lane >= self.filters.len() || lane >= self.matches.len() {
            return;
        }
        self.selected_lane = Some(lane);
        self.pending_toast =
            Some("Left Arrow/Right Arrow to select previous/Next filter occurrence".to_string());
    }

    /// Move to the previous occurrence in the selected lane, wrapping around.
    pub fn select_lane_previous(&mut self) {
        self.navigate_selected_lane(false);
    }

    /// Move to the next occurrence in the selected lane, wrapping around.
    pub fn select_lane_next(&mut self) {
        self.navigate_selected_lane(true);
    }

    fn navigate_selected_lane(&mut self, next: bool) {
        let Some(lane) = self.selected_lane else {
            return;
        };
        let Some(matches) = self.matches.get(lane) else {
            return;
        };
        if matches.is_empty() {
            return;
        }

        let current = self.context_line;
        let target = if let Some(current) = current {
            if next {
                matches
                    .iter()
                    .map(|&line| line as usize)
                    .find(|&line| line > current)
                    .unwrap_or(matches[0] as usize)
            } else {
                matches
                    .iter()
                    .rev()
                    .map(|&line| line as usize)
                    .find(|&line| line < current)
                    .unwrap_or(*matches.last().unwrap() as usize)
            }
        } else if next {
            matches[0] as usize
        } else {
            *matches.last().unwrap() as usize
        };

        self.context_line = Some(target);
        self.pending_scroll = Some(target);
        self.sync_navigation_positions(target);
        self.ensure_visible();
    }

    /// Keep the lane and Log View search cursors aligned when both are active.
    /// Each cursor moves to the nearest occurrence in its own sorted list.
    pub(crate) fn sync_navigation_positions(&mut self, line: usize) {
        if let Some(nearest) = nearest_occurrence(self.find_matches.iter().copied(), line) {
            self.find_pos = Some(nearest.0);
        }
    }

    /// Derive the current lane occurrence instead of storing duplicate
    /// selection state. An arbitrary log line is not an occurrence unless it
    /// belongs to the selected, enabled lane.
    pub(crate) fn selected_occurrence(&self) -> Option<(usize, usize)> {
        let lane = self.selected_lane?;
        let line = self.context_line?;
        let active = self.lane_active.get(lane).copied().unwrap_or(true);
        let matches_line = self
            .matches
            .get(lane)
            .is_some_and(|occurrences| occurrences.binary_search(&(line as u32)).is_ok());
        (active && matches_line).then_some((lane, line))
    }

    /// Select a line chosen from the timeline. When a lane supplied the click,
    /// that lane remains selected even if another lane also matches the line.
    /// Disabled lanes can still navigate to a line, but cannot become selected.
    pub(crate) fn select_timeline_line(&mut self, line: usize, clicked_lane: Option<usize>) {
        if let Some(lane) = clicked_lane {
            if self.lane_active.get(lane).copied().unwrap_or(true) {
                self.selected_lane = Some(lane);
            } else if self.selected_lane == Some(lane) {
                self.selected_lane = None;
            }
        }
        self.context_line = Some(line);
        self.pending_scroll = Some(line);
        self.ensure_visible();
    }

    /// Jump to match index `i` (clamped to match list length).
    fn goto_find_match(&mut self, i: usize) {
        let line = self.find_matches[i.min(self.find_matches.len().saturating_sub(1))] as usize;
        self.find_pos = Some(i.min(self.find_matches.len().saturating_sub(1)));
        self.context_line = Some(line);
        self.pending_scroll = Some(line);
        self.ensure_visible();
    }

    /// Clear in-flight find and reset all find state.
    pub fn clear_find(&mut self) {
        if let Some((_, cancel)) = &self.find_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        self.find_rx = None;
        self.find_query.clear();
        self.find_matches.clear();
        self.find_pos = None;
        self.find_highlighter = None;
        self.find_template_id = None;
        self.find_active_spec = None;
        self.find_validate_at = None;
        self.find_error = None;
        self.find_error_dismissed = false;
    }

    /// Start a typed Template ID search from the Templates tab and bring the
    /// Log View control into focus for immediate refinement.
    pub fn start_template_id_search(&mut self, template_id: u32) {
        self.find_regex = false;
        self.find_template_id_mode = true;
        self.find_input = format!("T{{{template_id}}}");
        self.start_find(self.find_input.clone());
        self.trigger_search_focus();
    }

    /// Set the keyword highlight (double-click). Case-insensitive, matching the
    /// find box, so every case variant of the word highlights across the view.
    pub fn set_keyword_highlight(&mut self, kw: Option<String>) {
        self.keyword_highlight = kw.clone();
        self.keyword_automaton = kw
            .map(|k| search::build_find_automaton(&k, true))
            .flatten()
            .map(Arc::new);
    }

    /// Request the log search box to grab focus (Cmd/Ctrl+F). Consumed by the
    /// log view, which also selects any existing text and pulses the border.
    pub fn trigger_search_focus(&mut self) {
        self.search_focus_requested = true;
        self.search_focus_anim = Some(Instant::now());
    }

    /// Convenience default used by focused model tests and legacy callers.
    #[allow(dead_code)]
    pub fn push_filter(&mut self, text: &str, color: Color32) -> Option<usize> {
        self.push_filter_with_options(text, color, true, false)
    }

    /// Add a filter with the options selected in the Add Filter field.
    pub fn push_filter_with_options(
        &mut self,
        text: &str,
        color: Color32,
        case_sensitive: bool,
        regex: bool,
    ) -> Option<usize> {
        let trimmed = text.trim();
        if trimmed.is_empty() || self.filters.len() >= MAX_FILTERS {
            return None;
        }
        if regex && search::validate_regex(trimmed, case_sensitive).is_err() {
            return None;
        }
        if self.filters.iter().any(|k| k.text == trimmed) {
            return None;
        }
        let idx = self.filters.len();
        self.filters.push(Filter {
            text: trimmed.to_string(),
            color,
        });
        self.filter_case_sensitive.push(case_sensitive);
        self.filter_exclude.push(false);
        self.filter_regex.push(regex);
        self.filter_template_ids.push(None);
        let history_entry = logotomy::core::settings::RecentFilter {
            text: trimmed.to_string(),
            case_sensitive,
            regex,
        };
        self.filter_history.retain(|entry| entry.text != trimmed);
        self.filter_history.insert(0, history_entry.clone());
        self.filter_history.truncate(20);
        self.pending_recent_filter = Some(history_entry);
        self.filter_highlight = Some((idx, Instant::now()));
        self.rescan_filters();
        Some(idx)
    }

    /// Add a normal timeline lane that selects a mined Drain template by ID.
    pub fn push_template_filter(&mut self, template_id: u32, color: Color32) -> Option<usize> {
        if self.filters.len() >= MAX_FILTERS
            || self
                .filter_template_ids
                .iter()
                .any(|id| *id == Some(template_id))
        {
            return None;
        }
        let idx = self.filters.len();
        self.filters.push(Filter {
            text: format!("T{{{template_id}}}"),
            color,
        });
        self.filter_case_sensitive.push(true);
        self.filter_exclude.push(false);
        self.filter_regex.push(false);
        self.filter_template_ids.push(Some(template_id));
        self.filter_highlight = Some((idx, Instant::now()));
        self.rescan_filters();
        Some(idx)
    }

    /// Parse and add a typed Template ID filter from Timeline input.
    pub fn push_template_filter_input(&mut self, input: &str, color: Color32) -> Option<usize> {
        let template_id = search::parse_template_id(input).ok()?;
        self.push_template_filter(template_id, color)
    }
}
