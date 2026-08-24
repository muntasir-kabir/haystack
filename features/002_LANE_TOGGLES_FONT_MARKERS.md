# Timeline lane toggles, log font size, color markers, smart axis labels

**What was built**

Five UI/UX improvements to the log analyzer:

1. **Timeline lane checkboxes + "Everything Else" lane + log filtering**
   - Each keyword lane now has a checkbox in the left legend column. Clicking toggles the lane on/off.
   - An "Everything Else" lane at the top shows lines matching no keyword, also with a checkbox.
   - When any lane is unchecked, the middle log view filters to show only lines from active lanes.
   - `LogTab` gains `lane_active`, `everything_else_active`, `visible_lines` fields.
   - `rebuild_visible_lines()` builds a filtered index (or `None` fast path when all active).
   - Filter rebuilds on keyword scan completion, keyword add/remove, and lane toggle.

2. **Bigger timeline legend text + more space**
   - Left column width: 40px → 120px.
   - Label font: 7.5 → 10.5 monospace.
   - Truncation limit: 5 → 14 chars.

3. **Smart axis labels + duration-between-labels**
   - Axis labels now shorten based on the visible time span:
     - Same hour → `MM:SS.ms` (drops date and hour)
     - Same date → `HH:MM:SS.ms` (drops date)
     - Multi-day → full `YYYY-MM-DD HH:MM:SS.ms`
   - A duration label (e.g. `Δ 14s`, `Δ 5m 2s`, `Δ 1h 5m`) appears centered below the axis row.
   - Works for both Time and Sequence domains.

4. **Text size A+/A− buttons**
   - Both the middle log view and the bottom context panel have `A−` / `A+` buttons.
   - Shared `log_font_size` state on `LogTab`, clamped to [8, 24]px.
   - Current size displayed as `{n}px`.

5. **Color marker at start of each log line**
   - Each line in the log view and context panel now starts with a `▌` character.
   - Color = first matching keyword's color, or `theme.text_muted` (grey) if unmatched.
   - Uses the same background as the rest of the line (selection highlighting preserved).

**Files changed:** `src/ui/app.rs`, `src/ui/timeline_view.rs`, `src/ui/log_view.rs`, `src/ui/context_view.rs`, `changes.md`, `.clinerules/Project.md`, `UserGuide.md`, `feature.md`

**Tests:** All 23 existing unit tests pass. Build is clean (no warnings).

## Retrospection

### What went well
- Deferred toggle pattern avoided mutable borrow conflicts during timeline iteration.
- `visible_lines: Option<Vec<usize>>` with `None` fast path keeps filtering zero-cost when all lanes active.
- Smart axis labels are O(n_ticks) — negligible cost, big UX improvement.
- Color marker reuses the existing Aho-Corasick first-match — no extra scan pass.

### What could be improved
- `rebuild_visible_lines()` allocates a `Vec<bool>` of size `total_lines` — fine for 5M lines (~5ms), but could be optimized with a bitset or sparse approach.
- The "Everything Else" lane currently has no density line or diamonds — it's just a label + checkbox. Could add a grey density line showing unmatched line distribution.
- Font size buttons are simple `A−`/`A+` — could be a slider or dropdown for finer control.
- The checkbox drawing is manual (painted rect + ✓ text) rather than using egui's native `Checkbox` widget, because the timeline uses a custom `Painter` region. This works but doesn't get native keyboard focus.