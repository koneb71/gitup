//! Typography.
//!
//! Inter ships as a variable font, so weight is an axis rather than a set of
//! separate files: `RichText::variation(b"wght", 600.0)` gives semibold from the
//! same data as regular. That is why there is one Inter file in `assets/` and
//! not five, and why the helpers below are the only place weights are chosen.

use egui::{FontData, FontDefinitions, FontFamily, FontId, RichText, Style, TextStyle};
use std::sync::Arc;

/// Family names registered with egui.
pub const UI: &str = "inter";
pub const MONO: &str = "jetbrains";
pub const MONO_BOLD: &str = "jetbrains-bold";
pub const ICONS: &str = "phosphor";
pub const ICONS_FILL: &str = "phosphor-fill";

/// Weights on Inter's `wght` axis.
pub mod weight {
    pub const REGULAR: f32 = 400.0;
    pub const MEDIUM: f32 = 500.0;
    pub const SEMIBOLD: f32 = 600.0;
    pub const BOLD: f32 = 700.0;
}

/// Type scale, in points.
pub mod size {
    pub const DISPLAY: f32 = 19.0;
    pub const TITLE: f32 = 15.0;
    pub const SUBTITLE: f32 = 13.5;
    pub const BODY: f32 = 13.0;
    pub const LABEL: f32 = 12.0;
    pub const CAPTION: f32 = 11.0;
    pub const MONO: f32 = 12.5;
    pub const ICON: f32 = 15.0;
}

pub fn mono_family() -> FontFamily {
    FontFamily::Name(MONO.into())
}

pub fn mono_bold_family() -> FontFamily {
    FontFamily::Name(MONO_BOLD.into())
}

pub fn icon_family() -> FontFamily {
    FontFamily::Name(ICONS.into())
}

pub fn icon_fill_family() -> FontFamily {
    FontFamily::Name(ICONS_FILL.into())
}

/// Named text styles, so widgets ask for a role rather than a number.
pub fn style_caption() -> TextStyle {
    TextStyle::Name("caption".into())
}

pub fn style_label() -> TextStyle {
    TextStyle::Name("label".into())
}

pub fn style_title() -> TextStyle {
    TextStyle::Name("title".into())
}

pub fn style_display() -> TextStyle {
    TextStyle::Name("display".into())
}

/// Register the bundled fonts. Called once, before the first frame.
///
/// Icons get their own family rather than riding on the Proportional fallback
/// chain. That is not tidiness — Inter v4 maps its own glyphs into the Unicode
/// private-use area, overlapping Phosphor's codepoints. As a fallback, Inter
/// wins for every icon it happens to cover and draws an unrelated letterform,
/// so `U+E278` (git-branch) renders as a hooked "a". Asking for the icon family
/// explicitly is the only way to be sure which font answers.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };

    add(UI, include_bytes!("../../assets/fonts/InterVariable.ttf"));
    add(
        MONO,
        include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf"),
    );
    add(
        MONO_BOLD,
        include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf"),
    );
    add(ICONS, include_bytes!("../../assets/fonts/Phosphor.ttf"));
    add(
        ICONS_FILL,
        include_bytes!("../../assets/fonts/Phosphor-Fill.ttf"),
    );

    // Text families deliberately exclude the icon fonts; see above.
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.insert(0, UI.to_owned());

    let monospace = fonts.families.entry(FontFamily::Monospace).or_default();
    monospace.insert(0, MONO.to_owned());

    fonts.families.insert(
        FontFamily::Name(MONO.into()),
        vec![MONO.to_owned(), UI.to_owned()],
    );
    fonts.families.insert(
        FontFamily::Name(MONO_BOLD.into()),
        vec![MONO_BOLD.to_owned(), UI.to_owned()],
    );
    // Icon families put the icon font first so it always wins, with Inter last
    // only so that a stray non-icon character still renders as something.
    fonts.families.insert(
        FontFamily::Name(ICONS.into()),
        vec![ICONS.to_owned(), UI.to_owned()],
    );
    fonts.families.insert(
        FontFamily::Name(ICONS_FILL.into()),
        vec![ICONS_FILL.to_owned(), ICONS.to_owned(), UI.to_owned()],
    );

    ctx.set_fonts(fonts);
}

/// Map egui's built-in text styles onto the scale above.
pub fn install_text_styles(style: &mut Style) {
    use FontFamily::Proportional;
    let s = &mut style.text_styles;
    s.insert(TextStyle::Heading, FontId::new(size::TITLE, Proportional));
    s.insert(TextStyle::Body, FontId::new(size::BODY, Proportional));
    s.insert(TextStyle::Button, FontId::new(size::BODY, Proportional));
    s.insert(TextStyle::Small, FontId::new(size::CAPTION, Proportional));
    s.insert(TextStyle::Monospace, FontId::new(size::MONO, mono_family()));
    s.insert(style_display(), FontId::new(size::DISPLAY, Proportional));
    s.insert(style_title(), FontId::new(size::TITLE, Proportional));
    s.insert(style_label(), FontId::new(size::LABEL, Proportional));
    s.insert(style_caption(), FontId::new(size::CAPTION, Proportional));
}

fn ui_text(text: impl Into<String>, sz: f32, wght: f32) -> RichText {
    RichText::new(text)
        .font(FontId::new(sz, FontFamily::Proportional))
        .variation(b"wght", wght)
}

pub fn display(text: impl Into<String>) -> RichText {
    ui_text(text, size::DISPLAY, weight::SEMIBOLD)
}

pub fn title(text: impl Into<String>) -> RichText {
    ui_text(text, size::TITLE, weight::SEMIBOLD)
}

pub fn subtitle(text: impl Into<String>) -> RichText {
    ui_text(text, size::SUBTITLE, weight::MEDIUM)
}

pub fn body(text: impl Into<String>) -> RichText {
    ui_text(text, size::BODY, weight::REGULAR)
}

/// Body text with emphasis — the app's equivalent of bold.
pub fn strong(text: impl Into<String>) -> RichText {
    ui_text(text, size::BODY, weight::SEMIBOLD)
}

pub fn medium(text: impl Into<String>) -> RichText {
    ui_text(text, size::BODY, weight::MEDIUM)
}

pub fn label(text: impl Into<String>) -> RichText {
    ui_text(text, size::LABEL, weight::MEDIUM)
}

pub fn caption(text: impl Into<String>) -> RichText {
    ui_text(text, size::CAPTION, weight::REGULAR)
}

/// Small all-caps section headers used in the sidebar.
pub fn overline(text: impl AsRef<str>) -> RichText {
    ui_text(
        text.as_ref().to_uppercase(),
        size::CAPTION,
        weight::SEMIBOLD,
    )
    .extra_letter_spacing(0.6)
}

pub fn mono(text: impl Into<String>) -> RichText {
    RichText::new(text).font(FontId::new(size::MONO, mono_family()))
}

pub fn mono_sized(text: impl Into<String>, sz: f32) -> RichText {
    RichText::new(text).font(FontId::new(sz, mono_family()))
}

/// A commit hash. Always monospace, always slightly dimmer than body text.
pub fn hash(text: impl Into<String>) -> RichText {
    RichText::new(text).font(FontId::new(size::LABEL, mono_family()))
}

pub fn icon(glyph: &str) -> RichText {
    RichText::new(glyph).font(FontId::new(size::ICON, icon_family()))
}

pub fn icon_sized(glyph: &str, sz: f32) -> RichText {
    RichText::new(glyph).font(FontId::new(sz, icon_family()))
}

/// [`FontId`] for painting an icon directly with a [`egui::Painter`].
pub fn icon_font(sz: f32) -> FontId {
    FontId::new(sz, icon_family())
}

/// An icon and a word, laid out as one run so a button can take them.
///
/// A `Button` holds a single widget, and the icon and the label come from
/// different font families — so putting a word beside an icon means laying out
/// two font runs together rather than placing two labels. Without this the
/// choice at each call site is a bare icon that says nothing or a bare word
/// that looks unlike every other control, which is how the interface drifts.
pub fn icon_label(glyph: &str, label: &str, color: egui::Color32) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};

    let mut job = LayoutJob::default();
    job.append(
        glyph,
        0.0,
        TextFormat {
            font_id: FontId::new(size::LABEL, icon_family()),
            color,
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job.append(
        label,
        // A word space is too tight next to a glyph that already has bearing.
        6.0,
        TextFormat {
            // Inter is the Proportional family, not a named one — `UI` names
            // the font data, not a family binding.
            font_id: FontId::new(size::LABEL, FontFamily::Proportional),
            color,
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job
}

/// The filled Phosphor variant, for selected or active states.
pub fn icon_filled(glyph: &str, sz: f32) -> RichText {
    RichText::new(glyph).font(FontId::new(sz, icon_fill_family()))
}
