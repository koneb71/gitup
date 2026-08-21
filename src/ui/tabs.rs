//! The tab bar.
//!
//! One tab per open repository. Tabs carry a badge for anything that wants
//! attention — uncommitted work, conflicts, an operation mid-flight — because
//! the whole point of having several open is that you are not looking at most
//! of them, and a repository that quietly went into conflict while you were
//! elsewhere is exactly what you need told about.

use super::{icons, radius, space, text, Palette};
use crate::state::{Session, SessionBadge};
use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};

const HEIGHT: f32 = 32.0;
/// Narrow enough for a short name, wide enough that the label is not a stub.
const MIN_WIDTH: f32 = 88.0;
const MAX_WIDTH: f32 = 200.0;
/// Padding either side of a tab's contents.
const PAD: f32 = 10.0;
/// Space kept for the close button, whether or not it is being drawn.
///
/// Reserved always so that hovering a tab cannot change its width — a row of
/// tabs that reflows as the pointer crosses it is worse than one that carries
/// a little trailing space.
const CLOSE: f32 = 20.0;
/// Space kept for the status badge.
const BADGE: f32 = 14.0;

#[derive(Debug, Default)]
pub struct TabsResponse {
    /// Show the tab at this position.
    pub activate: Option<usize>,
    pub close: Option<usize>,
    pub open_new: bool,
}

/// One tab's worth of what the bar needs to draw.
pub struct TabInfo<'a> {
    pub session: &'a Session,
    /// True while this repository has an operation running.
    pub busy: bool,
}

pub struct TabBar<'a> {
    pub palette: &'a Palette,
    /// Sessions in tab order.
    pub tabs: Vec<TabInfo<'a>>,
    pub active: usize,
}

impl TabBar<'_> {
    pub fn show(&self, ui: &mut Ui) -> TabsResponse {
        let mut out = TabsResponse::default();
        let p = self.palette;

        // Two tabs of the same repository name — a worktree, a second clone —
        // are common enough that the name alone would be ambiguous. Only the
        // ones that collide get a qualifier, so the common case stays clean.
        let duplicated = self.duplicated_titles();

        // Tabs shrink to fit rather than scrolling, up to a floor: hunting for
        // a tab that is off-screen is worse than a slightly cramped label.
        let reserved = HEIGHT; // the "+" button
        let available = (ui.available_width() - reserved - space::MD).max(MIN_WIDTH);
        let widths = self.widths(ui, available);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;

            egui::ScrollArea::horizontal()
                .id_salt("tab_scroll")
                .max_width(ui.available_width() - reserved)
                // Shrink to the tabs when they fit, so the "+" sits beside the
                // last one rather than stranded at the window edge.
                .auto_shrink([true, true])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for (position, tab) in self.tabs.iter().enumerate() {
                            let qualifier = duplicated
                                .contains(&tab.session.title())
                                .then(|| tab.session.disambiguator())
                                .flatten();
                            let width = widths.get(position).copied().unwrap_or(MIN_WIDTH);
                            self.tab(ui, position, tab, qualifier, width, &mut out);
                        }
                    });
                });

            if ui
                .add(
                    egui::Button::new(text::icon_sized(icons::PLUS, 13.0).color(p.text_muted))
                        .fill(Color32::TRANSPARENT)
                        .stroke(Stroke::NONE)
                        .min_size(Vec2::new(HEIGHT, HEIGHT)),
                )
                .on_hover_text(format!(
                    "Open another repository  {}",
                    crate::ui::keymap::Chord::cmd(egui::Key::T).display()
                ))
                .clicked()
            {
                out.open_new = true;
            }
        });

        out
    }

    /// How wide each tab should be.
    ///
    /// Sized to its own label rather than to an equal share of the bar. Equal
    /// shares are what a spreadsheet's column headers look like: with four
    /// repositories open, "cap3" was given exactly as much room as
    /// "DevCapBackend", and the bar read as a table rather than as tabs.
    ///
    /// When the natural widths do not fit, every tab is scaled by the same
    /// factor down to [`MIN_WIDTH`] — so they stay in proportion to each other
    /// instead of the last one being cut off.
    fn widths(&self, ui: &Ui, available: f32) -> Vec<f32> {
        let font = FontId::new(text::size::LABEL, egui::FontFamily::Proportional);
        let natural: Vec<f32> = self
            .tabs
            .iter()
            .map(|tab| {
                let label = ui.painter().layout_no_wrap(
                    tab.session.title(),
                    font.clone(),
                    Color32::PLACEHOLDER,
                );
                let badge = if tab.busy || tab.session.badge().is_some() {
                    BADGE
                } else {
                    0.0
                };
                (PAD + badge + label.size().x + CLOSE + PAD).clamp(MIN_WIDTH, MAX_WIDTH)
            })
            .collect();

        let total: f32 = natural.iter().sum();
        if total <= available || total <= 0.0 {
            return natural;
        }
        let factor = available / total;
        natural
            .iter()
            .map(|w| (w * factor).max(MIN_WIDTH))
            .collect()
    }

    fn duplicated_titles(&self) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        let mut duplicated: Vec<String> = Vec::new();
        for tab in &self.tabs {
            let title = tab.session.title();
            if seen.contains(&title) {
                if !duplicated.contains(&title) {
                    duplicated.push(title.clone());
                }
            } else {
                seen.push(title);
            }
        }
        duplicated
    }

    #[allow(clippy::too_many_arguments)]
    fn tab(
        &self,
        ui: &mut Ui,
        position: usize,
        tab: &TabInfo<'_>,
        qualifier: Option<String>,
        width: f32,
        out: &mut TabsResponse,
    ) {
        let p = self.palette;
        let session = tab.session;
        let active = position == self.active;
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, HEIGHT), Sense::click());

        // The active tab is filled with the colour of the content below it, so
        // it reads as the front of that content rather than as a cell in a
        // table. Rounding only the top two corners is what joins it to what it
        // is showing; the accent sits on the top edge, away from that join.
        let top_rounded = CornerRadius {
            nw: radius::MD,
            ne: radius::MD,
            sw: 0,
            se: 0,
        };
        if active {
            ui.painter().rect_filled(rect, top_rounded, p.bg_base);
            ui.painter().rect_filled(
                Rect::from_min_size(
                    Pos2::new(rect.left(), rect.top()),
                    Vec2::new(rect.width(), 2.0),
                ),
                CornerRadius {
                    nw: radius::MD,
                    ne: radius::MD,
                    sw: 0,
                    se: 0,
                },
                p.accent,
            );
        } else if response.hovered() {
            ui.painter().rect_filled(rect, top_rounded, p.hover);
        }

        // No divider after the active tab or its left neighbour: a rule running
        // into a rounded corner is what made the bar look ruled rather than
        // tabbed. Inactive neighbours still get a hairline, inset so it reads
        // as a separator rather than as a column edge.
        let next_is_active = position + 1 == self.active;
        if !active && !next_is_active && position + 1 < self.tabs.len() {
            let inset = 7.0;
            ui.painter().vline(
                rect.right(),
                egui::Rangef::new(rect.top() + inset, rect.bottom() - inset),
                Stroke::new(1.0, p.border),
            );
        }

        let painter = ui.painter();
        let cy = rect.center().y;
        let mut left = rect.left() + PAD;

        // The badge sits before the name, where a status light belongs. Work in
        // flight outranks everything else: it is the one state that is about to
        // change on its own.
        let badge = if tab.busy {
            Some((p.accent, icons::CIRCLE_NOTCH))
        } else {
            session.badge().map(|badge| match badge {
                SessionBadge::Conflicts(_) => (p.danger, icons::WARNING),
                SessionBadge::InProgress => (p.warning, icons::CIRCLE_NOTCH),
                SessionBadge::Changes(_) => (p.modified, icons::CIRCLE),
            })
        };
        if let Some((colour, glyph)) = badge {
            painter.text(
                Pos2::new(left, cy),
                Align2::LEFT_CENTER,
                glyph,
                text::icon_font(9.0),
                colour,
            );
            left += BADGE;
        }

        // The close button only appears on the active or hovered tab, so a row
        // of tabs is not a row of crosses.
        // The close button's room is always reserved (see `CLOSE`), so the
        // title's clip does not move when the pointer arrives.
        let mut right = rect.right() - PAD;
        if active || response.hovered() {
            let button = Rect::from_center_size(Pos2::new(right - 8.0, cy), Vec2::new(16.0, 16.0));
            let hit = ui.interact(
                button,
                ui.id().with(("tab_close", position)),
                Sense::click(),
            );
            if hit.hovered() {
                ui.painter()
                    .rect_filled(button, CornerRadius::same(radius::SM), p.bg_overlay);
            }
            ui.painter().text(
                button.center(),
                Align2::CENTER_CENTER,
                icons::X,
                text::icon_font(9.0),
                if hit.hovered() { p.text } else { p.text_muted },
            );
            if hit.clicked() {
                out.close = Some(position);
            }
        }
        right -= CLOSE - PAD;

        let title = session.title();
        let colour = if active { p.text } else { p.text_secondary };
        let font = FontId::new(text::size::LABEL, egui::FontFamily::Proportional);
        let avail = (right - left).max(20.0);

        let galley = ui.painter().layout(title, font.clone(), colour, avail);
        let title_width = galley.size().x;
        ui.painter()
            .with_clip_rect(Rect::from_min_max(
                Pos2::new(left, rect.top()),
                Pos2::new(right, rect.bottom()),
            ))
            .galley(Pos2::new(left, cy - galley.size().y / 2.0), galley, colour);

        if let Some(qualifier) = &qualifier {
            let x = left + title_width + space::SM;
            if x < right {
                ui.painter()
                    .with_clip_rect(Rect::from_min_max(
                        Pos2::new(x, rect.top()),
                        Pos2::new(right, rect.bottom()),
                    ))
                    .text(
                        Pos2::new(x, cy),
                        Align2::LEFT_CENTER,
                        qualifier,
                        FontId::new(text::size::CAPTION, egui::FontFamily::Proportional),
                        p.text_muted,
                    );
            }
        }

        let tooltip = {
            let mut lines = Vec::new();
            if let Some(key) = &session.repo.key {
                lines.push(key.0.display().to_string());
            }
            if let Some(head) = &session.repo.head {
                lines.push(head.display_name());
            }
            if tab.busy {
                lines.push("working…".to_owned());
            }
            match session.badge() {
                Some(SessionBadge::Conflicts(n)) => {
                    lines.push(format!("{n} conflicted"));
                }
                Some(SessionBadge::Changes(n)) => lines.push(format!("{n} changed")),
                Some(SessionBadge::InProgress) => lines.push("operation in progress".to_owned()),
                None => {}
            }
            lines.join("\n")
        };
        let response = response.on_hover_text(tooltip);

        if response.clicked() {
            out.activate = Some(position);
        }
        // Middle-click closes, as it does in every tabbed interface.
        if response.middle_clicked() {
            out.close = Some(position);
        }
    }
}

/// The bar's height, so callers can size the panel to match.
pub const fn height() -> f32 {
    HEIGHT
}
