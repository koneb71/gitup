//! The conflict resolution view.
//!
//! Three columns — theirs, the common ancestor, ours — aligned line by line, so
//! the question "what did each side do to this?" has a visual answer. Taking a
//! whole side is one click; anything finer is done by editing the merged result
//! directly, with the markers still in it, which is what a person familiar with
//! git will reach for anyway.

use super::{icons, metrics, radius, space, text, Palette};
use crate::git::conflict::{Conflict, ConflictKind, Conflicts, Resolution};
use crate::git::diff::LineKind;
use egui::{
    Align, Align2, CornerRadius, FontId, Frame, Layout, Margin, Pos2, Rect, Sense, Stroke, Ui, Vec2,
};

const ROW: f32 = 17.0;
const FILE_ROW: f32 = 24.0;

#[derive(Debug, Default)]
pub struct ConflictResponse {
    pub selected_file: Option<String>,
    pub resolve: Option<(String, Resolution)>,
    /// The user edited the merged text and asked to keep it.
    pub resolve_edited: Option<(String, String)>,
    /// Switch between the three-way view and editing the merged file.
    pub toggle_edit: bool,
}

pub struct ConflictView<'a> {
    pub palette: &'a Palette,
    pub conflicts: &'a Conflicts,
    pub active: Option<&'a str>,
    pub editing: bool,
    /// Scratch buffer for the manual editor, owned by the app.
    pub edit_buffer: &'a mut String,
}

impl ConflictView<'_> {
    pub fn show(&mut self, ui: &mut Ui) -> ConflictResponse {
        let mut out = ConflictResponse::default();
        let p = self.palette;
        ui.set_min_size(ui.available_size());

        if self.conflicts.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.label(text::icon_sized(icons::CHECK_CIRCLE, 24.0).color(p.added));
                ui.add_space(space::MD);
                ui.label(text::subtitle("All conflicts resolved").color(p.text_secondary));
                ui.add_space(space::XS);
                ui.label(text::caption("Commit to finish the merge.").color(p.text_muted));
            });
            return out;
        }

        egui::Panel::left(egui::Id::new("conflict_files"))
            .resizable(true)
            .default_size(230.0)
            .size_range(160.0..=380.0)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(p.bg_base))
            .show(ui, |ui| {
                ui.painter().vline(
                    ui.max_rect().right(),
                    ui.max_rect().y_range(),
                    Stroke::new(1.0, p.border),
                );
                out.selected_file = self.file_list(ui);
            });

        egui::CentralPanel::no_frame()
            .frame(Frame::NONE.fill(p.bg_base))
            .show(ui, |ui| {
                let Some(conflict) = self
                    .active
                    .and_then(|path| self.conflicts.find(path))
                    .or_else(|| self.conflicts.files.first())
                else {
                    return;
                };
                self.toolbar(ui, conflict, &mut out);
                if self.editing {
                    self.editor(ui, conflict, &mut out);
                } else {
                    self.three_way(ui, conflict);
                }
            });

        out
    }

    fn file_list(&self, ui: &mut Ui) -> Option<String> {
        let p = self.palette;
        let mut clicked = None;

        ui.add_space(space::SM);
        ui.horizontal(|ui| {
            ui.add_space(space::LG);
            let n = self.conflicts.files.len();
            ui.label(text::overline(crate::util::words::plural(n, "conflict")).color(p.danger));
        });
        ui.add_space(space::XS);
        ui.spacing_mut().item_spacing.y = 0.0;

        for conflict in &self.conflicts.files {
            let active = self.active == Some(conflict.path.as_str());
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), FILE_ROW), Sense::click());
            if active {
                ui.painter()
                    .rect_filled(rect, CornerRadius::ZERO, p.selected);
                ui.painter().rect_filled(
                    Rect::from_min_size(rect.left_top(), Vec2::new(2.0, rect.height())),
                    CornerRadius::ZERO,
                    p.accent,
                );
            } else if response.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::ZERO, p.hover);
            }

            let painter = ui.painter();
            let cy = rect.center().y;
            painter.text(
                Pos2::new(rect.left() + space::LG, cy),
                Align2::LEFT_CENTER,
                icons::WARNING,
                text::icon_font(11.0),
                p.danger,
            );
            painter.text(
                Pos2::new(rect.left() + space::LG + 16.0, cy),
                Align2::LEFT_CENTER,
                conflict.path.rsplit('/').next().unwrap_or(&conflict.path),
                FontId::new(text::size::BODY, egui::FontFamily::Proportional),
                if active { p.text } else { p.text_secondary },
            );

            if response.on_hover_text(&conflict.path).clicked() {
                clicked = Some(conflict.path.clone());
            }
        }

        clicked
    }

    fn toolbar(&self, ui: &mut Ui, conflict: &Conflict, out: &mut ConflictResponse) {
        let p = self.palette;
        Frame::NONE
            .fill(p.bg_surface)
            .inner_margin(Margin::symmetric(space::LG as i8, space::MD as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = space::SM;
                    ui.label(text::medium(&conflict.path).color(p.text));
                    ui.label(text::caption(conflict.kind.describe()).color(p.text_muted));

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    text::label(if self.editing {
                                        "Compare"
                                    } else {
                                        "Edit merged file"
                                    })
                                    .color(p.text_secondary),
                                )
                                .fill(p.bg_raised)
                                .stroke(Stroke::NONE)
                                .corner_radius(CornerRadius::same(radius::SM))
                                .min_size(Vec2::new(0.0, metrics::BUTTON_COMPACT)),
                            )
                            .clicked()
                        {
                            out.toggle_edit = true;
                        }

                        // Taking a whole side is the common resolution, so the
                        // three buttons are always visible rather than hidden
                        // in a menu.
                        for (resolution, label, colour) in [
                            (Resolution::Both, "Keep both", p.info),
                            (Resolution::Theirs, "Take theirs", p.modified),
                            (Resolution::Ours, "Take ours", p.accent),
                        ] {
                            let enabled = match resolution {
                                Resolution::Both => {
                                    conflict.kind == ConflictKind::BothModified
                                        || conflict.kind == ConflictKind::BothAdded
                                }
                                _ => true,
                            };
                            let button =
                                egui::Button::new(text::label(label).color(p.text_on_accent))
                                    .fill(colour)
                                    .stroke(Stroke::NONE)
                                    .corner_radius(CornerRadius::same(radius::SM))
                                    .min_size(Vec2::new(0.0, metrics::BUTTON_COMPACT));
                            if ui.add_enabled(enabled, button).clicked() {
                                out.resolve = Some((conflict.path.clone(), resolution));
                            }
                        }
                    });
                });
            });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.min_rect().bottom(),
            Stroke::new(1.0, p.border),
        );
    }

    fn three_way(&self, ui: &mut Ui, conflict: &Conflict) {
        let p = self.palette;
        if conflict.kind == ConflictKind::Binary {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.label(
                    text::caption("Binary file — take one side or the other").color(p.text_muted),
                );
            });
            return;
        }

        let rows = crate::git::conflict::align(conflict);
        let mono = FontId::new(text::size::MONO, text::mono_family());

        // Column headers, so it's never ambiguous which side is which.
        Frame::NONE
            .fill(p.bg_base)
            .inner_margin(Margin::symmetric(0, space::XS as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let width = ui.available_width() / 3.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (label, colour) in [
                        ("Theirs (incoming)", p.modified),
                        ("Common ancestor", p.text_muted),
                        ("Ours (current)", p.accent),
                    ] {
                        ui.allocate_ui(Vec2::new(width, 18.0), |ui| {
                            ui.horizontal(|ui| {
                                ui.add_space(space::MD);
                                ui.label(text::overline(label).color(colour));
                            });
                        });
                    }
                });
            });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.min_rect().bottom(),
            Stroke::new(1.0, p.border),
        );

        egui::ScrollArea::vertical()
            .id_salt("three_way")
            .auto_shrink([false, false])
            .show_rows(ui, ROW, rows.len(), |ui, range| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.0;
                let column = ui.available_width() / 3.0;

                for index in range {
                    let row = &rows[index];
                    let (rect, _) = ui
                        .allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::hover());
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    let painter = ui.painter();
                    let cy = rect.center().y;

                    for (slot, (line, kind, tint)) in [
                        (&row.theirs, row.theirs_kind, p.modified),
                        (&row.base, LineKind::Context, p.text_muted),
                        (&row.ours, row.ours_kind, p.accent),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let x = rect.left() + column * slot as f32;
                        let cell = Rect::from_min_size(
                            Pos2::new(x, rect.top()),
                            Vec2::new(column, rect.height()),
                        );

                        match line {
                            Some(text_line) => {
                                if kind == LineKind::Addition {
                                    painter.rect_filled(
                                        cell,
                                        CornerRadius::ZERO,
                                        tint.gamma_multiply(0.14),
                                    );
                                }
                                painter.with_clip_rect(cell).text(
                                    Pos2::new(x + space::MD, cy),
                                    Align2::LEFT_CENTER,
                                    text_line,
                                    mono.clone(),
                                    p.text,
                                );
                            }
                            None => {
                                // An absent line is drawn as a struck-through
                                // band, so "this side removed it" is visible
                                // rather than merely empty.
                                if kind == LineKind::Deletion {
                                    painter.rect_filled(
                                        cell,
                                        CornerRadius::ZERO,
                                        p.removed.gamma_multiply(0.10),
                                    );
                                }
                            }
                        }
                        painter.vline(cell.right(), rect.y_range(), Stroke::new(1.0, p.border));
                    }
                }
            });
    }

    fn editor(&mut self, ui: &mut Ui, conflict: &Conflict, out: &mut ConflictResponse) {
        let p = self.palette;
        if self.edit_buffer.is_empty() {
            *self.edit_buffer = conflict.merged.clone();
        }

        let still_conflicted = crate::git::conflict::has_markers(self.edit_buffer);
        Frame::NONE
            .inner_margin(Margin::same(space::MD as i8))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = space::SM;
                    if still_conflicted {
                        ui.label(text::icon_sized(icons::WARNING, 12.0).color(p.warning));
                        ui.label(
                            text::caption("Conflict markers are still present").color(p.warning),
                        );
                    } else {
                        ui.label(text::icon_sized(icons::CHECK_CIRCLE, 12.0).color(p.added));
                        ui.label(text::caption("No markers left").color(p.added));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let button = egui::Button::new(text::medium("Mark resolved").color(
                            if still_conflicted {
                                p.text_muted
                            } else {
                                p.text_on_accent
                            },
                        ))
                        .fill(if still_conflicted {
                            p.bg_raised
                        } else {
                            p.accent
                        })
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(radius::SM))
                        .min_size(Vec2::new(0.0, 24.0));
                        if ui.add_enabled(!still_conflicted, button).clicked() {
                            out.resolve_edited =
                                Some((conflict.path.clone(), self.edit_buffer.clone()));
                        }
                    });
                });
                ui.add_space(space::SM);
                ui.add_sized(
                    ui.available_size(),
                    egui::TextEdit::multiline(self.edit_buffer)
                        .font(egui::TextStyle::Monospace)
                        .code_editor(),
                );
            });
    }
}
