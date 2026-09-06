//! Theme system: dark and light mode colour palettes.
//! Every UI component should source its colours from the active `Theme`
//! rather than hardcoding `Color32` literals.

use eframe::egui::Color32;

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
    /// Minimap bar colour.
    pub minimap_bar: Color32,
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
    /// Drag overlay colour (drop zone).
    pub overlay_bg: Color32,
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
            bg: Color32::from_rgb(0x1e, 0x1e, 0x2e),
            surface: Color32::from_rgb(0x2a, 0x2a, 0x3e),
            text: Color32::from_rgb(0xe0, 0xe0, 0xe0),
            text_muted: Color32::from_rgb(0x90, 0x90, 0xa0),
            selection_bg: Color32::from_rgb(0x3a, 0x3a, 0x5c),
            accent: Color32::from_rgb(0x89, 0xb4, 0xfa),
            histogram: Color32::from_gray(78),
            minimap_bg: Color32::from_gray(38),
            minimap_bar: Color32::from_gray(90),
            gutter: Color32::from_rgba_unmultiplied(150, 150, 160, 180),
            gutter_bg: Color32::from_rgba_unmultiplied(255, 255, 255, 12),
            log_text: Color32::from_gray(215),
            axis: Color32::from_gray(130),
            hint: Color32::from_gray(135),
            overlay_bg: Color32::from_rgba_unmultiplied(20, 60, 120, 110),
            brush_fill: Color32::from_rgba_unmultiplied(100, 160, 240, 40),
            brush_stroke: Color32::from_rgb(120, 180, 255),
            minimap_zoom: Color32::from_rgb(100, 160, 240),
            selection_line: Color32::WHITE,
            occurrence_hover: Color32::WHITE,
            url_text: Color32::LIGHT_BLUE,
            placeholder: Color32::GRAY,
            viewport_shadow: Color32::from_rgba_unmultiplied(100, 160, 240, 30),
            viewport_shadow_stroke: Color32::from_rgba_unmultiplied(100, 160, 240, 80),
            warning: Color32::from_rgb(255, 180, 50),
            analysis_text: Color32::from_rgb(255, 140, 140),
            selection_range_bg: Color32::from_rgba_unmultiplied(137, 180, 250, 40),
            filter_colors: FILTER_COLORS_DARK,
            search_highlight_bg: Color32::from_rgba_unmultiplied(255, 180, 50, 90),
            keyword_highlight_bg: Color32::from_rgba_unmultiplied(35, 210, 220, 70),
            embedded_data: Color32::from_rgb(166, 227, 161),
            timestamp: Color32::from_rgb(137, 180, 250),
        }
    }

    pub fn light() -> Self {
        Self {
            bg: Color32::from_rgb(0xf5, 0xf5, 0xf0),
            surface: Color32::from_rgb(0xff, 0xff, 0xff),
            text: Color32::from_rgb(0x1a, 0x1a, 0x1a),
            text_muted: Color32::from_rgb(0x66, 0x66, 0x66),
            selection_bg: Color32::from_rgb(0xe0, 0xd0, 0xff),
            accent: Color32::from_rgb(0x1e, 0x66, 0xf5),
            histogram: Color32::from_gray(180),
            minimap_bg: Color32::from_gray(220),
            minimap_bar: Color32::from_gray(140),
            gutter: Color32::from_rgba_unmultiplied(90, 90, 90, 175),
            gutter_bg: Color32::from_rgba_unmultiplied(0, 0, 0, 10),
            log_text: Color32::from_gray(30),
            axis: Color32::from_gray(100),
            hint: Color32::from_gray(105),
            overlay_bg: Color32::from_rgba_unmultiplied(20, 60, 120, 110),
            brush_fill: Color32::from_rgba_unmultiplied(100, 160, 240, 40),
            brush_stroke: Color32::from_rgb(60, 120, 200),
            minimap_zoom: Color32::from_rgb(60, 120, 200),
            selection_line: Color32::from_rgb(0, 0, 0),
            occurrence_hover: Color32::from_rgb(0, 0, 0),
            url_text: Color32::from_rgb(0, 80, 180),
            placeholder: Color32::GRAY,
            viewport_shadow: Color32::from_rgba_unmultiplied(80, 130, 200, 60),
            viewport_shadow_stroke: Color32::from_rgba_unmultiplied(80, 130, 200, 120),
            warning: Color32::from_rgb(200, 100, 0),
            analysis_text: Color32::from_rgb(200, 50, 50),
            selection_range_bg: Color32::from_rgba_unmultiplied(30, 102, 245, 30),
            filter_colors: FILTER_COLORS_LIGHT,
            search_highlight_bg: Color32::from_rgba_unmultiplied(255, 200, 0, 110),
            keyword_highlight_bg: Color32::from_rgba_unmultiplied(15, 155, 175, 60),
            embedded_data: Color32::from_rgb(25, 125, 55),
            timestamp: Color32::from_rgb(55, 95, 170),
        }
    }
}
