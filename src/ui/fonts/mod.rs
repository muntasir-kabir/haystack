//! Embedded UI and log fonts.
//!
//! Space Mono (<https://fonts.google.com/specimen/Space+Mono>) is distributed
//! under the **SIL Open Font License 1.1** — the full license text ships next
//! to the bundled `.ttf` files in `src/ui/fonts/Space_Mono/OFL.txt`. OFL 1.1
//! explicitly allows bundling/embedding fonts in applications as long as the
//! license text accompanies them and the fonts aren't sold by themselves. We
//! embed the four faces **unmodified**, so the "Space Mono" name restriction
//! (Reserved Font Name) doesn't apply either.
//!
//! Inter Regular is used for all normal application UI text. It is registered as
//! the first face in egui's proportional and monospace families, so labels,
//! buttons, text edits, and explicit non-log monospace UI all use Inter.
//! Space Mono is registered under a dedicated named egui family (`space_mono`)
//! and only log content opts into it — the log view, log previews, the pin
//! modal preview, and the pinned-lines panel. The embedded bytes live in the
//! binary via `include_bytes!`.

use std::sync::Arc;

use eframe::egui::{Context, FontData, FontDefinitions, FontFamily, FontId};

/// Registry keys for the embedded faces (internal names only — the font
/// files themselves stay the unmodified Space Mono originals).
const FONT_REGULAR: &str = "space_mono_regular";
const FONT_BOLD: &str = "space_mono_bold";
const FONT_ITALIC: &str = "space_mono_italic";
const FONT_BOLD_ITALIC: &str = "space_mono_bold_italic";
const FONT_INTER_REGULAR: &str = "inter_regular";

/// Name of the egui family under which the Space Mono faces are registered.
const LOG_FAMILY_NAME: &str = "space_mono";

/// The egui font family used for log text.
pub fn log_font_family() -> FontFamily {
    FontFamily::Name(LOG_FAMILY_NAME.into())
}

/// A [`FontId`] for log text at the given size, using the embedded
/// Space Mono face.
pub fn log_font(size: f32) -> FontId {
    FontId::new(size, log_font_family())
}

/// Build the [`FontDefinitions`] to install at startup: Inter Regular is the
/// application-wide UI face, while the four Space Mono faces are registered
/// under a separate log family. Both families retain egui's built-in glyph
/// fallbacks for emoji, box-drawing, and other characters they do not cover.
pub fn font_definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();

    fonts.font_data.insert(
        FONT_INTER_REGULAR.to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "Inter/Inter-Regular.ttf"
        ))),
    );

    let faces: [(&str, &'static [u8]); 4] = [
        (
            FONT_REGULAR,
            include_bytes!("Space_Mono/SpaceMono-Regular.ttf"),
        ),
        (FONT_BOLD, include_bytes!("Space_Mono/SpaceMono-Bold.ttf")),
        (
            FONT_ITALIC,
            include_bytes!("Space_Mono/SpaceMono-Italic.ttf"),
        ),
        (
            FONT_BOLD_ITALIC,
            include_bytes!("Space_Mono/SpaceMono-BoldItalic.ttf"),
        ),
    ];
    for (name, bytes) in faces {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    }

    let log_family = fonts.families.entry(log_font_family()).or_default();
    log_family.push(FONT_REGULAR.to_owned());
    log_family.push(FONT_BOLD.to_owned());
    log_family.push(FONT_ITALIC.to_owned());
    log_family.push(FONT_BOLD_ITALIC.to_owned());
    // Fallbacks for anything Space Mono is missing — egui's built-in faces,
    // in the same priority order egui uses for its monospace family.
    log_family.push("Hack".to_owned());
    log_family.push("Ubuntu-Light".to_owned());
    log_family.push("NotoEmoji-Regular".to_owned());
    log_family.push("emoji-icon-font".to_owned());

    // Inter Regular is the application-wide UI face. Keep it at the front of both
    // built-in families so default labels/buttons and explicit `.monospace()`
    // UI controls do not fall back to egui's Hack or Ubuntu fonts. Log text
    // uses the named `space_mono` family above and is unaffected by this.
    let ui_family = vec![
        FONT_INTER_REGULAR.to_owned(),
        "Ubuntu-Light".to_owned(),
        "NotoEmoji-Regular".to_owned(),
        "emoji-icon-font".to_owned(),
    ];
    fonts
        .families
        .insert(FontFamily::Proportional, ui_family.clone());
    fonts.families.insert(FontFamily::Monospace, ui_family);

    fonts
}

/// Install the embedded Space Mono log font into an egui context. Call once
/// at app startup, before the first frame, so all viewports share it.
pub fn install(ctx: &Context) {
    ctx.set_fonts(font_definitions());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_inter_regular_with_valid_ttf_magic() {
        let fonts = font_definitions();
        let data = fonts
            .font_data
            .get(FONT_INTER_REGULAR)
            .expect("Inter Regular is registered");
        let bytes = data.font.as_ref();
        assert!(bytes.len() > 1_000, "Inter Regular is unexpectedly small");
        assert_eq!(&bytes[..4], &[0x00, 0x01, 0x00, 0x00]);
        // Inter 4.1's static Regular face is distinct from the old 414,676-byte
        // ExtraLight asset and is 411,640 bytes in the bundled release.
        assert_eq!(bytes.len(), 411_640, "embedded face is not Inter Regular");
    }

    #[test]
    fn embeds_all_four_space_mono_faces_with_valid_ttf_magic() {
        let fonts = font_definitions();
        for name in [FONT_REGULAR, FONT_BOLD, FONT_ITALIC, FONT_BOLD_ITALIC] {
            let data = fonts
                .font_data
                .get(name)
                .unwrap_or_else(|| panic!("face {name} is not registered"));
            let bytes = data.font.as_ref();
            assert!(bytes.len() > 1_000, "face {name} is unexpectedly small");
            // TrueType "sfnt" magic: 0x00010000 (big-endian).
            assert_eq!(
                &bytes[..4],
                &[0x00, 0x01, 0x00, 0x00],
                "face {name} is not a TrueType font"
            );
        }
    }

    #[test]
    fn registers_log_family_with_regular_first_then_egui_fallbacks() {
        let fonts = font_definitions();
        let family = fonts
            .families
            .get(&log_font_family())
            .expect("space_mono family is registered");
        assert_eq!(family.first().map(String::as_str), Some(FONT_REGULAR));
        // egui's built-in faces are appended as glyph fallbacks.
        assert!(family.iter().any(|n| n == "Hack"));
        assert!(family.iter().any(|n| n == "NotoEmoji-Regular"));
        // UI families use Inter Regular and must not reference Space Mono.
        assert_eq!(
            fonts.families[&FontFamily::Monospace]
                .first()
                .map(String::as_str),
            Some(FONT_INTER_REGULAR)
        );
        assert_eq!(
            fonts.families[&FontFamily::Proportional]
                .first()
                .map(String::as_str),
            Some(FONT_INTER_REGULAR)
        );
        assert!(!fonts.families[&FontFamily::Monospace]
            .iter()
            .any(|n| n.starts_with("space_mono")));
        assert!(!fonts.families[&FontFamily::Proportional]
            .iter()
            .any(|n| n.starts_with("space_mono")));
    }

    #[test]
    fn log_font_uses_the_log_family() {
        let id = log_font(12.0);
        assert_eq!(id.size, 12.0);
        assert_eq!(id.family, log_font_family());
    }
}
