//! Design tokens.
//!
//! Every colour, radius, and spacing value in the app comes from here. Nothing
//! else hard-codes a `Color32`. That is what makes the light and dark themes
//! actually consistent instead of approximately consistent.
//!
//! The palette is deliberately not another blue-on-grey IDE: cool slate
//! surfaces with a single warm amber accent, so the one thing that is
//! interactive reads as interactive at a glance.

use egui::{Color32, CornerRadius, Stroke, Visuals};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

impl ThemeMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb(
        ((hex >> 16) & 0xFF) as u8,
        ((hex >> 8) & 0xFF) as u8,
        (hex & 0xFF) as u8,
    )
}

/// The complete colour set for one theme.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    // Surfaces, deepest to highest.
    pub bg_sunken: Color32,
    pub bg_base: Color32,
    pub bg_surface: Color32,
    pub bg_raised: Color32,
    pub bg_overlay: Color32,

    // Interaction states applied over a surface.
    pub hover: Color32,
    pub active: Color32,
    pub selected: Color32,
    pub selected_inactive: Color32,

    // Lines.
    pub border: Color32,
    pub border_strong: Color32,

    // Text.
    pub text: Color32,
    pub text_secondary: Color32,
    pub text_muted: Color32,
    pub text_on_accent: Color32,

    // The single accent.
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_dim: Color32,

    // Semantic.
    pub added: Color32,
    pub added_bg: Color32,
    pub removed: Color32,
    pub removed_bg: Color32,
    pub modified: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub info: Color32,
    pub staged: Color32,

    /// Commit-graph lane colours, cycled by lane index. Chosen to stay
    /// distinguishable from each other *and* from the accent.
    pub lanes: [Color32; 8],
}

pub const DARK: Palette = Palette {
    bg_sunken: rgb(0x080A0E),
    bg_base: rgb(0x0E1116),
    bg_surface: rgb(0x151A21),
    bg_raised: rgb(0x1C222B),
    bg_overlay: rgb(0x232B35),

    hover: rgb(0x232B35),
    active: rgb(0x2C353F),
    selected: rgb(0x2A3441),
    selected_inactive: rgb(0x1E242C),

    border: rgb(0x232A33),
    border_strong: rgb(0x38424F),

    text: rgb(0xE7EDF5),
    text_secondary: rgb(0x9AA8BB),
    text_muted: rgb(0x68768A),
    text_on_accent: rgb(0x1A0E06),

    accent: rgb(0xF2853F),
    accent_hover: rgb(0xFF9A5C),
    accent_dim: rgb(0x7A4522),

    added: rgb(0x4FBF67),
    added_bg: rgb(0x11291A),
    removed: rgb(0xF06355),
    removed_bg: rgb(0x2E1618),
    modified: rgb(0xD9A238),
    warning: rgb(0xD9A238),
    danger: rgb(0xF06355),
    info: rgb(0x5CA8FF),
    staged: rgb(0x4FBF67),

    lanes: [
        rgb(0x5CA8FF),
        rgb(0xF2853F),
        rgb(0x4FBF67),
        rgb(0xC07BF5),
        rgb(0xE8CB55),
        rgb(0x4FD1C5),
        rgb(0xF272A8),
        rgb(0x8B98FF),
    ],
};

pub const LIGHT: Palette = Palette {
    bg_sunken: rgb(0xEDF0F4),
    bg_base: rgb(0xF7F9FB),
    bg_surface: rgb(0xFFFFFF),
    // Elevation runs the other way in a light theme. Making "raised" lighter
    // than the surface it sits on means white on white: chips, buttons, and
    // loading skeletons all vanish. A step *darker* is what reads as raised
    // here, which is why this is not simply the dark palette inverted.
    bg_raised: rgb(0xEBEEF3),
    bg_overlay: rgb(0xFFFFFF),

    hover: rgb(0xEDF1F6),
    active: rgb(0xE1E8F0),
    selected: rgb(0xDCE8F7),
    selected_inactive: rgb(0xECEFF3),

    border: rgb(0xDCE2E9),
    border_strong: rgb(0xB6C0CC),

    text: rgb(0x121820),
    text_secondary: rgb(0x4D5A6A),
    text_muted: rgb(0x77848F),
    text_on_accent: rgb(0xFFFFFF),

    accent: rgb(0xCF6018),
    accent_hover: rgb(0xE87427),
    accent_dim: rgb(0xF6DCC9),

    added: rgb(0x1A7F37),
    added_bg: rgb(0xE2F5E7),
    removed: rgb(0xC3372B),
    removed_bg: rgb(0xFBE6E4),
    modified: rgb(0x9A6700),
    warning: rgb(0x9A6700),
    danger: rgb(0xC3372B),
    info: rgb(0x1667CE),
    staged: rgb(0x1A7F37),

    lanes: [
        rgb(0x1667CE),
        rgb(0xCF6018),
        rgb(0x1A7F37),
        rgb(0x8B37C9),
        rgb(0x9A6700),
        rgb(0x0F8A80),
        rgb(0xC42B7C),
        rgb(0x4B54C9),
    ],
};

/// Spacing scale, in points. A 4pt base keeps everything on a common rhythm.
pub mod space {
    pub const XS: f32 = 2.0;
    pub const SM: f32 = 4.0;
    pub const MD: f32 = 8.0;
    pub const LG: f32 = 12.0;
    pub const XL: f32 = 16.0;
    pub const XXL: f32 = 24.0;
}

pub mod radius {
    pub const SM: u8 = 4;
    pub const MD: u8 = 6;
    pub const LG: u8 = 10;
    pub const PILL: u8 = 100;
}

/// Row heights, shared so the graph, diff, and file lists line up.
pub mod metrics {
    /// Button heights, by the part a button plays.
    ///
    /// Named rather than written at each call site because they had quietly
    /// drifted to five different values for three roles — a 20px Reset beside
    /// a 22px Stage all, a 26px Commit beside a 28px Done. Nothing about that
    /// is visible in any one file, only in the whole.
    ///
    /// `COMPACT` is a secondary action inside a panel; `ACTION` is the
    /// prominent one that finishes a task; `HERO` is the single call to action
    /// on the welcome screen.
    pub const BUTTON_COMPACT: f32 = 22.0;
    pub const BUTTON_ACTION: f32 = 28.0;
    pub const BUTTON_HERO: f32 = 32.0;

    pub const ROW: f32 = 24.0;
    pub const ROW_COMPACT: f32 = 20.0;
    pub const TOOLBAR: f32 = 40.0;
    pub const STATUSBAR: f32 = 26.0;
    pub const RAIL: f32 = 48.0;
    pub const SIDEBAR_DEFAULT: f32 = 260.0;
    pub const SIDEBAR_MIN: f32 = 180.0;

    /// The two header bands the detail pane carries above the diff: the
    /// staged/unstaged bar and the per-column path row.
    ///
    /// Measured from a rendered frame rather than derived, because both are
    /// laid out by their content and no constant in the code adds up to them.
    pub const DETAIL_HEADERS: f32 = 80.0;

    /// The commit box, before the user drags it.
    ///
    /// Sized to its content rather than to a share: it holds a subject line
    /// and the start of a body, and that need does not grow with the screen.
    /// Three lines of message — enough that a drafted subject and the first
    /// line of its body are both visible without scrolling, which is what the
    /// Draft button produces for anything touching more than three files.
    pub const COMMIT_BOX: f32 = 108.0;
}

impl Palette {
    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => DARK,
            ThemeMode::Light => LIGHT,
        }
    }

    pub fn is_dark(&self) -> bool {
        self.bg_base.r() < 128
    }

    /// Blend `top` into `bottom`, producing an **opaque** colour.
    ///
    /// `Color32::gamma_multiply` scales alpha, which is right for a tint drawn
    /// over something already painted but wrong for anything that is itself the
    /// background — a translucent panel fill leaves a see-through strip. Use
    /// this wherever the result is the only thing painted in that region.
    pub fn mix(bottom: Color32, top: Color32, amount: f32) -> Color32 {
        let t = amount.clamp(0.0, 1.0);
        let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Color32::from_rgb(
            lerp(bottom.r(), top.r()),
            lerp(bottom.g(), top.g()),
            lerp(bottom.b(), top.b()),
        )
    }

    /// A surface tinted toward `accent`, for banners and callouts.
    pub fn tinted(&self, accent: Color32, amount: f32) -> Color32 {
        Self::mix(self.bg_surface, accent, amount)
    }

    /// Lane colour for a graph lane index.
    pub fn lane(&self, index: usize) -> Color32 {
        self.lanes[index % self.lanes.len()]
    }

    /// Colour for a status delta, used by file lists and the diff gutter.
    pub fn delta(&self, delta: crate::git::Delta) -> Color32 {
        use crate::git::Delta;
        match delta {
            Delta::Added | Delta::Untracked => self.added,
            Delta::Deleted => self.removed,
            Delta::Modified | Delta::TypeChange => self.modified,
            Delta::Renamed | Delta::Copied => self.info,
            Delta::Conflicted => self.danger,
            Delta::Ignored | Delta::Unmodified => self.text_muted,
        }
    }

    /// Push egui's own styling to match these tokens.
    ///
    /// egui 0.36 keeps a separate `Style` per theme, so both are installed once
    /// at startup and switching themes afterwards is a single preference change
    /// rather than a restyle.
    pub fn apply_to(&self, ctx: &egui::Context, theme: egui::Theme) {
        let mut style = (*ctx.style_of(theme)).clone();
        let mut v = if self.is_dark() {
            Visuals::dark()
        } else {
            Visuals::light()
        };

        v.override_text_color = Some(self.text);
        v.panel_fill = self.bg_base;
        v.window_fill = self.bg_overlay;
        v.extreme_bg_color = self.bg_sunken;
        v.faint_bg_color = self.bg_surface;
        v.code_bg_color = self.bg_surface;
        v.hyperlink_color = self.info;
        v.warn_fg_color = self.warning;
        v.error_fg_color = self.danger;
        v.selection.bg_fill = self.accent.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, self.accent);

        v.window_stroke = Stroke::new(1.0, self.border);
        v.window_corner_radius = CornerRadius::same(radius::LG);
        v.menu_corner_radius = CornerRadius::same(radius::MD);
        v.popup_shadow.color = Color32::from_black_alpha(if self.is_dark() { 140 } else { 40 });
        v.window_shadow.color = Color32::from_black_alpha(if self.is_dark() { 170 } else { 50 });

        let r = CornerRadius::same(radius::MD);

        v.widgets.noninteractive.bg_fill = self.bg_surface;
        v.widgets.noninteractive.weak_bg_fill = self.bg_surface;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, self.text_secondary);
        v.widgets.noninteractive.corner_radius = r;

        v.widgets.inactive.bg_fill = self.bg_raised;
        v.widgets.inactive.weak_bg_fill = self.bg_raised;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, self.border);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, self.text);
        v.widgets.inactive.corner_radius = r;

        v.widgets.hovered.bg_fill = self.hover;
        v.widgets.hovered.weak_bg_fill = self.hover;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, self.border_strong);
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, self.text);
        v.widgets.hovered.corner_radius = r;

        v.widgets.active.bg_fill = self.active;
        v.widgets.active.weak_bg_fill = self.active;
        v.widgets.active.bg_stroke = Stroke::new(1.0, self.accent);
        v.widgets.active.fg_stroke = Stroke::new(1.0, self.text);
        v.widgets.active.corner_radius = r;

        v.widgets.open.bg_fill = self.bg_overlay;
        v.widgets.open.weak_bg_fill = self.bg_overlay;
        v.widgets.open.bg_stroke = Stroke::new(1.0, self.border_strong);
        v.widgets.open.fg_stroke = Stroke::new(1.0, self.text);
        v.widgets.open.corner_radius = r;

        style.visuals = v;
        style.spacing.item_spacing = egui::vec2(space::MD, space::SM);
        style.spacing.button_padding = egui::vec2(space::MD, space::SM);
        style.spacing.menu_margin = egui::Margin::same(space::SM as i8);
        style.spacing.indent = 16.0;
        style.spacing.scroll.bar_width = 10.0;
        style.spacing.scroll.floating = true;
        style.spacing.interact_size.y = metrics::ROW;

        super::text::install_text_styles(&mut style);

        ctx.set_style_of(theme, style);
    }
}

/// Install both palettes. Call once, before the first frame.
pub fn install_styles(ctx: &egui::Context) {
    DARK.apply_to(ctx, egui::Theme::Dark);
    LIGHT.apply_to(ctx, egui::Theme::Light);
}

/// Choose which of the installed palettes is active.
pub fn set_mode(ctx: &egui::Context, mode: ThemeMode) {
    ctx.set_theme(match mode {
        ThemeMode::Dark => egui::ThemePreference::Dark,
        ThemeMode::Light => egui::ThemePreference::Light,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixing_always_produces_an_opaque_colour() {
        let result = Palette::mix(DARK.bg_surface, DARK.warning, 0.16);
        assert_eq!(result.a(), 255, "a panel fill must not be see-through");
    }

    #[test]
    fn mixing_moves_toward_the_top_colour() {
        let base = DARK.bg_surface;
        let none = Palette::mix(base, DARK.warning, 0.0);
        let all = Palette::mix(base, DARK.warning, 1.0);
        assert_eq!(none.to_array()[..3], base.to_array()[..3]);
        assert_eq!(all.to_array()[..3], DARK.warning.to_array()[..3]);
    }

    #[test]
    fn amounts_outside_the_range_are_clamped() {
        let base = LIGHT.bg_surface;
        assert_eq!(
            Palette::mix(base, LIGHT.danger, 2.0),
            Palette::mix(base, LIGHT.danger, 1.0)
        );
        assert_eq!(
            Palette::mix(base, LIGHT.danger, -1.0),
            Palette::mix(base, LIGHT.danger, 0.0)
        );
    }

    #[test]
    fn raised_surfaces_are_distinguishable_from_what_they_sit_on() {
        // A "raised" element the same colour as its background is invisible,
        // which is exactly how a light-theme skeleton disappears.
        for (name, palette) in [("dark", DARK), ("light", LIGHT)] {
            for (label, under) in [("base", palette.bg_base), ("surface", palette.bg_surface)] {
                let difference = channel_distance(palette.bg_raised, under);
                assert!(
                    difference >= 8,
                    "{name}: raised is only {difference} from {label}"
                );
            }
        }
    }

    /// Largest per-channel difference between two colours.
    fn channel_distance(a: Color32, b: Color32) -> i32 {
        (0..3)
            .map(|i| (a.to_array()[i] as i32 - b.to_array()[i] as i32).abs())
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn every_palette_surface_is_opaque() {
        for palette in [DARK, LIGHT] {
            for colour in [
                palette.bg_sunken,
                palette.bg_base,
                palette.bg_surface,
                palette.bg_raised,
                palette.bg_overlay,
            ] {
                assert_eq!(colour.a(), 255);
            }
        }
    }
}
