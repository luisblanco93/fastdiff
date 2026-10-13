use super::model::TreeRow;
use egui::{Align2, Color32, Pos2, Rect, Response, Stroke, Ui, Vec2, pos2, vec2};
use fastdiff_core::FileChange;

pub(super) fn search_field(ui: &mut Ui, filter: &mut String) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(18.0, 24.0), egui::Sense::hover());
        let center = rect.center() - vec2(2.0, 2.0);
        let stroke = Stroke::new(1.4, ui.visuals().weak_text_color());
        ui.painter().circle_stroke(center, 4.0, stroke);
        ui.painter()
            .line_segment([center + vec2(3.0, 3.0), center + vec2(6.0, 6.0)], stroke);
        ui.add_sized(
            vec2(ui.available_width(), 24.0),
            egui::TextEdit::singleline(filter)
                .id(egui::Id::new("filename-filter"))
                .vertical_align(egui::Align::Center)
                .hint_text("Filter files…"),
        );
    });
    ui.add_space(5.0);
}

pub(super) fn row_button(
    ui: &mut Ui,
    row: &TreeRow,
    file: Option<&FileChange>,
    selected: bool,
    expanded: bool,
    theme: bool,
) -> Response {
    let response = ui
        .push_id((row.file.is_some(), &row.key), |ui| {
            ui.add_sized(
                vec2(ui.available_width(), 24.0),
                egui::Button::new("").frame(false).selected(selected),
            )
        })
        .inner;
    let label = file.map(|file| file.path.display()).unwrap_or_else(|| {
        format!(
            "{} folder {}",
            if expanded { "Collapse" } else { "Expand" },
            row.label
        )
    });
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, &label)
    });
    if ui.is_rect_visible(response.rect) {
        let rect = response.rect;
        let visuals = ui.style().interact_selectable(&response, selected);
        if selected || response.hovered() || response.has_focus() {
            ui.painter().rect_filled(rect, 3.0, visuals.weak_bg_fill);
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.shrink(0.5),
                3.0,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Inside,
            );
        }
        let available = rect.width();
        let indent = (row.depth as f32 * 16.0).min((available - 100.0).max(0.0));
        let color = visuals.text_color();
        let icon_color = ui.visuals().weak_text_color();
        let arrow = pos2(rect.left() + indent + 10.0, rect.center().y);
        if file.is_none() {
            disclosure(ui, arrow, expanded, icon_color);
        }
        let icon = Rect::from_center_size(arrow + vec2(20.0, 0.0), vec2(14.0, 14.0));
        if !theme || !super::icons::paint(ui, icon, file.map(|file| file.path.0.as_slice())) {
            if file.is_none() {
                folder_icon(ui, icon, expanded, icon_color);
            } else {
                document_icon(ui, icon, icon_color);
            }
        }
        let text_left = icon.right() + 7.0;
        let badge = file.map(status_badge).unwrap_or("");
        let badge_width = if badge.is_empty() { 0.0 } else { 35.0 };
        let label_rect = Rect::from_min_max(
            pos2(text_left, rect.top()),
            pos2(
                (rect.right() - badge_width - 5.0).max(text_left),
                rect.bottom(),
            ),
        );
        let font = egui::TextStyle::Body.resolve(ui.style());
        let mut job = egui::text::LayoutJob::simple(
            row.label.clone(),
            font.clone(),
            color,
            label_rect.width(),
        );
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        ui.painter()
            .with_clip_rect(label_rect.intersect(ui.clip_rect()))
            .galley(
                pos2(text_left, rect.center().y - galley.size().y / 2.0),
                galley,
                color,
            );
        if !badge.is_empty() {
            let badge_color = file
                .map(|file| {
                    if file.conflict {
                        Color32::from_rgb(245, 120, 120)
                    } else if file.untracked {
                        Color32::from_rgb(110, 205, 145)
                    } else {
                        Color32::from_rgb(145, 180, 215)
                    }
                })
                .unwrap_or(color);
            ui.painter().text(
                pos2(rect.right() - 6.0, rect.center().y),
                Align2::RIGHT_CENTER,
                badge,
                font,
                badge_color,
            );
        }
    }
    response
}

pub(super) fn status_badge(file: &FileChange) -> &'static str {
    if file.conflict {
        "!"
    } else if file.untracked {
        "+"
    } else if file.staged && file.unstaged {
        "S+U"
    } else if file.staged {
        "S"
    } else if file.unstaged {
        "U"
    } else {
        ""
    }
}

fn disclosure(ui: &Ui, center: Pos2, expanded: bool, color: Color32) {
    let points = if expanded {
        [
            center + vec2(-3.5, -2.0),
            center + vec2(3.5, -2.0),
            center + vec2(0.0, 2.5),
        ]
    } else {
        [
            center + vec2(-2.0, -3.5),
            center + vec2(-2.0, 3.5),
            center + vec2(2.5, 0.0),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points.to_vec(),
        color,
        Stroke::NONE,
    ));
}

fn folder_icon(ui: &Ui, rect: Rect, open: bool, color: Color32) {
    let left = rect.left();
    let top = rect.top() + 3.0;
    let points = vec![
        pos2(left, top),
        pos2(left + 5.0, top),
        pos2(left + 7.0, top + 2.0),
        pos2(rect.right(), top + 2.0),
        pos2(rect.right(), rect.bottom() - 1.0),
        pos2(left, rect.bottom() - 1.0),
    ];
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        color.gamma_multiply(0.55),
        Stroke::new(1.0, color),
    ));
    if open {
        ui.painter().line_segment(
            [pos2(left, top + 5.0), pos2(rect.right(), top + 5.0)],
            Stroke::new(1.0, color),
        );
    }
}

fn document_icon(ui: &Ui, rect: Rect, color: Color32) {
    let rect = rect.shrink2(Vec2::new(2.0, 0.0));
    let points = vec![
        rect.left_top(),
        pos2(rect.right() - 3.0, rect.top()),
        pos2(rect.right(), rect.top() + 3.0),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    ui.painter()
        .add(egui::Shape::line(points, Stroke::new(1.1, color)));
    ui.painter().line_segment(
        [
            pos2(rect.right() - 3.0, rect.top()),
            pos2(rect.right() - 3.0, rect.top() + 3.0),
        ],
        Stroke::new(1.1, color),
    );
    for y in [6.0, 9.0] {
        ui.painter().line_segment(
            [
                rect.left_top() + vec2(2.0, y),
                rect.right_top() + vec2(-2.0, y),
            ],
            Stroke::new(1.0, color),
        );
    }
}
