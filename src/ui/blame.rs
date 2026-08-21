//! The blame view: every line, and who last touched it.
//!
//! The left rail is the point of the view. Metadata is drawn once per run of
//! lines sharing a commit rather than repeated on every line, and each run
//! carries an age bar shaded by how recent it is — so "what changed lately"
//! is answerable by looking, without reading a single date.

use super::{space, text, Palette};
use crate::git::blame::BlameResult;
use crate::util::time;
use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use git2::Oid;

const ROW: f32 = 17.0;
const AGE_BAR: f32 = 3.0;
const META_WIDTH: f32 = 250.0;
const LINE_NO_WIDTH: f32 = 52.0;

#[derive(Debug, Default)]
pub struct BlameResponse {
    /// A commit was clicked in the rail.
    pub selected: Option<Oid>,
    /// Re-blame this file as of the parent of this commit.
    pub reblame_before: Option<Oid>,
}

pub struct BlameView<'a> {
    pub palette: &'a Palette,
    pub result: &'a BlameResult,
    pub highlighted: Option<Oid>,
}

impl BlameView<'_> {
    pub fn show(&self, ui: &mut Ui) -> BlameResponse {
        let mut out = BlameResponse::default();
        let p = self.palette;
        let now = time::now();

        if self.result.lines.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.35);
                ui.label(text::caption("Nothing to blame here").color(p.text_muted));
            });
            return out;
        }

        let mono = FontId::new(text::size::MONO, text::mono_family());
        let meta_font = FontId::new(text::size::CAPTION, egui::FontFamily::Proportional);

        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show_rows(ui, ROW, self.result.lines.len(), |ui, range| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.0;

                for index in range {
                    let line = &self.result.lines[index];
                    let (rect, response) = ui
                        .allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }

                    let is_highlighted = self.highlighted == Some(line.commit);
                    if is_highlighted {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::ZERO, p.selected_inactive);
                    } else if response.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::ZERO, p.hover);
                    }

                    let painter = ui.painter();
                    let cy = rect.center().y;

                    // Age bar: brighter is newer.
                    let recency = self.result.recency(line.commit);
                    let colour = p
                        .lane(*self.result.colors.get(&line.commit).unwrap_or(&0))
                        .gamma_multiply(0.35 + 0.65 * recency);
                    painter.rect_filled(
                        Rect::from_min_size(
                            Pos2::new(rect.left(), rect.top()),
                            Vec2::new(AGE_BAR, rect.height()),
                        ),
                        CornerRadius::ZERO,
                        colour,
                    );

                    if line.starts_group {
                        // Hairline above each group, so runs read as blocks.
                        painter.hline(rect.x_range(), rect.top(), Stroke::new(1.0, p.border));
                        if let Some(commit) = self.result.commit(line.commit) {
                            painter.text(
                                Pos2::new(rect.left() + AGE_BAR + space::MD, cy),
                                Align2::LEFT_CENTER,
                                &commit.short_id,
                                FontId::new(text::size::CAPTION, text::mono_family()),
                                p.accent,
                            );
                            let author_x = rect.left() + AGE_BAR + space::MD + 58.0;
                            painter
                                .with_clip_rect(Rect::from_min_max(
                                    Pos2::new(author_x, rect.top()),
                                    Pos2::new(rect.left() + META_WIDTH - 74.0, rect.bottom()),
                                ))
                                .text(
                                    Pos2::new(author_x, cy),
                                    Align2::LEFT_CENTER,
                                    &commit.author_name,
                                    meta_font.clone(),
                                    p.text_secondary,
                                );
                            painter.text(
                                Pos2::new(rect.left() + META_WIDTH - space::MD, cy),
                                Align2::RIGHT_CENTER,
                                time::relative(commit.time, now),
                                meta_font.clone(),
                                p.text_muted,
                            );
                        }
                    }

                    // Separator between the rail and the code.
                    painter.vline(
                        rect.left() + META_WIDTH,
                        rect.y_range(),
                        Stroke::new(1.0, p.border),
                    );

                    painter.text(
                        Pos2::new(rect.left() + META_WIDTH + LINE_NO_WIDTH - space::MD, cy),
                        Align2::RIGHT_CENTER,
                        line.line_no,
                        FontId::new(text::size::CAPTION, text::mono_family()),
                        p.text_muted,
                    );
                    paint_code(
                        painter,
                        Pos2::new(rect.left() + META_WIDTH + LINE_NO_WIDTH, cy),
                        &line.content,
                        &line.spans,
                        &mono,
                        p.text,
                    );

                    if response.clicked() {
                        out.selected = Some(line.commit);
                    }

                    let tip = match self.result.commit(line.commit) {
                        Some(commit) => format!(
                            "{}\n{} · {}\n\nRight-click to blame before this commit",
                            commit.summary,
                            commit.author_name,
                            time::date_time(commit.time, commit.tz_offset_minutes)
                        ),
                        None => "Unknown commit".to_owned(),
                    };
                    let response = response.on_hover_text(tip);
                    let commit_oid = line.commit;
                    response.context_menu(|ui| {
                        if ui.button("Blame before this commit").clicked() {
                            out.reblame_before = Some(commit_oid);
                            ui.close();
                        }
                        if ui.button("Show this commit").clicked() {
                            out.selected = Some(commit_oid);
                            ui.close();
                        }
                    });
                }
            });

        out
    }
}

/// Draw a line of code with its syntax spans applied.
///
/// The same shape as the diff view's renderer, kept separate because the two
/// disagree about what a "line" is: a diff line carries an old and a new number
/// and a change kind, a blame line carries neither.
fn paint_code(
    painter: &egui::Painter,
    pos: Pos2,
    content: &str,
    spans: &[crate::git::highlight::Span],
    font: &FontId,
    fallback: Color32,
) {
    if spans.is_empty() {
        painter.text(pos, Align2::LEFT_CENTER, content, font.clone(), fallback);
        return;
    }

    let mut job = egui::text::LayoutJob::default();
    let mut offset = 0usize;
    for span in spans {
        let end = (offset + span.len as usize).min(content.len());
        if end <= offset {
            break;
        }
        // Byte offsets from the model; a malformed one must not panic the UI.
        let Some(text) = content.get(offset..end) else {
            break;
        };
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color: Color32::from_rgb(span.color[0], span.color[1], span.color[2]),
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
