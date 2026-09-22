use super::*;

impl LogTab {
    /// Switch Timeline presentation while keeping Log View in source order.
    /// Line and Time share one zoom because only their captions differ.
    pub(crate) fn set_timeline_display_mode(&mut self, mode: TimelineDisplayMode) {
        if mode == self.timeline_display_mode {
            return;
        }
        if mode != TimelineDisplayMode::Line && self.doc.time_range.is_none() {
            return;
        }

        if self.timeline_display_mode.uses_real_time_coordinates() {
            self.timeline_real_time_zoom = self.timeline_zoom;
        } else {
            self.timeline_line_zoom = self.timeline_zoom;
        }

        self.timeline = if mode.uses_real_time_coordinates() {
            Timeline::build_real_time_shared_u32(&self.doc, &self.matches, DEFAULT_BUCKETS)
        } else {
            Timeline::build_shared_u32(&self.doc, &self.matches, DEFAULT_BUCKETS)
        };
        self.timeline_display_mode = mode;
        self.timeline_zoom = if mode.uses_real_time_coordinates() {
            self.timeline_real_time_zoom
        } else {
            self.timeline_line_zoom
        };
        self.timeline_brush_start = None;
        self.pending_timeline_display_mode = Some(mode);
        self.ensure_visible();
    }

    /// Open the transient Shift-click context using the completed filter
    /// indexes. This is deliberately independent of lane visibility: the
    /// popup answers "where are the surrounding occurrences of each filter?"
    /// rather than recreating the filtered main viewport.
    pub(crate) fn open_occurrence_overlay(&mut self, selected_line: usize) {
        let selected_line = selected_line.min(self.doc.total_lines().saturating_sub(1));
        let (before, after) = occurrence_context_rows(&self.matches, selected_line);
        let mut overlay = OccurrenceOverlayState {
            selected_line,
            before,
            after,
            center_selected: true,
            embedded_detections: Arc::new(Vec::new()),
            embedded_rx: None,
        };
        self.start_occurrence_overlay_embedded_scan(&mut overlay);
        self.occurrence_overlay = Some(overlay);
    }

    pub(crate) fn close_occurrence_overlay(&mut self) {
        self.occurrence_overlay = None;
        self.annotation_hover = None;
    }

    fn start_occurrence_overlay_embedded_scan(&self, overlay: &mut OccurrenceOverlayState) {
        let interest_lines: Vec<usize> = overlay
            .rows()
            .map(|line| self.doc.trim_start + line)
            .collect();
        if interest_lines.is_empty() {
            return;
        }
        let trim_start = self.doc.trim_start;
        let trim_end = self.doc.trim_end;
        let mut intervals: Vec<(usize, usize)> = interest_lines
            .iter()
            .map(|&line| {
                self.doc
                    .record_range_containing(line)
                    .map(|record| (record.start.max(trim_start), record.end.min(trim_end)))
                    .unwrap_or_else(|| {
                        (
                            line.saturating_sub(32).max(trim_start),
                            (line + 513).min(trim_end),
                        )
                    })
            })
            .collect();
        intervals.sort_unstable();
        let mut merged = Vec::<(usize, usize)>::new();
        for interval in intervals {
            if let Some(last) = merged.last_mut() {
                if interval.0 <= last.1 {
                    last.1 = last.1.max(interval.1);
                    continue;
                }
            }
            merged.push(interval);
        }

        let doc = Arc::clone(&self.doc);
        let (tx, rx) = crossbeam_channel::bounded(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        crate::ui::worker_pool::spawn(move || {
            let engine = EmbeddedDataEngine::default();
            let limits = AnalysisLimits {
                max_bytes: 8 * 1024 * 1024,
                max_lines: 20_000,
                ..AnalysisLimits::default()
            };
            let mut detections = Vec::new();
            let mut seen = HashSet::<SourceSpan>::new();
            for (start, end) in merged {
                if worker_cancel.load(Ordering::Relaxed) {
                    return;
                }
                for detection in
                    engine.analyze_original_lines(&doc, start..end, limits, &worker_cancel)
                {
                    if interest_lines
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
                let _ = tx.send(Arc::new(detections));
            }
        });
        overlay.embedded_rx = Some((rx, cancel));
    }

    fn poll_occurrence_overlay_embedded_data(&mut self) -> bool {
        let Some(overlay) = self.occurrence_overlay.as_mut() else {
            return false;
        };
        let Some((rx, _)) = &overlay.embedded_rx else {
            return false;
        };
        match rx.try_recv() {
            Ok(detections) => {
                overlay.embedded_detections = detections;
                overlay.embedded_rx = None;
                false
            }
            Err(crossbeam_channel::TryRecvError::Empty) => true,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                overlay.embedded_rx = None;
                false
            }
        }
    }

    pub fn refresh_field_suggestions(&mut self, input: &str) {
        let Some((field, prefix)) = FieldQuery::completion_context(input) else {
            self.field_suggestion_key = None;
            self.field_suggestions = None;
            if let Some((_, _, cancel)) = self.field_suggestion_rx.take() {
                cancel.store(true, Ordering::Relaxed);
            }
            return;
        };
        let key = format!(
            "{}:{}:{}:{field}\0{prefix}",
            self.doc.file_size, self.doc.trim_start, self.doc.trim_end
        );
        if self.field_suggestion_key.as_deref() != Some(&key) {
            if let Some((_, _, cancel)) = self.field_suggestion_rx.take() {
                cancel.store(true, Ordering::Relaxed);
            }
            self.field_suggestion_key = Some(key.clone());
            self.field_suggestions = None;
            if self
                .doc
                .record_field_type(field)
                .is_some_and(crate::ui::field_query_ui::is_field_criteria_kind)
            {
                let (tx, rx) = crossbeam_channel::bounded(1);
                let cancel = Arc::new(AtomicBool::new(false));
                let worker_cancel = Arc::clone(&cancel);
                let doc = Arc::clone(&self.doc);
                let field = field.to_owned();
                let prefix = prefix.to_owned();
                crate::ui::worker_pool::spawn(move || {
                    let result = haystack::core::field_query::suggest_field_values(
                        &doc,
                        &field,
                        &prefix,
                        12,
                        &worker_cancel,
                    );
                    let _ = tx.send(result);
                });
                self.field_suggestion_rx = Some((key, rx, cancel));
            }
        }
        if let Some((key, rx, _)) = &self.field_suggestion_rx {
            match rx.try_recv() {
                Ok(Ok(values)) if self.field_suggestion_key.as_deref() == Some(key) => {
                    self.field_suggestions = Some(values);
                    self.field_suggestion_rx = None;
                }
                Ok(_) | Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.field_suggestion_rx = None;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {}
            }
        }
    }

    pub(super) fn filter_specs_snapshot(&self) -> Vec<search::FilterSpec> {
        self.filters
            .iter()
            .enumerate()
            .map(|(index, filter)| {
                let query = self.filter_field_queries.get(index).cloned().flatten();
                let valid = query
                    .as_ref()
                    .is_none_or(|query| query.compile(&self.doc).is_ok());
                search::FilterSpec {
                    text: if valid {
                        filter.text.clone()
                    } else {
                        String::new()
                    },
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
                    field_query: valid.then_some(query).flatten(),
                }
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
        self.filter_field_queries = specs.iter().map(|spec| spec.field_query.clone()).collect();
        self.filter_join = join;
        self.rescan_filters();
    }

    pub(super) fn apply_mcp_search(&mut self, request: haystack::mcp::GuiSearch) {
        if !Arc::ptr_eq(&self.doc, &request.doc) {
            return;
        }
        self.clear_find();
        let spec = request.spec;
        self.find_case_sensitive = spec.case_sensitive;
        self.find_regex = spec.regex;
        self.find_template_id_mode = spec.template_id.is_some();
        self.find_field_mode = spec.field_query.is_some();
        self.find_template_id = spec.template_id;
        self.find_input = spec.text.clone();
        self.find_query = spec.text.clone();
        self.find_highlighter = search::build_filter_highlighter(&[spec.clone()])
            .ok()
            .map(Arc::new);
        self.find_active_spec = Some(spec.clone());
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
        if let Some(query) = spec.field_query.as_ref() {
            self.field_search_history.retain(|entry| entry != query);
            self.field_search_history.insert(0, query.clone());
            self.field_search_history.truncate(20);
            self.pending_recent_field_search = Some(query.clone());
        } else {
            let find_query = self.find_query.clone();
            self.search_history.retain(|query| query != &find_query);
            self.search_history.insert(0, find_query.clone());
            self.search_history.truncate(20);
            self.pending_recent_search = Some(find_query);
        }
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
        let mut dock_state = DockState::new(vec![ViewTab::Log(LogViewId::INITIAL)]);
        let [_main_surface, _bottom_surface] = dock_state.main_surface_mut().split_below(
            egui_dock::NodeIndex::root(),
            0.8,
            vec![ViewTab::Pinned, ViewTab::Templates],
        );

        let initial_view = LogViewState::new(
            LogViewId::INITIAL,
            LogViewId::INITIAL.0,
            embedded_inspector_mode,
        );
        let log_views = BTreeMap::from([(LogViewId::INITIAL, initial_view)]);

        LogTab {
            log_views,
            focused_log_view_id: LogViewId::INITIAL,
            log_view_mru: vec![LogViewId::INITIAL],
            next_log_view_id: LogViewId::INITIAL.0 + 1,
            doc: Arc::clone(&doc),
            filters: Vec::new(),
            filter_case_sensitive: Vec::new(),
            filter_exclude: Vec::new(),
            filter_regex: Vec::new(),
            filter_template_ids: Vec::new(),
            filter_field_queries: Vec::new(),
            filter_join: haystack::core::search::FilterJoin::Any,
            filter_history: Vec::new(),
            filter_current_range: false,
            matches: Arc::new(Vec::new()),
            matched_filter_specs: Arc::new(Vec::new()),
            matched_filter_scope: None,
            matched_filter_doc: Arc::downgrade(&doc),
            matched_filter_doc_key: FilterDocumentKey::of(&doc),
            timeline,
            timeline_display_mode: TimelineDisplayMode::Line,
            template_browser: TemplateBrowserState::default(),
            timeline_zoom: None,
            timeline_line_zoom: None,
            timeline_real_time_zoom: None,
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
            filter_input_field_mode: false,
            filter_input_regex_validate_at: None,
            filter_input_regex_error: None,
            filter_input_regex_error_dismissed: false,
            highlighter: None,
            search_rx: None,
            filter_scan_error: None,
            filter_scan_progress: None,
            visible_rx: None,
            tail_rx: None,
            lane_active: Vec::new(),
            everything_else_active: true,
            pending_filter_removal: None,
            pending_clear_filters: false,
            timeline_detached: false,
            log_focus_mode: false,
            visible_lines: None,
            log_font_size: 12.0,
            log_line_display_mode: haystack::core::settings::LogLineDisplayMode::default(),
            pins: Vec::new(),
            bottom_panel_open: false,
            pin_comment: String::new(),
            pin_modal: None,
            pin_edit_index: None,
            applied_filter: None,
            search_history: Vec::new(),
            field_search_history: Vec::new(),
            pending_recent_search: None,
            pending_recent_field_search: None,
            pending_recent_filter: None,
            pending_timeline_display_mode: None,
            filter_suggestions_open: false,
            filter_highlight: None,
            dock_state,
            detached_views: HashSet::new(),
            detached_locations: HashMap::new(),
            detached_dock_states: HashMap::new(),
            detached_window_geometry: HashMap::new(),
            pending_detached_window_geometry: HashSet::new(),
            next_dock_window_id: 1,
            just_closed_viewports: Vec::new(),
            pending_detach: None,
            pending_add_log_view: None,
            active_dock_drag: None,
            mcp_serving: false,
            stale: false,
            pending_sidecar_restore: None,
            last_sidecar_snapshot: None,
        }
    }

    #[allow(dead_code)] // Staged for the MLV3 tab controls.
    pub fn log_view_count(&self) -> usize {
        self.log_views.len()
    }

    pub fn focused_log_view(&self) -> &LogViewState {
        self.log_views
            .get(&self.focused_log_view_id)
            .expect("LogTab must always own its focused Log View")
    }

    #[allow(dead_code)] // Reserved for explicit shared/view render contexts in MLV6.
    pub fn focused_log_view_mut(&mut self) -> &mut LogViewState {
        self.log_views
            .get_mut(&self.focused_log_view_id)
            .expect("LogTab must always own its focused Log View")
    }

    pub fn find_input_widget_id(&self) -> egui::Id {
        egui::Id::new(("log_find_input", self.focused_log_view_id))
    }

    pub fn focus_log_view(&mut self, id: LogViewId) -> bool {
        if !self.log_views.contains_key(&id) {
            return false;
        }
        self.focused_log_view_id = id;
        self.log_view_mru.retain(|candidate| *candidate != id);
        self.log_view_mru.insert(0, id);
        // A Log View owns its selection while the timeline zoom is shared by
        // the file. Switching views must bring the newly focused selection
        // back into the current timeline scope when necessary.
        self.ensure_visible();
        true
    }

    /// Create an independent view at the focused view's current location.
    /// Find, selection, inspectors, popups, and workers intentionally start
    /// empty; only durable location state is forked.
    pub fn add_log_view(&mut self) -> LogViewId {
        let id = LogViewId(self.next_log_view_id);
        self.next_log_view_id = self.next_log_view_id.saturating_add(1);
        let source = self.focused_log_view();
        let view = LogViewState::fork_from(source, id, id.0);
        self.log_views.insert(id, view);
        self.focus_log_view(id);
        id
    }

    pub fn detached_window_for(&self, view_tab: ViewTab) -> Option<DockWindowId> {
        self.detached_dock_states
            .iter()
            .find_map(|(window, state)| state.find_tab(&view_tab).map(|_| *window))
    }

    pub fn is_view_detached(&self, view_tab: ViewTab) -> bool {
        self.detached_window_for(view_tab).is_some()
    }

    pub fn dock_target_accepts(tab: ViewTab, target: DockDropTarget) -> bool {
        match target {
            DockDropTarget::MainLog => matches!(tab, ViewTab::Log(_)),
            DockDropTarget::MainUtility => matches!(tab, ViewTab::Pinned | ViewTab::Templates),
            DockDropTarget::JoinDetached { .. } | DockDropTarget::SplitDetached { .. } => {
                !matches!(tab, ViewTab::Timeline)
            }
            DockDropTarget::TabInsert {
                container, sibling, ..
            } => match container {
                DockContainer::Detached(_) => !matches!(tab, ViewTab::Timeline),
                DockContainer::Main => {
                    matches!((tab, sibling), (ViewTab::Log(_), ViewTab::Log(_)))
                        || (matches!(tab, ViewTab::Pinned | ViewTab::Templates)
                            && matches!(sibling, ViewTab::Pinned | ViewTab::Templates))
                }
            },
        }
    }

    /// Move a workspace tab between the main window's fixed homes and any
    /// detached free-form dock window.
    pub fn dock_view(&mut self, view_tab: ViewTab, target: DockDropTarget) -> bool {
        if matches!(view_tab, ViewTab::Log(id) if !self.log_views.contains_key(&id))
            || !Self::dock_target_accepts(view_tab, target)
        {
            return false;
        }
        let _target_window = match target {
            DockDropTarget::JoinDetached { window, .. }
            | DockDropTarget::SplitDetached { window, .. }
                if self.detached_views.contains(&window)
                    && self.detached_dock_states.contains_key(&window) =>
            {
                Some(window)
            }
            DockDropTarget::TabInsert {
                container: DockContainer::Detached(window),
                ..
            } if self.detached_views.contains(&window)
                && self.detached_dock_states.contains_key(&window) =>
            {
                Some(window)
            }
            DockDropTarget::JoinDetached { .. }
            | DockDropTarget::SplitDetached { .. }
            | DockDropTarget::TabInsert {
                container: DockContainer::Detached(_),
                ..
            } => return false,
            DockDropTarget::MainLog | DockDropTarget::MainUtility => None,
            DockDropTarget::TabInsert {
                container: DockContainer::Main,
                ..
            } => None,
        };
        let source_main = self.dock_state.find_tab(&view_tab).is_some();
        let source_window = self.detached_window_for(view_tab);
        if !source_main && source_window.is_none() {
            return false;
        }
        if matches!(target,
            DockDropTarget::JoinDetached { sibling, .. }
                | DockDropTarget::SplitDetached { sibling, .. }
                | DockDropTarget::TabInsert { sibling, .. }
                if sibling == view_tab
        ) {
            return false;
        }
        let target_exists = match target {
            DockDropTarget::JoinDetached { window, sibling }
            | DockDropTarget::SplitDetached {
                window, sibling, ..
            } => self
                .detached_dock_states
                .get(&window)
                .is_some_and(|state| state.find_tab(&sibling).is_some()),
            DockDropTarget::TabInsert {
                container: DockContainer::Main,
                sibling,
                ..
            } => self.dock_state.find_tab(&sibling).is_some(),
            DockDropTarget::TabInsert {
                container: DockContainer::Detached(window),
                sibling,
                ..
            } => self
                .detached_dock_states
                .get(&window)
                .is_some_and(|state| state.find_tab(&sibling).is_some()),
            DockDropTarget::MainLog | DockDropTarget::MainUtility => true,
        };
        if !target_exists {
            return false;
        }

        if source_main {
            let location = self.dock_state.find_tab(&view_tab).unwrap();
            self.dock_state.remove_tab(location);
        } else if let Some(window) = source_window {
            let state = self.detached_dock_states.get_mut(&window).unwrap();
            let location = state.find_tab(&view_tab).unwrap();
            state.remove_tab(location);
        }

        match target {
            DockDropTarget::JoinDetached { window, sibling } => {
                let state = self.detached_dock_states.get_mut(&window).unwrap();
                let leaf = state.find_tab(&sibling).unwrap().node_path();
                state.set_focused_node_and_surface(leaf);
                state.push_to_focused_leaf(view_tab);
                if source_main {
                    self.normalize_main_dock_layout();
                }
            }
            DockDropTarget::SplitDetached {
                window,
                sibling,
                split,
            } => {
                let state = self.detached_dock_states.get_mut(&window).unwrap();
                let leaf = state.find_tab(&sibling).unwrap().node_path();
                state[leaf.surface].split_tabs(leaf.node, split, 0.5, vec![view_tab]);
                if source_main {
                    self.normalize_main_dock_layout();
                }
            }
            DockDropTarget::TabInsert {
                container,
                sibling,
                after,
            } => {
                let state = match container {
                    DockContainer::Main => &mut self.dock_state,
                    DockContainer::Detached(window) => {
                        self.detached_dock_states.get_mut(&window).unwrap()
                    }
                };
                let Some(sibling_path) = state.find_tab(&sibling) else {
                    return false;
                };
                let Ok(leaf) = state.leaf_mut(sibling_path.node_path()) else {
                    return false;
                };
                let index = sibling_path.tab.0 + usize::from(after);
                leaf.insert_tab(egui_dock::TabIndex(index.min(leaf.tabs().len())), view_tab);
                if source_main && !matches!(container, DockContainer::Main) {
                    self.normalize_main_dock_layout();
                }
            }
            DockDropTarget::MainLog | DockDropTarget::MainUtility => {
                self.dock_view_in_main(view_tab)
            }
        }

        if let Some(window) = source_window {
            let source_is_empty = self
                .detached_dock_states
                .get(&window)
                .is_some_and(|state| state.iter_all_tabs().next().is_none());
            if source_is_empty {
                self.detached_dock_states.remove(&window);
                self.detached_views.remove(&window);
                self.detached_window_geometry.remove(&window);
                self.pending_detached_window_geometry.remove(&window);
            }
        }
        if let ViewTab::Log(id) = view_tab {
            self.focus_log_view(id);
        }
        true
    }

    fn dock_view_in_main(&mut self, view_tab: ViewTab) {
        let mut logs: Vec<_> = self
            .dock_state
            .iter_all_tabs()
            .filter_map(|(_, tab)| match tab {
                ViewTab::Log(existing) => Some(*existing),
                _ => None,
            })
            .collect();
        let mut utilities: Vec<_> = self
            .dock_state
            .iter_all_tabs()
            .filter_map(|(_, tab)| {
                matches!(tab, ViewTab::Pinned | ViewTab::Templates).then_some(*tab)
            })
            .collect();
        match view_tab {
            ViewTab::Log(id) if !logs.contains(&id) => logs.push(id),
            ViewTab::Pinned | ViewTab::Templates if !utilities.contains(&view_tab) => {
                utilities.push(view_tab)
            }
            _ => {}
        }
        self.rebuild_main_dock(logs, utilities);
    }

    /// Restore the only legal main-window arrangement: Log Views in the top
    /// leaf, with Pinned and Templates sharing the lower leaf. This also
    /// sanitizes layouts saved before the placement restriction existed.
    pub fn normalize_main_dock_layout(&mut self) {
        let logs = self
            .dock_state
            .iter_all_tabs()
            .filter_map(|(_, tab)| match tab {
                ViewTab::Log(id) if self.log_views.contains_key(id) => Some(*id),
                _ => None,
            })
            .collect();
        let utilities = self
            .dock_state
            .iter_all_tabs()
            .filter_map(|(_, tab)| {
                matches!(tab, ViewTab::Pinned | ViewTab::Templates).then_some(*tab)
            })
            .collect();
        self.rebuild_main_dock(logs, utilities);
    }

    /// Whether the visible main dock still matches the fixed top-Log / lower
    /// utility-panel contract. Detached windows intentionally do not use this
    /// check and stay free-form.
    pub fn main_dock_layout_is_legal(&self) -> bool {
        let mut log_leaf = None;
        let mut utility_leaf = None;
        let mut pinned = 0usize;
        let mut templates = 0usize;
        for (path, tab) in self.dock_state.iter_all_tabs() {
            match tab {
                ViewTab::Log(id) if self.log_views.contains_key(id) => {
                    if log_leaf
                        .replace(path.node_path())
                        .is_some_and(|leaf| leaf != path.node_path())
                    {
                        return false;
                    }
                }
                ViewTab::Pinned => {
                    pinned += 1;
                    if utility_leaf
                        .replace(path.node_path())
                        .is_some_and(|leaf| leaf != path.node_path())
                    {
                        return false;
                    }
                }
                ViewTab::Templates => {
                    templates += 1;
                    if utility_leaf
                        .replace(path.node_path())
                        .is_some_and(|leaf| leaf != path.node_path())
                    {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        pinned <= 1
            && templates <= 1
            && match (log_leaf, utility_leaf) {
                (Some(log_leaf), Some(utility_leaf)) => log_leaf != utility_leaf,
                _ => true,
            }
    }

    fn rebuild_main_dock(&mut self, mut logs: Vec<LogViewId>, mut utilities: Vec<ViewTab>) {
        logs.sort();
        logs.dedup();
        utilities.sort();
        utilities.dedup();
        if logs.is_empty() {
            self.dock_state = DockState::new(utilities);
            return;
        }
        let mut dock_state = DockState::new(logs.into_iter().map(ViewTab::Log).collect());
        if !utilities.is_empty() {
            dock_state
                .main_surface_mut()
                .split_below(egui_dock::NodeIndex::root(), 0.8, utilities);
        }
        if let Some(path) = dock_state.find_tab(&ViewTab::Log(self.focused_log_view_id)) {
            let _ = dock_state.set_active_tab(path);
        }
        self.dock_state = dock_state;
    }

    /// Permanently remove a Log View. The final view is an invariant and can
    /// never be removed through model, middle-click, or UI close paths.
    pub fn close_log_view(&mut self, id: LogViewId) -> bool {
        if self.log_views.len() <= 1 || !self.log_views.contains_key(&id) {
            return false;
        }
        let view_tab = ViewTab::Log(id);
        self.detached_locations.remove(&view_tab);
        for state in self.detached_dock_states.values_mut() {
            state.retain_tabs(|candidate| *candidate != view_tab);
        }
        let emptied: Vec<_> = self
            .detached_dock_states
            .iter()
            .filter_map(|(window, state)| state.iter_all_tabs().next().is_none().then_some(*window))
            .collect();
        for window in emptied {
            self.detached_dock_states.remove(&window);
            self.detached_views.remove(&window);
            self.detached_window_geometry.remove(&window);
            self.pending_detached_window_geometry.remove(&window);
        }
        if self.pending_detach == Some(view_tab) {
            self.pending_detach = None;
        }
        self.log_views.remove(&id);
        self.log_view_mru.retain(|candidate| *candidate != id);
        if self.focused_log_view_id == id {
            let replacement = self
                .log_view_mru
                .iter()
                .copied()
                .find(|candidate| self.log_views.contains_key(candidate))
                .or_else(|| self.log_views.keys().next().copied())
                .expect("closing a Log View must leave one survivor");
            self.focused_log_view_id = replacement;
            self.log_view_mru
                .retain(|candidate| *candidate != replacement);
            self.log_view_mru.insert(0, replacement);
        }
        true
    }

    /// Move one dock item into the app's native detached-viewport registry.
    /// Each item retains its own best-effort return location.
    pub fn detach_dock_view(&mut self, view_tab: ViewTab) -> Option<DockWindowId> {
        if matches!(view_tab, ViewTab::Log(id) if !self.log_views.contains_key(&id)) {
            return None;
        }
        let Some(location) = self.dock_state.find_tab(&view_tab) else {
            return None;
        };
        let window = DockWindowId(self.next_dock_window_id);
        self.next_dock_window_id = self.next_dock_window_id.saturating_add(1);
        self.detached_locations.insert(view_tab, location);
        self.detached_views.insert(window);
        self.detached_dock_states
            .insert(window, DockState::new(vec![view_tab]));
        self.detached_window_geometry
            .insert(window, DetachedWindowGeometry::default());
        self.pending_detached_window_geometry.insert(window);
        self.dock_state.remove_tab(location);
        self.normalize_main_dock_layout();
        Some(window)
    }

    /// Move a dockable tab into a new native dock window at a screen position.
    pub fn detach_view_to_window(
        &mut self,
        view_tab: ViewTab,
        position: Option<egui::Pos2>,
    ) -> Option<DockWindowId> {
        if matches!(view_tab, ViewTab::Timeline)
            || matches!(view_tab, ViewTab::Log(id) if !self.log_views.contains_key(&id))
        {
            return None;
        }
        let source_main = self.dock_state.find_tab(&view_tab);
        let source_window = self.detached_window_for(view_tab);
        if source_main.is_none() && source_window.is_none() {
            return None;
        }
        if let Some(location) = source_main {
            self.detached_locations.insert(view_tab, location);
            self.dock_state.remove_tab(location);
            self.normalize_main_dock_layout();
        } else if let Some(window) = source_window {
            let state = self.detached_dock_states.get_mut(&window)?;
            let location = state.find_tab(&view_tab)?;
            state.remove_tab(location);
            if state.iter_all_tabs().next().is_none() {
                self.detached_dock_states.remove(&window);
                self.detached_views.remove(&window);
                self.detached_window_geometry.remove(&window);
                self.pending_detached_window_geometry.remove(&window);
            }
        }

        let window = DockWindowId(self.next_dock_window_id);
        self.next_dock_window_id = self.next_dock_window_id.saturating_add(1);
        self.detached_views.insert(window);
        self.detached_dock_states
            .insert(window, DockState::new(vec![view_tab]));
        self.detached_window_geometry.insert(
            window,
            DetachedWindowGeometry {
                position,
                ..Default::default()
            },
        );
        self.pending_detached_window_geometry.insert(window);
        if let ViewTab::Log(id) = view_tab {
            self.focus_log_view(id);
        }
        Some(window)
    }

    /// Return a native viewport to its recorded dock leaf. If dock edits made
    /// that leaf invalid while the window was open, use the focused leaf.
    pub fn redock_view(&mut self, window: DockWindowId) -> bool {
        let was_detached = self.detached_views.remove(&window);
        self.detached_window_geometry.remove(&window);
        self.pending_detached_window_geometry.remove(&window);
        let detached_state = self.detached_dock_states.remove(&window);
        if !was_detached {
            return false;
        }
        let tabs: Vec<_> = detached_state
            .into_iter()
            .flat_map(|state| {
                state
                    .iter_all_tabs()
                    .map(|(_, tab)| *tab)
                    .collect::<Vec<_>>()
            })
            .collect();
        let tabs = if tabs.is_empty() {
            // Sidecar restores intentionally keep only logical detached
            // membership; rebuild the initial one-tab panel lazily.
            return false;
        } else {
            tabs
        };
        for tab in tabs {
            self.detached_locations.remove(&tab);
            self.dock_view_in_main(tab);
        }
        self.normalize_main_dock_layout();
        true
    }

    pub fn cancel_all_log_view_workers(&self) {
        for view in self.log_views.values() {
            view.cancel_workers();
        }
    }

    /// Poll view-owned workers without treating render order as focus order.
    pub fn poll_log_view_workers(&mut self) -> bool {
        let focused = self.focused_log_view_id;
        let ids: Vec<_> = self.log_views.keys().copied().collect();
        let mut pending = false;
        for id in ids {
            self.focused_log_view_id = id;
            pending |= self.poll_find();
            pending |= self.poll_embedded_data();
            pending |= self.poll_occurrence_overlay_embedded_data();
        }
        self.focused_log_view_id = focused;
        pending
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

    pub(super) fn invalidate_all_log_view_embedded_data(&mut self) {
        let focused = self.focused_log_view_id;
        let ids: Vec<_> = self.log_views.keys().copied().collect();
        for id in ids {
            self.focused_log_view_id = id;
            self.invalidate_embedded_data();
            // A document replacement can rebase source rows, so a transient
            // occurrence snapshot is safer to close than to silently point at
            // different lines.
            self.occurrence_overlay = None;
        }
        self.focused_log_view_id = focused;
    }

    fn capture_all_viewport_anchors(&mut self) {
        for view in self.log_views.values_mut() {
            view.preserve_anchor = view.viewport_range.map(|(first, _)| first);
        }
    }

    fn restart_all_log_view_searches(&mut self) {
        let focused = self.focused_log_view_id;
        let searches: Vec<_> = self
            .log_views
            .iter()
            .filter(|(_, view)| !view.find_query.is_empty())
            .map(|(id, view)| (*id, view.find_query.clone()))
            .collect();
        for (id, query) in searches {
            self.focused_log_view_id = id;
            self.start_find(query);
        }
        self.focused_log_view_id = focused;
    }

    /// Rebuild the filtered visible-lines list based on active lanes.
    /// If all lanes + Everything Else are active, sets visible_lines to None (fast path).
    pub fn rebuild_visible_lines(&mut self) {
        let n = self.doc.total_lines();
        if n == 0 {
            self.install_visible_lines(None, n);
            return;
        }
        while self.lane_active.len() < self.filters.len() {
            self.lane_active.push(true);
        }
        self.lane_active.truncate(self.filters.len());

        // Capture the top-visible real line so the log viewport can be
        // preserved across a filter change.
        self.capture_all_viewport_anchors();

        let visible_lines = build_visible_lines(
            n,
            &self.matches,
            &self.lane_active,
            &self.filter_exclude,
            self.filter_join,
            self.everything_else_active,
            None,
        );
        self.install_visible_lines(visible_lines, n);
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
        self.capture_all_viewport_anchors();
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
                let _ = tx.send(VisibleLinesResult { visible_lines });
            }
        });
        self.visible_rx = Some((rx, cancel));
    }

    /// Install a rebuilt visible-line index while keeping the current
    /// selection authoritative. A viewport anchor is used only when there is
    /// no selection that can be retained or replaced.
    fn install_visible_lines(&mut self, visible_lines: Option<Arc<Vec<u32>>>, n: usize) {
        self.visible_lines = visible_lines;
        let visible_lines = self.visible_lines.clone();
        for view in self.log_views.values_mut() {
            Self::reconcile_log_view_visibility(
                view,
                visible_lines.as_deref().map(Vec::as_slice),
                n,
            );
        }
        self.restart_all_log_view_searches();
    }

    fn reconcile_log_view_visibility(
        view: &mut LogViewState,
        visible_lines: Option<&[u32]>,
        n: usize,
    ) {
        let nearest_visible = |line: usize| match visible_lines {
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
        };

        if let Some(selected) = view.context_line {
            // Selection is a real document line, rather than a virtual-row
            // index. Keep that identity exactly when the filter update still
            // renders it; only a hidden selection is allowed to move.
            let selected_is_visible = match visible_lines {
                Some(visible) => visible.binary_search(&(selected as u32)).is_ok(),
                None => selected < n,
            };
            let replacement = selected_is_visible
                .then_some(selected)
                .or_else(|| nearest_visible(selected));
            view.context_line = replacement;
            view.preserve_anchor = None;
            view.pending_scroll = replacement.filter(|&line| {
                line != selected
                    || !view
                        .viewport_range
                        .is_some_and(|(first, last)| first <= line && line <= last)
            });
            return;
        }

        if let Some(anchor) = view.preserve_anchor {
            let found = nearest_visible(anchor);
            if !matches!(found, Some(line) if line == anchor) {
                view.preserve_anchor = found;
            }
        }
        if let Some(anchor) = view.preserve_anchor.take() {
            view.pending_scroll = Some(anchor);
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
        if idx < self.filter_field_queries.len() {
            self.filter_field_queries.remove(idx);
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
        let field_query = self.filter_field_queries.get(idx).cloned().flatten();
        self.remove_filter(idx);
        self.undo_delete = Some(UndoDelete::Filter {
            index: idx,
            filter,
            active,
            case_sensitive,
            exclude,
            regex,
            template_id,
            field_query,
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
        if active {
            if let Some(error) = self
                .filter_field_queries
                .get(idx)
                .and_then(Option::as_ref)
                .and_then(|query| query.compile(&self.doc).err())
            {
                self.pending_toast = Some(format!("Field filter needs review: {error}"));
                return false;
            }
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
                field_query,
            } => {
                let index = index.min(self.filters.len());
                let active = active
                    && field_query
                        .as_ref()
                        .is_none_or(|query| query.compile(&self.doc).is_ok());
                self.filters.insert(index, filter);
                self.filter_case_sensitive
                    .insert(index.min(self.filter_case_sensitive.len()), case_sensitive);
                self.filter_exclude
                    .insert(index.min(self.filter_exclude.len()), exclude);
                self.filter_regex
                    .insert(index.min(self.filter_regex.len()), regex);
                self.filter_template_ids
                    .insert(index.min(self.filter_template_ids.len()), template_id);
                self.filter_field_queries
                    .insert(index.min(self.filter_field_queries.len()), field_query);
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
        let focused_log_tab = ViewTab::Log(self.focused_log_view_id);
        if !self.is_view_detached(focused_log_tab) {
            if let Some(path) = self.dock_state.find_tab(&focused_log_tab) {
                let _ = self.dock_state.set_active_tab(path);
            }
        }
        true
    }

    /// Called after the main dock has rendered, completing Timeline pin
    /// navigation without preventing the Log View from scrolling first.
    pub fn finish_pin_navigation(&mut self) {
        if !std::mem::take(&mut self.pending_pin_activation)
            || self.is_view_detached(ViewTab::Pinned)
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
        for (index, flag) in self.lane_active.iter_mut().enumerate() {
            let compatible = self
                .filter_field_queries
                .get(index)
                .and_then(Option::as_ref)
                .is_none_or(|query| query.compile(&self.doc).is_ok());
            *flag = !all_active && compatible;
        }
        if !self.lane_active.iter().any(|&a| a) && !self.everything_else_active {
            self.everything_else_active = true;
        }
        self.rebuild_visible_lines_background();
    }

    /// Re-scan the document for the current filter set in the background.
    pub fn rescan_filters(&mut self) {
        if self.timeline_display_mode != TimelineDisplayMode::Line && self.doc.time_range.is_none()
        {
            self.timeline_display_mode = TimelineDisplayMode::Line;
            self.timeline_real_time_zoom = None;
            self.timeline_zoom = self.timeline_line_zoom;
            self.timeline_brush_start = None;
        }
        if let Some((_, cancel)) = &self.search_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        self.filter_scan_progress = None;
        self.filter_scan_error = None;
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
        self.filter_field_queries.resize(self.filters.len(), None);

        let scan_zoom = self
            .filter_current_range
            .then_some(self.timeline_zoom)
            .flatten();
        let current_doc_key = FilterDocumentKey::of(&self.doc);
        let real_time = self.timeline_display_mode.uses_real_time_coordinates();
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
            } else if real_time {
                Timeline::build_real_time_shared_u32(&self.doc, &[], DEFAULT_BUCKETS)
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
                    Err(error) => {
                        let _ = tx.send(Err(error));
                        return;
                    }
                }
            };
            for ((new_index, lane), spec) in
                missing_indexes.into_iter().zip(scanned).zip(missing_specs)
            {
                let lane = if let Some((start, end)) = scan_zoom {
                    lane.into_iter()
                        .filter(|&line| {
                            let value = match scan_domain {
                                haystack::core::timeline::TimelineDomain::Time { .. } => {
                                    doc.ts_at(line as usize)
                                }
                                haystack::core::timeline::TimelineDomain::Sequence => line as i64,
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
                None if real_time => {
                    Timeline::build_real_time_shared_u32(&doc, &matches, DEFAULT_BUCKETS)
                }
                None => Timeline::build_shared_u32(&doc, &matches, DEFAULT_BUCKETS),
            };
            if !cancel_worker.load(Ordering::Relaxed) {
                let _ = tx.send(Ok(FilterScanResult {
                    matches,
                    timeline,
                    specs: filters,
                    scope: scan_zoom,
                }));
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
            let result = match updated.append_new_data_detailed() {
                Ok(Some(update)) => Ok(TailUpdateResult {
                    doc: Box::new(updated),
                    old_line_count,
                    first_changed_line: update.first_changed_line.saturating_sub(doc.trim_start),
                    added_lines: update.added_lines,
                }),
                Ok(None) => Err("file no longer has appended data".to_string()),
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
    fn extend_filters_after_append(&mut self, old_line_count: usize, first_changed_line: usize) {
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
        let real_time = self.timeline_display_mode.uses_real_time_coordinates();
        let timeline_base = self.timeline.base_without_filters();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancel);
        let progress = Arc::new(search::ScanProgress::default());
        let progress_worker = Arc::clone(&progress);
        crate::ui::worker_pool::spawn(move || {
            let appended = if filters.is_empty() {
                progress_worker.total_lines.store(
                    doc.total_lines().saturating_sub(first_changed_line),
                    Ordering::Relaxed,
                );
                progress_worker.scanned_lines.store(
                    doc.total_lines().saturating_sub(first_changed_line),
                    Ordering::Relaxed,
                );
                Vec::new()
            } else {
                match search::scan_advanced_range(
                    &doc,
                    &filters,
                    first_changed_line..doc.total_lines(),
                    &cancel_worker,
                    Some(&progress_worker),
                ) {
                    Ok(matches) => matches,
                    Err(error) => {
                        let _ = tx.send(Err(error));
                        return;
                    }
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
                        let retained =
                            old.partition_point(|&line| (line as usize) < first_changed_line);
                        let mut combined = Vec::with_capacity(retained + new.len());
                        combined.extend_from_slice(&old[..retained]);
                        combined.extend(new);
                        Arc::new(combined)
                    })
                    .collect(),
            );
            let timeline = timeline_base
                .extend_append(&doc, old_line_count, &matches)
                .unwrap_or_else(|| {
                    if real_time {
                        Timeline::build_real_time_shared_u32(&doc, &matches, DEFAULT_BUCKETS)
                    } else {
                        Timeline::build_shared_u32(&doc, &matches, DEFAULT_BUCKETS)
                    }
                });
            if !cancel_worker.load(Ordering::Relaxed) {
                let _ = tx.send(Ok(FilterScanResult {
                    matches,
                    timeline,
                    specs: filters,
                    scope: None,
                }));
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
                let new_lines = result.added_lines;
                let file_name = result.doc.file_name.clone();
                let can_extend = self.can_extend_filters_after_append(result.old_line_count);
                self.doc = Arc::new(*result.doc);
                self.tail_rx = None;
                self.stale = false;
                self.invalidate_all_log_view_embedded_data();
                if can_extend {
                    self.extend_filters_after_append(
                        result.old_line_count,
                        result.first_changed_line,
                    );
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

    /// Clamp all doc-positioned Log View state to the current visible window.
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
        for view in self.log_views.values_mut() {
            clamp_pos(&mut view.context_line);
            clamp_pos(&mut view.pending_scroll);
            clamp_pos(&mut view.scroll_top_line);
            clamp_pos(&mut view.preserve_anchor);
            clamp_pos(&mut view.drag_start_line);
            clamp_pos(&mut view.drag_current_line);
            clamp_range(&mut view.viewport_range);
            clamp_range(&mut view.selection_range);
            clamp_range(&mut view.pending_selection);
            view.pending_scroll_restore =
                view.pending_scroll_restore.and_then(|(line, fraction)| {
                    (n > 0).then_some((line.min(n.saturating_sub(1)), fraction))
                });
            if let Some(state) = view.analysis_popup.as_mut() {
                if n == 0 {
                    view.analysis_popup = None;
                } else {
                    state.range.0 = state.range.0.min(n - 1);
                    state.range.1 = state.range.1.min(n - 1);
                }
            }
            view.occurrence_overlay = None;
            view.wrap_layout = None;
        }
        clamp_range(&mut self.pin_modal);
    }

    /// Ensure the timeline zoom window includes the current context_line.
    /// If the line is outside the visible range, auto-pan to center on it.
    pub fn ensure_visible(&mut self) {
        let Some(line) = self.context_line else {
            return;
        };
        let v = match self.timeline.domain {
            haystack::core::timeline::TimelineDomain::Time { .. } => {
                // context_line can hold a stale index after an MCP doc swap and
                // exceed the current window; never let the unchecked ts_at index
                // out of bounds (regression: index 1060 vs len 1000 panic).
                match self.doc.ts_at_opt(line) {
                    Some(t) if t >= 0 => t,
                    _ => return,
                }
            }
            haystack::core::timeline::TimelineDomain::Sequence => line as i64,
        };
        if v < 0 {
            return;
        }
        let (full_start, full_end) = match self.timeline.domain {
            haystack::core::timeline::TimelineDomain::Time { start_ms, end_ms } => {
                (start_ms, end_ms)
            }
            haystack::core::timeline::TimelineDomain::Sequence => {
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
        if first_line >= self.doc.total_lines() || last_line >= self.doc.total_lines() {
            return;
        }
        let (v0, v1) = match self.timeline.domain {
            haystack::core::timeline::TimelineDomain::Time { .. } => {
                let mut min = i64::MAX;
                let mut max = i64::MIN;
                for line in first_line..=last_line {
                    if let Some(timestamp) = self.doc.ts_at_opt(line).filter(|value| *value >= 0) {
                        min = min.min(timestamp);
                        max = max.max(timestamp);
                    }
                }
                if min > max {
                    return;
                }
                (min, max)
            }
            haystack::core::timeline::TimelineDomain::Sequence => {
                (first_line as i64, last_line as i64)
            }
        };
        // Match the shadow renderer: only act when the mapped values are valid.
        if v0 < 0 || v1 < 0 {
            return;
        }
        let (full_start, full_end) = match self.timeline.domain {
            haystack::core::timeline::TimelineDomain::Time { start_ms, end_ms } => {
                (start_ms, end_ms)
            }
            haystack::core::timeline::TimelineDomain::Sequence => {
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
        self.rebase_all_log_view_lines(previous_start, self.doc.trim_start);
        self.undo_delete = Some(UndoDelete::Trim {
            start: previous_start,
            end_exclusive: previous_end,
        });

        // Rebuild derived state. Investigation state remains intact: pins are
        // rebased to their original log positions and out-of-window anchors
        // are hidden by the Pin panel rather than discarded.
        self.visible_lines = None;
        self.timeline_zoom = None;
        self.timeline_line_zoom = None;
        self.timeline_real_time_zoom = None;
        self.pending_filter_removal = None;
        self.pending_clear_filters = false;
        self.invalidate_all_log_view_embedded_data();
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
        self.rebase_all_log_view_lines(previous_start, self.doc.trim_start);
        self.undo_delete = Some(UndoDelete::Trim {
            start: previous_start,
            end_exclusive: previous_end,
        });

        // Reset state.
        self.visible_lines = None;
        self.timeline_zoom = None;
        self.timeline_line_zoom = None;
        self.timeline_real_time_zoom = None;
        self.pending_filter_removal = None;
        self.pending_clear_filters = false;
        self.invalidate_all_log_view_embedded_data();
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
        self.rebase_all_log_view_lines(previous_start, self.doc.trim_start);
        self.visible_lines = None;
        self.timeline_zoom = None;
        self.timeline_line_zoom = None;
        self.timeline_real_time_zoom = None;
        self.invalidate_all_log_view_embedded_data();
        self.clamp_view_state();
        self.rescan_filters();
    }

    /// Pin coordinates are trim-relative in memory. Rebase them whenever the
    /// visible window moves so they continue to refer to the same source rows.
    pub(super) fn rebase_pinned_lines(&mut self, old_trim_start: usize, new_trim_start: usize) {
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

    /// Preserve each view's physical source location when the shared trim
    /// window moves. Anchors outside the new window clamp to its nearest edge.
    pub(super) fn rebase_all_log_view_lines(
        &mut self,
        old_trim_start: usize,
        new_trim_start: usize,
    ) {
        let n = self.doc.total_lines();
        let rebase = |line: usize| {
            (n > 0).then(|| {
                old_trim_start
                    .saturating_add(line)
                    .saturating_sub(new_trim_start)
                    .min(n.saturating_sub(1))
            })
        };
        let rebase_pos = |position: &mut Option<usize>| {
            *position = position.and_then(rebase);
        };
        let rebase_range = |range: &mut Option<(usize, usize)>| {
            *range = range.and_then(|(first, last)| Some((rebase(first)?, rebase(last)?)));
        };
        for view in self.log_views.values_mut() {
            view.cancel_workers();
            view.find_rx = None;
            view.field_suggestion_rx = None;
            view.find_matches.clear();
            view.find_pos = None;
            rebase_pos(&mut view.context_line);
            rebase_pos(&mut view.pending_scroll);
            rebase_pos(&mut view.scroll_top_line);
            rebase_pos(&mut view.preserve_anchor);
            rebase_pos(&mut view.drag_start_line);
            rebase_pos(&mut view.drag_current_line);
            rebase_range(&mut view.viewport_range);
            rebase_range(&mut view.selection_range);
            rebase_range(&mut view.pending_selection);
            view.pending_scroll_restore = view
                .pending_scroll_restore
                .and_then(|(line, fraction)| rebase(line).map(|line| (line, fraction)));
            if let Some(state) = view.analysis_popup.as_mut() {
                match (rebase(state.range.0), rebase(state.range.1)) {
                    (Some(first), Some(last)) => state.range = (first, last),
                    _ => view.analysis_popup = None,
                }
            }
            view.occurrence_overlay = None;
            view.wrap_layout = None;
        }
    }

    /// Poll the background filter scan and install its already-built timeline.
    pub fn poll_search(&mut self) -> bool {
        let Some((rx, _)) = &self.search_rx else {
            return false;
        };
        match rx.try_recv() {
            Ok(Ok(result)) => {
                let overlay_line = self
                    .occurrence_overlay
                    .as_ref()
                    .map(|overlay| overlay.selected_line);
                self.matches = result.matches;
                self.timeline = result.timeline;
                self.matched_filter_specs = result.specs;
                self.matched_filter_scope = result.scope;
                self.matched_filter_doc = Arc::downgrade(&self.doc);
                self.matched_filter_doc_key = FilterDocumentKey::of(&self.doc);
                self.search_rx = None;
                self.filter_scan_progress = None;
                self.rebuild_visible_lines();
                if let Some(line) = overlay_line {
                    self.open_occurrence_overlay(line);
                }
                false
            }
            Ok(Err(error)) => {
                self.filter_scan_error = Some(error);
                self.search_rx = None;
                self.filter_scan_progress = None;
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
                self.install_visible_lines(result.visible_lines, self.doc.total_lines());
                self.visible_rx = None;
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
                field_query: None,
            });
        }
        if self.find_field_mode {
            let query =
                FieldQuery::parse(text, self.find_case_sensitive)?.bind_to_doc(&self.doc)?;
            return Ok(search::FilterSpec {
                text: query.expression(),
                case_sensitive: query.case_sensitive,
                polarity: search::FilterPolarity::Include,
                regex: false,
                template_id: None,
                field_query: Some(query),
            });
        }
        let spec = search::FilterSpec {
            text: text.to_owned(),
            case_sensitive: self.find_case_sensitive,
            polarity: search::FilterPolarity::Include,
            regex: self.find_regex,
            template_id: None,
            field_query: None,
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
        if let Some(query) = spec.field_query.as_ref() {
            self.field_search_history.retain(|entry| entry != query);
            self.field_search_history.insert(0, query.clone());
            self.field_search_history.truncate(20);
            self.pending_recent_field_search = Some(query.clone());
        } else {
            self.search_history.retain(|entry| entry != trimmed);
            self.search_history.insert(0, trimmed.to_string());
            self.search_history.truncate(20);
            self.pending_recent_search = Some(trimmed.to_string());
        }
        self.find_template_id = spec.template_id;
        self.find_active_spec = Some(spec.clone());
        self.find_highlighter = search::build_filter_highlighter(&[spec.clone()])
            .ok()
            .map(Arc::new);
        self.find_error = None;
        self.find_error_dismissed = false;
        self.find_matches.clear();
        self.find_record_count = 0;
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
            );
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
            Ok(Ok(matches)) => {
                self.find_matches = matches;
                self.find_record_count = if self
                    .find_active_spec
                    .as_ref()
                    .is_some_and(|spec| spec.field_query.is_some())
                {
                    let mut last_owner = None;
                    self.find_matches
                        .iter()
                        .filter(|&&line| {
                            let owner = self
                                .doc
                                .record_range_containing(self.doc.trim_start + line as usize)
                                .map(|record| record.start);
                            if owner.is_none() || owner == last_owner {
                                return false;
                            }
                            last_owner = owner;
                            true
                        })
                        .count()
                } else {
                    0
                };
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
            Ok(Err(error)) => {
                self.find_error = Some(error);
                self.find_error_dismissed = false;
                self.find_rx = None;
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
        self.find_record_count = 0;
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
        self.find_field_mode = false;
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
        self.filter_field_queries.push(None);
        let history_entry = haystack::core::settings::RecentFilter {
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
        self.filter_field_queries.push(None);
        self.filter_highlight = Some((idx, Instant::now()));
        self.rescan_filters();
        Some(idx)
    }

    /// Parse and add a typed Template ID filter from Timeline input.
    pub fn push_template_filter_input(&mut self, input: &str, color: Color32) -> Option<usize> {
        let template_id = search::parse_template_id(input).ok()?;
        self.push_template_filter(template_id, color)
    }

    /// Add a typed field expression as a record-scoped Timeline lane.
    pub fn push_field_filter_input(
        &mut self,
        input: &str,
        color: Color32,
    ) -> Result<usize, String> {
        let query = FieldQuery::parse(input, self.filter_input_case_sensitive)?;
        self.push_field_filter(query, color)
    }

    pub fn push_field_filter(
        &mut self,
        query: FieldQuery,
        color: Color32,
    ) -> Result<usize, String> {
        if self.filters.len() >= MAX_FILTERS {
            return Err("A maximum of 20 filters can be added.".into());
        }
        let query = query.bind_to_doc(&self.doc)?;
        let text = query.expression();
        if self
            .filter_field_queries
            .iter()
            .flatten()
            .any(|existing| existing == &query)
        {
            return Err("This field filter is already active.".into());
        }
        let index = self.filters.len();
        self.filters.push(Filter { text, color });
        self.filter_case_sensitive.push(query.case_sensitive);
        self.filter_exclude.push(false);
        self.filter_regex.push(false);
        self.filter_template_ids.push(None);
        self.filter_field_queries.push(Some(query));
        self.filter_highlight = Some((index, Instant::now()));
        self.rescan_filters();
        Ok(index)
    }
}
