//! The diff pane: a file list and a virtualized view of the selected file.
//!
//! Rows are addressed by index rather than materialized into a list. A file's
//! rows are its hunks' headers plus their lines, so a prefix-sum over hunks maps
//! a global row index to a specific line in O(log n) with no allocation. That
//! matters because this runs every frame: building a 30 000-element vector at
//! 60fps to display forty rows would be absurd.

use super::{icons, space, text, Palette};
use crate::git::diff::{DiffModel, FileDiff, Hunk, LineKind, Omitted};
use crate::git::Delta;
use egui::{Align2, CornerRadius, FontId, Frame, Margin, Pos2, Rect, Sense, Stroke, Ui, Vec2};

const LINE_HEIGHT: f32 = 17.0;
const GUTTER_NUM_WIDTH: f32 = 40.0;
const SIGN_WIDTH: f32 = 14.0;
const FILE_ROW_HEIGHT: f32 = 24.0;

/// How the diff body is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffLayout {
    /// One column, additions and deletions interleaved.
    #[default]
    Unified,
    /// Two columns, old on the left and new on the right.
    SideBySide,
}

impl DiffLayout {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unified => "Unified",
            Self::SideBySide => "Side by side",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Unified => Self::SideBySide,
            Self::SideBySide => Self::Unified,
        }
    }
}

/// Maps global row indices onto hunks and lines without building a list.
struct FileLayout<'a> {
    file: &'a FileDiff,
    /// `starts[i]` is the global row index of hunk `i`'s header.
    starts: Vec<usize>,
    total: usize,
    mode: DiffLayout,
    /// Pairings for the most recently addressed hunk.
    ///
    /// Side-by-side rows are computed per hunk on demand rather than for the
    /// whole file: the visible rows are contiguous, so they touch one or two
    /// hunks, and a 30 000-line file has no business being paired in full on
    /// every frame.
    cache: std::cell::RefCell<Option<(usize, Vec<crate::git::diff::Pair>)>>,
}

impl<'a> FileLayout<'a> {
    fn new(file: &'a FileDiff, mode: DiffLayout) -> Self {
        let mut starts = Vec::with_capacity(file.hunks.len());
        let mut total = 0;
        for hunk in &file.hunks {
            starts.push(total);
            total += 1 + match mode {
                DiffLayout::Unified => hunk.lines.len(),
                DiffLayout::SideBySide => hunk.paired_len(),
            };
        }
        Self {
            file,
            starts,
            total,
            mode,
            cache: std::cell::RefCell::new(None),
        }
    }

    /// The pairing for a hunk, computing it if it isn't the cached one.
    fn pairs(&self, hunk_index: usize) -> Vec<crate::git::diff::Pair> {
        let mut cache = self.cache.borrow_mut();
        if let Some((cached, pairs)) = cache.as_ref() {
            if *cached == hunk_index {
                return pairs.clone();
            }
        }
        let pairs = self.file.hunks[hunk_index].pair_lines();
        *cache = Some((hunk_index, pairs.clone()));
        pairs
    }

    fn row(&self, index: usize) -> Option<Row<'a>> {
        // The last hunk starting at or before `index` is the one containing it.
        let hunk_index = match self.starts.binary_search(&index) {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        let hunk = self.file.hunks.get(hunk_index)?;
        let offset = index - self.starts[hunk_index];
        if offset == 0 {
            return Some(Row::Header(hunk_index, hunk));
        }
        match self.mode {
            DiffLayout::Unified => hunk
                .lines
                .get(offset - 1)
                .map(|line| Row::Line(hunk_index, offset - 1, line)),
            DiffLayout::SideBySide => self
                .pairs(hunk_index)
                .get(offset - 1)
                .map(|pair| Row::Pair(hunk_index, *pair)),
        }
    }
}

enum Row<'a> {
    Header(usize, &'a Hunk),
    Line(usize, usize, &'a crate::git::diff::DiffLine),
    Pair(usize, crate::git::diff::Pair),
}

/// What a click on a file row meant.
enum FileRowHit {
    None,
    Select,
    Action(RowAction),
}

/// What can be done to a change from here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    Stage,
    Unstage,
    Discard,
}

impl RowAction {
    fn label(self) -> &'static str {
        match self {
            Self::Stage => "Stage",
            Self::Unstage => "Unstage",
            Self::Discard => "Discard",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Stage => icons::PLUS,
            Self::Unstage => icons::MINUS,
            Self::Discard => icons::ARROW_COUNTER_CLOCKWISE,
        }
    }
}

/// Which direction staging goes in the pane currently being shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageDirection {
    /// Viewing unstaged work: changes can be staged or discarded.
    Unstaged,
    /// Viewing the index: changes can be unstaged.
    Staged,
}

impl StageDirection {
    fn primary(self) -> RowAction {
        match self {
            Self::Unstaged => RowAction::Stage,
            Self::Staged => RowAction::Unstage,
        }
    }

    fn allows_discard(self) -> bool {
        self == Self::Unstaged
    }
}

#[derive(Debug, Default)]
pub struct DiffResponse {
    pub selected_file: Option<String>,
    /// A diff line was clicked, with whether shift was held.
    pub line_clicked: Option<(usize, usize, bool)>,
    pub hunk_action: Option<(usize, RowAction)>,
    /// A whole file was actioned from its row in the list.
    pub file_action: Option<(String, RowAction)>,
    pub blame_file: Option<String>,
    pub file_history: Option<String>,
    /// Pixels left for diff lines once the path header has taken its share.
    ///
    /// Reported so a test can assert the diff is not squeezed to a few lines
    /// by some future change to the panels above it. Cramping is invisible to
    /// every other kind of test: the widgets are all present and correct, there
    /// is simply nowhere to read them.
    pub body_height: f32,
}

pub struct DiffPane<'a> {
    pub palette: &'a Palette,
    pub model: &'a DiffModel,
    pub active_file: Option<&'a str>,
    /// Width of the file-list column.
    pub list_width: f32,
    /// `None` when viewing a commit, which cannot be staged.
    pub direction: Option<StageDirection>,
    pub layout: DiffLayout,
    /// Currently selected `(hunk, line)` pairs in the active file.
    pub line_selection: &'a std::collections::BTreeSet<(usize, usize)>,
    /// Whether the arrow keys are moving through this list right now.
    ///
    /// Two lists showing a bright selection at once cannot both be the one the
    /// keyboard will move, so the idle one dims its marker — the same
    /// convention every desktop list uses, and the only cue that says where a
    /// keypress will land.
    pub focused: bool,
}

impl DiffPane<'_> {
    pub fn show(&self, ui: &mut Ui) -> DiffResponse {
        let mut response = DiffResponse::default();
        let p = self.palette;
        // Same reason as the detail pane: never report a short content height,
        // or an enclosing panel will persist it.
        ui.set_min_size(ui.available_size());

        if self.model.is_empty() {
            self.empty_state(ui, "No changes in this commit");
            return response;
        }

        egui::Panel::left(egui::Id::new("diff_file_list"))
            .resizable(true)
            .default_size(self.list_width)
            .size_range(160.0..=420.0)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(p.bg_base))
            .show(ui, |ui| {
                ui.painter().vline(
                    ui.max_rect().right(),
                    ui.max_rect().y_range(),
                    Stroke::new(1.0, p.border),
                );
                let (selected, action) = self.file_list(ui);
                response.selected_file = selected;
                response.file_action = action;
            });

        egui::CentralPanel::no_frame()
            .frame(Frame::NONE.fill(p.bg_base))
            .show(ui, |ui| {
                match self.active_file.and_then(|path| self.model.find(path)) {
                    Some(file) => self.file_body(ui, file, &mut response),
                    None => self.empty_state(ui, "Select a file"),
                }
            });

        response
    }

    fn empty_state(&self, ui: &mut Ui, message: &str) {
        let p = self.palette;
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.35);
            ui.label(text::caption(message).color(p.text_muted));
        });
    }

    // ------------------------------------------------------------- file list

    fn file_list(&self, ui: &mut Ui) -> (Option<String>, Option<(String, RowAction)>) {
        let p = self.palette;
        let mut clicked = None;
        let mut action_out = None;

        ui.add_space(space::SM);
        ui.horizontal(|ui| {
            ui.add_space(space::LG);
            let n = self.model.files.len();
            ui.label(text::overline(crate::util::words::plural(n, "file")).color(p.text_muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(space::LG);
                if self.model.deletions > 0 {
                    ui.label(text::caption(format!("−{}", self.model.deletions)).color(p.removed));
                }
                if self.model.additions > 0 {
                    ui.label(text::caption(format!("+{}", self.model.additions)).color(p.added));
                }
            });
        });
        ui.add_space(space::XS);

        egui::ScrollArea::vertical()
            .id_salt("diff_files")
            .auto_shrink([false, false])
            .show_rows(ui, FILE_ROW_HEIGHT, self.model.files.len(), |ui, range| {
                ui.set_width(ui.available_width());
                // See the note in `ui::graph`: row height must match exactly.
                ui.spacing_mut().item_spacing.y = 0.0;
                for i in range {
                    let file = &self.model.files[i];
                    let active = self.active_file == Some(file.path.as_str());
                    match self.file_row(ui, file, active) {
                        FileRowHit::Select => clicked = Some(file.path.clone()),
                        FileRowHit::Action(action) => {
                            action_out = Some((file.path.clone(), action))
                        }
                        FileRowHit::None => {}
                    }
                }
            });

        (clicked, action_out)
    }

    fn file_row(&self, ui: &mut Ui, file: &FileDiff, active: bool) -> FileRowHit {
        let p = self.palette;
        let (rect, resp) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), FILE_ROW_HEIGHT),
            Sense::click(),
        );
        if !ui.is_rect_visible(rect) {
            return FileRowHit::None;
        }

        if active {
            ui.painter()
                .rect_filled(rect, CornerRadius::ZERO, p.selected);
            ui.painter().rect_filled(
                Rect::from_min_size(rect.left_top(), Vec2::new(2.0, rect.height())),
                CornerRadius::ZERO,
                if self.focused {
                    p.accent
                } else {
                    p.border_strong
                },
            );
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, p.hover);
        }

        let painter = ui.painter();
        let cy = rect.center().y;
        let colour = p.delta(file.status);

        painter.text(
            Pos2::new(rect.left() + space::LG, cy),
            Align2::LEFT_CENTER,
            file.status.code(),
            FontId::new(text::size::LABEL, text::mono_family()),
            colour,
        );

        // Counts sit at the right, so the eye can scan magnitude down a column.
        let mut right = rect.right() - space::MD;
        if file.omitted.is_none() {
            for (count, colour) in [(file.deletions, p.removed), (file.additions, p.added)] {
                if count == 0 {
                    continue;
                }
                let sign = if colour == p.added { '+' } else { '−' };
                let galley = painter.layout_no_wrap(
                    format!("{sign}{count}"),
                    FontId::new(text::size::CAPTION, text::mono_family()),
                    colour,
                );
                right -= galley.size().x;
                painter.galley(Pos2::new(right, cy - galley.size().y / 2.0), galley, colour);
                right -= space::SM;
            }
        }

        let name_x = rect.left() + space::LG + 14.0;
        let name_color = if active { p.text } else { p.text_secondary };
        let avail = (right - name_x - space::MD).max(20.0);
        let galley = painter.layout(
            file.file_name().to_owned(),
            FontId::new(text::size::BODY, egui::FontFamily::Proportional),
            name_color,
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
                name_color,
            );

        let tip = match (&file.old_path, file.omitted) {
            (Some(old), _) => format!("{old} → {}", file.path),
            (_, Some(Omitted::Binary)) => format!("{} — binary", file.path),
            (_, Some(Omitted::TooLarge)) => format!("{} — too large to display", file.path),
            (_, Some(Omitted::Submodule)) => format!("{} — submodule", file.path),
            _ => file.path.clone(),
        };
        let resp = resp.on_hover_text(tip);

        // The whole-file action button appears on hover only: it is a
        // destructive-adjacent control and should not compete with the
        // filenames for attention when you are just reading.
        if let Some(direction) = self.direction {
            if resp.hovered() || active {
                let action = direction.primary();
                let button = Rect::from_min_size(
                    Pos2::new(rect.right() - 26.0, rect.center().y - 9.0),
                    Vec2::new(18.0, 18.0),
                );
                let hit = ui.interact(
                    button,
                    ui.id().with(("file_action", &file.path)),
                    Sense::click(),
                );
                let colour = if hit.hovered() {
                    p.accent
                } else {
                    p.text_muted
                };
                ui.painter().rect_filled(
                    button,
                    CornerRadius::same(super::radius::SM),
                    if hit.hovered() {
                        p.bg_overlay
                    } else {
                        p.bg_raised
                    },
                );
                ui.painter().text(
                    button.center(),
                    Align2::CENTER_CENTER,
                    action.glyph(),
                    text::icon_font(11.0),
                    colour,
                );
                if hit
                    .on_hover_text(format!("{} this file", action.label()))
                    .clicked()
                {
                    return FileRowHit::Action(action);
                }
            }
        }

        if resp.clicked() {
            FileRowHit::Select
        } else {
            FileRowHit::None
        }
    }

    // ------------------------------------------------------------- file body

    fn file_body(&self, ui: &mut Ui, file: &FileDiff, out: &mut DiffResponse) {
        let p = self.palette;

        // Path header, so you always know what you are reading.
        Frame::NONE
            .fill(p.bg_surface)
            .inner_margin(Margin::symmetric(space::LG as i8, space::SM as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = space::SM;
                    ui.label(
                        text::icon_sized(icon_for(file.status), 13.0).color(p.delta(file.status)),
                    );
                    if let Some(old) = &file.old_path {
                        ui.label(text::caption(old).color(p.text_muted));
                        ui.label(text::caption("→").color(p.text_muted));
                    }
                    ui.label(text::medium(&file.path).color(p.text));

                    // Inspection actions live on the file they apply to, which
                    // is where you are already looking when you want them.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // A deleted or binary file has nothing to blame.
                        if file.omitted.is_none()
                            && file.status != Delta::Deleted
                            && self.header_button(ui, icons::USERS_THREE, "Blame this file")
                        {
                            out.blame_file = Some(file.path.clone());
                        }
                        if self.header_button(
                            ui,
                            icons::CLOCK_COUNTER_CLOCKWISE,
                            "History of this file",
                        ) {
                            out.file_history = Some(file.path.clone());
                        }
                    });
                });
            });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.min_rect().bottom(),
            Stroke::new(1.0, p.border),
        );

        if let Some(lfs) = &file.lfs {
            self.lfs_change(ui, file, lfs);
            return;
        }

        if let Some(preview) = &file.image {
            self.image_diff(ui, file, preview);
            return;
        }

        if let Some(reason) = file.omitted {
            let message = match reason {
                Omitted::Binary => "Binary file — no textual diff to show",
                Omitted::TooLarge => "This file is too large to display",
                Omitted::Submodule => "Submodule pointer changed",
            };
            self.empty_state(ui, message);
            return;
        }
        if file.hunks.is_empty() {
            self.empty_state(ui, "No content changes (mode or metadata only)");
            return;
        }

        out.body_height = ui.available_height();

        let layout = FileLayout::new(file, self.layout);
        let num_font = FontId::new(text::size::CAPTION, text::mono_family());
        let body_font = FontId::new(text::size::MONO, text::mono_family());
        // The body font is monospace, so one advance width positions any
        // character index. Measuring once per frame beats laying out prefixes.
        let advance = ui
            .painter()
            .layout_no_wrap("0".to_owned(), body_font.clone(), egui::Color32::WHITE)
            .size()
            .x;

        egui::ScrollArea::both()
            .id_salt("diff_body")
            .auto_shrink([false, false])
            .show_rows(ui, LINE_HEIGHT, layout.total, |ui, range| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.0;
                for index in range {
                    let Some(row) = layout.row(index) else {
                        continue;
                    };
                    let (rect, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), LINE_HEIGHT),
                        Sense::hover(),
                    );
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    match row {
                        Row::Header(hunk_index, hunk) => {
                            if let Some(action) =
                                self.hunk_header(ui, rect, hunk_index, hunk, &num_font)
                            {
                                out.hunk_action = Some((hunk_index, action));
                            }
                        }
                        Row::Line(hunk_index, line_index, line) => {
                            let selected = self.line_selection.contains(&(hunk_index, line_index));
                            if self
                                .diff_line(ui, rect, line, selected, &num_font, &body_font, advance)
                            {
                                let shift = ui.input(|i| i.modifiers.shift);
                                out.line_clicked = Some((hunk_index, line_index, shift));
                            }
                        }
                        Row::Pair(hunk_index, pair) => {
                            if let Some((line_index, shift)) = self.paired_row(
                                ui,
                                rect,
                                &file.hunks[hunk_index],
                                hunk_index,
                                pair,
                                &num_font,
                                &body_font,
                            ) {
                                out.line_clicked = Some((hunk_index, line_index, shift));
                            }
                        }
                    }
                }
            });
    }

    /// Describe a change to a file stored in Git LFS.
    ///
    /// The pointer's own three lines are never shown: they are an
    /// implementation detail, and diffing them tells you a hash changed, which
    /// you already knew from the fact that the file changed.
    fn lfs_change(&self, ui: &mut Ui, file: &FileDiff, lfs: &crate::git::lfs::LfsChange) {
        let p = self.palette;
        Frame::NONE
            .inner_margin(Margin::same(space::XL as i8))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = space::SM;
                    ui.label(text::icon_sized(icons::CLOUD_ARROW_DOWN, 14.0).color(p.info));
                    ui.label(text::medium("Stored in Git LFS").color(p.text));
                });
                ui.add_space(space::SM);

                if let Some(delta) = lfs.size_delta() {
                    let (label, colour) = if delta > 0 {
                        (format!("grew by {}", bytes(delta.unsigned_abs())), p.added)
                    } else if delta < 0 {
                        (
                            format!("shrank by {}", bytes(delta.unsigned_abs())),
                            p.removed,
                        )
                    } else {
                        ("same size".to_owned(), p.text_muted)
                    };
                    ui.label(text::caption(label).color(colour));
                    ui.add_space(space::MD);
                }

                for (title, pointer, downloaded) in [
                    ("Before", lfs.old.as_ref(), lfs.old_downloaded),
                    ("After", lfs.new.as_ref(), lfs.new_downloaded),
                ] {
                    ui.label(text::overline(title).color(p.text_muted));
                    ui.add_space(space::XS);
                    match pointer {
                        Some(pointer) => {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = space::MD;
                                ui.label(text::hash(pointer.short()).color(p.text_secondary));
                                ui.label(text::caption(bytes(pointer.size)).color(p.text));
                                // Saying whether the object is here matters:
                                // otherwise "nothing to show" is ambiguous
                                // between "unchanged" and "not downloaded".
                                if downloaded {
                                    ui.label(text::caption("downloaded").color(p.added));
                                } else {
                                    ui.label(text::caption("not downloaded").color(p.warning))
                                        .on_hover_text("Run `git lfs pull` to fetch it");
                                }
                            });
                        }
                        None => {
                            ui.label(
                                text::caption(if title == "Before" {
                                    "added in this change"
                                } else {
                                    "deleted in this change"
                                })
                                .color(p.text_muted),
                            );
                        }
                    }
                    ui.add_space(space::MD);
                }

                // A downloaded image is worth showing, LFS or not.
                if let Some(preview) = &file.image {
                    ui.separator();
                    ui.add_space(space::MD);
                    self.image_diff(ui, file, preview);
                }
            });
    }

    /// Show both versions of a changed image side by side.
    ///
    /// "Binary file — no textual diff" is technically true of a picture and
    /// entirely useless, so images get the one comparison that actually answers
    /// the question: what did it look like, and what does it look like now.
    fn image_diff(&self, ui: &mut Ui, file: &FileDiff, preview: &crate::git::diff::ImagePreview) {
        let p = self.palette;
        egui::ScrollArea::vertical()
            .id_salt("image_diff")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(space::LG);
                let column = (ui.available_width() - space::XL * 3.0) / 2.0;

                // Both sides are drawn at the *same* scale, and never enlarged
                // past their natural size. Fitting each one to its own column
                // independently would make a small image look as big as a large
                // one — hiding the very difference the view exists to show.
                let widest = preview
                    .old
                    .iter()
                    .chain(preview.new.iter())
                    .map(|s| s.width)
                    .max()
                    .unwrap_or(1)
                    .max(1) as f32;
                let tallest = preview
                    .old
                    .iter()
                    .chain(preview.new.iter())
                    .map(|s| s.height)
                    .max()
                    .unwrap_or(1)
                    .max(1) as f32;
                let scale = (column / widest).min(320.0 / tallest).min(1.0);

                ui.horizontal_top(|ui| {
                    ui.add_space(space::XL);
                    for (label, side, accent) in [
                        ("Before", preview.old.as_ref(), p.removed),
                        ("After", preview.new.as_ref(), p.added),
                    ] {
                        ui.allocate_ui(Vec2::new(column, ui.available_height()), |ui| {
                            ui.vertical(|ui| {
                                ui.label(text::overline(label).color(accent));
                                ui.add_space(space::SM);

                                match side {
                                    Some(side) => {
                                        // A stable URI per content, so the
                                        // texture cache doesn't reload on every
                                        // frame or collide between files.
                                        let uri = format!(
                                            "bytes://{}-{label}-{}",
                                            file.path,
                                            side.bytes.len()
                                        );
                                        ui.add(
                                            egui::Image::from_bytes(uri, (*side.bytes).clone())
                                                .fit_to_exact_size(Vec2::new(
                                                    side.width as f32 * scale,
                                                    side.height as f32 * scale,
                                                ))
                                                .corner_radius(CornerRadius::same(
                                                    super::radius::SM,
                                                ))
                                                .show_loading_spinner(true),
                                        );
                                        ui.add_space(space::SM);
                                        ui.label(
                                            text::caption(format!(
                                                "{} × {}  ·  {}",
                                                side.width,
                                                side.height,
                                                humansize::format_size(
                                                    side.bytes.len(),
                                                    humansize::DECIMAL,
                                                )
                                            ))
                                            .color(p.text_muted),
                                        );
                                    }
                                    None => {
                                        ui.label(
                                            text::caption(if label == "Before" {
                                                "Added in this change"
                                            } else {
                                                "Deleted in this change"
                                            })
                                            .color(p.text_muted),
                                        );
                                    }
                                }
                            });
                        });
                        ui.add_space(space::XL);
                    }
                });
                ui.add_space(space::LG);
            });
    }

    /// A small icon button for the file header strip.
    fn header_button(&self, ui: &mut Ui, glyph: &str, tip: &str) -> bool {
        let p = self.palette;
        ui.add(
            egui::Button::new(text::icon_sized(glyph, 13.0).color(p.text_muted))
                .fill(egui::Color32::TRANSPARENT)
                .stroke(Stroke::NONE)
                .min_size(Vec2::new(24.0, 20.0)),
        )
        .on_hover_text(tip)
        .clicked()
    }

    /// One side-by-side row: old on the left, new on the right.
    #[allow(clippy::too_many_arguments)]
    fn paired_row(
        &self,
        ui: &mut Ui,
        rect: Rect,
        hunk: &Hunk,
        hunk_index: usize,
        pair: crate::git::diff::Pair,
        num_font: &FontId,
        body_font: &FontId,
    ) -> Option<(usize, bool)> {
        let p = self.palette;
        let mid = rect.center().x;
        let mut clicked = None;

        for (slot, line_index) in [(0usize, pair.left), (1usize, pair.right)] {
            let cell = if slot == 0 {
                Rect::from_min_max(rect.left_top(), Pos2::new(mid, rect.bottom()))
            } else {
                Rect::from_min_max(Pos2::new(mid, rect.top()), rect.right_bottom())
            };

            let Some(line_index) = line_index else {
                // No counterpart on this side: a neutral band, so the eye can
                // see that the two columns are still in step.
                ui.painter()
                    .rect_filled(cell, CornerRadius::ZERO, p.bg_surface);
                continue;
            };
            let line = &hunk.lines[line_index];
            let selected = self.line_selection.contains(&(hunk_index, line_index));

            let selectable = self.direction.is_some() && line.kind != LineKind::Context;
            let response = selectable.then(|| {
                ui.interact(
                    cell,
                    ui.id().with(("sbs", rect.top().to_bits(), slot)),
                    Sense::click(),
                )
            });

            let (bg, fg) = match line.kind {
                LineKind::Addition => (Some(p.added_bg), p.text),
                LineKind::Deletion => (Some(p.removed_bg), p.text),
                LineKind::Context => (None, p.text_secondary),
                LineKind::NoNewline => (None, p.text_muted),
            };
            if let Some(bg) = bg {
                ui.painter().rect_filled(cell, CornerRadius::ZERO, bg);
            }
            if selected {
                ui.painter()
                    .rect_filled(cell, CornerRadius::ZERO, p.accent.gamma_multiply(0.18));
            } else if response.as_ref().is_some_and(|r| r.hovered()) {
                ui.painter()
                    .rect_filled(cell, CornerRadius::ZERO, p.hover.gamma_multiply(0.6));
            }

            let painter = ui.painter();
            let cy = cell.center().y;
            let number = if slot == 0 {
                line.old_lineno
            } else {
                line.new_lineno
            };
            if let Some(n) = number {
                painter.text(
                    Pos2::new(cell.left() + GUTTER_NUM_WIDTH - space::SM, cy),
                    Align2::RIGHT_CENTER,
                    n,
                    num_font.clone(),
                    p.text_muted,
                );
            }
            painter.vline(
                cell.left() + GUTTER_NUM_WIDTH,
                cell.y_range(),
                Stroke::new(1.0, p.border),
            );

            let text_x = cell.left() + GUTTER_NUM_WIDTH + space::SM;
            let clipped = painter.with_clip_rect(cell);
            self.paint_code(&clipped, Pos2::new(text_x, cy), line, body_font, fg);

            if response.is_some_and(|r| r.clicked()) {
                clicked = Some((line_index, ui.input(|i| i.modifiers.shift)));
            }
        }

        // The divider between the two columns, drawn last so nothing covers it.
        ui.painter()
            .vline(mid, rect.y_range(), Stroke::new(1.0, p.border_strong));
        clicked
    }

    /// Draw a diff line's text, applying its syntax spans.
    ///
    /// Shared by both layouts: the colours live in the model, so the only
    /// difference between unified and side-by-side is where the text goes.
    fn paint_code(
        &self,
        painter: &egui::Painter,
        pos: Pos2,
        line: &crate::git::diff::DiffLine,
        font: &FontId,
        fallback: egui::Color32,
    ) {
        let content = line.display_text();
        if line.spans.is_empty() {
            painter.text(pos, Align2::LEFT_CENTER, content, font.clone(), fallback);
            return;
        }

        let mut job = egui::text::LayoutJob::default();
        let mut offset = 0usize;
        for span in &line.spans {
            let end = (offset + span.len as usize).min(content.len());
            if end <= offset {
                break;
            }
            // Spans are byte offsets; a malformed one must not panic the UI.
            let Some(text) = content.get(offset..end) else {
                break;
            };
            job.append(
                text,
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color: egui::Color32::from_rgb(span.color[0], span.color[1], span.color[2]),
                    ..Default::default()
                },
            );
            offset = end;
        }
        if offset < content.len() {
            if let Some(rest) = content.get(offset..) {
                job.append(
                    rest,
                    0.0,
                    egui::TextFormat {
                        font_id: font.clone(),
                        color: fallback,
                        ..Default::default()
                    },
                );
            }
        }
        let galley = painter.layout_job(job);
        painter.galley(
            Pos2::new(pos.x, pos.y - galley.size().y / 2.0),
            galley,
            fallback,
        );
    }

    fn hunk_header(
        &self,
        ui: &mut Ui,
        rect: Rect,
        hunk_index: usize,
        hunk: &Hunk,
        font: &FontId,
    ) -> Option<RowAction> {
        let p = self.palette;
        ui.painter()
            .rect_filled(rect, CornerRadius::ZERO, p.bg_surface);
        ui.painter()
            .hline(rect.x_range(), rect.top(), Stroke::new(1.0, p.border));
        ui.painter().text(
            Pos2::new(rect.left() + space::MD, rect.center().y),
            Align2::LEFT_CENTER,
            &hunk.header,
            font.clone(),
            p.text_muted,
        );

        let direction = self.direction?;
        let hovered = ui.rect_contains_pointer(rect);
        if !hovered {
            return None;
        }

        // Actions are laid out right to left so the primary one sits furthest
        // right, where the pointer already is after reading the hunk.
        let mut actions = vec![direction.primary()];
        if direction.allows_discard() {
            actions.push(RowAction::Discard);
        }

        let mut result = None;
        let mut right = rect.right() - space::MD;
        for action in actions {
            let label = action.label();
            let galley = ui.painter().layout_no_wrap(
                label.to_owned(),
                FontId::new(text::size::CAPTION, egui::FontFamily::Proportional),
                p.text,
            );
            let width = galley.size().x + 18.0;
            let button = Rect::from_min_size(
                Pos2::new(right - width, rect.center().y - 8.0),
                Vec2::new(width, 16.0),
            );
            let hit = ui.interact(
                button,
                ui.id().with(("hunk_action", hunk_index, label)),
                Sense::click(),
            );
            let (bg, fg) = match (hit.hovered(), action) {
                (true, RowAction::Discard) => (p.danger.gamma_multiply(0.25), p.danger),
                (true, _) => (p.accent.gamma_multiply(0.25), p.accent),
                (false, _) => (p.bg_raised, p.text_secondary),
            };
            ui.painter()
                .rect_filled(button, CornerRadius::same(super::radius::SM), bg);
            ui.painter().text(
                Pos2::new(button.left() + 6.0, button.center().y),
                Align2::LEFT_CENTER,
                action.glyph(),
                text::icon_font(9.0),
                fg,
            );
            ui.painter().text(
                Pos2::new(button.left() + 16.0, button.center().y),
                Align2::LEFT_CENTER,
                label,
                FontId::new(text::size::CAPTION, egui::FontFamily::Proportional),
                fg,
            );
            if hit.clicked() {
                result = Some(action);
            }
            right -= width + space::SM;
        }
        result
    }

    /// Returns true when the line was clicked.
    #[allow(clippy::too_many_arguments)]
    fn diff_line(
        &self,
        ui: &mut Ui,
        rect: Rect,
        line: &crate::git::diff::DiffLine,
        selected: bool,
        num_font: &FontId,
        body_font: &FontId,
        advance: f32,
    ) -> bool {
        let p = self.palette;
        // Only changed lines are selectable: staging a context line is
        // meaningless, and letting it highlight would suggest otherwise.
        let selectable = self.direction.is_some() && line.kind != LineKind::Context;
        let response = selectable.then(|| {
            ui.interact(
                rect,
                ui.id().with(("diff_line", rect.top().to_bits())),
                Sense::click(),
            )
        });
        let painter = ui.painter();

        let (bg, fg) = match line.kind {
            LineKind::Addition => (Some(p.added_bg), p.text),
            LineKind::Deletion => (Some(p.removed_bg), p.text),
            LineKind::Context => (None, p.text_secondary),
            LineKind::NoNewline => (None, p.text_muted),
        };
        if let Some(bg) = bg {
            painter.rect_filled(rect, CornerRadius::ZERO, bg);
        }
        if selected {
            painter.rect_filled(rect, CornerRadius::ZERO, p.accent.gamma_multiply(0.18));
            // A solid edge marker, so a run of selected lines reads as a block.
            painter.rect_filled(
                Rect::from_min_size(rect.left_top(), Vec2::new(3.0, rect.height())),
                CornerRadius::ZERO,
                p.accent,
            );
        } else if response.as_ref().is_some_and(|r| r.hovered()) {
            painter.rect_filled(rect, CornerRadius::ZERO, p.hover.gamma_multiply(0.6));
        }

        let cy = rect.center().y;
        let mut x = rect.left();

        // Two line-number columns: old, then new. A line only has the number
        // for the side it exists on, which is what makes an add or a delete
        // legible at a glance without reading the sign.
        for number in [line.old_lineno, line.new_lineno] {
            if let Some(n) = number {
                painter.text(
                    Pos2::new(x + GUTTER_NUM_WIDTH - space::SM, cy),
                    Align2::RIGHT_CENTER,
                    n,
                    num_font.clone(),
                    p.text_muted,
                );
            }
            x += GUTTER_NUM_WIDTH;
        }

        painter.vline(x, rect.y_range(), Stroke::new(1.0, p.border));

        let sign_colour = match line.kind {
            LineKind::Addition => p.added,
            LineKind::Deletion => p.removed,
            _ => p.text_muted,
        };
        if line.kind != LineKind::Context {
            painter.text(
                Pos2::new(x + space::SM, cy),
                Align2::LEFT_CENTER,
                line.kind.sign(),
                body_font.clone(),
                sign_colour,
            );
        }
        x += SIGN_WIDTH;

        // The parts of the line that actually differ get a stronger tint, drawn
        // under the text. Without this, a one-token change looks exactly like a
        // rewritten line.
        if !line.emphasis.is_empty() {
            let tint = match line.kind {
                LineKind::Addition => p.added.gamma_multiply(0.28),
                LineKind::Deletion => p.removed.gamma_multiply(0.28),
                _ => egui::Color32::TRANSPARENT,
            };
            for mark in &line.emphasis {
                let x0 = x + mark.start as f32 * advance;
                let x1 = x + mark.end as f32 * advance;
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(x0, rect.top()), Pos2::new(x1, rect.bottom())),
                    CornerRadius::same(2),
                    tint,
                );
            }
        }

        self.paint_code(painter, Pos2::new(x, cy), line, body_font, fg);

        if line.truncated {
            painter.text(
                Pos2::new(rect.right() - space::MD, cy),
                Align2::RIGHT_CENTER,
                "…truncated",
                num_font.clone(),
                p.warning,
            );
        }

        response.is_some_and(|r| r.clicked())
    }
}

/// Human-readable byte count.
fn bytes(count: u64) -> String {
    humansize::format_size(count, humansize::DECIMAL)
}

fn icon_for(delta: Delta) -> &'static str {
    match delta {
        Delta::Added | Delta::Untracked => icons::FILE_PLUS,
        Delta::Deleted => icons::FILE_MINUS,
        Delta::Renamed | Delta::Copied => icons::ARROW_BEND_UP_RIGHT,
        Delta::Conflicted => icons::WARNING,
        _ => icons::FILE,
    }
}
