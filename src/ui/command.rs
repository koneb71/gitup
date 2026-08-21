//! The command palette.
//!
//! Every action the app can perform is reachable by typing part of its name.
//! That is the point of the design: a Git client has too many verbs to fit in a
//! toolbar, and burying them in nested menus means the ones you use rarely are
//! the ones you can never find. Branch names and commit hashes match too, so
//! "switch to the parser branch" and "go to commit abc123" are the same gesture
//! as "push".

use super::{icons, radius, space, text, Palette};
use egui::{
    Align2, Color32, CornerRadius, FontId, Frame, Id, Key, Margin, Pos2, Rect, Stroke, Vec2,
};

/// What a palette entry does when chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    OpenRepository,
    /// Show the tab at this position in the bar.
    SwitchTab(usize),
    CloseTab,
    CloneRepository,
    Refresh,
    ToggleTheme,
    ToggleIgnored,
    ToggleSyntax,
    ToggleAutoRefresh,

    StageAll,
    UnstageAll,
    FocusCommitMessage,
    DraftMessage,
    AmendLast,

    Fetch,
    Pull,
    PullRebase,
    Push,

    NewBranch,
    NewTag,
    StashChanges,
    AddRemote,
    AddSubmodule,
    UpdateSubmodules,

    Checkout(String),
    CheckoutRemote(String),
    MergeBranch(String),
    RebaseOnto(String),
    ApplyStash(usize),

    GoToCommit(String),
    SearchMessages,
    ShowHistory,

    AbortOperation,
}

/// One row in the palette.
#[derive(Debug, Clone)]
pub struct Entry {
    pub command: Command,
    pub title: String,
    pub detail: Option<String>,
    pub icon: &'static str,
    pub shortcut: Option<String>,
    /// Higher sorts first among equal match scores.
    pub weight: i32,
}

impl Entry {
    pub fn new(command: Command, title: impl Into<String>, icon: &'static str) -> Self {
        Self {
            command,
            title: title.into(),
            detail: None,
            icon,
            shortcut: None,
            weight: 0,
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Label the entry with the chord that runs it.
    ///
    /// Takes a `String` rather than a literal because the chord depends on the
    /// platform and on the user's own bindings, so it cannot be written down
    /// at the call site.
    pub fn shortcut(mut self, shortcut: String) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    /// As [`shortcut`](Self::shortcut), for an action the user may have unbound.
    pub fn shortcut_opt(mut self, shortcut: Option<String>) -> Self {
        self.shortcut = shortcut;
        self
    }

    pub fn weight(mut self, weight: i32) -> Self {
        self.weight = weight;
        self
    }
}

/// Score `text` against `query` as a subsequence match.
///
/// Returns `None` when the query isn't a subsequence at all. Higher is better:
/// matches at word starts and runs of consecutive characters score highest,
/// which is what makes "nb" find "New branch" ahead of "Rebase onto".
pub fn score(text: &str, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let haystack: Vec<char> = text.chars().collect();
    let needle: Vec<char> = query.chars().collect();

    let mut total = 0i32;
    let mut position = 0usize;
    let mut previous_index: Option<usize> = None;

    for wanted in needle {
        let wanted = wanted.to_ascii_lowercase();
        let found = haystack[position..]
            .iter()
            .position(|c| c.to_ascii_lowercase() == wanted)
            .map(|offset| position + offset)?;

        let mut points = 1;
        // A character at the start, or right after a separator, is a word start.
        let is_word_start =
            found == 0 || matches!(haystack[found - 1], ' ' | '-' | '_' | '/' | '.' | ':');
        if is_word_start {
            points += 8;
        }
        if previous_index == Some(found.wrapping_sub(1)) {
            points += 5;
        }
        // Earlier matches are better, mildly.
        points += ((32 - found.min(32)) / 8) as i32;

        total += points;
        previous_index = Some(found);
        position = found + 1;
    }

    // Shorter titles that matched are more likely to be what was meant.
    total += (40i32 - haystack.len().min(40) as i32) / 4;
    Some(total)
}

/// Filter and rank entries for a query.
pub fn filter(entries: &[Entry], query: &str) -> Vec<Entry> {
    let query = query.trim();
    let mut scored: Vec<(i32, &Entry)> = entries
        .iter()
        .filter_map(|entry| {
            let title = score(&entry.title, query);
            let detail = entry
                .detail
                .as_deref()
                // A detail match counts for less than a title match.
                .and_then(|d| score(d, query).map(|s| s - 12));
            let best = match (title, detail) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            }?;
            Some((best + entry.weight, entry))
        })
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.title.cmp(&b.1.title)));
    scored.into_iter().map(|(_, e)| e.clone()).collect()
}

#[derive(Debug, Default)]
pub struct PaletteResponse {
    pub chosen: Option<Command>,
    pub dismissed: bool,
}

pub struct CommandPalette<'a> {
    pub palette: &'a Palette,
    pub query: &'a mut String,
    pub selected: &'a mut usize,
    pub entries: &'a [Entry],
    /// Set on the frame the palette opens, to claim keyboard focus.
    pub just_opened: bool,
}

impl CommandPalette<'_> {
    pub fn show(&mut self, ctx: &egui::Context) -> PaletteResponse {
        let mut out = PaletteResponse::default();
        let p = self.palette;
        let matches = filter(self.entries, self.query);
        *self.selected = (*self.selected).min(matches.len().saturating_sub(1));

        // Arrow keys move the highlight; they are consumed before the text
        // field sees them so the caret doesn't move instead.
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, Key::ArrowDown) && !matches.is_empty() {
                *self.selected = (*self.selected + 1) % matches.len();
            }
            if i.consume_key(egui::Modifiers::NONE, Key::ArrowUp) && !matches.is_empty() {
                *self.selected = self.selected.checked_sub(1).unwrap_or(matches.len() - 1);
            }
        });

        let response = egui::Modal::new(Id::new("command_palette"))
            .frame(
                Frame::NONE
                    .fill(p.bg_overlay)
                    .stroke(Stroke::new(1.0, p.border_strong))
                    .corner_radius(CornerRadius::same(radius::LG))
                    .inner_margin(Margin::same(space::SM as i8))
                    .shadow(ctx.style_of(ctx.theme()).visuals.window_shadow),
            )
            .show(ctx, |ui| {
                ui.set_width(540.0);

                ui.horizontal(|ui| {
                    ui.add_space(space::MD);
                    ui.label(text::icon_sized(icons::MAGNIFYING_GLASS, 15.0).color(p.text_muted));
                    let field = ui.add_sized(
                        Vec2::new(ui.available_width() - space::MD, 28.0),
                        egui::TextEdit::singleline(self.query)
                            .hint_text("Type a command, branch, or hash…")
                            .frame(Frame::NONE)
                            .font(egui::TextStyle::Body),
                    );
                    if self.just_opened {
                        field.request_focus();
                    }
                    if field.changed() {
                        *self.selected = 0;
                    }
                });

                ui.add_space(space::SM);
                if matches.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(space::LG);
                        ui.label(text::caption("No matching commands").color(p.text_muted));
                    });
                    ui.add_space(space::MD);
                    return;
                }

                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (index, entry) in matches.iter().enumerate() {
                            if self.row(ui, entry, index == *self.selected) {
                                out.chosen = Some(entry.command.clone());
                            }
                        }
                    });
            });

        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
            if let Some(entry) = matches.get(*self.selected) {
                out.chosen = Some(entry.command.clone());
            }
        }
        if response.should_close() || ctx.input(|i| i.key_pressed(Key::Escape)) {
            out.dismissed = true;
        }
        out
    }

    fn row(&self, ui: &mut egui::Ui, entry: &Entry, selected: bool) -> bool {
        let p = self.palette;
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), egui::Sense::click());
        if selected {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(radius::MD), p.selected);
        } else if response.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(radius::MD), p.hover);
        }

        let painter = ui.painter();
        let cy = rect.center().y;
        painter.text(
            Pos2::new(rect.left() + space::LG, cy),
            Align2::LEFT_CENTER,
            entry.icon,
            text::icon_font(14.0),
            if selected { p.accent } else { p.text_muted },
        );

        let mut right = rect.right() - space::LG;
        if let Some(shortcut) = entry.shortcut.clone() {
            let galley = painter.layout_no_wrap(
                shortcut,
                FontId::new(text::size::CAPTION, egui::FontFamily::Proportional),
                p.text_muted,
            );
            right -= galley.size().x;
            painter.galley(
                Pos2::new(right, cy - galley.size().y / 2.0),
                galley,
                p.text_muted,
            );
            right -= space::MD;
        }

        let x = rect.left() + 40.0;
        painter.text(
            Pos2::new(x, cy),
            Align2::LEFT_CENTER,
            &entry.title,
            FontId::new(text::size::BODY, egui::FontFamily::Proportional),
            p.text,
        );
        if let Some(detail) = &entry.detail {
            let title_width = painter
                .layout_no_wrap(
                    entry.title.clone(),
                    FontId::new(text::size::BODY, egui::FontFamily::Proportional),
                    p.text,
                )
                .size()
                .x;
            let detail_x = x + title_width + space::MD;
            if detail_x < right {
                painter
                    .with_clip_rect(Rect::from_min_max(
                        Pos2::new(detail_x, rect.top()),
                        Pos2::new(right, rect.bottom()),
                    ))
                    .text(
                        Pos2::new(detail_x, cy),
                        Align2::LEFT_CENTER,
                        detail,
                        FontId::new(text::size::CAPTION, egui::FontFamily::Proportional),
                        p.text_muted,
                    );
            }
        }

        response.clicked()
    }
}

/// Unused; keeps `Color32` in scope as rows gain states.
#[allow(dead_code)]
fn _color_in_scope(_: Color32) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str) -> Entry {
        Entry::new(Command::Refresh, title, icons::CIRCLE)
    }

    #[test]
    fn an_empty_query_matches_everything() {
        let entries = vec![entry("Push"), entry("Pull")];
        assert_eq!(filter(&entries, "").len(), 2);
    }

    #[test]
    fn non_subsequences_do_not_match() {
        assert!(score("Push", "xyz").is_none());
        assert!(score("Push", "hsup").is_none(), "order matters");
        assert!(score("Push", "psh").is_some(), "gaps are allowed");
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(score("New Branch", "nb").is_some());
        assert!(score("new branch", "NB").is_some());
    }

    #[test]
    fn initials_beat_incidental_matches() {
        let entries = vec![entry("New branch"), entry("Rebase onto branch")];
        let ranked = filter(&entries, "nb");
        assert_eq!(ranked[0].title, "New branch", "got {ranked:?}");
    }

    #[test]
    fn consecutive_matches_beat_scattered_ones() {
        let together = score("stash", "sta").expect("match");
        let scattered = score("set author", "sta").expect("match");
        assert!(
            together > scattered,
            "consecutive {together} should beat scattered {scattered}"
        );
    }

    #[test]
    fn exact_prefixes_rank_first() {
        let entries = vec![entry("Fetch"), entry("Force push"), entry("Refresh")];
        let ranked = filter(&entries, "fe");
        assert_eq!(ranked[0].title, "Fetch", "got {ranked:?}");
    }

    #[test]
    fn a_detail_match_is_found_but_ranks_below_a_title_match() {
        let entries = vec![
            entry("Check out").detail("feature/parser"),
            entry("Parser things"),
        ];
        let ranked = filter(&entries, "parser");
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].title, "Parser things", "title wins");
    }

    #[test]
    fn weight_breaks_ties_toward_important_commands() {
        let entries = vec![
            entry("Push").weight(0),
            Entry::new(Command::Pull, "Push", icons::CIRCLE).weight(50),
        ];
        let ranked = filter(&entries, "push");
        assert_eq!(ranked[0].command, Command::Pull, "the weighted one first");
    }
}
