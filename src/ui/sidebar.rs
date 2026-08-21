//! The reference sidebar: working tree, branches, remotes, tags, stashes.

use super::{icons, metrics, radius, space, text, Palette};
use crate::git::refs::{RefTree, RemoteGroup};
use crate::state::Selection;
use egui::{
    collapsing_header::CollapsingState, Align2, Color32, CornerRadius, FontId, Id, Layout, Pos2,
    Rect, Sense, Stroke, Ui, Vec2,
};
use git2::Oid;

const ROW: f32 = 24.0;
const INDENT: f32 = 12.0;

/// Something the user asked for from the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarAction {
    Select(Selection),
    /// Scroll the history to this commit.
    Reveal(Oid),
    CheckoutBranch(String),
    CheckoutRemote(String),
    /// Open the new-branch dialog starting from this revision.
    NewBranchFrom {
        start: String,
        label: String,
    },
    RenameBranch(String),
    DeleteBranch(String),
    PushBranch(String),
    ClearUpstream(String),
    DeleteTag(String),
    /// Clone or reset a submodule; `None` means all of them.
    UpdateSubmodule(Option<String>),
    /// Open a submodule as its own repository.
    OpenSubmodule(String),
    RemoveSubmodule(String),
    AddSubmodule,
    StashApply(usize),
    StashPop(usize),
    StashDrop(usize),
    CopyText(String),
}

#[derive(Debug, Default)]
pub struct SidebarResponse {
    pub action: Option<SidebarAction>,
}

pub struct Sidebar<'a> {
    pub palette: &'a Palette,
    pub refs: Option<&'a RefTree>,
    pub submodules: Option<&'a crate::git::submodule::Submodules>,
    pub selection: Option<Selection>,
    /// `None` until the status has been read. Distinct from `Some(0)`: a
    /// repository whose state is unknown must not be described as clean.
    pub uncommitted: Option<usize>,
    pub conflicts: usize,
}

impl Sidebar<'_> {
    pub fn show(&self, ui: &mut Ui) -> SidebarResponse {
        let mut out = SidebarResponse::default();
        ui.spacing_mut().item_spacing.y = 0.0;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(space::MD);
                self.working_tree(ui, &mut out);

                let Some(refs) = self.refs else {
                    self.skeleton(ui);
                    return;
                };

                self.section(ui, "branches", "Branches", refs.local.len(), true, |ui| {
                    for branch in &refs.local {
                        if let Some(action) = self.branch_row(ui, branch) {
                            out.action = Some(action);
                        }
                    }
                });

                if !refs.remotes.is_empty() {
                    let total: usize = refs.remotes.iter().map(|r| r.branches.len()).sum();
                    self.section(ui, "remotes", "Remotes", total, false, |ui| {
                        for remote in &refs.remotes {
                            self.remote_group(ui, remote, &mut out);
                        }
                    });
                }

                if !refs.tags.is_empty() {
                    self.section(ui, "tags", "Tags", refs.tags.len(), false, |ui| {
                        for tag in &refs.tags {
                            let row = self.simple_row(
                                ui,
                                if tag.annotated {
                                    icons::TAG
                                } else {
                                    icons::TAG_SIMPLE
                                },
                                &tag.name,
                                self.palette.modified,
                                1,
                            );
                            if row.clicked {
                                if let Some(oid) = tag.target {
                                    out.action = Some(SidebarAction::Reveal(oid));
                                }
                            }
                            row.response.context_menu(|ui| {
                                if menu_item(
                                    ui,
                                    self.palette,
                                    icons::GIT_BRANCH,
                                    "New branch from here",
                                ) {
                                    out.action = Some(SidebarAction::NewBranchFrom {
                                        start: tag.name.clone(),
                                        label: format!("tag {}", tag.name),
                                    });
                                    ui.close();
                                }
                                if menu_item(ui, self.palette, icons::COPY, "Copy name") {
                                    out.action = Some(SidebarAction::CopyText(tag.name.clone()));
                                    ui.close();
                                }
                                ui.separator();
                                if menu_item(ui, self.palette, icons::TRASH, "Delete tag") {
                                    out.action = Some(SidebarAction::DeleteTag(tag.name.clone()));
                                    ui.close();
                                }
                            });
                        }
                    });
                }

                if let Some(submodules) = self.submodules {
                    if !submodules.is_empty() {
                        self.section(
                            ui,
                            "submodules",
                            "Submodules",
                            submodules.entries.len(),
                            submodules.needing_attention() > 0,
                            |ui| {
                                for entry in &submodules.entries {
                                    self.submodule_row(ui, entry, &mut out);
                                }
                            },
                        );
                    }
                }

                if !refs.stashes.is_empty() {
                    self.section(ui, "stashes", "Stashes", refs.stashes.len(), false, |ui| {
                        for stash in &refs.stashes {
                            let label = stash
                                .message
                                .split_once(": ")
                                .map(|(_, rest)| rest)
                                .unwrap_or(&stash.message);
                            let row = self.simple_row(
                                ui,
                                icons::ARCHIVE,
                                label,
                                self.palette.text_secondary,
                                1,
                            );
                            if row.clicked {
                                out.action = Some(SidebarAction::Reveal(stash.target));
                            }
                            let index = stash.index;
                            row.response.context_menu(|ui| {
                                if menu_item(ui, self.palette, icons::ARROW_U_UP_LEFT, "Apply") {
                                    out.action = Some(SidebarAction::StashApply(index));
                                    ui.close();
                                }
                                if menu_item(ui, self.palette, icons::ARROW_U_UP_LEFT, "Pop") {
                                    out.action = Some(SidebarAction::StashPop(index));
                                    ui.close();
                                }
                                ui.separator();
                                if menu_item(ui, self.palette, icons::TRASH, "Drop") {
                                    out.action = Some(SidebarAction::StashDrop(index));
                                    ui.close();
                                }
                            });
                        }
                    });
                }

                ui.add_space(space::XL);
            });

        out
    }

    // ------------------------------------------------------------- sections

    fn section(
        &self,
        ui: &mut Ui,
        id: &str,
        title: &str,
        count: usize,
        default_open: bool,
        body: impl FnOnce(&mut Ui),
    ) {
        let p = self.palette;
        let state_id = Id::new(("sidebar_section", id));
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), state_id, default_open);

        ui.add_space(space::MD);
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::click());
        if resp.clicked() {
            state.toggle(ui);
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let painter = ui.painter();
        let cy = rect.center().y;
        painter.text(
            Pos2::new(rect.left() + space::MD, cy),
            Align2::LEFT_CENTER,
            if state.is_open() {
                icons::CARET_DOWN
            } else {
                icons::CARET_RIGHT
            },
            text::icon_font(10.0),
            p.text_muted,
        );
        painter.text(
            Pos2::new(rect.left() + 22.0, cy),
            Align2::LEFT_CENTER,
            title.to_uppercase(),
            FontId::new(text::size::CAPTION, egui::FontFamily::Proportional),
            p.text_muted,
        );
        painter.text(
            Pos2::new(rect.right() - space::LG, cy),
            Align2::RIGHT_CENTER,
            count,
            FontId::new(text::size::CAPTION, egui::FontFamily::Proportional),
            p.text_muted,
        );

        state.show_body_unindented(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            body(ui);
        });
    }

    fn working_tree(&self, ui: &mut Ui, out: &mut SidebarResponse) {
        let p = self.palette;
        let selected = self.selection == Some(Selection::Workdir);
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());

        self.row_background(ui, rect, selected, resp.hovered());

        let painter = ui.painter();
        let cy = rect.center().y;

        // Three states, not two. Saying "clean" before the status has been read
        // is a claim about the repository that happens to be a guess — and if
        // the read never lands, the guess is all the user ever sees.
        let (glyph, colour, label, dim) = match (self.conflicts, self.uncommitted) {
            (n, _) if n > 0 => (icons::WARNING, p.danger, "Conflicts", false),
            (_, None) => (icons::CIRCLE_NOTCH, p.text_muted, "Reading status…", true),
            (_, Some(0)) => (icons::CHECK_CIRCLE, p.added, "Working tree clean", true),
            (_, Some(_)) => (
                icons::PENCIL_SIMPLE,
                p.modified,
                "Uncommitted changes",
                false,
            ),
        };

        painter.text(
            Pos2::new(rect.left() + space::LG, cy),
            Align2::LEFT_CENTER,
            glyph,
            text::icon_font(14.0),
            colour,
        );
        painter.text(
            Pos2::new(rect.left() + 34.0, cy),
            Align2::LEFT_CENTER,
            label,
            FontId::new(text::size::BODY, egui::FontFamily::Proportional),
            if dim { p.text_secondary } else { p.text },
        );
        if let Some(count) = self.uncommitted.filter(|n| *n > 0) {
            painter.text(
                Pos2::new(rect.right() - space::LG, cy),
                Align2::RIGHT_CENTER,
                count,
                FontId::new(text::size::CAPTION, egui::FontFamily::Proportional),
                p.text_muted,
            );
        }

        if resp.clicked() {
            out.action = Some(SidebarAction::Select(Selection::Workdir));
        }
    }

    /// One submodule, with its state as the thing you read first.
    fn submodule_row(
        &self,
        ui: &mut Ui,
        entry: &crate::git::submodule::SubmoduleEntry,
        out: &mut SidebarResponse,
    ) {
        use crate::git::submodule::SubmoduleState;
        let p = self.palette;
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
        if !ui.is_rect_visible(rect) {
            return;
        }
        if resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, p.hover);
        }

        let colour = match entry.state {
            SubmoduleState::Uninitialized => p.text_muted,
            SubmoduleState::OutOfDate => p.warning,
            SubmoduleState::Dirty => p.modified,
            SubmoduleState::UpToDate => p.added,
        };

        let painter = ui.painter();
        let cy = rect.center().y;
        let x = rect.left() + space::LG + INDENT;
        painter.text(
            Pos2::new(x, cy),
            Align2::LEFT_CENTER,
            icons::PACKAGE,
            text::icon_font(12.0),
            colour,
        );

        // The state label sits at the right, so a column of submodules can be
        // scanned for the ones that need attention.
        let mut right = rect.right() - space::LG;
        if entry.state != SubmoduleState::UpToDate {
            let galley = painter.layout_no_wrap(
                entry.state.label().to_owned(),
                FontId::new(text::size::CAPTION, egui::FontFamily::Proportional),
                colour,
            );
            right -= galley.size().x;
            painter.galley(Pos2::new(right, cy - galley.size().y / 2.0), galley, colour);
            right -= space::SM;
        }

        let name_x = x + 18.0;
        let avail = (right - name_x).max(20.0);
        let galley = painter.layout(
            entry.path.clone(),
            FontId::new(text::size::BODY, egui::FontFamily::Proportional),
            p.text_secondary,
            avail,
        );
        painter
            .with_clip_rect(Rect::from_min_max(
                Pos2::new(name_x, rect.top()),
                Pos2::new(name_x + avail, rect.bottom()),
            ))
            .galley(
                Pos2::new(name_x, cy - galley.size().y / 2.0),
                galley,
                p.text_secondary,
            );

        let tooltip = {
            let mut lines = vec![entry.path.clone(), entry.state.describe().to_owned()];
            if let Some(url) = &entry.url {
                lines.push(url.clone());
            }
            match (entry.short_recorded(), entry.short_checked_out()) {
                (Some(recorded), Some(checked_out)) if recorded != checked_out => {
                    lines.push(format!("recorded {recorded}, checked out {checked_out}"));
                }
                (Some(recorded), _) => lines.push(format!("recorded {recorded}")),
                _ => {}
            }
            lines.join("\n")
        };
        let resp = resp.on_hover_text(tooltip);

        let path = entry.path.clone();
        let initialized = entry.state != SubmoduleState::Uninitialized;
        resp.context_menu(|ui| {
            if initialized && menu_item(ui, p, icons::ARROW_SQUARE_OUT, "Open") {
                out.action = Some(SidebarAction::OpenSubmodule(path.clone()));
                ui.close();
            }
            if menu_item(
                ui,
                p,
                icons::ARROWS_CLOCKWISE,
                if initialized { "Update" } else { "Initialize" },
            ) {
                out.action = Some(SidebarAction::UpdateSubmodule(Some(path.clone())));
                ui.close();
            }
            if let Some(url) = &entry.url {
                if menu_item(ui, p, icons::COPY, "Copy URL") {
                    out.action = Some(SidebarAction::CopyText(url.clone()));
                    ui.close();
                }
            }
            ui.separator();
            if menu_item(ui, p, icons::ARROWS_CLOCKWISE, "Update all submodules") {
                out.action = Some(SidebarAction::UpdateSubmodule(None));
                ui.close();
            }
            if menu_item(ui, p, icons::PLUS, "Add submodule…") {
                out.action = Some(SidebarAction::AddSubmodule);
                ui.close();
            }
            ui.separator();
            if menu_item(ui, p, icons::TRASH, "Remove") {
                out.action = Some(SidebarAction::RemoveSubmodule(path.clone()));
                ui.close();
            }
        });

        if resp.double_clicked() && initialized {
            out.action = Some(SidebarAction::OpenSubmodule(entry.path.clone()));
        }
    }

    fn remote_group(&self, ui: &mut Ui, remote: &RemoteGroup, out: &mut SidebarResponse) {
        let p = self.palette;
        let state_id = Id::new(("sidebar_remote", &remote.name));
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), state_id, true);

        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
        if resp.clicked() {
            state.toggle(ui);
        }
        if resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, p.hover);
        }

        let painter = ui.painter();
        let cy = rect.center().y;
        let x = rect.left() + space::LG + INDENT;
        painter.text(
            Pos2::new(x, cy),
            Align2::LEFT_CENTER,
            if state.is_open() {
                icons::CARET_DOWN
            } else {
                icons::CARET_RIGHT
            },
            text::icon_font(9.0),
            p.text_muted,
        );
        painter.text(
            Pos2::new(x + 14.0, cy),
            Align2::LEFT_CENTER,
            &remote.name,
            FontId::new(text::size::LABEL, egui::FontFamily::Proportional),
            p.text_secondary,
        );
        if let Some(url) = &remote.url {
            resp.on_hover_text(url);
        }

        state.show_body_unindented(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for branch in &remote.branches {
                let full = format!("{}/{}", remote.name, branch.name);
                let row = self.simple_row(ui, icons::GIT_BRANCH, &branch.name, p.text_muted, 2);
                if row.clicked {
                    if let Some(oid) = branch.target {
                        out.action = Some(SidebarAction::Reveal(oid));
                    }
                }
                row.response.context_menu(|ui| {
                    if menu_item(ui, p, icons::SIGN_IN, "Check out") {
                        // Checking out a remote branch creates a local one that
                        // tracks it; detaching HEAD is almost never the intent.
                        out.action = Some(SidebarAction::CheckoutRemote(full.clone()));
                        ui.close();
                    }
                    if menu_item(ui, p, icons::GIT_BRANCH, "New branch from here") {
                        out.action = Some(SidebarAction::NewBranchFrom {
                            start: full.clone(),
                            label: full.clone(),
                        });
                        ui.close();
                    }
                    if menu_item(ui, p, icons::COPY, "Copy name") {
                        out.action = Some(SidebarAction::CopyText(full.clone()));
                        ui.close();
                    }
                });
            }
        });
    }

    // ----------------------------------------------------------------- rows

    fn row_background(&self, ui: &Ui, rect: Rect, selected: bool, hovered: bool) {
        let p = self.palette;
        if selected {
            ui.painter()
                .rect_filled(rect, CornerRadius::ZERO, p.selected);
            ui.painter().rect_filled(
                Rect::from_min_size(rect.left_top(), Vec2::new(2.0, rect.height())),
                CornerRadius::ZERO,
                p.accent,
            );
        } else if hovered {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, p.hover);
        }
    }

    fn branch_row(&self, ui: &mut Ui, branch: &crate::git::BranchEntry) -> Option<SidebarAction> {
        let p = self.palette;
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
        if !ui.is_rect_visible(rect) {
            return None;
        }
        self.row_background(ui, rect, false, resp.hovered());

        let painter = ui.painter();
        let cy = rect.center().y;
        let x = rect.left() + space::LG + INDENT;
        let colour = if branch.is_head { p.accent } else { p.info };

        painter.text(
            Pos2::new(x, cy),
            Align2::LEFT_CENTER,
            icons::GIT_BRANCH,
            text::icon_font(12.0),
            colour,
        );

        // Ahead/behind first, so the name can be measured against what's left.
        let mut right = rect.right() - space::LG;
        for (count, glyph, colour) in [
            (branch.behind, icons::ARROW_DOWN, p.info),
            (branch.ahead, icons::ARROW_UP, p.added),
        ] {
            if count == 0 {
                continue;
            }
            let galley = painter.layout_no_wrap(
                format!("{glyph}{count}"),
                FontId::new(text::size::CAPTION, egui::FontFamily::Proportional),
                colour,
            );
            right -= galley.size().x;
            painter.galley(Pos2::new(right, cy - galley.size().y / 2.0), galley, colour);
            right -= space::SM;
        }

        let name_x = x + 18.0;
        let avail = (right - name_x - space::SM).max(20.0);
        let name_colour = if branch.is_head {
            p.text
        } else {
            p.text_secondary
        };
        let galley = painter.layout(
            branch.name.clone(),
            FontId::new(text::size::BODY, egui::FontFamily::Proportional),
            name_colour,
            avail,
        );
        painter
            .with_clip_rect(Rect::from_min_max(
                Pos2::new(name_x, rect.top()),
                Pos2::new(name_x + avail, rect.bottom()),
            ))
            .galley(
                Pos2::new(name_x, cy - galley.size().y / 2.0),
                galley,
                name_colour,
            );

        let tip = match &branch.upstream {
            Some(up) => format!("{} → {up}\n\nDouble-click to check out", branch.name),
            None => format!("{} (no upstream)\n\nDouble-click to check out", branch.name),
        };
        let resp = resp.on_hover_text(tip);

        let mut action = None;
        let name = branch.name.clone();
        resp.context_menu(|ui| {
            if !branch.is_head && menu_item(ui, p, icons::SIGN_IN, "Check out") {
                action = Some(SidebarAction::CheckoutBranch(name.clone()));
                ui.close();
            }
            if menu_item(ui, p, icons::GIT_BRANCH, "New branch from here") {
                action = Some(SidebarAction::NewBranchFrom {
                    start: name.clone(),
                    label: name.clone(),
                });
                ui.close();
            }
            if menu_item(ui, p, icons::CLOUD_ARROW_UP, "Push…") {
                action = Some(SidebarAction::PushBranch(name.clone()));
                ui.close();
            }
            ui.separator();
            if menu_item(ui, p, icons::PENCIL_SIMPLE, "Rename…") {
                action = Some(SidebarAction::RenameBranch(name.clone()));
                ui.close();
            }
            if branch.upstream.is_some() && menu_item(ui, p, icons::LINK_BREAK, "Stop tracking") {
                action = Some(SidebarAction::ClearUpstream(name.clone()));
                ui.close();
            }
            if menu_item(ui, p, icons::COPY, "Copy name") {
                action = Some(SidebarAction::CopyText(name.clone()));
                ui.close();
            }
            if !branch.is_head {
                ui.separator();
                if menu_item(ui, p, icons::TRASH, "Delete…") {
                    action = Some(SidebarAction::DeleteBranch(name.clone()));
                    ui.close();
                }
            }
        });

        if action.is_some() {
            return action;
        }
        if resp.double_clicked() && !branch.is_head {
            return Some(SidebarAction::CheckoutBranch(branch.name.clone()));
        }
        if resp.clicked() {
            return branch.target.map(SidebarAction::Reveal);
        }
        None
    }

    fn simple_row(
        &self,
        ui: &mut Ui,
        glyph: &str,
        label: &str,
        colour: Color32,
        depth: usize,
    ) -> RowResponse {
        let p = self.palette;
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
        if !ui.is_rect_visible(rect) {
            return RowResponse {
                clicked: false,
                response: resp,
            };
        }
        if resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, p.hover);
        }

        let painter = ui.painter();
        let cy = rect.center().y;
        let x = rect.left() + space::LG + INDENT * depth as f32;
        painter.text(
            Pos2::new(x, cy),
            Align2::LEFT_CENTER,
            glyph,
            text::icon_font(11.0),
            colour,
        );

        let name_x = x + 16.0;
        let avail = (rect.right() - space::LG - name_x).max(20.0);
        let galley = painter.layout(
            label.to_owned(),
            FontId::new(text::size::LABEL, egui::FontFamily::Proportional),
            p.text_secondary,
            avail,
        );
        painter
            .with_clip_rect(Rect::from_min_max(
                Pos2::new(name_x, rect.top()),
                Pos2::new(name_x + avail, rect.bottom()),
            ))
            .galley(
                Pos2::new(name_x, cy - galley.size().y / 2.0),
                galley,
                p.text_secondary,
            );

        let resp = resp.on_hover_text(label);
        RowResponse {
            clicked: resp.clicked(),
            response: resp,
        }
    }

    fn skeleton(&self, ui: &mut Ui) {
        let p = self.palette;
        ui.add_space(space::XL);
        for i in 0..6 {
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::hover());
            let w = rect.width() * (0.3 + 0.15 * ((i % 3) as f32));
            ui.painter().rect_filled(
                Rect::from_min_size(
                    Pos2::new(rect.left() + space::XL, rect.center().y - 4.0),
                    Vec2::new(w, 8.0),
                ),
                CornerRadius::same(radius::SM),
                p.bg_raised,
            );
        }
    }
}

/// A row's click state plus its response, so callers can attach a context menu.
pub struct RowResponse {
    pub clicked: bool,
    pub response: egui::Response,
}

/// One entry in a context menu, styled to match the rest of the app rather than
/// egui's default button.
fn menu_item(ui: &mut Ui, p: &Palette, glyph: &str, label: &str) -> bool {
    let desired = Vec2::new(ui.available_width().max(170.0), 24.0);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
    let destructive = glyph == icons::TRASH;
    let fg = if destructive { p.danger } else { p.text };

    if response.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(radius::SM),
            if destructive {
                p.danger.gamma_multiply(0.18)
            } else {
                p.hover
            },
        );
    }
    ui.painter().text(
        Pos2::new(rect.left() + space::MD, rect.center().y),
        Align2::LEFT_CENTER,
        glyph,
        text::icon_font(12.0),
        if response.hovered() { fg } else { p.text_muted },
    );
    ui.painter().text(
        Pos2::new(rect.left() + 28.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::new(text::size::BODY, egui::FontFamily::Proportional),
        fg,
    );
    response.clicked()
}

/// Exposed so the panel can size itself consistently with these rows.
pub const fn row_height() -> f32 {
    ROW
}

/// Unused for now; keeps the import list honest as sections grow.
#[allow(dead_code)]
fn _metrics_in_scope() -> f32 {
    metrics::ROW
}

/// Unused placeholder retained for symmetry with other views.
#[allow(dead_code)]
fn _layout_in_scope(_: Layout) {}

/// Unused placeholder.
#[allow(dead_code)]
fn _stroke_in_scope(_: Stroke) {}
