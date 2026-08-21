//! Modal dialogs.
//!
//! Deliberately few, and only where a decision genuinely needs more than one
//! field. Everything reachable from a single click stays a single click; these
//! exist for the cases with real parameters — a branch needs a name and a start
//! point, a push needs a remote and a decision about upstream.

use super::{icons, metrics, radius, space, text, Palette};
use egui::{Align, Color32, CornerRadius, Frame, Id, Layout, Margin, Stroke, Ui, Vec2};
use git2::Oid;

/// Which dialog is open, along with its in-progress input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    CreateBranch {
        name: String,
        /// Revision to branch from; `None` means HEAD.
        start_point: Option<String>,
        start_label: String,
        checkout: bool,
    },
    RenameBranch {
        old: String,
        name: String,
    },
    DeleteBranch {
        name: String,
        /// Set once the first attempt reported unmerged commits.
        force: bool,
        warning: Option<String>,
    },
    CreateTag {
        name: String,
        message: String,
        target: Oid,
        target_label: String,
    },
    DeleteTag {
        name: String,
    },
    StashSave {
        message: String,
        include_untracked: bool,
    },
    Push {
        remote: String,
        branch: String,
        set_upstream: bool,
        force: bool,
    },
    AddRemote {
        name: String,
        url: String,
    },
    AddSubmodule {
        url: String,
        path: String,
    },
    Clone {
        url: String,
        parent: std::path::PathBuf,
        name: String,
    },
    DiscardConfirm {
        paths: Vec<String>,
        untracked: bool,
    },
    Reset {
        target: Oid,
        target_label: String,
        kind: crate::git::merge::ResetKind,
        /// Set when there is uncommitted work a hard reset would destroy.
        dirty: usize,
    },
    RebasePlan {
        plan: Box<crate::git::rebase::RebasePlan>,
        original: Vec<Oid>,
    },
    /// Choosing which parent of a merge to apply or undo against.
    Mainline {
        oid: Oid,
        short_id: String,
        summary: String,
        /// `(parent number, short id, summary)`.
        parents: Vec<(u32, String, String)>,
        chosen: u32,
        /// True for cherry-pick, false for revert.
        picking: bool,
    },
}

/// What the user chose.
#[derive(Debug, Clone)]
pub enum DialogAction {
    Cancel,
    Confirm(Dialog),
    /// Open a folder picker for the clone destination.
    BrowseCloneParent,
}

impl Dialog {
    fn title(&self) -> &'static str {
        match self {
            Self::CreateBranch { .. } => "New branch",
            Self::RenameBranch { .. } => "Rename branch",
            Self::DeleteBranch { .. } => "Delete branch",
            Self::CreateTag { .. } => "New tag",
            Self::DeleteTag { .. } => "Delete tag",
            Self::StashSave { .. } => "Stash changes",
            Self::Push { .. } => "Push",
            Self::AddRemote { .. } => "Add remote",
            Self::AddSubmodule { .. } => "Add submodule",
            Self::Clone { .. } => "Clone repository",
            Self::DiscardConfirm { .. } => "Discard changes",
            Self::Reset { .. } => "Reset branch",
            Self::RebasePlan { .. } => "Rebase",
            Self::Mainline { picking: true, .. } => "Cherry-pick a merge",
            Self::Mainline { .. } => "Revert a merge",
        }
    }

    fn confirm_label(&self) -> &'static str {
        match self {
            Self::CreateBranch { .. } => "Create",
            Self::RenameBranch { .. } => "Rename",
            Self::DeleteBranch { .. } | Self::DeleteTag { .. } => "Delete",
            Self::CreateTag { .. } => "Create tag",
            Self::StashSave { .. } => "Stash",
            Self::Push { .. } => "Push",
            Self::AddRemote { .. } => "Add",
            Self::AddSubmodule { .. } => "Add",
            Self::Clone { .. } => "Clone",
            Self::DiscardConfirm { .. } => "Discard",
            Self::Reset { .. } => "Reset",
            Self::RebasePlan { .. } => "Run rebase",
            Self::Mainline { picking: true, .. } => "Cherry-pick",
            Self::Mainline { .. } => "Revert",
        }
    }

    /// Destructive actions get a red confirm button; nothing else does.
    fn is_destructive(&self) -> bool {
        matches!(
            self,
            Self::DeleteBranch { .. } | Self::DeleteTag { .. } | Self::DiscardConfirm { .. }
        ) || matches!(
            self,
            Self::Reset {
                kind: crate::git::merge::ResetKind::Hard,
                ..
            }
        )
    }

    /// Why the confirm button is disabled, if it is.
    fn blocker(&self) -> Option<&'static str> {
        match self {
            Self::CreateBranch { name, .. }
            | Self::RenameBranch { name, .. }
            | Self::CreateTag { name, .. } => {
                name.trim().is_empty().then_some("A name is required")
            }
            Self::AddRemote { name, url } => {
                if name.trim().is_empty() {
                    Some("A name is required")
                } else if url.trim().is_empty() {
                    Some("A URL is required")
                } else {
                    None
                }
            }
            Self::AddSubmodule { url, path } => {
                if url.trim().is_empty() {
                    Some("A URL is required")
                } else if path.trim().is_empty() {
                    Some("A path is required")
                } else {
                    None
                }
            }
            Self::Clone { url, name, .. } => {
                if url.trim().is_empty() {
                    Some("A URL is required")
                } else if name.trim().is_empty() {
                    Some("A folder name is required")
                } else {
                    None
                }
            }
            Self::Push { remote, branch, .. } => (remote.trim().is_empty()
                || branch.trim().is_empty())
            .then_some("Choose a remote and a branch"),
            Self::RebasePlan { plan, original } => plan.first_problem().or_else(|| {
                plan.is_noop(original)
                    .then_some("This plan changes nothing")
            }),
            Self::Mainline { chosen, .. } => {
                (*chosen == 0).then_some("Choose which parent to apply against")
            }
            _ => None,
        }
    }
}

/// Draw the open dialog, returning the user's choice when they make one.
pub fn show(
    ctx: &egui::Context,
    palette: &Palette,
    dialog: &mut Dialog,
    remotes: &[String],
) -> Option<DialogAction> {
    let p = palette;
    let mut action = None;

    let response = egui::Modal::new(Id::new("gitup_dialog"))
        .frame(
            Frame::NONE
                .fill(p.bg_overlay)
                .stroke(Stroke::new(1.0, p.border_strong))
                .corner_radius(CornerRadius::same(radius::LG))
                .inner_margin(Margin::same(space::XL as i8))
                .shadow(ctx.style_of(ctx.theme()).visuals.window_shadow),
        )
        .show(ctx, |ui| {
            ui.set_width(if matches!(dialog, Dialog::RebasePlan { .. }) {
                600.0
            } else {
                460.0
            });

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = space::MD;
                ui.label(text::icon_sized(icon_for(dialog), 16.0).color(
                    if dialog.is_destructive() {
                        p.danger
                    } else {
                        p.accent
                    },
                ));
                ui.label(text::title(dialog.title()).color(p.text));
            });
            ui.add_space(space::LG);

            body(ui, p, dialog, remotes);

            ui.add_space(space::XL);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = space::MD;

                let blocker = dialog.blocker();
                let confirm_fill = if dialog.is_destructive() {
                    p.danger
                } else {
                    p.accent
                };
                let confirm = egui::Button::new(text::medium(dialog.confirm_label()).color(
                    if blocker.is_some() {
                        p.text_muted
                    } else {
                        p.text_on_accent
                    },
                ))
                .fill(if blocker.is_some() {
                    p.bg_raised
                } else {
                    confirm_fill
                })
                .stroke(Stroke::NONE)
                .corner_radius(CornerRadius::same(radius::MD))
                .min_size(Vec2::new(96.0, 28.0));

                let response = ui.add_enabled(blocker.is_none(), confirm);
                let response = match blocker {
                    Some(reason) => response.on_disabled_hover_text(reason),
                    None => response,
                };
                if response.clicked() {
                    action = Some(DialogAction::Confirm(dialog.clone()));
                }

                let cancel = egui::Button::new(text::body("Cancel").color(p.text_secondary))
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::new(1.0, p.border_strong))
                    .corner_radius(CornerRadius::same(radius::MD))
                    .min_size(Vec2::new(80.0, metrics::BUTTON_ACTION));
                if ui.add(cancel).clicked() {
                    action = Some(DialogAction::Cancel);
                }

                if let Dialog::Clone { .. } = dialog {
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    text::body("Choose folder…").color(p.text_secondary),
                                )
                                .fill(Color32::TRANSPARENT)
                                .stroke(Stroke::new(1.0, p.border))
                                .corner_radius(CornerRadius::same(radius::MD)),
                            )
                            .clicked()
                        {
                            action = Some(DialogAction::BrowseCloneParent);
                        }
                    });
                }
            });
        });

    // Escape, or a click on the backdrop, cancels — the same as every other
    // dialog on the system.
    if action.is_none()
        && (response.should_close() || ctx.input(|i| i.key_pressed(egui::Key::Escape)))
    {
        action = Some(DialogAction::Cancel);
    }
    action
}

fn icon_for(dialog: &Dialog) -> &'static str {
    match dialog {
        Dialog::CreateBranch { .. } | Dialog::RenameBranch { .. } => icons::GIT_BRANCH,
        Dialog::DeleteBranch { .. } => icons::TRASH,
        Dialog::CreateTag { .. } | Dialog::DeleteTag { .. } => icons::TAG,
        Dialog::StashSave { .. } => icons::ARCHIVE,
        Dialog::Push { .. } => icons::CLOUD_ARROW_UP,
        Dialog::AddRemote { .. } => icons::CLOUD,
        Dialog::AddSubmodule { .. } => icons::PACKAGE,
        Dialog::Clone { .. } => icons::DOWNLOAD_SIMPLE,
        Dialog::DiscardConfirm { .. } => icons::WARNING,
        Dialog::Reset { .. } => icons::ARROW_U_UP_LEFT,
        Dialog::RebasePlan { .. } => icons::STACK,
        Dialog::Mainline { .. } => icons::GIT_MERGE,
    }
}

fn body(ui: &mut Ui, p: &Palette, dialog: &mut Dialog, remotes: &[String]) {
    match dialog {
        Dialog::CreateBranch {
            name,
            start_label,
            checkout,
            ..
        } => {
            field(ui, p, "Name", name, "feature/parser");
            ui.add_space(space::MD);
            note(ui, p, &format!("Starting from {start_label}"));
            ui.add_space(space::MD);
            ui.checkbox(checkout, text::body("Switch to it now").color(p.text));
        }

        Dialog::RenameBranch { old, name } => {
            note(ui, p, &format!("Renaming ‘{old}’"));
            ui.add_space(space::MD);
            field(ui, p, "New name", name, "");
        }

        Dialog::DeleteBranch { name, warning, .. } => {
            ui.label(text::body(format!("Delete the branch ‘{name}’?")).color(p.text));
            if let Some(warning) = warning {
                ui.add_space(space::MD);
                callout(ui, p, warning);
            }
        }

        Dialog::DeleteTag { name } => {
            ui.label(text::body(format!("Delete the tag ‘{name}’?")).color(p.text));
            ui.add_space(space::SM);
            note(ui, p, "This only removes it locally.");
        }

        Dialog::CreateTag {
            name,
            message,
            target_label,
            ..
        } => {
            field(ui, p, "Tag name", name, "v1.0.0");
            ui.add_space(space::MD);
            note(ui, p, &format!("Tagging {target_label}"));
            ui.add_space(space::MD);
            ui.label(text::overline("Message").color(p.text_muted));
            ui.add_space(space::XS);
            ui.add(
                egui::TextEdit::multiline(message)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .hint_text("Leave empty for a lightweight tag"),
            );
        }

        Dialog::StashSave {
            message,
            include_untracked,
        } => {
            field(ui, p, "Description", message, "optional");
            ui.add_space(space::MD);
            ui.checkbox(
                include_untracked,
                text::body("Include untracked files").color(p.text),
            );
        }

        Dialog::Push {
            remote,
            branch,
            set_upstream,
            force,
        } => {
            ui.horizontal(|ui| {
                ui.label(text::overline("Remote").color(p.text_muted));
            });
            ui.add_space(space::XS);
            egui::ComboBox::from_id_salt("push_remote")
                .selected_text(text::body(remote.as_str()).color(p.text))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for candidate in remotes {
                        ui.selectable_value(remote, candidate.clone(), candidate);
                    }
                });
            ui.add_space(space::MD);
            field(ui, p, "Branch", branch, "");
            ui.add_space(space::MD);
            ui.checkbox(
                set_upstream,
                text::body("Track this branch on the remote").color(p.text),
            );
            ui.add_space(space::XS);
            ui.checkbox(force, text::body("Force (with lease)").color(p.text))
                .on_hover_text(
                    "Refuses if the remote moved since you last fetched, \
                     so it can't overwrite someone else's work",
                );
        }

        Dialog::AddRemote { name, url } => {
            field(ui, p, "Name", name, "origin");
            ui.add_space(space::MD);
            field(ui, p, "URL", url, "git@github.com:user/repo.git");
        }

        Dialog::AddSubmodule { url, path } => {
            let before = url.clone();
            field(
                ui,
                p,
                "Repository URL",
                url,
                "https://github.com/user/lib.git",
            );
            // Suggest a path from the URL until the user types their own.
            if *url != before && path.trim().is_empty() {
                *path = crate::git::remote::default_clone_name(url);
            }
            ui.add_space(space::MD);
            field(ui, p, "Path in this repository", path, "vendor/lib");
        }

        Dialog::Clone { url, parent, name } => {
            let before = url.clone();
            field(
                ui,
                p,
                "Repository URL",
                url,
                "https://github.com/user/repo.git",
            );
            // Keep the folder name in step with the URL until the user edits it.
            if *url != before {
                *name = crate::git::remote::default_clone_name(url);
            }
            ui.add_space(space::MD);
            note(ui, p, &format!("Into {}", parent.display()));
            ui.add_space(space::MD);
            field(ui, p, "Folder name", name, "repo");
        }

        Dialog::Reset {
            target_label,
            kind,
            dirty,
            ..
        } => {
            note(ui, p, &format!("Move the current branch to {target_label}"));
            ui.add_space(space::MD);
            for candidate in [
                crate::git::merge::ResetKind::Soft,
                crate::git::merge::ResetKind::Mixed,
                crate::git::merge::ResetKind::Hard,
            ] {
                let selected = *kind == candidate;
                if ui
                    .radio(selected, text::body(candidate.label()).color(p.text))
                    .clicked()
                {
                    *kind = candidate;
                }
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    ui.label(text::caption(candidate.description()).color(p.text_muted));
                });
                ui.add_space(space::XS);
            }
            if *kind == crate::git::merge::ResetKind::Hard && *dirty > 0 {
                ui.add_space(space::SM);
                callout(
                    ui,
                    p,
                    &format!(
                        "{} will be destroyed. This cannot be undone.",
                        crate::util::words::plural(*dirty, "uncommitted change")
                    ),
                );
            }
        }

        Dialog::RebasePlan { plan, .. } => {
            note(
                ui,
                p,
                "Oldest first. Squash and fixup fold into the commit above.",
            );
            ui.add_space(space::MD);
            rebase_steps(ui, p, plan);
        }

        Dialog::Mainline {
            short_id,
            summary,
            parents,
            chosen,
            picking,
            ..
        } => {
            ui.label(text::body(summary.as_str()).color(p.text));
            ui.add_space(space::XS);
            note(ui, p, &format!("Merge commit {short_id}"));
            ui.add_space(space::MD);
            note(
                ui,
                p,
                if *picking {
                    "A merge has no single set of changes — one per parent. Pick the side                      the changes should be measured against; usually the first."
                } else {
                    "A merge has no single set of changes — one per parent. Pick the side                      the branch stayed on; usually the first."
                },
            );
            ui.add_space(space::MD);

            for (number, parent_short, parent_summary) in parents.iter() {
                let selected = *chosen == *number;
                if ui
                    .radio(
                        selected,
                        text::body(format!("Parent {number}  ·  {parent_short}")).color(p.text),
                    )
                    .clicked()
                {
                    *chosen = *number;
                }
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    ui.label(text::caption(parent_summary.as_str()).color(p.text_muted));
                });
                ui.add_space(space::XS);
            }
        }

        Dialog::DiscardConfirm { paths, untracked } => {
            let count = paths.len();
            ui.label(
                text::body(if count == 1 {
                    format!("Discard changes to {}?", paths[0])
                } else {
                    format!(
                        "Discard changes to {}?",
                        crate::util::words::plural(count, "file")
                    )
                })
                .color(p.text),
            );
            ui.add_space(space::MD);
            callout(
                ui,
                p,
                if *untracked {
                    "These files aren't tracked, so this deletes them. It cannot be undone."
                } else {
                    "This cannot be undone."
                },
            );
        }
    }
}

/// The interactive-rebase step list: one row per commit, with an action, a drag
/// handle, and buttons to nudge it.
///
/// Both ways of reordering are offered on purpose. Dragging is the natural
/// gesture for moving a commit several places; the buttons are precise, work
/// without a pointer, and remain usable when the list is long enough to scroll
/// while dragging.
fn rebase_steps(ui: &mut Ui, p: &Palette, plan: &mut crate::git::rebase::RebasePlan) {
    use crate::git::rebase::StepAction;

    let mut move_up: Option<usize> = None;
    let mut move_down: Option<usize> = None;
    // A completed drag, as (source index, index to insert before).
    let mut dropped: Option<(usize, usize)> = None;
    let count = plan.steps.len();
    let dragging = egui::DragAndDrop::has_payload_of_type::<usize>(ui.ctx());

    egui::ScrollArea::vertical()
        .max_height(300.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, step) in plan.steps.iter_mut().enumerate() {
                let dropped_step = step.action == StepAction::Drop;
                let row = Frame::NONE
                    .fill(if dropped_step {
                        p.bg_base
                    } else {
                        p.bg_surface
                    })
                    .corner_radius(CornerRadius::same(radius::SM))
                    .inner_margin(Margin::symmetric(space::MD as i8, space::SM as i8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = space::SM;

                            // Only the grip is a drag source; the row is full of
                            // controls that would otherwise swallow the gesture.
                            let handle = ui
                                .dnd_drag_source(Id::new(("rebase_grip", index)), index, |ui| {
                                    ui.label(
                                        text::icon_sized(icons::DOTS_SIX_VERTICAL, 13.0)
                                            .color(p.text_muted),
                                    );
                                })
                                .response;
                            if handle.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                            }
                            handle.on_hover_text("Drag to reorder");

                            egui::ComboBox::from_id_salt(("rebase_action", index))
                                .selected_text(text::caption(step.action.label()).color(
                                    match step.action {
                                        StepAction::Drop => p.danger,
                                        StepAction::Pick => p.text_secondary,
                                        _ => p.accent,
                                    },
                                ))
                                .width(84.0)
                                .show_ui(ui, |ui| {
                                    for candidate in StepAction::all() {
                                        ui.selectable_value(
                                            &mut step.action,
                                            candidate,
                                            candidate.label(),
                                        )
                                        .on_hover_text(candidate.describe());
                                    }
                                });

                            ui.label(text::hash(&step.short_id).color(p.text_muted));
                            ui.label(text::caption(&step.summary).color(if dropped_step {
                                p.text_muted
                            } else {
                                p.text
                            }));

                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add_enabled(
                                        index + 1 < count,
                                        egui::Button::new(
                                            text::icon_sized(icons::CARET_DOWN, 10.0)
                                                .color(p.text_muted),
                                        )
                                        .fill(Color32::TRANSPARENT)
                                        .stroke(Stroke::NONE)
                                        .min_size(Vec2::new(18.0, 18.0)),
                                    )
                                    .clicked()
                                {
                                    move_down = Some(index);
                                }
                                if ui
                                    .add_enabled(
                                        index > 0,
                                        egui::Button::new(
                                            text::icon_sized(icons::CARET_UP, 10.0)
                                                .color(p.text_muted),
                                        )
                                        .fill(Color32::TRANSPARENT)
                                        .stroke(Stroke::NONE)
                                        .min_size(Vec2::new(18.0, 18.0)),
                                    )
                                    .clicked()
                                {
                                    move_up = Some(index);
                                }
                            });
                        });

                        if step.action == StepAction::Reword {
                            ui.add_space(space::XS);
                            ui.add(
                                egui::TextEdit::singleline(&mut step.message)
                                    .desired_width(f32::INFINITY)
                                    .hint_text(step.summary.clone()),
                            );
                        }
                    })
                    .response;

                // Where would a drop land? Above this row or below it, decided
                // by which half of the row the pointer is over — and shown as a
                // line, so the answer is visible before releasing.
                if dragging {
                    if let (Some(pointer), Some(_)) = (
                        ui.input(|i| i.pointer.interact_pos()),
                        row.dnd_hover_payload::<usize>(),
                    ) {
                        let rect = row.rect;
                        let stroke = Stroke::new(2.0, p.accent);
                        let target = if pointer.y < rect.center().y {
                            ui.painter().hline(rect.x_range(), rect.top(), stroke);
                            index
                        } else {
                            ui.painter().hline(rect.x_range(), rect.bottom(), stroke);
                            index + 1
                        };
                        if let Some(source) = row.dnd_release_payload::<usize>() {
                            dropped = Some((*source, target));
                        }
                    }
                }

                ui.add_space(space::XS);
            }
        });

    if let Some(index) = move_up {
        plan.steps.swap(index - 1, index);
    }
    if let Some(index) = move_down {
        plan.steps.swap(index, index + 1);
    }
    if let Some((from, to)) = dropped {
        move_step(&mut plan.steps, from, to);
    }
}

/// Move `from` so that it sits before what is currently at `to`.
///
/// `to` is an insertion point, not a destination index, so removing the source
/// first shifts everything after it — which is why the target is adjusted.
/// Getting this wrong puts the commit one place off, in the direction that is
/// hardest to notice.
fn move_step<T>(items: &mut Vec<T>, from: usize, to: usize) {
    if from >= items.len() || to > items.len() {
        return;
    }
    // Dropping either side of itself is a no-op, not a move by one.
    if to == from || to == from + 1 {
        return;
    }
    let item = items.remove(from);
    let adjusted = if to > from { to - 1 } else { to };
    items.insert(adjusted, item);
}

fn field(ui: &mut Ui, p: &Palette, label: &str, value: &mut String, hint: &str) {
    ui.label(text::overline(label).color(p.text_muted));
    ui.add_space(space::XS);
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(f32::INFINITY)
            .hint_text(hint),
    );
}

fn note(ui: &mut Ui, p: &Palette, message: &str) {
    ui.label(text::caption(message).color(p.text_muted));
}

fn callout(ui: &mut Ui, p: &Palette, message: &str) {
    Frame::NONE
        .fill(p.tinted(p.warning, 0.14))
        .corner_radius(CornerRadius::same(radius::SM))
        .inner_margin(Margin::symmetric(space::MD as i8, space::SM as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(text::caption(message).color(p.warning));
        });
}

#[cfg(test)]
mod tests {
    use super::move_step;

    fn moved(from: usize, to: usize) -> Vec<char> {
        let mut items = vec!['a', 'b', 'c', 'd'];
        move_step(&mut items, from, to);
        items
    }

    #[test]
    fn moving_down_lands_before_the_target() {
        // Drop 'a' before 'd': a is removed first, so the insertion point
        // shifts back by one.
        assert_eq!(moved(0, 3), vec!['b', 'c', 'a', 'd']);
    }

    #[test]
    fn moving_to_the_end_works() {
        assert_eq!(moved(0, 4), vec!['b', 'c', 'd', 'a']);
    }

    #[test]
    fn moving_up_does_not_need_adjusting() {
        assert_eq!(moved(3, 1), vec!['a', 'd', 'b', 'c']);
        assert_eq!(moved(2, 0), vec!['c', 'a', 'b', 'd']);
    }

    #[test]
    fn dropping_on_either_side_of_itself_changes_nothing() {
        assert_eq!(moved(1, 1), vec!['a', 'b', 'c', 'd']);
        assert_eq!(moved(1, 2), vec!['a', 'b', 'c', 'd']);
    }

    #[test]
    fn out_of_range_indices_are_ignored() {
        assert_eq!(moved(9, 1), vec!['a', 'b', 'c', 'd']);
        assert_eq!(moved(0, 99), vec!['a', 'b', 'c', 'd']);
    }

    #[test]
    fn every_item_survives_a_move() {
        for from in 0..4 {
            for to in 0..=4 {
                let result = moved(from, to);
                let mut sorted = result.clone();
                sorted.sort_unstable();
                assert_eq!(
                    sorted,
                    vec!['a', 'b', 'c', 'd'],
                    "{from} -> {to} lost an item"
                );
            }
        }
    }
}
