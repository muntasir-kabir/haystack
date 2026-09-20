//! Bottom panel: pinned log lines shown as stacked cards sorted by timestamp.
//!
//! Each pin card shows:
//! - A relative-time title ("At …" for the first card, "after …" for later ones)
//! - User comment in bold (if non-empty)
//! - Log lines (slightly smaller font, line-numbered) with "…" gaps between
//!   non-consecutive lines and a "… (N lines filtered)" marker
//! - A remove button
//! - A header-bar "Copy text" button that copies the whole container (titles,
//!   comments and numbered log lines, cards separated by blank lines)

use eframe::egui;
use egui::RichText;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::ui::app::model::LogTab;
use crate::ui::icons::{self, Icon};
use crate::ui::log_view::{line_job, Highlights};
use crate::ui::theme::Theme;
use haystack::core::time::format_ms;
use haystack::core::timeline::TimelineDomain;

/// Format a duration in milliseconds as a human-readable string.
fn format_duration_ms(ms: i64) -> String {
    let abs = ms.unsigned_abs();
    if abs < 1000 {
        return format!("{ms} ms");
    }
    let secs = abs as f64 / 1000.0;
    if secs < 60.0 {
        return format!("{:.1} sec", secs);
    }
    let mins = secs / 60.0;
    if mins < 60.0 {
        return format!("{:.1} min", mins);
    }
    let hrs = mins / 60.0;
    if hrs < 24.0 {
        return format!("{:.1} hr", hrs);
    }
    let days = hrs / 24.0;
    format!("{:.1} days", days)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::model::PinEntry;
    use haystack::core::document::LogDocument;

    #[test]
    fn text_only_analysis_is_first_in_pin_tab_and_copy_output() {
        let path = std::env::temp_dir().join(format!(
            "haystack-pin-analysis-{}-{}.log",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::write(
            &path,
            "2026-07-19T10:00:00.000Z INFO one\n2026-07-19T10:00:01.000Z ERROR two\n",
        )
        .unwrap();
        let mut tab = LogTab::new(LogDocument::open(&path).unwrap());
        std::fs::remove_file(path).ok();
        tab.pins = vec![
            PinEntry {
                start_line: 1,
                line_numbers: vec![1],
                start_ts: tab.doc.ts_at(1),
                end_ts: tab.doc.ts_at(1),
                comment: "Line-specific note".into(),
                unanchored: false,
            },
            PinEntry {
                start_line: 0,
                line_numbers: vec![],
                start_ts: -1,
                end_ts: -1,
                comment: "Read this first".into(),
                unanchored: true,
            },
        ];

        let sorted = sorted_pin_indices(&tab);
        assert_eq!(sorted, vec![1, 0]);
        assert_eq!(pin_header_text(&tab, 0, &sorted), "Analysis");
        let copied = pinned_content_text(&tab);
        assert!(copied.starts_with("Analysis\nRead this first"));
        assert!(!copied.contains("line 1\nRead this first"));
    }
}

/// Timestamp of the last Copy click (module-wide), drives the toast.
static COPY_TOAST_AT: Mutex<Option<Instant>> = Mutex::new(None);
/// How long the "Copied to clipboard" toast stays visible.
const COPY_TOAST_DURATION: Duration = Duration::from_secs(2);

/// Record that the Copy button was pressed so a toast can be drawn.
fn trigger_copy_toast() {
    if let Ok(mut g) = COPY_TOAST_AT.lock() {
        *g = Some(Instant::now());
    }
}

/// Small, self-dismissing "Copied to clipboard" toast drawn after Copy is pressed.
fn toast_ui(ctx: &egui::Context, theme: &Theme) {
    let mut showing = false;
    if let Ok(mut g) = COPY_TOAST_AT.lock() {
        if let Some(at) = *g {
            showing = at.elapsed() < COPY_TOAST_DURATION;
            if !showing {
                *g = None;
            }
        }
    }
    if !showing {
        return;
    }
    egui::Area::new(egui::Id::new("pin_copy_toast"))
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(theme.surface)
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.label(RichText::new("Copied to clipboard").color(theme.text));
                });
        });
    ctx.request_repaint();
}

/// Sorted pin indices by `start_ts` (then `start_line`), matching display order.
fn sorted_pin_indices(tab: &LogTab) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..tab.pins.len()).collect();
    indices.sort_by(|&a, &b| {
        let pa = &tab.pins[a];
        let pb = &tab.pins[b];
        // Text-only analyses deliberately lead the Pin tab so the user's
        // high-level guidance is visible before line-specific evidence.
        if pa.unanchored != pb.unanchored {
            return if pa.unanchored {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        if pa.start_ts >= 0 && pb.start_ts >= 0 {
            pa.start_ts.cmp(&pb.start_ts)
        } else if pa.start_ts >= 0 {
            std::cmp::Ordering::Less
        } else if pb.start_ts >= 0 {
            std::cmp::Ordering::Greater
        } else {
            pa.start_line.cmp(&pb.start_line)
        }
    });
    indices
}

/// Header/origin shown for a pin card:
/// - first card: `At {duration from log start}`
/// - later cards: `after {duration since the previous card}`
/// Falls back to the wall-clock timestamp (or `line N`) when no duration is valid.
fn pin_header_text(tab: &LogTab, pos: usize, sorted_indices: &[usize]) -> String {
    let pi = sorted_indices[pos];
    let pin = &tab.pins[pi];
    if pin.unanchored {
        return "Analysis".to_string();
    }
    let log_start = match tab.timeline.domain {
        TimelineDomain::Time { start_ms, .. } => start_ms,
        _ => -1,
    };

    if pos == 0 {
        if pin.start_ts >= 0 && log_start >= 0 && pin.start_ts >= log_start {
            return format!("At {}", format_duration_ms(pin.start_ts - log_start));
        }
    } else {
        let prev_pin = &tab.pins[sorted_indices[pos - 1]];
        if prev_pin.end_ts >= 0 && pin.start_ts >= 0 {
            let delta = pin.start_ts - prev_pin.end_ts;
            if delta >= 0 {
                return format!("after {}", format_duration_ms(delta));
            }
        }
    }

    if pin.start_ts >= 0 {
        format_ms(pin.start_ts)
    } else {
        format!("line {}", pin.start_line + 1)
    }
}

/// Render one pinned log line: a non-selectable line-number gutter followed by
/// the (filter-highlighted) log content.
fn pin_line(ui: &mut egui::Ui, tab: &LogTab, theme: &Theme, small_font: &egui::FontId, ln: usize) {
    ui.horizontal(|ui| {
        ui.add(
            egui::Label::new(
                RichText::new(format!("{:>7}: ", ln + 1))
                    .family(crate::ui::fonts::log_font_family())
                    .size(small_font.size)
                    .color(theme.gutter),
            )
            .selectable(false),
        );
        let job = line_job(
            &tab.doc,
            &Highlights::filters_only(&tab.filters, tab.highlighter.as_deref()),
            ln,
            false,
            small_font.clone(),
            theme,
        );
        ui.add(egui::Label::new(job));
    });
}

/// Render a "…" gap/filter marker aligned with the start of the log content.
/// Indented by the same (empty) line-number gutter width as the log lines so
/// the marker lines up with the log text start, not the gutter.
fn pin_gap(ui: &mut egui::Ui, theme: &Theme, small_font: &egui::FontId, text: impl Into<String>) {
    ui.horizontal(|ui| {
        ui.add(
            egui::Label::new(
                RichText::new(format!("{:>7}  ", ""))
                    .family(crate::ui::fonts::log_font_family())
                    .size(small_font.size)
                    .color(theme.gutter),
            )
            .selectable(false),
        );
        ui.label(
            RichText::new(text)
                .size((small_font.size * 0.8).max(6.0))
                .color(theme.text_muted)
                .italics(),
        );
    });
}

/// Plain-text rendering of the whole pinned view: each card's header, its user
/// comment (if any), and its numbered log lines. Cards are separated by a blank
/// line so the visual gap between cards is reproduced as a newline.
fn pinned_content_text(tab: &LogTab) -> String {
    let sorted = sorted_pin_indices(tab);
    let mut blocks: Vec<String> = Vec::with_capacity(sorted.len());
    for pos in 0..sorted.len() {
        let pin = &tab.pins[sorted[pos]];
        let mut block = pin_header_text(tab, pos, &sorted);
        if !pin.comment.is_empty() {
            block.push('\n');
            block.push_str(&pin.comment);
        }
        let lines: Vec<String> = pin
            .line_numbers
            .iter()
            .filter(|&&ln| ln < tab.doc.total_lines())
            .map(|&ln| format!("{:>7}: {}", ln + 1, tab.doc.line(ln)))
            .collect();
        if !lines.is_empty() {
            block.push('\n');
            block.push_str(&lines.join("\n"));
        }
        blocks.push(block);
    }
    blocks.join("\n\n")
}

pub fn show(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    let is_open = tab.bottom_panel_open;

    // ---- header bar ----
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = icons::ACTION_HEIGHT;
        let ctx = ui.ctx().clone();
        let header_icon = if is_open {
            Icon::Collapse
        } else {
            Icon::Expand
        };
        if icons::icon_action_button(
            ui,
            header_icon,
            theme.text,
            if is_open {
                "Collapse pinned evidence"
            } else {
                "Expand pinned evidence"
            },
        )
        .clicked()
        {
            tab.bottom_panel_open = !is_open;
        }
        ui.add(icons::icon_image(&ctx, Icon::Pin, 12.0, theme.text));
        let header_resp = ui.add(
            egui::Label::new(
                RichText::new(format!("{} pinned", tab.pins.len())).color(theme.text_muted),
            )
            .sense(egui::Sense::click()),
        );
        if header_resp.clicked() {
            tab.bottom_panel_open = !is_open;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if is_open
                && !tab.pins.is_empty()
                && icons::action_button(
                    ui,
                    Icon::Remove,
                    "Clear",
                    theme.text,
                    "Remove all pinned evidence and analyses",
                )
                .clicked()
            {
                tab.clear_pins_with_undo();
            }
            if is_open
                && !tab.pins.is_empty()
                && icons::action_button(
                    ui,
                    Icon::Save,
                    "Save Markdown",
                    theme.text,
                    "Save pinned evidence and analyses as Markdown",
                )
                .clicked()
            {
                save_markdown(tab);
            }
            // Copy button — to the left of "Clear all". Copies the full pinned
            // view content (headers, user comments, numbered log lines).
            if is_open && !tab.pins.is_empty() {
                if icons::action_button(
                    ui,
                    Icon::Copy,
                    "Copy text",
                    theme.text,
                    "Copy pinned evidence and analyses to the clipboard",
                )
                .clicked()
                {
                    ui.ctx().copy_text(pinned_content_text(tab));
                    trigger_copy_toast();
                }
            }
        });
    });

    if is_open {
        ui.separator();
        render_content(ui, tab, theme);
    }

    // Small, self-dismissing "Copied to clipboard" toast after Copy is clicked.
    toast_ui(ui.ctx(), theme);
}

fn save_markdown(tab: &mut LogTab) {
    let Some(path) = rfd::FileDialog::new()
        .set_file_name("investigation.md")
        .save_file()
    else {
        return;
    };
    let mut markdown = format!("# Investigation: {}\n\n", tab.doc.file_name);
    for (pos, &index) in sorted_pin_indices(tab).iter().enumerate() {
        let pin = &tab.pins[index];
        markdown.push_str(&format!(
            "## {}\n\n",
            pin_header_text(tab, pos, &sorted_pin_indices(tab))
        ));
        if !pin.comment.is_empty() {
            markdown.push_str(&pin.comment);
            markdown.push_str("\n\n");
        }
        let visible: Vec<_> = pin
            .line_numbers
            .iter()
            .copied()
            .filter(|&line| line < tab.doc.total_lines())
            .collect();
        if !visible.is_empty() {
            markdown.push_str("```text\n");
            for line in visible {
                markdown.push_str(&format!(
                    "{}: {}\n",
                    tab.doc.trim_start + line + 1,
                    tab.doc.line(line)
                ));
            }
            markdown.push_str("```\n\n");
        }
    }
    match std::fs::write(path, markdown) {
        Ok(()) => tab.pending_toast = Some("Pins and analysis saved as Markdown".into()),
        Err(error) => tab.pending_toast = Some(format!("Export failed: {error}")),
    }
}

fn render_content(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    if tab.pins.is_empty() {
        ui.label(
            RichText::new("No pinned evidence yet. Right-click a log line, or drag across rows, to pin useful context.")
                .small()
                .color(theme.text_muted),
        );
        return;
    }

    // Sort pins by start_ts, then by start_line for stability.
    let sorted_indices = sorted_pin_indices(tab);

    // Constant spacing between pin cards (10px).
    const PIN_SPACING: f32 = 10.0;

    let mut remove_pin: Option<usize> = None;
    let mut edit_pin: Option<usize> = None;
    let requested_pin = tab.pending_pin_scroll;
    let mut revealed_pin = None;
    let scroll_height = ui.available_height().max(60.0);

    egui::ScrollArea::vertical()
        .id_salt("pinned_cards")
        .auto_shrink([false, false])
        .max_height(scroll_height)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = PIN_SPACING;

            for (pos, &pi) in sorted_indices.iter().enumerate() {
                let pin = tab.pins[pi].clone();

                // ---- pin card frame ----
                let card = egui::Frame::default()
                    .fill(theme.surface)
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .show(ui, |ui| {
                        // Header: relative time as the message title + remove button
                        ui.horizontal(|ui| {
                            let title = pin_header_text(tab, pos, &sorted_indices);
                            let ctx = ui.ctx().clone();
                            ui.add(icons::icon_image(&ctx, Icon::Date, 12.0, theme.text));
                            if ui
                                .selectable_label(
                                    tab.selected_pin == Some(pi),
                                    RichText::new(title).strong().size(12.0),
                                )
                                .on_hover_text("Select pin (Delete removes it)")
                                .clicked()
                            {
                                tab.selected_pin = Some(pi);
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if icons::icon_action_button(
                                        ui,
                                        Icon::Remove,
                                        theme.text,
                                        "Remove this pinned evidence",
                                    )
                                    .clicked()
                                    {
                                        remove_pin = Some(pi);
                                    }
                                    if !pin.unanchored {
                                        if icons::icon_action_button(
                                            ui,
                                            Icon::Edit,
                                            theme.text,
                                            "Edit this pin and its analysis",
                                        )
                                        .clicked()
                                        {
                                            edit_pin = Some(pi);
                                        }
                                    }
                                },
                            );
                        });

                        // Analysis text / optional pin comment.
                        if !pin.comment.is_empty() {
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new(&pin.comment)
                                    .strong()
                                    .color(theme.analysis_text)
                                    .size(12.0),
                            );
                        }

                        // Log lines in slightly smaller font (embedded Space Mono)
                        let small_font =
                            crate::ui::fonts::log_font((tab.log_font_size * 0.85).max(8.0));
                        let visible_lines: Vec<usize> = pin
                            .line_numbers
                            .iter()
                            .copied()
                            .filter(|&line| line < tab.doc.total_lines())
                            .collect();
                        let total = visible_lines.len();
                        const SHOW_FIRST: usize = 10;
                        const SHOW_LAST: usize = 2;

                        // Render lines directly (no inner scroll) with 1px spacing
                        ui.spacing_mut().item_spacing.y = 0.0; // no gap between log lines

                        if total <= SHOW_FIRST + SHOW_LAST {
                            // Show all lines with gap markers
                            for (i, &ln) in visible_lines.iter().enumerate() {
                                if i > 0 && visible_lines[i - 1] + 1 != ln {
                                    pin_gap(ui, theme, &small_font, "…");
                                }
                                pin_line(ui, tab, theme, &small_font, ln);
                            }
                        } else {
                            // Show first SHOW_FIRST lines
                            for (i, &ln) in visible_lines.iter().enumerate().take(SHOW_FIRST) {
                                if i > 0 && visible_lines[i - 1] + 1 != ln {
                                    pin_gap(ui, theme, &small_font, "…");
                                }
                                pin_line(ui, tab, theme, &small_font, ln);
                            }
                            // "…" with filtered count
                            let filtered = total - SHOW_FIRST - SHOW_LAST;
                            pin_gap(
                                ui,
                                theme,
                                &small_font,
                                format!("… ({} lines filtered)", filtered),
                            );
                            // Show last SHOW_LAST lines
                            let start_last = total - SHOW_LAST;
                            for (i, &ln) in visible_lines.iter().enumerate().skip(start_last) {
                                if i > start_last && visible_lines[i - 1] + 1 != ln {
                                    pin_gap(ui, theme, &small_font, "…");
                                }
                                pin_line(ui, tab, theme, &small_font, ln);
                            }
                        }
                    });
                if requested_pin == Some(pi) {
                    ui.scroll_to_rect(card.response.rect, Some(egui::Align::Center));
                    revealed_pin = Some(pi);
                }
            }
        });

    if revealed_pin == requested_pin {
        tab.pending_pin_scroll = None;
    }

    if let Some(pi) = remove_pin {
        tab.selected_pin = Some(pi);
        tab.remove_selected_pin_with_undo();
    }

    // Edit: reopen the pin creation window pre-filled for this pin. save_pin
    // (via the app-level pin modal) updates the entry in place.
    if let Some(pi) = edit_pin {
        if let Some(pin) = tab.pins.get(pi) {
            let last_line = pin.line_numbers.last().copied().unwrap_or(pin.start_line);
            tab.pin_comment = pin.comment.clone();
            tab.pin_modal = Some((pin.start_line, last_line));
            tab.pin_edit_index = Some(pi);
        }
    }
}
