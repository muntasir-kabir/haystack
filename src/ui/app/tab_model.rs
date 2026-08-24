use super::*;

impl LogTab {
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

        // Set up the default dock layout. Timeline is a fixed top panel
        // (always fully visible), so the dock only contains Log + Pinned.
        let mut dock_state = DockState::new(vec![ViewTab::Log]);
        let [_main_surface, _bottom_surface] = dock_state.main_surface_mut().split_below(
            egui_dock::NodeIndex::root(),
            0.8,
            vec![ViewTab::Pinned],
        );

        LogTab {
            doc,
            filters: Vec::new(),
            matches: Arc::new(Vec::new()),
            timeline,
            context_line: None,
            pending_scroll: None,
            show_templates: false,
            timeline_zoom: None,
            selected_diamond: None,
            selected_lane: None,
            pending_toast: None,
            filter_input: String::new(),
            highlighter: None,
            search_rx: None,
            visible_rx: None,
            tail_rx: None,
            lane_active: Vec::new(),
            everything_else_active: true,
            pending_filter_removal: None,
            pending_clear_filters: false,
            timeline_detached: false,
            visible_lines: None,
            log_font_size: 12.0,
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
            selection_popup_opened_at: None,
            viewport_range: None,
            preserve_anchor: None,
            applied_filter: None,
            find_input: String::new(),
            find_query: String::new(),
            find_case_sensitive: false,
            find_matches: Vec::new(),
            find_pos: None,
            find_automaton: None,
            find_rx: None,
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
            search_focus_requested: false,
            search_focus_anim: None,
            filter_highlight: None,
            dock_state,
            detached_views: HashSet::new(),
            detached_locations: HashMap::new(),
            just_closed_viewports: Vec::new(),
            pending_detach: None,
            saved_dock_state: None,
            mcp_serving: false,
            stale: false,
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
                let begin = lines.partition_point(|&line| line < first);
                let end = lines.partition_point(|&line| line <= last);
                lines[begin..end].to_vec()
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
        std::thread::spawn(move || {
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

        self.visible_lines = build_visible_lines(
            n,
            &self.matches,
            &self.lane_active,
            self.everything_else_active,
            None,
        );

        // Verify the preserved anchor is still in the new filter. If not,
        // fall back to the nearest still-visible line (or clear it if none).
        self.resolve_preserve_anchor(n);
        self.scroll_to_preserved_anchor();

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
        let everything_else_active = self.everything_else_active;
        let (tx, rx) = crossbeam_channel::bounded(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancel);
        std::thread::spawn(move || {
            let visible_lines = build_visible_lines(
                n,
                &matches,
                &active,
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

    fn resolve_preserve_anchor(&mut self, n: usize) {
        if let Some(anchor) = self.preserve_anchor {
            let found = match &self.visible_lines {
                Some(vis) => vis.binary_search(&anchor).is_ok(),
                None => anchor < n,
            };
            if !found {
                self.preserve_anchor = self.visible_lines.as_ref().and_then(|vis| {
                    let insertion = vis.binary_search(&anchor).unwrap_or_else(|e| e);
                    match (
                        insertion.checked_sub(1).map(|i| vis[i]),
                        vis.get(insertion).copied(),
                    ) {
                        (Some(lower), Some(upper)) => Some(if anchor - lower <= upper - anchor {
                            lower
                        } else {
                            upper
                        }),
                        (Some(lower), None) => Some(lower),
                        (None, Some(upper)) => Some(upper),
                        (None, None) => None,
                    }
                });
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
            self.selected_diamond = None;
        } else if let Some(selected) = self.selected_lane {
            if selected > idx {
                self.selected_lane = Some(selected - 1);
            }
        }
        self.filters.remove(idx);
        self.rescan_filters();
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
        self.selected_diamond = None;
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
        if let Some((_, cancel)) = &self.visible_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        self.visible_rx = None;
        self.highlighter = search::build_automaton(
            &self
                .filters
                .iter()
                .map(|k| k.text.clone())
                .collect::<Vec<_>>(),
        )
        .map(Arc::new);

        while self.lane_active.len() < self.filters.len() {
            self.lane_active.push(true);
        }
        self.lane_active.truncate(self.filters.len());

        if self.filters.is_empty() {
            self.selected_lane = None;
            self.selected_diamond = None;
            self.matches = Arc::new(Vec::new());
            self.timeline = Timeline::build_u32(&self.doc, &[], DEFAULT_BUCKETS);
            self.search_rx = None;
            self.rebuild_visible_lines();
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let doc = Arc::clone(&self.doc);
        let filters: Vec<String> = self.filters.iter().map(|k| k.text.clone()).collect();
        let cancel_worker = Arc::clone(&cancel);
        std::thread::spawn(move || {
            let matches = Arc::new(search::scan_document_u32(&doc, &filters, &cancel_worker));
            if cancel_worker.load(Ordering::Relaxed) {
                return;
            }
            let timeline = Timeline::build_u32(&doc, &matches, DEFAULT_BUCKETS);
            if !cancel_worker.load(Ordering::Relaxed) {
                let _ = tx.send(FilterScanResult { matches, timeline });
            }
        });
        self.search_rx = Some((rx, cancel));
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
        std::thread::spawn(move || {
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
                self.doc = Arc::new(*result.doc);
                self.tail_rx = None;
                self.stale = false;
                self.invalidate_embedded_data();
                self.rescan_filters();
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
                (1, self.doc.total_lines() as i64)
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
            logotomy::core::timeline::TimelineDomain::Sequence => first_line as i64 + 1,
        };
        let v1 = match self.timeline.domain {
            logotomy::core::timeline::TimelineDomain::Time { .. } => {
                match self.doc.ts_at_opt(last_line) {
                    Some(t) if t >= 0 => t,
                    _ => return,
                }
            }
            logotomy::core::timeline::TimelineDomain::Sequence => last_line as i64 + 1,
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
                (1, self.doc.total_lines() as i64)
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
        if let Some((_, cancel)) = &self.visible_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        self.visible_rx = None;

        // Apply the trim to the document.
        let doc = Arc::make_mut(&mut self.doc);
        match action {
            TrimAction::TrimRight(l) => doc.trim_right(l),
            TrimAction::TrimLeft(l) => doc.trim_left(l),
        }

        // Reset state that depends on the old line count.
        self.visible_lines = None;
        self.timeline_zoom = None;
        self.context_line = None;
        self.selected_diamond = None;
        self.pending_filter_removal = None;
        self.pending_clear_filters = false;
        self.pins.clear();
        self.bottom_panel_open = false;
        self.pin_modal = None;
        self.pin_edit_index = None;
        self.pin_comment.clear();
        self.selection_range = None;
        self.pending_selection = None;
        self.drag_selecting = false;
        self.drag_start_line = None;
        self.drag_current_line = None;
        self.drag_start_pos = None;
        self.clear_find();
        self.find_input.clear();
        self.invalidate_embedded_data();

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
        if let Some((_, cancel)) = &self.visible_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        self.visible_rx = None;

        let doc = Arc::make_mut(&mut self.doc);
        doc.reset_trim();

        // Reset state.
        self.visible_lines = None;
        self.timeline_zoom = None;
        self.context_line = None;
        self.selected_diamond = None;
        self.pending_filter_removal = None;
        self.pending_clear_filters = false;
        self.pins.clear();
        self.bottom_panel_open = false;
        self.pin_modal = None;
        self.pin_edit_index = None;
        self.pin_comment.clear();
        self.selection_range = None;
        self.pending_selection = None;
        self.drag_selecting = false;
        self.drag_start_line = None;
        self.drag_current_line = None;
        self.drag_start_pos = None;
        self.clear_find();
        self.find_input.clear();
        self.invalidate_embedded_data();

        // Rebuild filters + timeline for the full document.
        self.rescan_filters();
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
                self.search_rx = None;
                self.rebuild_visible_lines();
                false
            }
            Err(crossbeam_channel::TryRecvError::Empty) => true,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.search_rx = None;
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
                self.visible_lines = result.visible_lines;
                self.preserve_anchor = result.preserve_anchor;
                self.resolve_preserve_anchor(self.doc.total_lines());
                self.scroll_to_preserved_anchor();
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

    /// Start a background find scan for `query`. Empty query clears find state.
    pub fn start_find(&mut self, query: String) {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            self.clear_find();
            self.find_input.clear();
            return;
        }
        if let Some((_, cancel)) = &self.find_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        self.find_query = trimmed.to_string();
        self.find_automaton =
            search::build_find_automaton(trimmed, !self.find_case_sensitive).map(Arc::new);
        self.find_matches.clear();
        self.find_pos = None;

        let doc = Arc::clone(&self.doc);
        // Cloning the Arc is constant-time. Cloning a large filtered Vec here
        // used to create a noticeable UI hitch just before the find worker
        // began its background scan.
        let subset = self.visible_lines.as_ref().map(Arc::clone);
        let needle = trimmed.to_string();
        let case_insensitive = !self.find_case_sensitive;
        let (tx, rx) = crossbeam_channel::bounded(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancel);
        std::thread::spawn(move || {
            let _ = tx.send(search::find_lines(
                &doc,
                subset.as_deref().map(Vec::as_slice),
                &needle,
                case_insensitive,
                &cancel_worker,
            ));
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
                        .unwrap_or(self.find_matches[0]);
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
                .and_then(|line| visible.binary_search(&line).ok())
            {
                Some(position) if next => visible[(position + 1).min(visible.len() - 1)],
                Some(position) => visible[position.saturating_sub(1)],
                None if next => visible[0],
                None => visible[visible.len() - 1],
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
        self.sync_timeline_selection_to_line(target);
        self.ensure_visible();
    }

    /// Select a filter lane for left/right occurrence navigation.
    pub fn select_lane(&mut self, lane: usize) {
        if lane >= self.filters.len() || lane >= self.matches.len() {
            return;
        }
        self.selected_lane = Some(lane);
        self.selected_diamond = None;
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

        let current = self
            .selected_diamond
            .filter(|(selected_lane, _)| *selected_lane == lane)
            .map(|(_, line)| line)
            .or(self.context_line);
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
        self.selected_diamond = Some((lane, target));
        self.sync_navigation_positions(target);
        self.ensure_visible();
    }

    /// Keep the lane and Log View search cursors aligned when both are active.
    /// Each cursor moves to the nearest occurrence in its own sorted list.
    pub(crate) fn sync_navigation_positions(&mut self, line: usize) {
        if let Some(lane) = self.selected_lane {
            if let Some(matches) = self.matches.get(lane) {
                if let Some(nearest) = nearest_occurrence(matches.iter().map(|&m| m as usize), line)
                {
                    self.selected_diamond = Some((lane, nearest.1));
                }
            }
        }
        if let Some(nearest) = nearest_occurrence(self.find_matches.iter().copied(), line) {
            self.find_pos = Some(nearest.0);
        }
    }

    /// Update the selected diamond for a newly selected log line without
    /// changing the user's selected lane. A line that does not match that
    /// lane clears the diamond and its occurrence status.
    pub(crate) fn sync_timeline_selection_to_line(&mut self, line: usize) {
        if let Some(lane) = self.selected_lane {
            let matches_line = self
                .matches
                .get(lane)
                .is_some_and(|occurrences| occurrences.binary_search(&(line as u32)).is_ok())
                && self.lane_active.get(lane).copied().unwrap_or(true);
            self.selected_diamond = matches_line.then_some((lane, line));
        } else {
            self.selected_diamond = None;
        }
    }

    /// Select a line chosen from the timeline. When a lane supplied the click,
    /// that lane remains selected even if another lane also matches the line.
    pub(crate) fn select_timeline_line(&mut self, line: usize, clicked_lane: Option<usize>) {
        if let Some(lane) = clicked_lane {
            self.selected_lane = Some(lane);
            let matches_line = self
                .matches
                .get(lane)
                .is_some_and(|occurrences| occurrences.binary_search(&(line as u32)).is_ok());
            self.selected_diamond = matches_line.then_some((lane, line));
        } else if self
            .matches
            .iter()
            .any(|occurrences| occurrences.binary_search(&(line as u32)).is_ok())
        {
            self.sync_timeline_selection_to_line(line);
        } else {
            self.selected_diamond = None;
        }
        self.context_line = Some(line);
        self.pending_scroll = Some(line);
        self.ensure_visible();
    }

    /// Jump to match index `i` (clamped to match list length).
    fn goto_find_match(&mut self, i: usize) {
        let line = self.find_matches[i.min(self.find_matches.len().saturating_sub(1))];
        self.find_pos = Some(i.min(self.find_matches.len().saturating_sub(1)));
        self.context_line = Some(line);
        self.pending_scroll = Some(line);
        self.sync_timeline_selection_to_line(line);
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
        self.find_automaton = None;
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

    /// Add a filter to the timeline (shared by the toolbar strip and the search
    /// box "Add Filter" button). Skips empty / duplicate / at-cap filters.
    /// Returns the new filter index, or `None` if nothing was added. Triggers a
    /// background rescan and stamps a short highlight on the timeline lane.
    pub fn push_filter(&mut self, text: &str, color: Color32) -> Option<usize> {
        let trimmed = text.trim();
        if trimmed.is_empty() || self.filters.len() >= MAX_FILTERS {
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
        self.filter_highlight = Some((idx, Instant::now()));
        self.rescan_filters();
        Some(idx)
    }
}
