//! Closeable validation bubble anchored to a control.

use eframe::egui::{self, Color32, Id, Order, Pos2, Rect, Vec2};

use crate::ui::icons::{self, Icon};

/// Side of the anchor on which an [`error_bubble`] is placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Reusable API: current caller uses Above; others are intentional.
pub enum BubbleAlign {
    Above,
    Below,
    Left,
    Right,
}

/// Show a closeable error bubble beside `anchor` while `open` is true.
///
/// Closing this only dismisses the presentation; callers should retain their
/// validation error separately so a dismissed error cannot authorize an
/// invalid action.
pub fn error_bubble(
    ctx: &egui::Context,
    id: impl std::hash::Hash + std::fmt::Debug,
    anchor: Rect,
    align: BubbleAlign,
    message: &str,
    open: &mut bool,
) {
    if !*open {
        return;
    }
    let estimated = Vec2::new(300.0, 38.0);
    let desired = match align {
        BubbleAlign::Above => Pos2::new(
            anchor.center().x - estimated.x / 2.0,
            anchor.top() - estimated.y - 6.0,
        ),
        BubbleAlign::Below => {
            Pos2::new(anchor.center().x - estimated.x / 2.0, anchor.bottom() + 6.0)
        }
        BubbleAlign::Left => Pos2::new(
            anchor.left() - estimated.x - 6.0,
            anchor.center().y - estimated.y / 2.0,
        ),
        BubbleAlign::Right => {
            Pos2::new(anchor.right() + 6.0, anchor.center().y - estimated.y / 2.0)
        }
    };
    let screen = ctx.content_rect();
    let pos = Pos2::new(
        desired.x.clamp(
            screen.left() + 4.0,
            (screen.right() - estimated.x - 4.0).max(screen.left() + 4.0),
        ),
        desired.y.clamp(
            screen.top() + 4.0,
            (screen.bottom() - estimated.y - 4.0).max(screen.top() + 4.0),
        ),
    );
    let area = egui::Area::new(Id::new(("error_bubble", id)))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(300.0);
                ui.horizontal_top(|ui| {
                    ui.colored_label(Color32::LIGHT_RED, message);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        if icons::icon_action_button(
                            ui,
                            Icon::Close,
                            ui.visuals().text_color(),
                            "Dismiss this message",
                        )
                        .clicked()
                        {
                            *open = false;
                        }
                    });
                });
            });
        });
    let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
    let click = ctx.input(|input| {
        input
            .pointer
            .any_click()
            .then(|| input.pointer.interact_pos())
            .flatten()
    });
    if escape
        || click.is_some_and(|position| {
            !anchor.contains(position) && !area.response.rect.contains(position)
        })
    {
        *open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bubble_alignment_variants_are_distinct() {
        assert_ne!(BubbleAlign::Above, BubbleAlign::Below);
        assert_ne!(BubbleAlign::Left, BubbleAlign::Right);
    }
}
