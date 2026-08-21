//! The history list: virtualized rows with hand-painted commit lanes.
//!
//! Only the visible rows are laid out, via `ScrollArea::show_rows`. Everything
//! a row needs to draw itself was computed on a worker (see
//! [`crate::git::graph`]), so drawing row 40 000 costs the same as drawing row
//! 3 and a hundred-thousand-commit repository scrolls at full frame rate.

use super::{icons, radius, space, text, Palette};
use crate::git::graph::{GraphPage, GraphRow, RefBadge, RefKind, Segment};
use crate::state::Selection;
use crate::util::time;
use egui::{
    epaint::CubicBezierShape, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape,
    Stroke, Ui, Vec2,
};

/// Horizontal distance between lane centres.
const LANE_WIDTH: f32 = 14.0;
/// Beyond this the gutter stops growing; deeper lanes are clamped to the edge
/// so one pathological merge can't push the message column off-screen.
const MAX_DRAWN_LANES: usize = 12;
const DOT_RADIUS: f32 = 3.6;
const LINE_WIDTH: f32 = 1.6;
const ROW_HEIGHT: f32 = 26.0;

/// Columns on the right-hand side, in points.
const AUTHOR_WIDTH: f32 = 130.0;
const DATE_WIDTH: f32 = 110.0;
const HASH_WIDTH: f32 = 62.0;

/// Something asked for from a commit's context menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitAction {
    CheckoutCommit(git2::Oid),
    CherryPick(git2::Oid),
    Revert(git2::Oid),
    Reset(git2::Oid),
    BranchFrom(git2::Oid),
    TagAt(git2::Oid),
    MergeInto(String),
    RebaseOnto(String),
    /// Open the interactive rebase planner for everything after this commit.
    RebaseFrom(git2::Oid),
    CopyHash(git2::Oid),
    CopySummary(String),
}

/// What the user did in the list this frame.
#[derive(Debug, Default)]
pub struct GraphResponse {
    pub selected: Option<Selection>,
    /// The list was scrolled close enough to the end to want more history.
    pub wants_more: bool,
    pub activated: Option<Selection>,
    pub action: Option<CommitAction>,
}

pub struct GraphView<'a> {
    pub palette: &'a Palette,
    pub page: &'a GraphPage,
    pub selection: Option<Selection>,
    /// Whether to show the synthetic "uncommitted changes" row at the top.
    pub show_workdir_row: bool,
    pub workdir_count: usize,
    pub has_more: bool,
    /// Row index to bring into view this frame.
    pub scroll_to: Option<usize>,
}

impl GraphView<'_> {
    pub fn show(&self, ui: &mut Ui) -> GraphResponse {
        let mut response = GraphResponse::default();
        let lane_count = self.page.max_width.clamp(1, MAX_DRAWN_LANES);
        let gutter = space::LG + lane_count as f32 * LANE_WIDTH;

        // The synthetic working-tree row occupies index 0 when present, so the
        // commit at graph index `i` is list index `i + offset`.
        let offset = usize::from(self.show_workdir_row);
        let total = self.page.rows.len() + offset;
        let now = time::now();

        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(index) = self.scroll_to {
            // Centre the target rather than putting it at the top edge: a
            // commit with no visible history around it is hard to place.
            let target = index as f32 * ROW_HEIGHT - ui.available_height() / 2.0;
            area = area.vertical_scroll_offset(target.max(0.0));
        }

        let scroll = area.show_rows(ui, ROW_HEIGHT, total, |ui, range| {
            // Take the full width so rows are click targets edge to edge.
            ui.set_width(ui.available_width());
            // `show_rows` positions the viewport assuming every row is
            // exactly `ROW_HEIGHT` tall. egui's default item spacing would
            // add 4pt between rows, so the rows would drift out of step
            // with the scroll offset — increasingly wrong further down.
            ui.spacing_mut().item_spacing.y = 0.0;

            for index in range {
                let (rect, resp) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), ROW_HEIGHT),
                    Sense::click(),
                );
                if !ui.is_rect_visible(rect) {
                    continue;
                }

                let selection = if index < offset {
                    Selection::Workdir
                } else {
                    Selection::Commit(self.page.rows[index - offset].commit.id)
                };
                let selected = self.selection == Some(selection);

                self.paint_row_background(ui, rect, selected, resp.hovered());

                if index < offset {
                    self.paint_workdir_row(ui, rect, gutter);
                } else {
                    self.paint_commit_row(
                        ui,
                        rect,
                        &self.page.rows[index - offset],
                        gutter,
                        lane_count,
                        now,
                    );
                }

                if resp.clicked() {
                    response.selected = Some(selection);
                }
                if resp.double_clicked() {
                    response.activated = Some(selection);
                }

                if index >= offset {
                    let row = &self.page.rows[index - offset];
                    if let Some(action) = self.context_menu(&resp, row) {
                        response.action = Some(action);
                        // Acting on a commit implies selecting it.
                        response.selected = Some(selection);
                    }
                }
            }
        });

        // Ask for more history once the viewport is within a few screens of the
        // end, so the extra walk finishes before the user gets there.
        if self.has_more {
            let remaining =
                scroll.content_size.y - (scroll.state.offset.y + scroll.inner_rect.height());
            if remaining < ROW_HEIGHT * 40.0 {
                response.wants_more = true;
            }
        }

        response
    }

    /// The per-commit menu. Everything here acts on one commit, which is why it
    /// lives on the commit rather than in a toolbar where the target would have
    /// to be inferred.
    fn context_menu(&self, response: &egui::Response, row: &GraphRow) -> Option<CommitAction> {
        let p = self.palette;
        let mut chosen = None;
        let oid = row.commit.id;
        let short = row.commit.short_id.clone();

        // A branch label on the commit makes branch-level operations sensible.
        let branch = row
            .refs
            .iter()
            .find(|r| r.kind == RefKind::LocalBranch && !r.is_head)
            .map(|r| r.name.clone());

        response.context_menu(|ui| {
            ui.set_min_width(210.0);
            ui.label(text::caption(format!("Commit {short}")).color(p.text_muted));
            ui.separator();

            if ui.button("Check out this commit").clicked() {
                chosen = Some(CommitAction::CheckoutCommit(oid));
                ui.close();
            }
            if ui.button("New branch here…").clicked() {
                chosen = Some(CommitAction::BranchFrom(oid));
                ui.close();
            }
            if ui.button("New tag here…").clicked() {
                chosen = Some(CommitAction::TagAt(oid));
                ui.close();
            }

            ui.separator();
            if let Some(name) = &branch {
                if ui.button(format!("Merge ‘{name}’ into current")).clicked() {
                    chosen = Some(CommitAction::MergeInto(name.clone()));
                    ui.close();
                }
                if ui.button(format!("Rebase current onto ‘{name}’")).clicked() {
                    chosen = Some(CommitAction::RebaseOnto(name.clone()));
                    ui.close();
                }
                ui.separator();
            }

            if ui.button("Cherry-pick onto current").clicked() {
                chosen = Some(CommitAction::CherryPick(oid));
                ui.close();
            }
            if ui.button("Revert this commit").clicked() {
                chosen = Some(CommitAction::Revert(oid));
                ui.close();
            }
            if ui.button("Rebase commits after this…").clicked() {
                chosen = Some(CommitAction::RebaseFrom(oid));
                ui.close();
            }

            ui.separator();
            if ui.button("Copy hash").clicked() {
                chosen = Some(CommitAction::CopyHash(oid));
                ui.close();
            }
            if ui.button("Copy summary").clicked() {
                chosen = Some(CommitAction::CopySummary(row.commit.summary.clone()));
                ui.close();
            }

            ui.separator();
            if ui
                .button("Reset current branch here…")
                .on_hover_text("Moves the branch, optionally discarding work")
                .clicked()
            {
                chosen = Some(CommitAction::Reset(oid));
                ui.close();
            }
        });

        chosen
    }

    fn paint_row_background(&self, ui: &Ui, rect: Rect, selected: bool, hovered: bool) {
        let p = self.palette;
        if selected {
            ui.painter()
                .rect_filled(rect, CornerRadius::ZERO, p.selected);
            // A left edge marker reads as "this one" even against a subtle fill.
            ui.painter().rect_filled(
                Rect::from_min_size(rect.left_top(), Vec2::new(2.0, rect.height())),
                CornerRadius::ZERO,
                p.accent,
            );
        } else if hovered {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, p.hover);
        }
    }

    /// Centre of a lane, clamped so deep lanes stay inside the gutter.
    fn lane_x(&self, rect: Rect, lane: usize, lane_count: usize) -> f32 {
        let clamped = lane.min(lane_count.saturating_sub(1));
        rect.left() + space::MD + clamped as f32 * LANE_WIDTH + LANE_WIDTH / 2.0
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_commit_row(
        &self,
        ui: &Ui,
        rect: Rect,
        row: &GraphRow,
        gutter: f32,
        lane_count: usize,
        now: i64,
    ) {
        let p = self.palette;
        let painter = ui.painter();
        let cy = rect.center().y;
        let dot_x = self.lane_x(rect, row.lane, lane_count);
        let dot = Pos2::new(dot_x, cy);

        // Lines that merely cross this row.
        for seg in &row.passthrough {
            let x = self.lane_x(rect, seg.lane, lane_count);
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(LINE_WIDTH, p.lane(seg.lane)),
            );
        }

        // Lines arriving from above and converging into this commit.
        for seg in &row.incoming {
            let x = self.lane_x(rect, seg.lane, lane_count);
            self.paint_link(painter, Pos2::new(x, rect.top()), dot, p.lane(seg.lane));
        }

        // Lines leaving downward, one per parent.
        for seg in &row.outgoing {
            let x = self.lane_x(rect, seg.lane, lane_count);
            self.paint_link(painter, dot, Pos2::new(x, rect.bottom()), p.lane(seg.lane));
        }

        // The commit dot. Merges are drawn hollow so the shape of the history
        // is readable without reading any text.
        let colour = p.lane(row.lane);
        if row.commit.is_merge() {
            painter.circle_filled(dot, DOT_RADIUS + 0.6, p.bg_base);
            painter.circle_stroke(dot, DOT_RADIUS, Stroke::new(2.0, colour));
        } else {
            painter.circle_filled(dot, DOT_RADIUS, colour);
        }

        // ---- text columns ----
        let right = rect.right() - space::LG;
        let mut x = rect.left() + gutter + space::MD;

        // Ref badges come before the summary: they say where you are.
        for badge in &row.refs {
            let advance = self.paint_badge(ui, Pos2::new(x, cy), badge, right - x);
            if advance <= 0.0 {
                break;
            }
            x += advance + space::SM;
        }

        let hash_x = right - HASH_WIDTH;
        let date_x = hash_x - DATE_WIDTH;
        let author_x = date_x - AUTHOR_WIDTH;

        let summary_color = p.text;
        let summary_width = (author_x - x - space::LG).max(40.0);
        let galley = painter.layout(
            row.commit.summary.clone(),
            FontId::new(text::size::BODY, egui::FontFamily::Proportional),
            summary_color,
            summary_width,
        );
        // One line only: a wrapped summary would break row virtualization,
        // which depends on every row being exactly `ROW_HEIGHT` tall.
        painter
            .with_clip_rect(Rect::from_min_max(
                Pos2::new(x, rect.top()),
                Pos2::new(x + summary_width, rect.bottom()),
            ))
            .galley(
                Pos2::new(x, cy - galley.size().y / 2.0),
                galley,
                summary_color,
            );

        let meta_font = FontId::new(text::size::CAPTION, egui::FontFamily::Proportional);
        painter
            .with_clip_rect(Rect::from_min_max(
                Pos2::new(author_x, rect.top()),
                Pos2::new(date_x - space::MD, rect.bottom()),
            ))
            .text(
                Pos2::new(author_x, cy),
                Align2::LEFT_CENTER,
                &row.commit.author_name,
                meta_font.clone(),
                p.text_muted,
            );

        painter.text(
            Pos2::new(date_x, cy),
            Align2::LEFT_CENTER,
            time::relative(row.commit.time, now),
            meta_font,
            p.text_muted,
        );

        painter.text(
            Pos2::new(right, cy),
            Align2::RIGHT_CENTER,
            &row.commit.short_id,
            FontId::new(text::size::CAPTION, text::mono_family()),
            p.text_muted,
        );
    }

    /// A line between two points in the graph gutter.
    ///
    /// Straight when the lane doesn't change, and an S-curve when it does —
    /// elbows read as noise once several branches are in play.
    fn paint_link(&self, painter: &egui::Painter, from: Pos2, to: Pos2, colour: Color32) {
        let stroke = Stroke::new(LINE_WIDTH, colour);
        if (from.x - to.x).abs() < 0.5 {
            painter.line_segment([from, to], stroke);
            return;
        }
        let dy = (to.y - from.y) * 0.5;
        painter.add(Shape::CubicBezier(CubicBezierShape::from_points_stroke(
            [
                from,
                Pos2::new(from.x, from.y + dy),
                Pos2::new(to.x, to.y - dy),
                to,
            ],
            false,
            Color32::TRANSPARENT,
            stroke,
        )));
    }

    /// Draw a branch or tag pill. Returns the width used, or 0 if it didn't fit.
    fn paint_badge(&self, ui: &Ui, left_center: Pos2, badge: &RefBadge, avail: f32) -> f32 {
        let p = self.palette;
        let painter = ui.painter();

        let (fg, glyph) = match badge.kind {
            RefKind::Head => (p.accent, icons::TARGET),
            RefKind::LocalBranch if badge.is_head => (p.accent, icons::GIT_BRANCH),
            RefKind::LocalBranch => (p.info, icons::GIT_BRANCH),
            RefKind::RemoteBranch => (p.text_secondary, icons::CLOUD),
            RefKind::Tag => (p.modified, icons::TAG),
        };

        let font = FontId::new(text::size::CAPTION, egui::FontFamily::Proportional);
        let label = painter.layout_no_wrap(badge.name.clone(), font, fg);
        let icon_w = 12.0;
        let pad = 5.0;
        let width = pad + icon_w + 3.0 + label.size().x + pad;
        if width > avail {
            return 0.0;
        }

        let height = 16.0;
        let rect = Rect::from_min_size(
            Pos2::new(left_center.x, left_center.y - height / 2.0),
            Vec2::new(width, height),
        );
        painter.rect_filled(
            rect,
            CornerRadius::same(radius::SM),
            fg.gamma_multiply(0.16),
        );
        if badge.is_head {
            painter.rect_stroke(
                rect,
                CornerRadius::same(radius::SM),
                Stroke::new(1.0, fg.gamma_multiply(0.7)),
                egui::StrokeKind::Inside,
            );
        }
        painter.text(
            Pos2::new(rect.left() + pad, left_center.y),
            Align2::LEFT_CENTER,
            glyph,
            text::icon_font(11.0),
            fg,
        );
        painter.galley(
            Pos2::new(
                rect.left() + pad + icon_w + 3.0,
                left_center.y - label.size().y / 2.0,
            ),
            label,
            fg,
        );
        width
    }

    /// The synthetic row for uncommitted work, pinned above the history.
    fn paint_workdir_row(&self, ui: &Ui, rect: Rect, gutter: f32) {
        let p = self.palette;
        let painter = ui.painter();
        let cy = rect.center().y;
        let x = rect.left() + space::MD + LANE_WIDTH / 2.0;

        // A dashed stub instead of a dot: this isn't a commit yet.
        let colour = p.modified;
        painter.circle_stroke(Pos2::new(x, cy), DOT_RADIUS, Stroke::new(1.4, colour));
        painter.line_segment(
            [Pos2::new(x, cy + DOT_RADIUS), Pos2::new(x, rect.bottom())],
            Stroke::new(LINE_WIDTH, colour.gamma_multiply(0.5)),
        );

        let text_x = rect.left() + gutter + space::MD;
        painter.text(
            Pos2::new(text_x, cy),
            Align2::LEFT_CENTER,
            "Uncommitted changes",
            FontId::new(text::size::BODY, egui::FontFamily::Proportional),
            p.text,
        );
        let label = crate::util::words::plural(self.workdir_count, "file");
        painter.text(
            Pos2::new(rect.right() - space::LG, cy),
            Align2::RIGHT_CENTER,
            label,
            FontId::new(text::size::CAPTION, egui::FontFamily::Proportional),
            p.text_muted,
        );
    }
}

/// Row height, exported so callers can size scroll offsets consistently.
pub const fn row_height() -> f32 {
    ROW_HEIGHT
}

/// Keyboard navigation over the list, returning the new selection.
pub fn step_selection(
    page: &GraphPage,
    show_workdir: bool,
    current: Option<Selection>,
    delta: i64,
) -> Option<Selection> {
    let offset = i64::from(show_workdir);
    let total = page.rows.len() as i64 + offset;
    if total == 0 {
        return None;
    }

    let index = match current {
        Some(Selection::Workdir) if show_workdir => 0,
        Some(Selection::Commit(oid)) => page
            .rows
            .iter()
            .position(|r| r.commit.id == oid)
            .map(|i| i as i64 + offset)
            .unwrap_or(0),
        _ => 0,
    };

    let next = (index + delta).clamp(0, total - 1);
    if next < offset {
        Some(Selection::Workdir)
    } else {
        page.rows
            .get((next - offset) as usize)
            .map(|r| Selection::Commit(r.commit.id))
    }
}

/// Unused today, but the segment type is part of this module's vocabulary.
#[allow(dead_code)]
fn _assert_segment_in_scope(_: Segment) {}
