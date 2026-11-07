use super::tests::viewer;
use super::*;
use crate::{
    render::markdown::{MARKDOWN_LIMIT_NOTICE, MARKDOWN_RENDER_LIMIT_BYTES, PREVIEW_SIDE_MARGIN},
    worker::PreparedDocument,
};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use fastdiff_core::CancelToken;

const CTRL_SHIFT: egui::Modifiers = egui::Modifiers {
    command: true,
    ctrl: !cfg!(target_os = "macos"),
    mac_cmd: cfg!(target_os = "macos"),
    ..egui::Modifiers::SHIFT
};

fn document(path: &str, text: &str) -> Arc<PreparedDocument> {
    Arc::new(PreparedDocument::from_text(
        path.into(),
        text.into(),
        &CancelToken::default(),
    ))
}

fn document_viewer(path: &str, text: &str) -> Fastdiff {
    let mut app = viewer();
    let tab = &mut app.tabs[0];
    tab.request.target = OpenTarget::Document;
    tab.request.repository = path.into();
    tab.has_repository = false;
    tab.document_mode = true;
    tab.comparison = 0;
    tab.presentation = Some(DiffPresentation::new(document(path, text), false));
    app
}

fn repository_viewer(path: &str) -> Fastdiff {
    let mut app = viewer();
    let tab = &mut app.tabs[0];
    tab.selected = Some(GitPath(path.as_bytes().to_vec()));
    tab.full_available = true;
    app
}

/// Stands in for the worker's document event after Full file was requested.
fn inject_document(harness: &mut Harness<'static, Fastdiff>, path: &str, text: &str) {
    let tab = &mut harness.state_mut().tabs[0];
    tab.presentation = Some(DiffPresentation::new(document(path, text), false));
    tab.loading = false;
    harness.run();
}

fn harness(app: Fastdiff) -> Harness<'static, Fastdiff> {
    let mut harness = Harness::new_state(|context, app| app.show(context), app);
    harness.run();
    harness
}

#[test]
fn markdown_document_defaults_to_rendered_and_toggles_with_the_shortcut() {
    let mut harness = harness(document_viewer("/d/README.md", "# Title\n\nbody"));
    assert!(!harness.state().tabs[0].markdown.is_source());
    assert!(harness.query_by_label("Title").is_some());
    assert!(harness.query_by_label("# Title").is_none());
    harness.key_press_modifiers(CTRL_SHIFT, egui::Key::V);
    harness.run();
    assert!(harness.state().tabs[0].markdown.is_source());
    assert!(harness.query_by_label("# Title").is_some());
    harness.key_press_modifiers(CTRL_SHIFT, egui::Key::V);
    harness.run();
    assert!(!harness.state().tabs[0].markdown.is_source());
}

#[test]
fn markdown_shortcut_works_when_native_input_delivers_only_the_release() {
    let mut harness = harness(document_viewer("/d/README.md", "# Title"));
    harness.input_mut().events = vec![
        egui::Event::Paste("clipboard".into()),
        egui::Event::Key {
            key: egui::Key::V,
            physical_key: Some(egui::Key::V),
            pressed: false,
            repeat: false,
            modifiers: CTRL_SHIFT,
        },
    ];
    harness.step();
    assert!(harness.state().tabs[0].markdown.is_source());
}

#[test]
fn markdown_buttons_switch_between_rendered_and_source() {
    let mut harness = harness(document_viewer("/d/README.md", "# Title"));
    harness.get_by_label("Source").click();
    harness.run();
    assert!(harness.state().tabs[0].markdown.is_source());
    harness.get_by_label("Preview").click();
    harness.run();
    assert!(!harness.state().tabs[0].markdown.is_source());
}

#[test]
fn markdown_search_entry_points_switch_back_to_source() {
    for use_shortcut in [true, false] {
        let mut app = document_viewer("/d/README.md", "# Title");
        app.tabs[0].search_hits = vec![0];
        let mut harness = harness(app);
        if use_shortcut {
            harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
        } else {
            harness.key_press(egui::Key::F3);
        }
        harness.run();
        assert!(harness.state().tabs[0].markdown.is_source());
    }
    let mut harness = harness(document_viewer("/d/README.md", "# Title"));
    harness.get_by_label("Search document").click();
    harness.run();
    assert!(!harness.state().tabs[0].markdown.is_source());
    harness.event(egui::Event::Text("tit".into()));
    harness.run();
    assert_eq!(harness.state().tabs[0].search, "tit");
    assert!(harness.state().tabs[0].markdown.is_source());
}

#[test]
fn markdown_preview_ignores_other_documents_and_the_shortcut() {
    let mut harness = harness(document_viewer("/d/notes.txt", "# Title"));
    assert!(harness.query_by_label("Preview").is_none());
    assert!(harness.query_by_label("# Title").is_some());
    harness.key_press_modifiers(CTRL_SHIFT, egui::Key::V);
    harness.run();
    assert!(!harness.state().tabs[0].markdown_eligible());
    assert!(!harness.state().tabs[0].markdown.is_source());
    assert!(harness.query_by_label("# Title").is_some());
}

#[test]
fn markdown_reload_rebuilds_the_rendered_text() {
    let mut harness = harness(document_viewer("/d/README.md", "# First"));
    assert!(harness.query_by_label("First").is_some());
    harness.state_mut().tabs[0].presentation = Some(DiffPresentation::new(
        document("/d/README.md", "# Second"),
        false,
    ));
    harness.run();
    assert!(harness.query_by_label("First").is_none());
    assert!(harness.query_by_label("Second").is_some());
}

#[test]
fn markdown_over_the_limit_shows_source_with_a_notice() {
    let large = format!("# Big\n{}", "x".repeat(MARKDOWN_RENDER_LIMIT_BYTES));
    let harness = harness(document_viewer("/d/README.md", &large));
    assert!(harness.query_by_label(MARKDOWN_LIMIT_NOTICE).is_some());
    assert!(harness.query_by_label("# Big").is_some());
}

#[test]
fn markdown_links_open_only_allowed_schemes() {
    let text = "[web](https://example.com) [local](other.md) [script](javascript:alert(1))";
    let mut harness = harness(document_viewer("/d/README.md", text));
    let opened = |harness: &mut Harness<'static, Fastdiff>, label: &str| {
        harness.get_by_label(label).click();
        harness.step();
        harness
            .output()
            .platform_output
            .commands
            .iter()
            .filter_map(|command| match command {
                egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(opened(&mut harness, "web"), ["https://example.com"]);
    assert!(opened(&mut harness, "local").is_empty());
    assert!(opened(&mut harness, "script").is_empty());
}

#[test]
fn selected_markdown_file_in_git_diff_offers_rendered_but_other_files_do_not() {
    let harness = harness(repository_viewer("docs/README.md"));
    assert!(harness.query_by_label("Preview").is_some());
    assert!(harness.query_by_label("Source").is_none());
    assert!(harness.query_by_label("Unified").is_some());
    let harness = self::harness(repository_viewer("src/main.rs"));
    assert!(harness.query_by_label("Preview").is_none());
}

#[test]
fn rendered_button_opens_the_working_tree_file_rendered() {
    let mut harness = harness(repository_viewer("docs/README.md"));
    harness.get_by_label("Preview").click();
    harness.run_steps(3);
    let tab = &harness.state().tabs[0];
    assert!(tab.document_mode);
    assert!(!tab.markdown.is_source());
    inject_document(&mut harness, "/repo/docs/README.md", "# Title\n\nbody");
    assert!(harness.query_by_label("Title").is_some());
    assert!(harness.query_by_label("# Title").is_none());
    assert!(harness.query_by_label("Source").is_some());
}

#[test]
fn rendered_button_is_disabled_when_full_file_is_unavailable() {
    let mut app = repository_viewer("docs/README.md");
    app.tabs[0].full_available = false;
    let mut harness = harness(app);
    assert!(
        harness
            .get_by_role_and_label(egui::accesskit::Role::Button, "Preview")
            .accesskit_node()
            .is_disabled()
    );
    harness.state_mut().tabs[0].open_markdown_rendered();
    assert!(!harness.state().tabs[0].document_mode);
}

#[test]
fn plain_full_file_of_a_markdown_file_opens_source() {
    let mut harness = harness(repository_viewer("docs/README.md"));
    harness.get_by_label("Full file").click();
    harness.run_steps(3);
    assert!(harness.state().tabs[0].document_mode);
    assert!(harness.state().tabs[0].markdown.is_source());
    inject_document(&mut harness, "/repo/docs/README.md", "# Title");
    assert!(harness.query_by_label("# Title").is_some());
}

#[test]
fn shortcut_in_git_diff_opens_rendered_then_toggles_source() {
    let mut harness = harness(repository_viewer("docs/README.md"));
    harness.state_mut().tabs[0].markdown.set_source(true);
    harness.key_press_modifiers(CTRL_SHIFT, egui::Key::V);
    harness.run_steps(3);
    assert!(harness.state().tabs[0].document_mode);
    assert!(!harness.state().tabs[0].markdown.is_source());
    inject_document(&mut harness, "/repo/docs/README.md", "# Title");
    harness.key_press_modifiers(CTRL_SHIFT, egui::Key::V);
    harness.run();
    assert!(harness.state().tabs[0].markdown.is_source());
    assert!(harness.state().tabs[0].document_mode);
}

#[test]
fn shortcut_is_ignored_while_a_text_field_has_focus() {
    let mut harness = harness(repository_viewer("docs/README.md"));
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
    harness.run();
    let focused = harness.ctx.memory(|memory| memory.focused());
    assert_eq!(focused, Some(egui::Id::new("diff-search")));
    harness.input_mut().events = vec![egui::Event::Key {
        key: egui::Key::V,
        physical_key: Some(egui::Key::V),
        pressed: false,
        repeat: false,
        modifiers: CTRL_SHIFT,
    }];
    harness.step();
    assert!(!harness.state().tabs[0].document_mode);
}

#[test]
fn leaving_full_file_or_selecting_a_file_drops_the_rendered_text() {
    for select in [false, true] {
        let mut harness = harness(repository_viewer("docs/README.md"));
        harness.state_mut().tabs[0].open_markdown_rendered();
        inject_document(&mut harness, "/repo/docs/README.md", "# Title");
        assert!(harness.query_by_label("Title").is_some());
        assert!(harness.state().tabs[0].markdown.has_text());
        let tab = &mut harness.state_mut().tabs[0];
        if select {
            tab.select_file(0, false);
        } else {
            tab.toggle_document(false);
        }
        assert!(!harness.state().tabs[0].markdown.has_text());
        assert!(!harness.state().tabs[0].markdown.is_source());
    }
}

#[test]
fn markdown_files_in_one_repository_tab_do_not_share_a_scroll_id() {
    let mut app = repository_viewer("a.md");
    let first = app.tabs[0].document_location();
    app.tabs[0].selected = Some(GitPath(b"b.md".to_vec()));
    assert_ne!(first, app.tabs[0].document_location());
}

#[test]
fn markdown_preview_centers_a_reading_column_in_wide_windows() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1400.0, 600.0))
        .build_state(
            |context, app| app.show(context),
            document_viewer("/d/README.md", "# Title\n\nbody"),
        );
    harness.run();
    let left = harness.get_by_label("Title").rect().left();
    assert!(
        left > PREVIEW_SIDE_MARGIN + 100.0,
        "heading starts at {left}, expected a centered column"
    );
}
