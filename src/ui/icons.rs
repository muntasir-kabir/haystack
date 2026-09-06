//! Icon management: SVG-based icons embedded in the binary.
//! All icons are embedded at compile time via `include_bytes!` and rendered
//! to textures using `resvg` + `tiny-skia`. This keeps the standalone binary
//! self-contained — no runtime file I/O for icons.

use std::collections::HashMap;
use std::sync::Mutex;

use eframe::egui;
use egui::{Color32, ColorImage, TextureHandle, Vec2};

/// Enum representing all icons used in the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub enum Icon {
    // General UI
    Expand,
    Collapse,
    Pin,
    Close,
    Remove,
    Reset,
    Settings,
    Copy,
    Add,
    Edit,
    Save,
    Search,
    History,
    Commands,
    ExternalWindow,
    Export,
    Help,
    Info,
    Bug,
    More,

    // Analysis & Data
    Analysis,
    Date,
    Target,

    // Timeline & Navigation
    Timeline,
    Log,
    Jump,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    FirstOccurrence,
    LastOccurrence,
    Enter,
    ArrowsVertical,
    ArrowsHorizontal,

    // Filters & Templates
    Key,
    Book,
    Puzzle,
    Star,
    StarOutline,
    Filter,

    // File & App
    App,
    OpenFile,
    Box,

    // Actions
    Trim,
    Start,
    Stop,

    // Misc
    ThemeLight,
    ThemeDark,
    Mcp,
    Integrate,
    Popcorn,

    // Checkbox / visibility
    Check,
    Uncheck,
    Visible,
    Invisible,
}

impl Icon {
    /// Complete icon catalog. Keep UI tests and icon-browser tooling tied to
    /// this list so a newly added asset cannot silently escape validation.
    #[cfg(test)]
    pub const ALL: &'static [Icon] = &[
        Icon::Expand,
        Icon::Collapse,
        Icon::Pin,
        Icon::Close,
        Icon::Remove,
        Icon::Reset,
        Icon::Settings,
        Icon::Copy,
        Icon::Add,
        Icon::Edit,
        Icon::Save,
        Icon::Search,
        Icon::History,
        Icon::Commands,
        Icon::ExternalWindow,
        Icon::Export,
        Icon::Help,
        Icon::Info,
        Icon::Bug,
        Icon::More,
        Icon::Analysis,
        Icon::Date,
        Icon::Target,
        Icon::Timeline,
        Icon::Log,
        Icon::Jump,
        Icon::ArrowUp,
        Icon::ArrowDown,
        Icon::ArrowLeft,
        Icon::ArrowRight,
        Icon::FirstOccurrence,
        Icon::LastOccurrence,
        Icon::Enter,
        Icon::ArrowsVertical,
        Icon::ArrowsHorizontal,
        Icon::Key,
        Icon::Book,
        Icon::Puzzle,
        Icon::Star,
        Icon::StarOutline,
        Icon::Filter,
        Icon::App,
        Icon::OpenFile,
        Icon::Box,
        Icon::Trim,
        Icon::Start,
        Icon::Stop,
        Icon::ThemeLight,
        Icon::ThemeDark,
        Icon::Mcp,
        Icon::Integrate,
        Icon::Popcorn,
        Icon::Check,
        Icon::Uncheck,
        Icon::Visible,
        Icon::Invisible,
    ];

    /// Returns the embedded SVG bytes for this icon.
    pub fn svg_bytes(&self) -> &'static [u8] {
        match self {
            Icon::Expand => include_bytes!("icons/expand.svg"),
            Icon::Collapse => include_bytes!("icons/collapse.svg"),
            Icon::Pin => include_bytes!("icons/pin.svg"),
            Icon::Close => include_bytes!("icons/close.svg"),
            Icon::Remove => include_bytes!("icons/remove.svg"),
            Icon::Reset => include_bytes!("icons/reset.svg"),
            Icon::Settings => include_bytes!("icons/settings.svg"),
            Icon::Copy => include_bytes!("icons/copy.svg"),
            Icon::Add => include_bytes!("icons/add.svg"),
            Icon::Edit => include_bytes!("icons/edit.svg"),
            Icon::Save => include_bytes!("icons/save.svg"),
            Icon::Search => include_bytes!("icons/search.svg"),
            Icon::History => include_bytes!("icons/history.svg"),
            Icon::Commands => include_bytes!("icons/commands.svg"),
            Icon::ExternalWindow => include_bytes!("icons/external-window.svg"),
            Icon::Export => include_bytes!("icons/export.svg"),
            Icon::Help => include_bytes!("icons/help.svg"),
            Icon::Info => include_bytes!("icons/info.svg"),
            Icon::Bug => include_bytes!("icons/bug.svg"),
            Icon::More => include_bytes!("icons/more.svg"),
            Icon::Analysis => include_bytes!("icons/analysis.svg"),
            Icon::Date => include_bytes!("icons/date.svg"),
            Icon::Target => include_bytes!("icons/target.svg"),
            Icon::Timeline => include_bytes!("icons/timeline.svg"),
            Icon::Log => include_bytes!("icons/log.svg"),
            Icon::Jump => include_bytes!("icons/jump.svg"),
            Icon::ArrowUp => include_bytes!("icons/arrow-up.svg"),
            Icon::ArrowDown => include_bytes!("icons/arrow-down.svg"),
            Icon::ArrowLeft => include_bytes!("icons/arrow-left.svg"),
            Icon::ArrowRight => include_bytes!("icons/arrow-right.svg"),
            Icon::FirstOccurrence => include_bytes!("icons/first-occurrence.svg"),
            Icon::LastOccurrence => include_bytes!("icons/last-occurrence.svg"),
            Icon::Enter => include_bytes!("icons/enter.svg"),
            Icon::ArrowsVertical => include_bytes!("icons/arrows-vertical.svg"),
            Icon::ArrowsHorizontal => include_bytes!("icons/arrows-horizontal.svg"),
            Icon::Key => include_bytes!("icons/key.svg"),
            Icon::Book => include_bytes!("icons/book.svg"),
            Icon::Puzzle => include_bytes!("icons/puzzle.svg"),
            Icon::Star => include_bytes!("icons/star.svg"),
            Icon::StarOutline => include_bytes!("icons/star-outline.svg"),
            Icon::Filter => include_bytes!("icons/filter.svg"),
            Icon::App => include_bytes!("icons/app.svg"),
            Icon::OpenFile => include_bytes!("icons/open-file.svg"),
            Icon::Box => include_bytes!("icons/box.svg"),
            Icon::Trim => include_bytes!("icons/trim.svg"),
            Icon::Start => include_bytes!("icons/start.svg"),
            Icon::Stop => include_bytes!("icons/stop.svg"),
            Icon::ThemeLight => include_bytes!("icons/theme-light.svg"),
            Icon::ThemeDark => include_bytes!("icons/theme-dark.svg"),
            Icon::Mcp => include_bytes!("icons/mcp.svg"),
            Icon::Integrate => include_bytes!("icons/integrate.svg"),
            Icon::Popcorn => include_bytes!("icons/popcorn.svg"),
            Icon::Check => include_bytes!("icons/check.svg"),
            Icon::Uncheck => include_bytes!("icons/uncheck.svg"),
            Icon::Visible => include_bytes!("icons/visible.svg"),
            Icon::Invisible => include_bytes!("icons/invisible.svg"),
        }
    }

    /// Returns a text/emoji representation of the icon (fallback for
    /// contexts where SVG rendering isn't available, e.g. painter.text()).
    #[allow(dead_code)]
    pub fn text(&self) -> &'static str {
        match self {
            Icon::Expand => "▶",
            Icon::Collapse => "▼",
            Icon::Pin => "📌",
            Icon::Close => "✕",
            Icon::Remove => "−",
            Icon::Reset => "↺",
            Icon::Settings => "⚙",
            Icon::Copy => "📋",
            Icon::Add => "+",
            Icon::Edit => "✎",
            Icon::Save => "💾",
            Icon::Search => "⌕",
            Icon::History => "↺",
            Icon::Commands => ">_",
            Icon::ExternalWindow => "↗",
            Icon::Export => "⇩",
            Icon::Help => "?",
            Icon::Info => "i",
            Icon::Bug => "!",
            Icon::More => "…",
            Icon::Analysis => "📊",
            Icon::Date => "📅",
            Icon::Target => "🎯",
            Icon::Timeline => "📈",
            Icon::Log => "📝",
            Icon::Jump => "↗",
            Icon::ArrowUp => "↑",
            Icon::ArrowDown => "↓",
            Icon::ArrowLeft => "←",
            Icon::ArrowRight => "→",
            Icon::FirstOccurrence => "<|",
            Icon::LastOccurrence => "|>",
            Icon::Enter => "↵",
            Icon::ArrowsVertical => "↕",
            Icon::ArrowsHorizontal => "↔",
            Icon::Key => "🔑",
            Icon::Book => "📖",
            Icon::Puzzle => "🧩",
            Icon::Star => "★",
            Icon::StarOutline => "☆",
            Icon::Filter => "⏷",
            Icon::App => "🖥",
            Icon::OpenFile => "📂",
            Icon::Box => "📦",
            Icon::Trim => "✂",
            Icon::Start => "▶",
            Icon::Stop => "■",
            Icon::ThemeLight => "☀",
            Icon::ThemeDark => "☾",
            Icon::Mcp => "🔌",
            Icon::Integrate => "🔗",
            Icon::Popcorn => "🍿",
            Icon::Check => "✓",
            Icon::Uncheck => "□",
            Icon::Visible => "👁",
            Icon::Invisible => "🚫",
        }
    }
}

// ---------------------------------------------------------------------------
// SVG rendering + texture caching
// ---------------------------------------------------------------------------

/// Cache key: (icon, color, pixel size).
type CacheKey = (Icon, Color32, u32);

/// Global texture cache so SVGs are rendered once per (icon, color, size)
/// and reused across frames. Cleared when the theme changes.
static TEXTURE_CACHE: Mutex<Option<HashMap<CacheKey, TextureHandle>>> = Mutex::new(None);

/// Clear the texture cache (call when theme changes so icons re-render
/// with the new color).
pub fn clear_cache() {
    if let Ok(mut cache) = TEXTURE_CACHE.lock() {
        *cache = None;
    }
}

/// Render an SVG to an egui texture, using the given color for `currentColor`.
fn render_svg_to_texture(
    ctx: &egui::Context,
    icon: Icon,
    size: f32,
    color: Color32,
) -> Option<TextureHandle> {
    let size_px = size.max(1.0).ceil() as u32;
    let key = (icon, color, size_px);

    // Check cache first.
    if let Ok(cache) = TEXTURE_CACHE.lock() {
        if let Some(tex) = cache.as_ref().and_then(|c| c.get(&key)) {
            return Some(tex.clone());
        }
    }

    // Parse the SVG, injecting a CSS rule so `currentColor` resolves to
    // the requested color. usvg cascades the `color` property down to
    // all `currentColor` references.
    let svg = icon.svg_bytes();
    let color_hex = format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b());
    let style_sheet = format!("svg {{ color: {color_hex}; }}");
    let opt = resvg::usvg::Options {
        style_sheet: Some(style_sheet),
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_data(svg, &opt).ok()?;

    // Compute target pixmap size (preserve aspect ratio).
    let tree_size = tree.size();
    let scale = size_px as f32 / tree_size.width().max(1.0);
    let height_px = (tree_size.height() * scale).max(1.0).ceil() as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size_px, height_px)?;

    // Render the SVG into the pixmap.
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // Convert to egui ColorImage (RGBA, unmultiplied).
    let image =
        ColorImage::from_rgba_unmultiplied([size_px as usize, height_px as usize], pixmap.data());
    let texture = ctx.load_texture(
        format!("icon_{:?}_{:?}_{}", icon, color, size_px),
        image,
        egui::TextureOptions::LINEAR,
    );

    // Cache it.
    if let Ok(mut cache) = TEXTURE_CACHE.lock() {
        cache
            .get_or_insert_with(HashMap::new)
            .insert(key, texture.clone());
    }

    Some(texture)
}

/// Returns an egui Image widget for the given icon at the requested size
/// and color. The SVG is rendered once and cached.
pub fn icon_image(
    ctx: &egui::Context,
    icon: Icon,
    size: f32,
    color: Color32,
) -> egui::Image<'static> {
    let texture = render_svg_to_texture(ctx, icon, size, color).unwrap_or_else(|| {
        // Fallback: a 1×1 colored pixel so the layout still works.
        let image = ColorImage::filled([1, 1], color);
        ctx.load_texture("fallback_icon", image, egui::TextureOptions::LINEAR)
    });
    let tex_size = texture.size_vec2();
    let source = egui::load::SizedTexture::new(texture.id(), tex_size);
    egui::Image::new(source).fit_to_exact_size(Vec2::splat(size))
}

/// Decoded RGBA pixels of the application logo (`logotomy_256.png`), decoded
/// once on first use and reused across frames.
static APP_LOGO: std::sync::OnceLock<ColorImage> = std::sync::OnceLock::new();

/// Texture handle cache for the app logo, so it's uploaded to the GPU once per
/// render size instead of every frame.
static APP_LOGO_TEX_CACHE: Mutex<Option<(u32, TextureHandle)>> = Mutex::new(None);

/// Render the application logo (the embedded `logotomy_256.png`) as a cached
/// egui image. The PNG is decoded at first use via the `image` crate and
/// loaded directly into a texture, so it doesn't depend on egui's built-in
/// (optional) image loaders — it always renders even when those loaders are
/// not compiled in.
pub fn app_logo(ctx: &egui::Context, size: f32) -> egui::Image<'static> {
    let size_px = size.max(1.0).ceil() as u32;

    // Reuse a previously created texture for the same size.
    if let Ok(cache) = APP_LOGO_TEX_CACHE.lock() {
        if let Some((s, tex)) = cache.as_ref() {
            if *s == size_px {
                let cached = egui::load::SizedTexture::new(tex.id(), tex.size_vec2());
                return egui::Image::new(cached).fit_to_exact_size(Vec2::splat(size));
            }
        }
    }

    let logo = APP_LOGO.get_or_init(|| {
        let bytes: &[u8] = include_bytes!("icons/logotomy_256.png");
        match image::load_from_memory_with_format(bytes, image::ImageFormat::Png) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                ColorImage::from_rgba_unmultiplied(
                    [rgba.width() as usize, rgba.height() as usize],
                    rgba.as_raw(),
                )
            }
            Err(_) => ColorImage::filled([1, 1], Color32::WHITE), // degenerate fallback
        }
    });

    let texture = ctx.load_texture(
        format!("app_logo_{size_px}"),
        logo.clone(),
        egui::TextureOptions::LINEAR,
    );
    if let Ok(mut cache) = APP_LOGO_TEX_CACHE.lock() {
        *cache = Some((size_px, texture.clone()));
    }

    let source = egui::load::SizedTexture::new(texture.id(), texture.size_vec2());
    egui::Image::new(source).fit_to_exact_size(Vec2::splat(size))
}

/// Compact standard height for application chrome so controls stay aligned
/// without competing with the timeline and log content.
pub const ACTION_HEIGHT: f32 = 22.0;

/// A consistent icon-and-text action with a required explanatory tooltip.
pub fn action_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: impl Into<egui::WidgetText>,
    color: Color32,
    tooltip: impl Into<egui::WidgetText>,
) -> egui::Response {
    let image = icon_image(ui.ctx(), icon, 14.0, color);
    ui.add(
        egui::Button::image_and_text(image, label)
            .gap(4.0)
            .frame_when_inactive(false)
            .min_size(egui::vec2(0.0, ACTION_HEIGHT)),
    )
    .on_hover_text(tooltip)
}

/// Disabled-capable form of [`action_button`].
pub fn action_button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: Icon,
    label: impl Into<egui::WidgetText>,
    color: Color32,
    tooltip: impl Into<egui::WidgetText>,
) -> egui::Response {
    let image = icon_image(ui.ctx(), icon, 14.0, color);
    ui.add_enabled(
        enabled,
        egui::Button::image_and_text(image, label)
            .gap(4.0)
            .frame_when_inactive(false)
            .min_size(egui::vec2(0.0, ACTION_HEIGHT)),
    )
    .on_hover_text(tooltip)
}

/// A visually emphasized icon-and-text action for dialog confirmation and
/// other single primary outcomes.
pub fn primary_action_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: impl Into<egui::WidgetText>,
    fill: Color32,
    tooltip: impl Into<egui::WidgetText>,
) -> egui::Response {
    let foreground = contrasting_text(fill);
    let image = icon_image(ui.ctx(), icon, 14.0, foreground);
    ui.add(
        egui::Button::image_and_text(image, label)
            .gap(4.0)
            .fill(fill)
            .min_size(egui::vec2(0.0, ACTION_HEIGHT + 2.0)),
    )
    .on_hover_text(tooltip)
}

/// Disabled-capable form of [`primary_action_button`].
pub fn primary_action_button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: Icon,
    label: impl Into<egui::WidgetText>,
    fill: Color32,
    tooltip: impl Into<egui::WidgetText>,
) -> egui::Response {
    let foreground = contrasting_text(fill);
    let image = icon_image(ui.ctx(), icon, 14.0, foreground);
    ui.add_enabled(
        enabled,
        egui::Button::image_and_text(image, label)
            .gap(4.0)
            .fill(fill)
            .min_size(egui::vec2(0.0, ACTION_HEIGHT + 2.0)),
    )
    .on_hover_text(tooltip)
}

fn contrasting_text(fill: Color32) -> Color32 {
    let luminance =
        0.2126 * f32::from(fill.r()) + 0.7152 * f32::from(fill.g()) + 0.0722 * f32::from(fill.b());
    if luminance > 150.0 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

/// A conventional compact icon-only secondary action with a required tooltip.
/// Primary actions should use [`action_button`].
pub fn icon_action_button(
    ui: &mut egui::Ui,
    icon: Icon,
    color: Color32,
    tooltip: impl Into<egui::WidgetText>,
) -> egui::Response {
    let image = icon_image(ui.ctx(), icon, 14.0, color);
    ui.add_sized(
        egui::vec2(ACTION_HEIGHT, ACTION_HEIGHT),
        egui::Button::new(image).frame_when_inactive(false),
    )
    .on_hover_text(tooltip)
}

/// A default egui `Button` with an SVG image inside, but positioned at an
/// exact `rect` instead of flowing with the layout. Used inside fully
/// custom (painter-layout) surfaces such as the timeline, where widgets
/// can't rely on normal auto-layout but should still get egui's native
/// button hover visuals + animation for free.
pub fn icon_button_at(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    icon: Icon,
    color: Color32,
) -> egui::Response {
    let icon_size = (rect.height() - 2.0).clamp(6.0, 16.0);
    let image = icon_image(ui.ctx(), icon, icon_size, color);
    // Exact positioning with zero button padding so the icon fills the rect
    // (the timer canvas lays everything out by hand).
    let prev_pad = ui.spacing_mut().button_padding;
    ui.spacing_mut().button_padding = egui::Vec2::ZERO;
    let resp = ui.put(rect, egui::Button::new(image).min_size(egui::Vec2::ZERO));
    ui.spacing_mut().button_padding = prev_pad;
    resp
}

/// Draw an SVG icon centered at `center` on the given painter.
/// Used for painter-based rendering (e.g. timeline panel icons).
pub fn paint_icon(
    ctx: &egui::Context,
    painter: &egui::Painter,
    icon: Icon,
    center: egui::Pos2,
    size: f32,
    color: Color32,
) {
    if let Some(texture) = render_svg_to_texture(ctx, icon, size, color) {
        let rect = egui::Rect::from_center_size(center, Vec2::splat(size));
        painter.image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every embedded SVG must parse cleanly through the same `usvg` pipeline
    /// used at runtime (`render_svg_to_texture`). If an icon fails to parse,
    /// `paint_icon`/`icon_image` silently draw nothing and the caller's default
    /// widget (e.g. egui_dock's close ✕ under our pop-out icon) shows through.
    #[test]
    fn all_embedded_icons_parse_in_usvg() {
        for &icon in Icon::ALL {
            let svg = icon.svg_bytes();
            let style_sheet = "svg { color: #ABCABC; }".to_string();
            let opt = resvg::usvg::Options {
                style_sheet: Some(style_sheet),
                ..Default::default()
            };
            let tree = resvg::usvg::Tree::from_data(svg, &opt)
                .unwrap_or_else(|e| panic!("icon {:?} failed to parse: {e}", icon));
            assert!(
                tree.size().width() > 0.0 && tree.size().height() > 0.0,
                "icon {:?} produced an empty image",
                icon
            );

            for size in [12_u32, 15, 16] {
                let scale = size as f32 / tree.size().width().max(1.0);
                let height = (tree.size().height() * scale).max(1.0).ceil() as u32;
                let mut pixmap = resvg::tiny_skia::Pixmap::new(size, height).unwrap();
                resvg::render(
                    &tree,
                    resvg::tiny_skia::Transform::from_scale(scale, scale),
                    &mut pixmap.as_mut(),
                );
                assert!(
                    pixmap.data().chunks_exact(4).any(|pixel| pixel[3] != 0),
                    "icon {icon:?} rendered no visible pixels at {size}px"
                );
            }
        }
    }
}
