//! Theme system: dark and light mode colour palettes.
//! Every UI component should source its colours from the active `Theme`
//! rather than hardcoding `Color32` literals.

use eframe::egui::{Color32, Context, CornerRadius, Margin, Stroke, Vec2, Visuals};
use haystack::core::settings::ThemeMode;

/// Filter lane palette for light mode: 20 vivid, medium-depth hues chosen to pop against
/// light surfaces. Visually similar hues are spaced far apart so neighbouring lanes stay
/// distinguishable. Cycled in order by filter index (see `MAX_FILTERS`).
pub const FILTER_COLORS_LIGHT: [Color32; 20] = [
    Color32::from_rgb(210, 35, 45),  // Red
    Color32::from_rgb(35, 105, 210), // Blue
    Color32::from_rgb(25, 155, 70),  // Green
    Color32::from_rgb(145, 45, 185), // Purple
    Color32::from_rgb(235, 110, 15), // Orange
    Color32::from_rgb(15, 155, 175), // Cyan
    Color32::from_rgb(185, 146, 15), // Yellow
    Color32::from_rgb(125, 70, 30),  // Brown
    Color32::from_rgb(105, 155, 25), // Lime
    Color32::from_rgb(190, 35, 125), // Magenta
    Color32::from_rgb(90, 50, 150),  // Indigo
    Color32::from_rgb(200, 80, 25),  // Dark Orange
    Color32::from_rgb(25, 125, 105), // Teal
    Color32::from_rgb(155, 115, 25), // Gold
    Color32::from_rgb(155, 45, 95),  // Raspberry
    Color32::from_rgb(60, 75, 150),  // Blue-gray
    Color32::from_rgb(45, 125, 55),  // Forest Green
    Color32::from_rgb(125, 55, 145), // Plum
    Color32::from_rgb(170, 95, 45),  // Tan
    Color32::from_rgb(80, 80, 80),   // Gray
];

/// Filter lane palette for dark mode: 20 bright, saturated hues that read clearly against
/// deep surfaces. Visually similar hues are spaced far apart so neighbouring lanes stay
/// distinguishable. Cycled in order by filter index (see `MAX_FILTERS`).
pub const FILTER_COLORS_DARK: [Color32; 20] = [
    Color32::from_rgb(255, 55, 65),   // Red
    Color32::from_rgb(55, 145, 255),  // Blue
    Color32::from_rgb(45, 220, 105),  // Green
    Color32::from_rgb(205, 85, 255),  // Purple
    Color32::from_rgb(255, 145, 35),  // Orange
    Color32::from_rgb(35, 210, 220),  // Cyan
    Color32::from_rgb(225, 195, 40),  // Yellow
    Color32::from_rgb(210, 125, 65),  // Brown
    Color32::from_rgb(150, 220, 40),  // Lime
    Color32::from_rgb(255, 70, 175),  // Magenta
    Color32::from_rgb(150, 75, 225),  // Indigo
    Color32::from_rgb(255, 100, 40),  // Dark Orange
    Color32::from_rgb(40, 190, 145),  // Teal
    Color32::from_rgb(195, 170, 45),  // Gold
    Color32::from_rgb(230, 70, 120),  // Raspberry
    Color32::from_rgb(100, 115, 220), // Blue-gray
    Color32::from_rgb(70, 200, 80),   // Forest Green
    Color32::from_rgb(185, 90, 205),  // Plum
    Color32::from_rgb(235, 135, 75),  // Tan
    Color32::from_rgb(185, 185, 185), // Gray
];

pub struct Theme {
    /// The application canvas behind panels and workspace content.
    pub canvas: Color32,
    /// The quiet reading surface used by log and data-heavy views.
    pub log_surface: Color32,
    /// The elevated surface reserved for controls, menus, and overlays.
    pub raised_surface: Color32,
    /// Decorative separators and resting control boundaries.
    pub border: Color32,
    /// The primary interface and log text colour.
    pub primary_text: Color32,
    /// Secondary labels, hints, and inactive content.
    pub secondary_text: Color32,
    /// Hover fill for compact controls and dock tabs.
    pub hover: Color32,
    /// Selection fill while the window is active.
    pub selection_focused: Color32,
    /// Selection fill when the window is inactive.
    pub selection_unfocused: Color32,
    /// Keyboard focus outline and active control indicator.
    pub focus_ring: Color32,
    /// Fill used by disabled controls.
    pub disabled_fill: Color32,
    /// Text and icon colour used by disabled controls.
    #[allow(dead_code)] // Used when a custom control needs an explicit disabled label.
    pub disabled_text: Color32,
    /// Semantic severity colours. Filter identity remains separate from these.
    pub severity_error: Color32,
    pub severity_warning: Color32,
    #[allow(dead_code)] // Reserved for parsed severity rendering.
    pub severity_info: Color32,
    #[allow(dead_code)] // Reserved for parsed severity rendering.
    pub severity_fault: Color32,
    /// Dedicated search-match cue, kept separate from row selection.
    #[allow(dead_code)] // The current highlighter consumes its translucent derivative below.
    pub search_match: Color32,
    /// Cue for categorical filter identity (the complete palette is below).
    #[allow(dead_code)] // `filter_colors` supplies the categorical palette today.
    pub filter_identity: Color32,
    /// Cue for embedded structured data.
    #[allow(dead_code)] // `embedded_data` remains the compatibility-facing cue.
    pub embedded_data_cue: Color32,
    /// Background of the main application window / panels.
    pub bg: Color32,
    /// Slightly lighter surface for cards, groups, etc.
    pub surface: Color32,
    /// Primary text colour.
    pub text: Color32,
    /// Muted/secondary text (labels, hints, less important info).
    pub text_muted: Color32,
    /// Background of the selected / active line.
    pub selection_bg: Color32,
    /// Accent colour for links, active indicators, etc.
    pub accent: Color32,
    /// Histogram bar colour (density plot).
    pub histogram: Color32,
    /// Minimap background colour.
    pub minimap_bg: Color32,
    /// Line number gutter colour.
    pub gutter: Color32,
    /// Translucent fill for the line-number gutter.
    pub gutter_bg: Color32,
    /// Log line base text colour.
    pub log_text: Color32,
    /// Tick / axis label colour.
    pub axis: Color32,
    /// Hint text colour (e.g. "scroll to zoom").
    pub hint: Color32,
    /// Brush selection fill colour.
    pub brush_fill: Color32,
    /// Brush selection stroke colour.
    pub brush_stroke: Color32,
    /// Zoom window highlight on minimap.
    pub minimap_zoom: Color32,
    /// Selection marker vertical line.
    pub selection_line: Color32,
    /// Timeline occurrence hover border.
    pub occurrence_hover: Color32,
    /// MCP URL text colour.
    pub url_text: Color32,
    /// Empty state / placeholder text.
    pub placeholder: Color32,
    /// Viewport shadow overlay on timeline (shows current scroll range).
    pub viewport_shadow: Color32,
    /// Viewport shadow stroke on timeline.
    pub viewport_shadow_stroke: Color32,
    /// Warning / alert colour (e.g. trim indicator).
    pub warning: Color32,
    /// Analysis text colour (red in the bottom panel).
    pub analysis_text: Color32,
    /// Background for multi-line drag selection in the log view.
    pub selection_range_bg: Color32,
    /// Filter lane palette for the active UI mode (light/dark), cycled by index.
    pub filter_colors: [Color32; 20],
    /// Background wash for search matches in the log view.
    pub search_highlight_bg: Color32,
    /// Background wash for keyword (double-click) matches in the log view.
    pub keyword_highlight_bg: Color32,
    /// Underline / rail color for embedded structured data.
    pub embedded_data: Color32,
    /// Quiet underline used for explicit source timestamps.
    pub timestamp: Color32,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            canvas: Color32::from_rgb(0x15, 0x19, 0x1f),
            log_surface: Color32::from_rgb(0x19, 0x1e, 0x26),
            raised_surface: Color32::from_rgb(0x22, 0x29, 0x34),
            border: Color32::from_rgb(0x39, 0x43, 0x51),
            primary_text: Color32::from_rgb(0xd6, 0xdd, 0xe7),
            secondary_text: Color32::from_rgb(0x9b, 0xa8, 0xba),
            hover: Color32::from_rgb(0x2c, 0x36, 0x44),
            selection_focused: Color32::from_rgb(0x31, 0x38, 0x43),
            selection_unfocused: Color32::from_rgb(0x28, 0x2e, 0x37),
            focus_ring: Color32::from_rgb(0x8a, 0xb4, 0xf8),
            disabled_fill: Color32::from_rgb(0x1c, 0x22, 0x2b),
            disabled_text: Color32::from_rgb(0x6e, 0x7a, 0x8b),
            severity_error: Color32::from_rgb(0xf0, 0x80, 0x88),
            severity_warning: Color32::from_rgb(0xe2, 0xb3, 0x5b),
            severity_info: Color32::from_rgb(0x7a, 0xb8, 0xdc),
            severity_fault: Color32::from_rgb(0xb5, 0xa0, 0xe8),
            search_match: Color32::from_rgb(0xe2, 0xb3, 0x5b),
            filter_identity: Color32::from_rgb(0x55, 0x91, 0xff),
            embedded_data_cue: Color32::from_rgb(0x9d, 0xd8, 0xb5),
            bg: Color32::from_rgb(0x15, 0x19, 0x1f),
            surface: Color32::from_rgb(0x22, 0x29, 0x34),
            text: Color32::from_rgb(0xd6, 0xdd, 0xe7),
            text_muted: Color32::from_rgb(0x9b, 0xa8, 0xba),
            selection_bg: Color32::from_rgb(0x29, 0x3d, 0x58),
            accent: Color32::from_rgb(0x8a, 0xb4, 0xf8),
            histogram: Color32::from_rgba_unmultiplied(78, 78, 78, 82),
            minimap_bg: Color32::from_gray(38),
            gutter: Color32::from_rgba_unmultiplied(150, 150, 160, 180),
            gutter_bg: Color32::from_rgba_unmultiplied(255, 255, 255, 12),
            log_text: Color32::from_gray(215),
            axis: Color32::from_gray(130),
            hint: Color32::from_gray(135),
            brush_fill: Color32::from_rgba_unmultiplied(100, 160, 240, 40),
            brush_stroke: Color32::from_rgb(120, 180, 255),
            minimap_zoom: Color32::from_rgb(100, 160, 240),
            selection_line: Color32::WHITE,
            occurrence_hover: Color32::WHITE,
            url_text: Color32::LIGHT_BLUE,
            placeholder: Color32::GRAY,
            viewport_shadow: Color32::from_rgba_unmultiplied(130, 145, 165, 18),
            viewport_shadow_stroke: Color32::from_rgba_unmultiplied(130, 145, 165, 80),
            warning: Color32::from_rgb(0xe2, 0xb3, 0x5b),
            analysis_text: Color32::from_rgb(0xd6, 0xdd, 0xe7),
            selection_range_bg: Color32::from_rgba_unmultiplied(137, 180, 250, 40),
            filter_colors: FILTER_COLORS_DARK,
            search_highlight_bg: Color32::from_rgba_unmultiplied(226, 179, 91, 90),
            keyword_highlight_bg: Color32::from_rgba_unmultiplied(35, 210, 220, 70),
            embedded_data: Color32::from_rgb(157, 216, 181),
            timestamp: Color32::from_rgb(128, 145, 166),
        }
    }

    pub fn light() -> Self {
        Self {
            canvas: Color32::from_rgb(0xf3, 0xf5, 0xf7),
            log_surface: Color32::from_rgb(0xfb, 0xfb, 0xfc),
            raised_surface: Color32::from_rgb(0xff, 0xff, 0xff),
            border: Color32::from_rgb(0xd5, 0xdb, 0xe3),
            primary_text: Color32::from_rgb(0x24, 0x30, 0x41),
            secondary_text: Color32::from_rgb(0x59, 0x65, 0x79),
            hover: Color32::from_rgb(0xea, 0xef, 0xf5),
            selection_focused: Color32::from_rgb(0xe4, 0xe7, 0xeb),
            selection_unfocused: Color32::from_rgb(0xee, 0xf0, 0xf2),
            focus_ring: Color32::from_rgb(0x24, 0x5f, 0xbb),
            disabled_fill: Color32::from_rgb(0xea, 0xee, 0xf2),
            disabled_text: Color32::from_rgb(0x8a, 0x95, 0xa5),
            severity_error: Color32::from_rgb(0xb4, 0x23, 0x32),
            severity_warning: Color32::from_rgb(0x94, 0x60, 0x00),
            severity_info: Color32::from_rgb(0x17, 0x6b, 0x91),
            severity_fault: Color32::from_rgb(0x65, 0x50, 0xa1),
            search_match: Color32::from_rgb(0x94, 0x60, 0x00),
            filter_identity: Color32::from_rgb(0x23, 0x69, 0xd2),
            embedded_data_cue: Color32::from_rgb(0x19, 0x7d, 0x37),
            bg: Color32::from_rgb(0xf3, 0xf5, 0xf7),
            surface: Color32::from_rgb(0xff, 0xff, 0xff),
            text: Color32::from_rgb(0x24, 0x30, 0x41),
            text_muted: Color32::from_rgb(0x59, 0x65, 0x79),
            selection_bg: Color32::from_rgb(0xe2, 0xec, 0xfa),
            accent: Color32::from_rgb(0x24, 0x5f, 0xbb),
            histogram: Color32::from_rgba_unmultiplied(120, 125, 135, 72),
            minimap_bg: Color32::from_gray(220),
            gutter: Color32::from_rgba_unmultiplied(90, 90, 90, 175),
            gutter_bg: Color32::from_rgba_unmultiplied(0, 0, 0, 10),
            log_text: Color32::from_gray(30),
            axis: Color32::from_gray(100),
            hint: Color32::from_gray(105),
            brush_fill: Color32::from_rgba_unmultiplied(100, 160, 240, 40),
            brush_stroke: Color32::from_rgb(60, 120, 200),
            minimap_zoom: Color32::from_rgb(60, 120, 200),
            selection_line: Color32::from_rgb(0, 0, 0),
            occurrence_hover: Color32::from_rgb(0, 0, 0),
            url_text: Color32::from_rgb(0, 80, 180),
            placeholder: Color32::GRAY,
            viewport_shadow: Color32::from_rgba_unmultiplied(100, 110, 125, 20),
            viewport_shadow_stroke: Color32::from_rgba_unmultiplied(80, 95, 115, 90),
            warning: Color32::from_rgb(0x94, 0x60, 0x00),
            analysis_text: Color32::from_rgb(0x24, 0x30, 0x41),
            selection_range_bg: Color32::from_rgba_unmultiplied(30, 102, 245, 30),
            filter_colors: FILTER_COLORS_LIGHT,
            search_highlight_bg: Color32::from_rgba_unmultiplied(226, 179, 91, 90),
            keyword_highlight_bg: Color32::from_rgba_unmultiplied(15, 155, 175, 60),
            embedded_data: Color32::from_rgb(25, 125, 55),
            timestamp: Color32::from_rgb(105, 115, 130),
        }
    }
}

/// Build egui visuals from the same semantic tokens used by custom painters.
pub fn egui_visuals(dark_mode: bool) -> Visuals {
    let theme = if dark_mode {
        Theme::dark()
    } else {
        Theme::light()
    };
    let mut visuals = if dark_mode {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    visuals.override_text_color = Some(theme.primary_text);
    visuals.weak_text_color = Some(theme.secondary_text);
    visuals.panel_fill = theme.canvas;
    visuals.window_fill = theme.raised_surface;
    visuals.window_stroke = Stroke::new(1.0, theme.border);
    visuals.faint_bg_color = theme.log_surface;
    visuals.extreme_bg_color = theme.log_surface;
    visuals.text_edit_bg_color = Some(theme.log_surface);
    visuals.code_bg_color = theme.log_surface;
    visuals.hyperlink_color = theme.accent;
    visuals.warn_fg_color = theme.severity_warning;
    visuals.error_fg_color = theme.severity_error;
    visuals.selection.bg_fill = theme.selection_focused;
    visuals.selection.stroke = Stroke::new(1.0, theme.focus_ring);
    visuals.window_corner_radius = CornerRadius::same(4);
    visuals.menu_corner_radius = CornerRadius::same(4);
    visuals.window_shadow = eframe::epaint::Shadow::NONE;
    visuals.popup_shadow = eframe::epaint::Shadow::NONE;
    visuals.disabled_alpha = 1.0;

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = theme.disabled_fill;
    widgets.noninteractive.weak_bg_fill = theme.disabled_fill;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, theme.border);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, theme.primary_text);
    widgets.inactive.bg_fill = theme.raised_surface;
    widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    widgets.inactive.bg_stroke = Stroke::new(1.0, theme.border);
    widgets.inactive.fg_stroke = Stroke::new(1.0, theme.primary_text);
    widgets.hovered.bg_fill = theme.hover;
    widgets.hovered.weak_bg_fill = theme.hover;
    widgets.hovered.bg_stroke = Stroke::new(1.0, theme.focus_ring);
    widgets.hovered.fg_stroke = Stroke::new(1.0, theme.primary_text);
    widgets.active.bg_fill = theme.selection_focused;
    widgets.active.weak_bg_fill = theme.selection_focused;
    widgets.active.bg_stroke = Stroke::new(1.5, theme.focus_ring);
    widgets.active.fg_stroke = Stroke::new(1.0, theme.primary_text);
    widgets.open = widgets.active;
    for widget in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        widget.corner_radius = CornerRadius::same(4);
    }
    visuals
}

/// Apply the token-driven visuals and compact desktop spacing to a context.
/// Resolve a persisted theme choice against the OS theme reported by egui.
pub fn resolve_dark_mode(ctx: &Context, mode: ThemeMode) -> bool {
    match mode {
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
        ThemeMode::System => ctx
            .system_theme()
            .map(|theme| theme == eframe::egui::Theme::Dark)
            .unwrap_or(true),
    }
}

pub fn apply_egui_theme(ctx: &Context, mode: ThemeMode) -> bool {
    let dark_mode = resolve_dark_mode(ctx, mode);
    let egui_theme = eframe::egui::Theme::from_dark_mode(dark_mode);
    ctx.set_theme(egui_theme);
    ctx.style_mut_of(egui_theme, |style| {
        style.visuals = egui_visuals(dark_mode);
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.window_margin = Margin::same(12);
        style.spacing.menu_margin = Margin::same(8);
        style.spacing.button_padding = Vec2::new(8.0, 5.0);
        style.spacing.interact_size.y = 27.0;
        style.spacing.icon_width = 16.0;
        style.spacing.icon_width_inner = 10.0;
        style.spacing.icon_spacing = 6.0;
    });
    dark_mode
}

/// Map the shared tokens to egui-dock, whose style is independent of egui's
/// standard widget visuals.
pub fn dock_style(dark_mode: bool) -> egui_dock::Style {
    let theme = if dark_mode {
        Theme::dark()
    } else {
        Theme::light()
    };
    let mut style = egui_dock::Style::from_egui(&eframe::egui::Style {
        visuals: egui_visuals(dark_mode),
        ..Default::default()
    });
    // The dock already draws a border around its body. Keep a small inset for
    // resize handles and focus rings without spending a full text row on
    // decoration around every workspace view.
    style.dock_area_padding = Some(Margin::same(4));
    style.main_surface_border_stroke = Stroke::new(1.0, theme.border);
    style.separator.color_idle = theme.border;
    style.separator.color_hovered = theme.focus_ring;
    style.separator.color_dragged = theme.focus_ring;
    style.tab_bar.bg_fill = theme.canvas;
    style.tab_bar.height = 26.0;
    style.tab_bar.inner_margin = Margin::symmetric(4, 1);
    style.tab_bar.hline_color = theme.border;
    style.tab.active.bg_fill = theme.raised_surface;
    style.tab.active.outline_color = theme.border;
    style.tab.active.text_color = theme.primary_text;
    style.tab.inactive.bg_fill = theme.canvas;
    style.tab.inactive.outline_color = theme.border;
    style.tab.inactive.text_color = theme.secondary_text;
    style.tab.focused.bg_fill = theme.selection_focused;
    style.tab.focused.outline_color = theme.focus_ring;
    style.tab.focused.text_color = theme.primary_text;
    style.tab.hovered.bg_fill = theme.hover;
    style.tab.hovered.outline_color = theme.focus_ring;
    style.tab.hovered.text_color = theme.primary_text;
    style.tab.inactive_with_kb_focus = style.tab.focused.clone();
    style.tab.active_with_kb_focus = style.tab.focused.clone();
    style.tab.focused_with_kb_focus = style.tab.focused.clone();
    style.tab.tab_body.bg_fill = theme.log_surface;
    style.tab.tab_body.stroke = Stroke::new(1.0, theme.border);
    style.tab.tab_body.inner_margin = Margin::same(4);
    style.buttons.add_tab_color = theme.secondary_text;
    style.buttons.add_tab_active_color = theme.primary_text;
    style.buttons.add_tab_bg_fill = theme.hover;
    style.buttons.add_tab_border_color = theme.border;
    style.buttons.close_tab_color = theme.secondary_text;
    style.buttons.close_tab_active_color = theme.primary_text;
    style.buttons.close_tab_bg_fill = theme.hover;
    style.overlay.selection_color = theme.selection_focused;
    style.overlay.button_color = theme.primary_text;
    style.overlay.button_border_stroke = Stroke::new(1.0, theme.focus_ring);
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_primary_text_and_visuals_share_the_token() {
        assert_eq!(Theme::dark().text, Color32::from_rgb(0xd6, 0xdd, 0xe7));
        assert_eq!(egui_visuals(true).text_color(), Theme::dark().primary_text);
    }

    #[test]
    fn light_primary_text_and_visuals_share_the_token() {
        assert_eq!(Theme::light().text, Color32::from_rgb(0x24, 0x30, 0x41));
        assert_eq!(
            egui_visuals(false).text_color(),
            Theme::light().primary_text
        );
    }

    #[test]
    fn visuals_and_dock_use_the_shared_selection_and_border_tokens() {
        let theme = Theme::dark();
        let visuals = egui_visuals(true);
        let dock = dock_style(true);
        assert_eq!(visuals.selection.bg_fill, theme.selection_focused);
        assert_eq!(visuals.widgets.hovered.bg_fill, theme.hover);
        assert_eq!(dock.tab.tab_body.bg_fill, theme.log_surface);
        assert_eq!(dock.separator.color_idle, theme.border);
    }
}
