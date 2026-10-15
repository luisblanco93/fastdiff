use super::selection::{Pane, Point};
use super::*;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};

fn fixture() -> DiffPresentation {
    let document = tests::prepare(
        (1..=200)
            .map(|n| tests::row(fastdiff_core::RowKind::Change, n))
            .collect(),
    );
    DiffPresentation::new(document, false)
}

fn harness() -> Harness<'static, DiffPresentation> {
    Harness::builder()
        .with_size(egui::vec2(800.0, 300.0))
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, presentation| presentation.show(ui, egui::Id::new("selection"), 13.0),
            fixture(),
        )
}

#[test]
fn search_hits_highlight_whole_lines_and_clear_with_results() {
    let mut harness = harness();
    let highlighted_rects = |harness: &Harness<'_, DiffPresentation>| {
        harness
            .output()
            .shapes
            .iter()
            .filter(|shape| {
                matches!(&shape.shape, egui::Shape::Rect(rect)
                if rect.fill == egui::Color32::from_rgb(55, 78, 115)
                    && rect.rect.width() > 300.0)
            })
            .count()
    };
    assert_eq!(highlighted_rects(&harness), 0);
    harness.state_mut().search_hits = vec![0];
    harness.run_steps(2);
    assert_eq!(highlighted_rects(&harness), 2);
    harness.state_mut().search_hits.clear();
    harness.run_steps(2);
    assert_eq!(highlighted_rects(&harness), 0);
}

fn pointer(
    harness: &mut Harness<'_, DiffPresentation>,
    position: egui::Pos2,
    button: egui::PointerButton,
    pressed: bool,
) {
    harness.event(egui::Event::PointerMoved(position));
    harness.event(egui::Event::PointerButton {
        pos: position,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(1);
}

fn clipboard(harness: &Harness<'_, DiffPresentation>) -> String {
    harness
        .output()
        .platform_output
        .commands
        .iter()
        .find_map(|command| match command {
            egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn arrow(harness: &mut Harness<'_, DiffPresentation>, key: egui::Key, modifiers: egui::Modifiers) {
    harness.event_modifiers(
        egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            repeat: false,
            physical_key: None,
        },
        modifiers,
    );
    harness.run_steps(3);
}

#[test]
fn arrow_scrolling_moves_through_intermediate_offsets() {
    let mut harness = harness();
    harness
        .ctx
        .style_mut(|style| style.scroll_animation = egui::style::ScrollAnimation::default());
    let document = PreparedDocument::from_text(
        "long.txt".into(),
        (0..200)
            .map(|_| "x".repeat(500))
            .collect::<Vec<_>>()
            .join("\n"),
        &fastdiff_core::CancelToken::default(),
    );
    *harness.state_mut() = DiffPresentation::new(Arc::new(document), false);
    harness.state_mut().selection.active = Some((0, Pane::Unified));
    harness.run();
    for key in [egui::Key::ArrowDown, egui::Key::ArrowRight] {
        arrow(&mut harness, key, egui::Modifiers::NONE);
        let offset = if key == egui::Key::ArrowDown {
            harness.state().scroll_y
        } else {
            harness.state().horizontal.unified
        };
        assert!(offset > 0.0 && offset < 18.0, "offset: {offset}");
        arrow(&mut harness, key, egui::Modifiers::NONE);
        harness.run_steps(12);
        let offset = if key == egui::Key::ArrowDown {
            harness.state().scroll_y
        } else {
            harness.state().horizontal.unified
        };
        assert_eq!(offset, 36.0);
    }
}

#[test]
fn wheel_scrolling_interrupts_arrow_animation() {
    let mut harness = harness();
    harness
        .ctx
        .style_mut(|style| style.scroll_animation = egui::style::ScrollAnimation::default());
    harness.state_mut().selection.active = Some((0, Pane::Old));
    let position = harness.get_by_label("let old_5 = 1;").rect().center();
    harness.hover_at(position);
    arrow(&mut harness, egui::Key::ArrowDown, egui::Modifiers::NONE);
    assert!(harness.state().arrow_target.is_some());
    harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -100.0),
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(30);
    assert!(harness.state().scroll_y > 18.0);
    assert!(harness.state().arrow_target.is_none());
}

#[test]
fn arrows_scroll_diff_and_full_file_without_changing_selection() {
    for full_file in [false, true] {
        let mut harness = harness();
        if full_file {
            harness.state_mut().document = Arc::new(PreparedDocument::from_text(
                "long.txt".into(),
                (0..200)
                    .map(|_| "x".repeat(500))
                    .collect::<Vec<_>>()
                    .join("\n"),
                &fastdiff_core::CancelToken::default(),
            ));
            let document = harness.state().document.clone();
            *harness.state_mut() = DiffPresentation::new(document, false);
        }
        let document = harness.state().document.clone();
        harness.state_mut().selection.select_all(
            &document,
            0,
            if full_file { Pane::Unified } else { Pane::Old },
        );
        harness.run();
        let selection = harness.state().selection.range;
        arrow(&mut harness, egui::Key::ArrowDown, egui::Modifiers::NONE);
        assert_eq!(harness.state().scroll_y, 18.0);
        arrow(&mut harness, egui::Key::ArrowUp, egui::Modifiers::NONE);
        assert_eq!(harness.state().scroll_y, 0.0);
        arrow(&mut harness, egui::Key::ArrowUp, egui::Modifiers::NONE);
        assert_eq!(harness.state().scroll_y, 0.0);
        assert_eq!(harness.state().selection.range, selection);
        if full_file {
            arrow(&mut harness, egui::Key::ArrowRight, egui::Modifiers::NONE);
            assert!(harness.state().horizontal.unified > 0.0);
            arrow(&mut harness, egui::Key::ArrowLeft, egui::Modifiers::NONE);
            assert_eq!(harness.state().horizontal.unified, 0.0);
        }
    }
}

#[test]
fn horizontal_arrows_scroll_only_the_active_split_pane() {
    let mut harness = harness();
    let mut row = tests::row(fastdiff_core::RowKind::Change, 1);
    row.left = "old ".repeat(500);
    row.right = "new ".repeat(500);
    *harness.state_mut() = DiffPresentation::new(tests::prepare(vec![row]), false);
    harness.state_mut().selection.active = Some((0, Pane::New));
    harness.run();
    arrow(&mut harness, egui::Key::ArrowRight, egui::Modifiers::NONE);
    assert_eq!(harness.state().horizontal.split[0], 0.0);
    assert!(harness.state().horizontal.split[1] > 0.0);
    arrow(&mut harness, egui::Key::ArrowLeft, egui::Modifiers::NONE);
    assert_eq!(harness.state().horizontal.split, [0.0; 2]);
}

#[test]
fn arrows_leave_modified_keys_alone() {
    let mut harness = harness();
    harness.state_mut().selection.active = Some((0, Pane::Old));
    for modifiers in [
        egui::Modifiers::ALT,
        egui::Modifiers::COMMAND,
        egui::Modifiers::SHIFT,
    ] {
        arrow(&mut harness, egui::Key::ArrowDown, modifiers);
        assert_eq!(harness.state().scroll_y, 0.0);
    }
}

#[test]
fn arrows_scroll_a_hovered_panel_without_a_selection() {
    let mut harness = harness();
    arrow(&mut harness, egui::Key::ArrowDown, egui::Modifiers::NONE);
    assert_eq!(harness.state().scroll_y, 0.0);
    let position = harness.get_by_label("let old_1 = 1;").rect().center();
    harness.hover_at(position);
    harness.run_steps(1);
    arrow(&mut harness, egui::Key::ArrowDown, egui::Modifiers::NONE);
    assert_eq!(harness.state().scroll_y, 18.0);
    assert!(harness.state().selection.active.is_none());
}

#[test]
fn arrows_leave_focused_text_fields_alone() {
    let mut presentation = fixture();
    presentation.selection.active = Some((0, Pane::Old));
    let mut harness = Harness::builder()
        .with_size(egui::vec2(800.0, 300.0))
        .build_ui_state(
            |ui, (presentation, text)| {
                ui.add(egui::TextEdit::singleline(text).id(egui::Id::new("search")));
                presentation.show(ui, egui::Id::new("selection"), 13.0);
            },
            (presentation, String::from("query")),
        );
    harness
        .ctx
        .memory_mut(|memory| memory.request_focus(egui::Id::new("search")));
    harness.event(egui::Event::Key {
        key: egui::Key::ArrowDown,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
        repeat: false,
        physical_key: None,
    });
    harness.run_steps(3);
    assert_eq!(harness.state().0.scroll_y, 0.0);
}

#[test]
fn dragging_across_divider_copies_only_starting_pane_and_survives_scroll() {
    let mut harness = harness();
    let start = harness.get_by_label("let old_1 = 1;").rect().left_center();
    let end = harness.get_by_label("let new_3 = 2;").rect().right_center();
    pointer(&mut harness, start, egui::PointerButton::Primary, true);
    pointer(&mut harness, end, egui::PointerButton::Primary, false);
    assert_eq!(
        harness.state().selection.text(&harness.state().document),
        "let old_1 = 1;\nlet old_2 = 1;\nlet old_3 = 1;"
    );
    harness.state_mut().pending_scroll = Some(1500.0);
    harness.run();
    harness.event(egui::Event::Key {
        key: egui::Key::C,
        pressed: true,
        modifiers: egui::Modifiers::COMMAND,
        repeat: false,
        physical_key: None,
    });
    harness.run_steps(1);
    assert_eq!(
        clipboard(&harness),
        "let old_1 = 1;\nlet old_2 = 1;\nlet old_3 = 1;"
    );
}

#[test]
fn right_click_preserves_selection_and_menu_selects_active_pane() {
    let mut harness = harness();
    let start = harness.get_by_label("let new_1 = 2;").rect().left_center();
    let end = harness.get_by_label("let new_3 = 2;").rect().right_center();
    pointer(&mut harness, start, egui::PointerButton::Primary, true);
    pointer(&mut harness, end, egui::PointerButton::Primary, false);
    let before = harness.state().selection.text(&harness.state().document);
    pointer(&mut harness, start, egui::PointerButton::Secondary, true);
    pointer(&mut harness, start, egui::PointerButton::Secondary, false);
    harness.run();
    assert_eq!(
        harness.state().selection.text(&harness.state().document),
        before
    );
    harness.get_by_label("Copy selection").click();
    harness.run_steps(1);
    assert_eq!(clipboard(&harness), before);
    pointer(&mut harness, start, egui::PointerButton::Secondary, true);
    pointer(&mut harness, start, egui::PointerButton::Secondary, false);
    harness.run();
    harness.get_by_label("Select all").click();
    harness.run();
    let text = harness.state().selection.text(&harness.state().document);
    assert!(text.contains("let new_200 = 2;"));
    assert!(!text.contains("old_"));
}

#[test]
fn font_changes_preserve_selection_and_layout_and_document_changes_clear_it() {
    let mut presentation = fixture();
    presentation
        .selection
        .select_all(&presentation.document, 0, Pane::Old);
    let before = presentation.selection.text(&presentation.document);
    presentation.set_layout(false, 800.0, 19.0);
    assert_eq!(presentation.selection.text(&presentation.document), before);
    presentation.set_layout(true, 800.0, 19.0);
    assert!(
        presentation
            .selection
            .text(&presentation.document)
            .is_empty()
    );
    presentation.selection.begin(
        &presentation.document,
        0,
        Pane::Unified,
        Point {
            row: 0,
            side: Side::Old,
            character: 0,
        },
        false,
        false,
    );
    let mut refreshed = fixture();
    refreshed.preserve_state_from(&presentation);
    assert!(refreshed.selection.active.is_none());
}

#[test]
fn dragging_beyond_viewport_scrolls_without_losing_the_anchor() {
    let mut harness = harness();
    let start = harness.get_by_label("let old_1 = 1;").rect().left_center();
    pointer(&mut harness, start, egui::PointerButton::Primary, true);
    harness.event(egui::Event::PointerMoved(egui::pos2(
        start.x + 150.0,
        350.0,
    )));
    harness.run_steps(30);
    assert!(harness.state().scroll_y > 100.0);
    let selection = &harness.state().selection;
    assert_eq!(selection.range.unwrap().0.row, 0);
    assert!(selection.range.unwrap().1.row > 15);
}

#[test]
fn double_click_selects_line_and_shift_click_extends_it_in_reverse() {
    let mut harness = harness();
    let word = harness.get_by_label("let old_3 = 1;").rect().left_center() + egui::vec2(44.0, 0.0);
    for _ in 0..2 {
        pointer(&mut harness, word, egui::PointerButton::Primary, true);
        pointer(&mut harness, word, egui::PointerButton::Primary, false);
    }
    assert_eq!(
        harness.state().selection.text(&harness.state().document),
        "let old_3 = 1;"
    );
    let start = harness.get_by_label("let old_1 = 1;").rect().left_center();
    for pressed in [true, false] {
        harness.event_modifiers(
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::SHIFT,
            },
            egui::Modifiers::SHIFT,
        );
        harness.run_steps(1);
    }
    assert_eq!(
        harness.state().selection.text(&harness.state().document),
        "let old_1 = 1;\nlet old_2 = 1;\n"
    );
}

#[test]
fn menu_copy_is_disabled_without_selected_text() {
    let mut harness = harness();
    let start = harness.get_by_label("let old_1 = 1;").rect().left_center();
    pointer(&mut harness, start, egui::PointerButton::Secondary, true);
    pointer(&mut harness, start, egui::PointerButton::Secondary, false);
    harness.run();
    assert!(
        harness
            .get_by_label("Copy selection")
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        !harness
            .get_by_label("Select all")
            .accesskit_node()
            .is_disabled()
    );
}

#[test]
fn coalesced_pointer_events_keep_the_original_drag_anchor() {
    let mut harness = harness();
    let start = harness.get_by_label("let old_1 = 1;").rect().left_center();
    let end = harness.get_by_label("let new_3 = 2;").rect().right_center();
    harness.input_mut().events = vec![
        egui::Event::PointerMoved(start),
        egui::Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::PointerMoved(end),
    ];
    harness.step();
    assert_eq!(
        harness.state().selection.text(&harness.state().document),
        "let old_1 = 1;\nlet old_2 = 1;\nlet old_3 = 1;"
    );
}

#[test]
fn coalesced_second_click_selects_the_line() {
    let mut harness = harness();
    let word = harness.get_by_label("let old_3 = 1;").rect().left_center() + egui::vec2(44.0, 0.0);
    pointer(&mut harness, word, egui::PointerButton::Primary, true);
    pointer(&mut harness, word, egui::PointerButton::Primary, false);
    harness.input_mut().events = [true, false]
        .into_iter()
        .map(|pressed| egui::Event::PointerButton {
            pos: word,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        })
        .collect();
    harness.step();
    assert_eq!(
        harness.state().selection.text(&harness.state().document),
        "let old_3 = 1;"
    );
}

#[test]
fn dragging_floating_scrollbar_preserves_text_selection() {
    let mut harness = harness();
    let document = harness.state().document.clone();
    harness
        .state_mut()
        .selection
        .select_all(&document, 0, Pane::Old);
    harness.run();
    let before = harness.state().selection.text(&harness.state().document);
    let row = harness.get_by_label("let new_1 = 2;").rect();
    // The floating scrollbar overlaps the source pane at the viewport's right edge.
    let start = egui::pos2(788.0, row.top() + 1.0);
    harness.hover_at(start);
    harness.run();
    pointer(&mut harness, start, egui::PointerButton::Primary, true);
    harness.event(egui::Event::PointerMoved(start + egui::vec2(0.0, 120.0)));
    harness.run_steps(1);
    pointer(
        &mut harness,
        start + egui::vec2(0.0, 120.0),
        egui::PointerButton::Primary,
        false,
    );
    assert!(harness.state().scroll_y > 100.0);
    assert_eq!(
        harness.state().selection.text(&harness.state().document),
        before
    );
}

#[test]
fn wheel_scrolling_after_source_click_does_not_select_text() {
    let mut harness = harness();
    let start = harness.get_by_label("let old_1 = 1;").rect().left_center();
    pointer(&mut harness, start, egui::PointerButton::Primary, true);
    pointer(&mut harness, start, egui::PointerButton::Primary, false);
    harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -100.0),
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(30);
    assert!(harness.state().scroll_y > 20.0);
    assert!(
        harness
            .state()
            .selection
            .text(&harness.state().document)
            .is_empty()
    );
}
