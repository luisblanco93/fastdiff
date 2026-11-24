//! Labels whose text one click highlights, so Ctrl+C copies it.
//!
//! egui cannot set a text selection from code and a drag-selectable label needs a precise drag,
//! so the highlight is drawn behind the text and Ctrl+C is handled here.

/// Draws `text` as a label that one click highlights, and returns its response. While it is
/// highlighted Ctrl+C copies `text`; a press elsewhere, Escape, or another text clears the
/// highlight. `salt` tells apart two such labels in the same `ui`. Give the label a layout that
/// does not stretch it, so only the text is clickable and highlighted. A truncated label already
/// shows its full text on hover.
pub(super) fn highlightable_label(
    ui: &mut egui::Ui,
    salt: &str,
    text: &str,
    rich: egui::RichText,
) -> egui::Response {
    let state_id = ui.id().with(("highlighted-label", salt));
    let highlighted = ui
        .data(|data| data.get_temp::<String>(state_id))
        .is_some_and(|marked| marked == text);
    let background = ui.painter().add(egui::Shape::Noop);
    let response = ui
        .add(
            egui::Label::new(rich)
                .truncate()
                .halign(egui::Align::Min)
                .selectable(false)
                .sense(egui::Sense::click()),
        )
        .on_hover_cursor(egui::CursorIcon::Text);
    let dismissed = ui.input(|input| {
        (input.pointer.any_pressed() && !response.hovered()) || input.key_pressed(egui::Key::Escape)
    });
    let show = if response.clicked() {
        ui.data_mut(|data| data.insert_temp(state_id, text.to_owned()));
        true
    } else if highlighted && dismissed {
        ui.data_mut(|data| data.remove_temp::<String>(state_id));
        false
    } else {
        highlighted
    };
    if show {
        ui.painter().set(
            background,
            egui::Shape::rect_filled(
                response.rect.expand(1.0),
                2.0,
                ui.visuals().selection.bg_fill,
            ),
        );
        let copy = ui.input(|input| {
            input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::Copy))
        });
        if copy && !ui.ctx().wants_keyboard_input() {
            ui.ctx().copy_text(text.to_owned());
        }
    }
    response
}
