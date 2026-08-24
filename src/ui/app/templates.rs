use eframe::egui;
use egui::RichText;

use crate::ui::app::model::LogTab;

pub(super) fn template_browser(ui: &mut egui::Ui, tab: &mut LogTab) {
    let mut order: Vec<usize> = (0..tab.doc.templates.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(tab.doc.templates[i].count));

    let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, row_height, order.len(), |ui, range| {
            for &ti in &order[range] {
                let (count, id, example_line) = {
                    let t = &tab.doc.templates[ti];
                    (t.count, t.id, t.example_line)
                };
                let pattern: String = tab.doc.templates[ti].pattern.chars().take(70).collect();
                let label = format!("×{:<7} T{:<4} {}", count, id, pattern);
                let resp = ui.add(
                    egui::Label::new(RichText::new(label).monospace().size(11.0))
                        .sense(egui::Sense::click()),
                );
                if resp.clicked() {
                    tab.context_line = Some(example_line);
                    tab.sync_timeline_selection_to_line(example_line);
                    tab.pending_scroll = Some(example_line);
                }
                if resp.hovered() {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                }
            }
        });
}
