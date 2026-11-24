use super::tests::viewer;
use super::*;
use egui::accesskit::Role;
use egui_kittest::{Harness, kittest::Queryable};

const PATH: &str = "src/main.rs";

fn harness() -> Harness<'static, Fastdiff> {
    let mut harness = Harness::new_state(|context, app| app.show(context), viewer());
    harness.run();
    harness
}

/// The path above the document; the sidebar button for the same file has the same text.
fn path_label<'a>(harness: &'a Harness<'static, Fastdiff>) -> egui_kittest::Node<'a> {
    harness.get_by_role_and_label(Role::Label, PATH)
}

/// Presses Ctrl+C and returns the text egui was asked to copy.
fn copied_after_ctrl_c(harness: &mut Harness<'static, Fastdiff>) -> Vec<String> {
    harness.input_mut().events.push(egui::Event::Copy);
    // One frame only: the output of a later frame no longer holds the copy command.
    harness.step();
    harness
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|command| match command {
            egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn nothing_is_copied_before_the_path_is_clicked() {
    let mut harness = harness();
    assert!(copied_after_ctrl_c(&mut harness).is_empty());
}

#[test]
fn one_click_on_the_path_highlights_it_and_ctrl_c_copies_it() {
    let mut harness = harness();
    path_label(&harness).click();
    harness.run();
    assert_eq!(copied_after_ctrl_c(&mut harness), [PATH.to_owned()]);
}

#[test]
fn clicking_elsewhere_clears_the_highlight() {
    let mut harness = harness();
    path_label(&harness).click();
    harness.run();
    harness.get_by_role_and_label(Role::Tab, "repo").click();
    harness.run();
    assert!(copied_after_ctrl_c(&mut harness).is_empty());
}

#[test]
fn escape_clears_the_highlight() {
    let mut harness = harness();
    path_label(&harness).click();
    harness.run();
    harness.key_press(egui::Key::Escape);
    harness.run();
    assert!(copied_after_ctrl_c(&mut harness).is_empty());
}

#[test]
fn the_highlight_covers_only_the_text_not_the_whole_row() {
    let mut harness = harness();
    let label = path_label(&harness).rect();
    path_label(&harness).click();
    harness.run();
    let window = harness.ctx.content_rect().width();
    assert!(
        label.width() < window / 2.0,
        "the path label spans {} of a {window} px window",
        label.width()
    );
}

#[test]
fn the_branch_name_highlights_on_one_click_and_copies_with_ctrl_c() {
    let mut app = viewer();
    app.tabs[0].branch = Some("feature/x".into());
    app.tabs[0].has_repository = true;
    let mut harness = Harness::new_state(|context, app| app.show(context), app);
    harness.run();
    harness
        .get_by_role_and_label(Role::Label, "feature/x")
        .click();
    harness.run();
    assert_eq!(copied_after_ctrl_c(&mut harness), ["feature/x".to_owned()]);
}
