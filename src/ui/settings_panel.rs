//! The settings and shortcuts sheet.
//!
//! Changes apply as they are made rather than on an OK button: every setting
//! here is instantly reversible, and a preference dialog that makes you confirm
//! a checkbox is asking for a decision that doesn't exist.

use super::{icons, metrics, radius, space, text, Palette, ThemeMode};
use crate::settings::Settings;
use crate::ui::diff::DiffLayout;
use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Frame, Id, Layout, Margin, Pos2, Rect, Stroke,
    Ui, Vec2,
};

/// Bindings that are not remappable, listed so they are still discoverable.
///
/// These are contextual rather than commands: what Escape does depends on what
/// is open, and the arrow keys move whatever list has focus. A binding table
/// cannot express that, so they stay fixed.
fn fixed_shortcuts() -> Vec<(String, &'static str)> {
    use crate::ui::keymap::Chord;
    use egui::Key;

    let cmd = |key| Chord::cmd(key).display();
    let cmd_shift = |key| Chord::cmd_shift(key).display();
    let plain = |key| Chord::plain(key).display();

    vec![
        (
            format!("{}…{}", cmd(Key::Num1), cmd(Key::Num8)),
            "Show that tab",
        ),
        (cmd(Key::Num9), "Show the last tab"),
        (cmd(Key::T), "Open a repository in a new tab"),
        (cmd(Key::W), "Close this tab"),
        (
            format!(
                "{} / {}",
                cmd_shift(Key::OpenBracket),
                cmd_shift(Key::CloseBracket)
            ),
            "Previous / next tab",
        ),
        (
            format!("{} / {}", plain(Key::ArrowUp), plain(Key::ArrowDown)),
            "Move through the list",
        ),
        (
            format!("{} / {}", plain(Key::ArrowRight), plain(Key::ArrowLeft)),
            "Move between history and the changed files",
        ),
        (plain(Key::Space), "Stage or unstage the selected file"),
        (plain(Key::Escape), "Dismiss, or leave the current view"),
    ]
}

#[derive(Debug, Default)]
pub struct SettingsResponse {
    pub close: bool,
    /// The theme changed and needs applying to the context.
    pub theme_changed: bool,
    /// Something changed that invalidates computed diffs.
    pub diffs_invalidated: bool,
    /// Something changed that requires re-reading the repository.
    pub reload: bool,
    pub watcher_changed: bool,
    /// The identity fields were edited and should be written.
    pub save_identity: bool,
    /// The level being edited changed, so the fields need reseeding.
    pub identity_scope_changed: bool,
}

pub struct SettingsSheet<'a> {
    pub palette: &'a Palette,
    pub settings: &'a mut Settings,
    pub git_version: Option<&'a str>,
    /// The action whose binding is being recorded, if any.
    pub recording: &'a mut Option<crate::ui::keymap::Action>,
    /// The identity at each level, once it has been read.
    pub identity: Option<&'a crate::git::identity::Identities>,
    /// The fields being edited, and the level they will be written to.
    pub identity_draft: &'a mut crate::git::identity::Identity,
    pub identity_scope: &'a mut crate::git::identity::Scope,
    /// Whether a repository is open, which is what the repository level needs.
    pub has_repository: bool,
    /// True while the fields differ from what is stored.
    pub identity_dirty: bool,
}

impl SettingsSheet<'_> {
    pub fn show(&mut self, ctx: &egui::Context) -> SettingsResponse {
        let mut out = SettingsResponse::default();
        let p = self.palette;

        let response = egui::Modal::new(Id::new("settings_sheet"))
            .frame(
                Frame::NONE
                    .fill(p.bg_overlay)
                    .stroke(Stroke::new(1.0, p.border_strong))
                    .corner_radius(CornerRadius::same(radius::LG))
                    .inner_margin(Margin::same(space::XL as i8))
                    .shadow(ctx.style_of(ctx.theme()).visuals.window_shadow),
            )
            .show(ctx, |ui| {
                ui.set_width(480.0);

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = space::MD;
                    ui.label(text::icon_sized(icons::GEAR, 16.0).color(p.accent));
                    ui.label(text::title("Settings").color(p.text));
                });
                ui.add_space(space::LG);

                // The sections scroll; the title above and the Done button
                // below stay put. Without this the sheet simply grew past the
                // window and was clipped at both ends — a modal has no
                // scrollbar of its own, so the parts that did not fit were not
                // reachable at all.
                let chrome = space::XL * 2.0 + metrics::BUTTON_ACTION + 64.0;
                let screen = ui.ctx().content_rect().height();
                egui::ScrollArea::vertical()
                    .max_height((screen - chrome).max(200.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        self.identity(ui, &mut out);
                        ui.add_space(space::LG);
                        self.appearance(ui, &mut out);
                        ui.add_space(space::LG);
                        self.behaviour(ui, &mut out);
                        ui.add_space(space::LG);
                        self.shortcuts(ui);

                        ui.add_space(space::LG);
                        self.about(ui);
                    });

                ui.add_space(space::LG);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let done = egui::Button::new(text::medium("Done").color(p.text_on_accent))
                        .fill(p.accent)
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(radius::MD))
                        .min_size(Vec2::new(88.0, metrics::BUTTON_ACTION));
                    if ui.add(done).clicked() {
                        out.close = true;
                    }
                });
            });

        if response.should_close() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            out.close = true;
        }
        out
    }

    fn section(&self, ui: &mut Ui, title: &str) {
        ui.label(text::overline(title).color(self.palette.text_muted));
        ui.add_space(space::SM);
    }

    fn appearance(&mut self, ui: &mut Ui, out: &mut SettingsResponse) {
        let p = self.palette;
        self.section(ui, "Appearance");

        ui.horizontal(|ui| {
            ui.label(text::body("Theme").color(p.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                for mode in [ThemeMode::Light, ThemeMode::Dark] {
                    let active = self.settings.theme == mode;
                    let button = egui::Button::new(text::label(mode.label()).color(if active {
                        p.text_on_accent
                    } else {
                        p.text_secondary
                    }))
                    .fill(if active { p.accent } else { p.bg_raised })
                    .stroke(Stroke::NONE)
                    .corner_radius(CornerRadius::same(radius::SM))
                    .min_size(Vec2::new(64.0, 22.0));
                    if ui.add(button).clicked() && !active {
                        self.settings.theme = mode;
                        out.theme_changed = true;
                    }
                }
            });
        });
        ui.add_space(space::SM);

        ui.horizontal(|ui| {
            ui.label(text::body("Diff layout").color(p.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                for layout in [DiffLayout::SideBySide, DiffLayout::Unified] {
                    let active = self.settings.diff_layout == layout;
                    let button = egui::Button::new(text::label(layout.label()).color(if active {
                        p.text_on_accent
                    } else {
                        p.text_secondary
                    }))
                    .fill(if active { p.accent } else { p.bg_raised })
                    .stroke(Stroke::NONE)
                    .corner_radius(CornerRadius::same(radius::SM))
                    .min_size(Vec2::new(84.0, metrics::BUTTON_COMPACT));
                    if ui.add(button).clicked() {
                        self.settings.diff_layout = layout;
                    }
                }
            });
        });
        ui.add_space(space::SM);

        if self.toggle(
            ui,
            "Syntax highlighting",
            "Colour diffs by language",
            self.settings.syntax_highlighting,
        ) {
            self.settings.syntax_highlighting = !self.settings.syntax_highlighting;
            out.diffs_invalidated = true;
        }
    }

    /// Who commits are authored as, at the global and repository levels.
    ///
    /// The effective identity is stated first and separately from the fields.
    /// Git resolves `user.name` through a chain of config files, so a screen
    /// that only showed the field being edited would let someone change it,
    /// see the change stick, and still have commits come out under a different
    /// name because the repository was overriding it.
    fn identity(&mut self, ui: &mut Ui, out: &mut SettingsResponse) {
        use crate::git::identity::Scope;

        self.section(ui, "Identity");
        let p = self.palette;

        let Some(identities) = self.identity else {
            ui.label(text::caption("Reading…").color(p.text_muted));
            return;
        };

        // The fact, before the controls.
        if identities.can_commit() {
            ui.horizontal(|ui| {
                ui.label(text::caption("Commits are authored as").color(p.text_muted));
                ui.label(text::label(identities.effective.display()).color(p.text));
            });
        } else {
            ui.label(
                text::caption("Git will refuse to commit until a name and email are set.")
                    .color(p.warning),
            );
        }
        ui.add_space(space::MD);

        ui.horizontal(|ui| {
            ui.label(text::body("Applies to").color(p.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                for scope in [Scope::Repository, Scope::Global] {
                    let active = *self.identity_scope == scope;
                    // Editing a repository's config needs a repository.
                    let enabled = scope == Scope::Global || self.has_repository;
                    let label = match scope {
                        Scope::Global => "Every repository",
                        Scope::Repository => "This repository",
                    };
                    let button = egui::Button::new(text::label(label).color(if active {
                        p.text_on_accent
                    } else if enabled {
                        p.text_secondary
                    } else {
                        p.text_muted
                    }))
                    .fill(if active { p.accent } else { p.bg_raised })
                    .stroke(Stroke::NONE)
                    .corner_radius(CornerRadius::same(radius::SM))
                    .min_size(Vec2::new(120.0, metrics::BUTTON_COMPACT));

                    let response = ui.add_enabled(enabled, button);
                    let response = if enabled {
                        response
                    } else {
                        response
                            .on_disabled_hover_text("Open a repository to give it its own identity")
                    };
                    if response.clicked() && !active {
                        *self.identity_scope = scope;
                        out.identity_scope_changed = true;
                    }
                }
            });
        });
        ui.add_space(space::SM);

        // Under the repository level the global values are what a blank field
        // falls back to, so showing them as the hint text makes "leave it
        // empty" a visible choice rather than something to be explained.
        let inherited = matches!(*self.identity_scope, Scope::Repository);
        self.identity_field(ui, "Name", inherited.then_some(&identities.global.name));
        ui.add_space(space::SM);
        self.identity_field(ui, "Email", inherited.then_some(&identities.global.email));
        ui.add_space(space::SM);

        ui.horizontal(|ui| {
            let hint = if !self.identity_draft.email.trim().is_empty()
                && !self.identity_draft.email.contains('@')
            {
                // A warning, not a rule: git does not validate addresses, and
                // some setups legitimately use something that is not one.
                (
                    "That does not look like an email address.".to_owned(),
                    p.warning,
                )
            } else {
                let text = match *self.identity_scope {
                    Scope::Global => {
                        "Used by every repository that does not set its own.".to_owned()
                    }
                    Scope::Repository if identities.is_overridden() => {
                        "Overrides your global identity, here only.".to_owned()
                    }
                    Scope::Repository => "Leave blank to use your global identity.".to_owned(),
                };
                (text, p.text_muted)
            };
            ui.label(text::caption(hint.0).color(hint.1));

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let save = egui::Button::new(text::label("Save").color(if self.identity_dirty {
                    p.text_on_accent
                } else {
                    p.text_muted
                }))
                .fill(if self.identity_dirty {
                    p.accent
                } else {
                    p.bg_raised
                })
                .stroke(Stroke::NONE)
                .corner_radius(CornerRadius::same(radius::SM))
                .min_size(Vec2::new(72.0, metrics::BUTTON_COMPACT));
                let response = ui.add_enabled(self.identity_dirty, save);
                if response.clicked() {
                    out.save_identity = true;
                }
            });
        });
    }

    /// One labelled text field, with the inherited value as its hint.
    fn identity_field(&mut self, ui: &mut Ui, label: &str, inherited: Option<&String>) {
        let p = self.palette;
        ui.horizontal(|ui| {
            ui.add_sized(
                Vec2::new(52.0, metrics::BUTTON_COMPACT),
                egui::Label::new(text::body(label).color(p.text_secondary)),
            );
            let value = match label {
                "Name" => &mut self.identity_draft.name,
                _ => &mut self.identity_draft.email,
            };
            let hint = inherited
                .filter(|v| !v.trim().is_empty())
                .cloned()
                .unwrap_or_default();
            ui.add_sized(
                Vec2::new(ui.available_width(), metrics::BUTTON_ACTION),
                egui::TextEdit::singleline(value).hint_text(hint),
            );
        });
    }

    fn behaviour(&mut self, ui: &mut Ui, out: &mut SettingsResponse) {
        self.section(ui, "Behaviour");

        if self.toggle(
            ui,
            "Watch for changes",
            "Refresh when files change outside the app",
            self.settings.auto_refresh,
        ) {
            self.settings.auto_refresh = !self.settings.auto_refresh;
            out.watcher_changed = true;
        }
        ui.add_space(space::SM);

        if self.toggle(
            ui,
            "Show ignored files",
            "Include files matched by .gitignore",
            self.settings.show_ignored,
        ) {
            self.settings.show_ignored = !self.settings.show_ignored;
            out.reload = true;
        }
    }

    /// A labelled switch. Returns true when clicked.
    fn toggle(&self, ui: &mut Ui, title: &str, detail: &str, on: bool) -> bool {
        let p = self.palette;
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), egui::Sense::click());
        if response.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(radius::SM), p.hover);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let painter = ui.painter();
        painter.text(
            Pos2::new(rect.left() + space::SM, rect.center().y - 7.0),
            Align2::LEFT_CENTER,
            title,
            FontId::new(text::size::BODY, egui::FontFamily::Proportional),
            p.text,
        );
        painter.text(
            Pos2::new(rect.left() + space::SM, rect.center().y + 8.0),
            Align2::LEFT_CENTER,
            detail,
            FontId::new(text::size::CAPTION, egui::FontFamily::Proportional),
            p.text_muted,
        );

        // The switch itself: a pill with a knob that slides.
        let track = egui::Rect::from_center_size(
            Pos2::new(rect.right() - 24.0, rect.center().y),
            Vec2::new(34.0, 18.0),
        );
        painter.rect_filled(
            track,
            CornerRadius::same(radius::PILL),
            if on { p.accent } else { p.bg_raised },
        );
        let knob_x = if on {
            track.right() - 9.0
        } else {
            track.left() + 9.0
        };
        painter.circle_filled(
            Pos2::new(knob_x, track.center().y),
            7.0,
            if on { p.text_on_accent } else { p.text_muted },
        );

        response.clicked()
    }

    fn shortcuts(&mut self, ui: &mut Ui) {
        use crate::ui::keymap::Action;
        let p = self.palette;

        ui.horizontal(|ui| {
            ui.label(text::overline("Keyboard").color(p.text_muted));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(text::caption("Reset").color(p.text_secondary))
                            .fill(p.bg_raised)
                            .stroke(Stroke::NONE)
                            .corner_radius(CornerRadius::same(radius::SM))
                            .min_size(Vec2::new(0.0, metrics::BUTTON_COMPACT)),
                    )
                    .on_hover_text("Restore the default bindings")
                    .clicked()
                {
                    self.settings.keymap.reset();
                    *self.recording = None;
                }
            });
        });
        ui.add_space(space::SM);

        if let Some(action) = *self.recording {
            // Capture the next chord. Escape cancels rather than binding
            // itself: it is the one key that has to keep meaning "never mind".
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                *self.recording = None;
            } else if let Some(chord) = capture_chord(ui.ctx()) {
                self.settings.keymap.set(action, chord);
                *self.recording = None;
            }
        }

        for action in Action::all() {
            let recording = *self.recording == Some(action);
            let chord = self.settings.keymap.chord(action);
            let conflicts = self.settings.keymap.conflicting(action);

            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), egui::Sense::click());
            if response.hovered() && !recording {
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(radius::SM), p.hover);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }

            let painter = ui.painter();
            let cy = rect.center().y;
            painter.text(
                Pos2::new(rect.left() + space::SM, cy),
                Align2::LEFT_CENTER,
                action.label(),
                FontId::new(text::size::BODY, egui::FontFamily::Proportional),
                p.text,
            );

            // The chord sits in a pill on the right, which doubles as the
            // target you click to change it.
            let label = match (recording, chord) {
                (true, _) => "press a key…".to_owned(),
                (_, Some(chord)) => chord.display(),
                (_, None) => "unbound".to_owned(),
            };
            let (fill, colour) = if recording {
                (p.accent, p.text_on_accent)
            } else if !conflicts.is_empty() {
                (p.tinted(p.danger, 0.25), p.danger)
            } else if chord.is_some() {
                (p.bg_raised, p.accent)
            } else {
                (p.bg_raised, p.text_muted)
            };

            let galley = painter.layout_no_wrap(
                label,
                FontId::new(text::size::LABEL, text::mono_family()),
                colour,
            );
            let pill = Rect::from_min_size(
                Pos2::new(rect.right() - galley.size().x - 16.0, cy - 10.0),
                Vec2::new(galley.size().x + 16.0, 20.0),
            );
            painter.rect_filled(pill, CornerRadius::same(radius::SM), fill);
            painter.galley(
                Pos2::new(pill.left() + 8.0, cy - galley.size().y / 2.0),
                galley,
                colour,
            );

            let hint = if conflicts.is_empty() {
                "Click to change".to_owned()
            } else {
                format!(
                    "Also bound to {}",
                    conflicts
                        .iter()
                        .map(|a| a.label())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            if response.on_hover_text(hint).clicked() {
                *self.recording = Some(action);
            }
        }

        ui.add_space(space::MD);
        egui::Grid::new("fixed_shortcuts")
            .num_columns(2)
            .spacing(Vec2::new(space::XL, space::XS))
            .show(ui, |ui| {
                for (keys, description) in fixed_shortcuts() {
                    ui.label(
                        egui::RichText::new(keys)
                            .font(FontId::new(text::size::LABEL, text::mono_family()))
                            .color(p.text_muted),
                    );
                    ui.label(text::caption(description).color(p.text_muted));
                    ui.end_row();
                }
            });
    }

    fn about(&self, ui: &mut Ui) {
        let p = self.palette;
        self.section(ui, "About");
        ui.label(
            text::caption(format!("Gitup {}", env!("CARGO_PKG_VERSION"))).color(p.text_secondary),
        );
        if let Some(version) = self.git_version {
            ui.label(text::caption(version).color(p.text_muted));
        }
        // The repository path deliberately isn't repeated here — it already
        // lives in the toolbar's tooltip, where it is next to the name it
        // belongs to.
    }
}

/// Read a chord from the current frame's input.
///
/// egui reports modifiers separately from keys, so holding shift on its own
/// produces no `Key` and nothing is captured until a real key follows — which
/// is what makes "press a key…" behave the way people expect.
fn capture_chord(ctx: &egui::Context) -> Option<crate::ui::keymap::Chord> {
    use crate::ui::keymap::Chord;
    ctx.input(|i| {
        let modifiers = i.modifiers;
        let key = egui::Key::ALL
            .iter()
            .copied()
            .find(|key| i.key_pressed(*key))?;
        Some(Chord {
            command: modifiers.command,
            shift: modifiers.shift,
            alt: modifiers.alt,
            key,
        })
    })
}

/// Unused; keeps `Color32` in scope for future states.
#[allow(dead_code)]
fn _color_in_scope(_: Color32) {}

#[cfg(test)]
mod tests {
    use super::fixed_shortcuts;

    #[test]
    fn the_fixed_shortcuts_are_filled_in() {
        for (keys, description) in fixed_shortcuts() {
            assert!(!keys.trim().is_empty(), "empty chord for {description:?}");
            assert!(!description.trim().is_empty(), "no description for {keys}");
        }
    }

    #[test]
    fn fixed_shortcuts_do_not_duplicate_bindable_ones() {
        // Anything listed as fixed must not also be remappable, or the sheet
        // would show the same key twice with different meanings.
        let keymap = crate::ui::keymap::Keymap::default();
        let bound: Vec<String> = crate::ui::keymap::Action::all()
            .into_iter()
            .filter_map(|a| keymap.chord(a))
            .map(|c| c.display())
            .collect();
        for (keys, _) in fixed_shortcuts() {
            assert!(
                !bound.iter().any(|b| b == &keys),
                "{keys} is both fixed and bindable"
            );
        }
    }
}
