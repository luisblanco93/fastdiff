use super::selection::{Pane, Point, Selection};
use crate::worker::PreparedDocument;
use std::sync::Arc;

impl super::DiffPresentation {
    pub(super) fn scroll_arrows(&mut self, ui: &egui::Ui, viewport: egui::Rect, font_size: f32) {
        if ui.ctx().wants_keyboard_input()
            || ui.memory(|memory| memory.focused().is_some())
            || egui::Popup::is_any_open(ui.ctx())
        {
            return;
        }
        let pointer = ui.ctx().pointer_hover_pos().filter(|position| {
            viewport.contains(*position) && ui.ctx().layer_id_at(*position) == Some(ui.layer_id())
        });
        let pane = match self.selection.active {
            Some((_, pane)) => pane,
            None => match pointer {
                Some(position) if !self.unified && position.x > viewport.center().x => Pane::New,
                Some(_) => Pane::Old,
                None => return,
            },
        };
        let delta = ui.input_mut(|input| {
            let mut delta = egui::Vec2::ZERO;
            input.events.retain(|event| {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = event
                    && modifiers.matches_exact(egui::Modifiers::NONE)
                {
                    delta += match key {
                        egui::Key::ArrowUp => egui::vec2(0.0, -1.0),
                        egui::Key::ArrowDown => egui::vec2(0.0, 1.0),
                        egui::Key::ArrowLeft => egui::vec2(-1.0, 0.0),
                        egui::Key::ArrowRight => egui::vec2(1.0, 0.0),
                        _ => return true,
                    };
                    return false;
                }
                true
            });
            delta * (font_size + 5.0)
        });
        if delta.y != 0.0 {
            let maximum =
                (self.lines.len() as f32 * (font_size + 5.0) - viewport.height()).max(0.0);
            self.arrow_target =
                Some((self.arrow_target.unwrap_or(self.scroll_y) + delta.y).clamp(0.0, maximum));
        }
        let pane = usize::from(!self.unified && pane == Pane::New);
        if delta.x != 0.0 {
            let offset = self.horizontal.active(self.unified)[pane];
            self.horizontal.arrow_targets[pane] =
                Some(self.horizontal.arrow_targets[pane].unwrap_or(offset) + delta.x);
            self.horizontal
                .clamp(&self.document, self.unified, viewport.width(), font_size);
        }
        if delta != egui::Vec2::ZERO {
            ui.ctx().request_repaint();
        }
    }
}

pub(super) struct SourceCell {
    pub rect: egui::Rect,
    pub origin: egui::Pos2,
    pub galley: Arc<egui::Galley>,
    pub comparison: usize,
    pub pane: Pane,
    pub point: Point,
    pub character_limit: usize,
    pub character_start: usize,
    pub response: egui::Response,
}

impl SourceCell {
    fn point_at(&self, pointer: egui::Pos2) -> Point {
        Point {
            character: self
                .galley
                .cursor_from_pos(egui::vec2(pointer.x - self.origin.x, 0.0))
                .index
                .min(self.character_limit)
                + self.character_start,
            ..self.point
        }
    }

    pub fn interact(&self, ui: &egui::Ui, document: &PreparedDocument, selection: &mut Selection) {
        let response = &self.response;
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        if response.secondary_clicked() {
            if let Some(focus) = ui.memory(|memory| memory.focused()) {
                ui.memory_mut(|memory| memory.surrender_focus(focus));
            }
            if selection.active != Some((self.comparison, self.pane)) {
                selection.active = Some((self.comparison, self.pane));
                selection.range = None;
            }
            selection.dragging = false;
        }
        response.context_menu(|ui| {
            let has_text = selection
                .range
                .is_some_and(|(anchor, focus)| anchor != focus);
            if ui
                .add_enabled(has_text, egui::Button::new("Copy selection"))
                .clicked()
            {
                ui.ctx().copy_text(selection.text(document));
                ui.close();
            }
            if ui.button("Select all").clicked() {
                selection.select_all(document, self.comparison, self.pane);
                ui.close();
            }
        });
    }
}

fn press(
    ui: &egui::Ui,
    document: &PreparedDocument,
    selection: &mut Selection,
    cells: &[SourceCell],
) {
    // Check gesture ownership separately from the original position of coalesced presses.
    if !cells.iter().any(|cell| {
        cell.response.is_pointer_button_down_on()
            || cell.response.clicked_by(egui::PointerButton::Primary)
    }) {
        return;
    }
    let press = ui.input(|input| {
        input.events.iter().find_map(|event| match event {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                ..
            } => Some(*pos),
            _ => None,
        })
    });
    if let Some(pointer) = press
        && let Some(cell) = cells.iter().find(|cell| cell.rect.contains(pointer))
        && ui.ctx().layer_id_at(pointer) == Some(ui.layer_id())
    {
        if let Some(focus) = ui.memory(|memory| memory.focused()) {
            ui.memory_mut(|memory| memory.surrender_focus(focus));
        }
        selection.begin(
            document,
            cell.comparison,
            cell.pane,
            cell.point_at(pointer),
            ui.input(|i| i.modifiers.shift),
            false,
        );
    }
}

pub(super) fn drag(
    ui: &egui::Ui,
    document: &PreparedDocument,
    selection: &mut Selection,
    cells: &[SourceCell],
    viewport: egui::Rect,
) -> Option<f32> {
    press(ui, document, selection, cells);
    for cell in cells {
        if cell.response.double_clicked()
            && let Some(pointer) = cell.response.interact_pointer_pos()
        {
            selection.begin(
                document,
                cell.comparison,
                cell.pane,
                cell.point_at(pointer),
                false,
                true,
            );
        }
    }
    if !selection.dragging {
        return None;
    }
    let pointer = ui.ctx().pointer_interact_pos()?;
    if let Some(cell) = cells
        .iter()
        .filter(|cell| {
            cell.rect.is_positive() && selection.active == Some((cell.comparison, cell.pane))
        })
        .min_by(|a, b| {
            (a.rect.center().y - pointer.y)
                .abs()
                .total_cmp(&(b.rect.center().y - pointer.y).abs())
        })
    {
        selection.extend(document, cell.point_at(pointer));
    }
    if !ui.input(|i| i.pointer.primary_down()) {
        selection.dragging = false;
        return None;
    }
    let outside = if pointer.y < viewport.top() {
        pointer.y - viewport.top()
    } else if pointer.y > viewport.bottom() {
        pointer.y - viewport.bottom()
    } else {
        0.0
    };
    if outside != 0.0 {
        ui.ctx().request_repaint();
        Some(outside.clamp(-240.0, 240.0) * ui.input(|i| i.stable_dt).min(0.1) * 8.0)
    } else {
        None
    }
}

pub(super) fn shortcuts(ui: &egui::Ui, document: &PreparedDocument, selection: &mut Selection) {
    if ui.ctx().wants_keyboard_input() {
        return;
    }
    let select_all = ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::A));
    if select_all && let Some((comparison, pane)) = selection.active {
        selection.select_all(document, comparison, pane);
    }
    let copy = ui.input_mut(|i| {
        let copy_key = i.consume_key(egui::Modifiers::CTRL, egui::Key::C);
        let copy_event = i
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::Copy));
        if copy_event {
            i.events.retain(|event| !matches!(event, egui::Event::Copy));
        }
        copy_key || copy_event
    });
    if copy {
        let text = selection.text(document);
        if !text.is_empty() {
            ui.ctx().copy_text(text);
        }
    }
}
