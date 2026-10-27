use super::*;
use crate::{
    sidebar::TreeModel,
    worker::{
        PreparedDocument,
        test_support::{ProgressGuard, poll_until},
    },
};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use fastdiff_core::{CancelToken, Comparison, DiffDocument, DiffRow};
use std::sync::Arc;

/// Hang guard for polling loops, not a speed requirement: hosts vary widely in how long a
/// repository load takes.
const WAIT_LIMIT: Duration = Duration::from_secs(30);

fn advance_tab_load(
    guard: &mut ProgressGuard,
    tab: &ReviewTab,
    initial_files: &Arc<Vec<FileChange>>,
) {
    if tab.root != "/repo" {
        guard.advance("repository discovered");
    }
    if tab.branch.is_some() {
        guard.advance("branch received");
    }
    if !Arc::ptr_eq(initial_files, &tab.files) {
        guard.advance("status received");
    }
    if tab.provisional {
        guard.advance("provisional document received");
    }
    if tab.presentation.is_some() {
        guard.advance("presentation received");
    }
}

pub(super) fn fixture() -> ReviewTab {
    let path = GitPath(b"src/main.rs".to_vec());
    let request = OpenRequest {
        target: fastdiff_cli::OpenTarget::Repository,
        repository: "/repo".into(),
        file: Some(path.clone()),
        view: View::All,
        wsl: None,
        theme: false,
    };
    let files = Arc::new(vec![FileChange {
        path: path.clone(),
        old_path: None,
        committed: None,
        staged: true,
        unstaged: true,
        untracked: false,
        conflict: false,
        status: "MM".into(),
    }]);
    let comparisons = ["Staged", "Unstaged"]
        .iter()
        .map(|label| Comparison {
            label: (*label).into(),
            patch: format!("{label} patch"),
            summary: None,
            rows: vec![DiffRow {
                kind: RowKind::Addition,
                old_number: None,
                new_number: Some(1),
                left: String::new(),
                right: "fn main() {}".into(),
            }],
        })
        .collect();
    let document = Arc::new(PreparedDocument::new(
        DiffDocument { comparisons },
        &CancelToken::default(),
    ));
    let mut tree = FileTree::default();
    tree.update(Arc::new(TreeModel::new(&files)));
    ReviewTab {
        request,
        root: "/repo".into(),
        has_repository: true,
        document_mode: false,
        reflog_open: false,
        reflog: None,
        reflog_generation: 0,
        reflog_loading: false,
        reflog_error: None,
        reflog_pending: None,
        reflog_scroll_reset: false,
        branch: None,
        full_available: true,
        notice: None,
        worker: None,
        foreground: true,
        generation: 7,
        files,
        filter: String::new(),
        visible_files: vec![0],
        tree,
        reveal_requested: false,
        selected: Some(path),
        view: View::All,
        presentation: Some(DiffPresentation::new(document, false)),
        loading: false,
        provisional: false,
        error: None,
        comparison: 1,
        search: String::new(),
        search_request_id: 0,
        search_hits: Vec::new(),
        search_index: 0,
        context: egui::Context::default(),
    }
}

pub(super) fn viewer() -> Fastdiff {
    Fastdiff {
        tabs: vec![fixture()],
        selected_tab: 0,
        preferences: Preferences::default(),
        server: None,
        benchmark: Benchmark::new(false),
        help_open: false,
        fallback_fonts: Default::default(),
        appearance: Default::default(),
        retired_workers: Vec::new(),
    }
}

#[test]
fn tab_activity_refreshes_activation_and_preserves_selection_across_index_shifts() {
    let mut app = viewer();
    app.tabs.push(fixture());
    app.sync_tab_activity();
    let hidden_generation = app.tabs[1].generation;
    assert!(!app.tabs[1].foreground);
    assert_eq!(app.tabs[0].generation, 7);
    app.tabs[1].reflog_open = true;
    app.selected_tab = 1;
    app.sync_tab_activity();
    assert!(!app.tabs[0].foreground);
    assert!(app.tabs[1].foreground);
    assert!(app.tabs[1].loading);
    assert!(app.tabs[1].reflog_loading);
    assert_eq!(app.tabs[1].generation, hidden_generation + 1);
    let generation = app.tabs[1].generation;
    app.tabs.remove(0);
    app.selected_tab = 0;
    app.sync_tab_activity();
    assert_eq!(app.tabs[0].generation, generation);
}

#[test]
fn reactivating_tab_rejects_previous_diff_and_reflog_results() {
    let mut app = viewer();
    app.tabs.push(fixture());
    app.sync_tab_activity();
    let old_generation = app.tabs[0].generation;
    app.tabs[0].reflog_open = true;
    let old_reflog_generation = app.tabs[0].reflog_generation;
    let document = Arc::clone(&app.tabs[0].presentation.as_ref().unwrap().document);
    app.selected_tab = 1;
    app.sync_tab_activity();
    app.selected_tab = 0;
    app.sync_tab_activity();
    let tab = &mut app.tabs[0];
    tab.apply_events(
        vec![
            Event::Diff {
                generation: old_generation,
                document,
            },
            Event::Reflog {
                generation: old_reflog_generation,
                list: reflog_list(Some("stale"), 1),
            },
        ],
        false,
    );
    assert!(tab.loading);
    assert!(tab.reflog_loading);
    assert!(tab.reflog.is_none());
}

#[test]
fn reopening_inactive_tab_refreshes_once_and_keeps_requested_reveal() {
    let mut app = viewer();
    let request = app.tabs[0].request.clone();
    let mut second = fixture();
    second.request.repository = "/another-repo".into();
    app.tabs.push(second);
    app.selected_tab = 1;
    app.sync_tab_activity();
    let generation = app.tabs[0].generation;
    app.open(request, &egui::Context::default());
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.selected_tab, 0);
    assert_eq!(app.tabs[0].generation, generation + 1);
    assert!(app.tabs[0].loading);
    assert!(app.tabs[0].reveal_requested);
    assert!(!app.tabs[1].foreground);
}

#[test]
fn rapid_tab_open_preserves_discovery_and_refreshes_the_paused_repository() {
    let directories: Vec<_> = (0..2).map(|_| tempfile::tempdir().unwrap()).collect();
    let context = egui::Context::default();
    let mut app = viewer();
    app.tabs.clear();
    let mut requests = Vec::new();
    for (index, directory) in directories.iter().enumerate() {
        let output = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        std::fs::write(directory.path().join("notes.txt"), format!("tab {index}\n")).unwrap();
        let request = OpenRequest {
            repository: directory.path().to_string_lossy().into_owned(),
            file: Some(GitPath(b"notes.txt".to_vec())),
            ..fixture().request
        };
        app.open(request.clone(), &context);
        requests.push(request);
    }
    for (index, request) in [(1, None), (0, Some(requests.remove(0)))] {
        if let Some(request) = request {
            app.open(request, &context);
            assert!(app.tabs[index].loading);
        }
        let initial_files = Arc::clone(&app.tabs[index].files);
        poll_until(format!("tab {index} load"), WAIT_LIMIT, |guard| {
            app.poll(&context);
            let tab = &app.tabs[index];
            advance_tab_load(guard, tab, &initial_files);
            (!tab.loading).then_some(())
        })
        .unwrap_or_else(|failure| panic!("{failure}"));
        let tab = &app.tabs[index];
        assert!(tab.error.is_none(), "{:?}", tab.error);
        assert!(
            tab.presentation
                .as_ref()
                .unwrap()
                .document
                .document
                .comparisons
                .iter()
                .flat_map(|comparison| &comparison.rows)
                .any(|row| row.right == format!("tab {index}"))
        );
        if index == 1 {
            assert!(app.tabs[0].loading, "Hidden events must stay unprocessed");
            assert!(app.tabs[0].files.is_empty());
        }
    }
}

#[test]
fn selected_file_path_stays_discoverable_when_filtered_out_and_loading() {
    let path = format!("src/review/{}漢字.rs", "long_filename_".repeat(16));
    let mut app = viewer();
    app.tabs[0].selected = Some(GitPath(path.as_bytes().to_vec()));
    app.tabs[0].filter = "no matching file".into();
    app.tabs[0].visible_files.clear();
    let mut harness = Harness::new_state(|context, app| app.show(context), app);
    assert!(harness.query_by_label(&path).is_some());
    harness.get_by_label(&path).hover();
    harness.run();
    assert!(harness.get_all_by_label(&path).count() >= 2);

    harness.state_mut().tabs[0].presentation = None;
    harness.state_mut().tabs[0].loading = true;
    harness.run_steps(2);
    assert!(
        harness
            .get_all_by_role_and_label(egui::accesskit::Role::Label, &path)
            .next()
            .is_some()
    );
}

#[test]
fn standalone_document_header_shows_its_path_instead_of_repository_selection() {
    let path = "/repo/docs/readme.txt";
    let mut app = viewer();
    app.tabs[0].request.target = OpenTarget::Document;
    app.tabs[0].request.repository = path.into();
    app.tabs[0].root = "/repo/docs".into();
    app.tabs[0].document_mode = true;
    let harness = Harness::new_state(|context, app| app.show(context), app);
    assert!(harness.query_by_label(path).is_some());
    assert!(harness.query_by_label("src/main.rs").is_none());
}

#[test]
fn minimal_toolbar_and_editor_search_focus() {
    let mut harness = Harness::new_state(|context, app| app.show(context), viewer());
    for label in [
        "Help",
        "Previous match",
        "Next match",
        "Search diff",
        "Unified",
        "UI −",
        "UI +",
    ] {
        assert!(harness.query_by_label(label).is_some(), "{label}");
    }
    for label in [
        "Copy path",
        "Refresh",
        "Next file",
        "Next hunk",
        "Smaller text",
    ] {
        assert!(harness.query_by_label(label).is_none(), "{label}");
    }
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
    harness.run();
    harness.event(egui::Event::Text("main".into()));
    harness.run();
    assert_eq!(harness.state().tabs[0].search, "main");
    assert!(harness.state().tabs[0].filter.is_empty());
    harness.key_press_modifiers(
        egui::Modifiers::COMMAND | egui::Modifiers::ALT,
        egui::Key::F,
    );
    harness.run();
    harness.event(egui::Event::Text("src".into()));
    harness.run();
    assert_eq!(harness.state().tabs[0].filter, "src");
}

#[test]
fn toolbar_layout_toggle_restores_split_at_wide_width() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1400.0, 720.0))
        .build_state(|context, app| app.show(context), viewer());
    harness.get_by_label("Unified").click();
    harness.run();
    assert!(harness.state().preferences.unified);
    assert!(
        harness.state().tabs[0]
            .presentation
            .as_ref()
            .unwrap()
            .unified
    );
    harness.get_by_label("Side by side").click();
    harness.run();
    assert!(!harness.state().preferences.unified);
    assert!(
        !harness.state().tabs[0]
            .presentation
            .as_ref()
            .unwrap()
            .unified
    );
}

#[test]
fn toolbar_layout_toggle_does_not_change_full_document_mode() {
    let mut app = viewer();
    app.tabs[0].document_mode = true;
    let mut harness = Harness::new_state(|context, app| app.show(context), app);
    harness.get_by_label("Unified").click();
    harness.run();
    assert!(!harness.state().preferences.unified);
    assert!(harness.state().tabs[0].document_mode);
}

#[test]
fn toolbar_ui_zoom_scales_navigation_without_changing_source_font_preference() {
    let mut harness = Harness::new_state(|context, app| app.show(context), viewer());
    let source_font_size = harness.state().preferences.font_size;
    let initial_zoom = harness.ctx.zoom_factor();
    harness.get_by_label("UI +").click();
    harness.run();
    assert!((harness.ctx.zoom_factor() - initial_zoom - 0.1).abs() < 0.001);
    assert_eq!(harness.state().preferences.font_size, source_font_size);
    // Kittest pointer clicks use pixel bounds; AccessKit remains correct after zooming.
    harness.get_by_label("UI −").click_accesskit();
    harness.run();
    assert!((harness.ctx.zoom_factor() - initial_zoom).abs() < 0.001);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Minus);
    harness.run();
    assert_eq!(
        harness.state().preferences.font_size,
        source_font_size - 1.0
    );
    assert!((harness.ctx.zoom_factor() - initial_zoom).abs() < 0.001);
}

#[test]
fn toolbar_ui_zoom_stays_within_readable_bounds() {
    for (zoom, label) in [(2.0, "UI +"), (0.75, "UI −")] {
        let mut harness = Harness::new_state(|context, app| app.show(context), viewer());
        harness.ctx.set_zoom_factor(zoom);
        harness.run();
        // AccessKit bounds are pixels; egui pointer events use logical points.
        let position = harness.get_by_label(label).rect().center() / harness.ctx.pixels_per_point();
        harness.event(egui::Event::PointerMoved(position));
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        harness.run();
        assert_eq!(harness.ctx.zoom_factor(), zoom);
        assert_eq!(harness.state().tabs[0].view, View::All);
        assert!(!harness.state().tabs[0].loading);
        assert_eq!(harness.state().preferences.font_size, 13.0);
    }
}

#[test]
fn copy_menu_selects_comparison_and_copies_original_patch_source_and_path() {
    let mut harness = Harness::new_state(|context, app| app.show(context), viewer());
    harness.get_by_label("Copy").click();
    harness.run();
    assert!(
        harness
            .query_by_label("Copy comparison: Unstaged")
            .is_some()
    );
    harness.get_by_label("Staged comparison").click();
    harness.run();
    assert_eq!(harness.state().tabs[0].comparison, 0);
    for (label, expected) in [
        ("Copy patch", "Staged patch"),
        ("Copy source", "fn main() {}"),
        ("Copy path", "src/main.rs"),
    ] {
        if harness.query_by_label(label).is_none() {
            harness.get_by_label("Copy").click();
            harness.run();
        }
        harness.get_by_label(label).click();
        harness.step();
        assert!(
            harness.output().platform_output.commands.iter().any(
                |command| matches!(command, egui::OutputCommand::CopyText(text) if text == expected)
            ),
            "{label}"
        );
        harness.run();
    }
}

#[test]
fn copy_menu_disables_comparison_actions_without_git_comparison_data() {
    use egui_kittest::kittest::NodeT;
    for full_document in [false, true] {
        let mut app = viewer();
        app.tabs[0].document_mode = full_document;
        if !full_document {
            app.tabs[0].presentation = None;
            app.tabs[0].selected = None;
        }
        let mut harness = Harness::new_state(|context, app| app.show(context), app);
        harness.get_by_label("Copy").click();
        harness.run();
        for label in ["Copy patch", "Copy source"] {
            assert!(harness.get_by_label(label).accesskit_node().is_disabled());
        }
        assert_eq!(
            harness
                .get_by_label("Copy path")
                .accesskit_node()
                .is_disabled(),
            !full_document
        );
    }
}

#[test]
fn all_scope_explains_included_changes_on_hover() {
    let mut harness = Harness::new_state(|context, app| app.show(context), viewer());
    harness.get_by_label("All").hover();
    harness.run();
    assert!(
        harness
            .query_by_label("Unpushed commits, staged edits, unstaged edits, and untracked files")
            .is_some()
    );
}

#[test]
fn empty_view_offers_relevant_next_step() {
    for (view, guidance) in [
        (
            View::Staged,
            "Try All to see other changes, or refresh (F5).",
        ),
        (
            View::All,
            "Check that this is the right repository, or refresh (F5).",
        ),
    ] {
        let mut app = viewer();
        app.tabs[0].view = view;
        app.tabs[0].presentation = None;
        let harness = Harness::new_state(|context, app| app.show(context), app);
        assert!(harness.query_by_label(guidance).is_some());
    }
}

#[test]
fn help_lists_shortcuts_and_comparison_and_escape_closes() {
    let mut harness = Harness::new_state(|context, app| app.show(context), viewer());
    harness.get_by_label("Help").click();
    harness.run();
    assert!(harness.query_by_label("Keyboard shortcuts").is_some());
    assert!(
        harness
            .query_by_label("Copy comparison: Unstaged")
            .is_some()
    );
    assert!(
        harness
            .query_by_label(&shortcuts::binding_label("Ctrl+Alt+F"))
            .is_some()
    );
    assert!(
        harness
            .query_by_label(&shortcuts::binding_label("Ctrl+K Ctrl+T"))
            .is_some()
    );
    assert!(harness.query_by_label("Open Theme menu").is_some());
    harness.key_press(egui::Key::Escape);
    harness.run();
    assert!(harness.query_by_label("Keyboard shortcuts").is_none());
    harness.key_press(egui::Key::F1);
    harness.run();
    assert!(harness.query_by_label("Keyboard shortcuts").is_some());
}

#[test]
fn switching_view_discards_previous_comparison_and_requests_refresh() {
    let mut harness = Harness::new_state(|context, app| app.show(context), viewer());
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Num3);
    harness.run_steps(2);
    assert_eq!(harness.state().tabs[0].view, View::Staged);
    assert!(harness.state().tabs[0].loading);
    assert!(harness.state().tabs[0].presentation.is_none());
}

#[test]
fn git_filters_keep_diff_mode_and_tree_when_selection_disappears() {
    for (label, view, key) in [
        ("All", View::All, egui::Key::Num1),
        ("Unstaged", View::Unstaged, egui::Key::Num2),
        ("Staged", View::Staged, egui::Key::Num3),
        ("Unpushed", View::Unpushed, egui::Key::Num4),
    ] {
        for keyboard in [false, true] {
            for empty in [false, true] {
                let mut app = viewer();
                app.tabs[0].view = if view == View::All {
                    View::Staged
                } else {
                    View::All
                };
                let mut files = app.tabs[0].files.as_ref().clone();
                files[0].path = GitPath(b"other.rs".to_vec());
                if empty {
                    files.clear();
                }
                let mut harness = Harness::new_state(|context, app| app.show(context), app);
                if keyboard {
                    harness.key_press_modifiers(egui::Modifiers::COMMAND, key);
                } else {
                    harness
                        .get_by_role_and_label(egui::accesskit::Role::Button, label)
                        .click();
                }
                harness.run_steps(2);
                assert!(harness.state().tabs[0].presentation.is_none());
                let tab = &mut harness.state_mut().tabs[0];
                tab.apply_events(
                    vec![Event::Status {
                        automatic: false,
                        generation: tab.generation,
                        model: Arc::new(TreeModel::new(&files)),
                        files: Arc::new(files),
                    }],
                    false,
                );
                assert_eq!(tab.view, view);
                assert!(!tab.document_mode);
                assert_eq!(
                    tab.selected,
                    (!empty).then(|| GitPath(b"other.rs".to_vec()))
                );
                harness.run_steps(2);
                assert_eq!(
                    harness
                        .get_all_by_role(egui::accesskit::Role::TextInput)
                        .count(),
                    2
                );
                if empty {
                    assert!(harness.query_by_label("No changes in this view").is_some());
                    assert!(harness.query_by_label("Git Diff").is_some());
                    assert!(
                        harness
                            .get_by_label("Full file")
                            .accesskit_node()
                            .is_disabled()
                    );
                }
            }
        }
    }
}

#[test]
fn full_file_disables_git_buttons_and_shortcuts() {
    let mut app = viewer();
    app.tabs[0].document_mode = true;
    let mut harness = Harness::new_state(|context, app| app.show(context), app);
    for (label, key) in [
        ("All", egui::Key::Num1),
        ("Unstaged", egui::Key::Num2),
        ("Staged", egui::Key::Num3),
        ("Unpushed", egui::Key::Num4),
    ] {
        assert!(
            harness
                .get_by_role_and_label(egui::accesskit::Role::Button, label)
                .accesskit_node()
                .is_disabled()
        );
        harness.key_press_modifiers(egui::Modifiers::COMMAND, key);
        harness.run();
        assert_eq!(harness.state().tabs[0].view, View::All);
        assert!(harness.state().tabs[0].document_mode);
        assert!(!harness.state().tabs[0].loading);
        assert!(harness.state().tabs[0].presentation.is_some());
    }
}

#[test]
fn full_file_can_return_to_git_when_selected_file_has_no_changes() {
    for empty in [false, true] {
        let mut app = viewer();
        let tab = &mut app.tabs[0];
        tab.document_mode = true;
        tab.selected = Some(GitPath(b"clean.txt".to_vec()));
        if empty {
            tab.files = Arc::new(Vec::new());
        }
        let files = tab.files.clone();
        let mut harness = Harness::new_state(|context, app| app.show(context), app);
        assert!(
            !harness
                .get_by_label("Git Diff")
                .accesskit_node()
                .is_disabled()
        );
        harness.get_by_label("Git Diff").click();
        harness.run_steps(2);
        let tab = &mut harness.state_mut().tabs[0];
        assert!(!tab.document_mode);
        assert!(tab.presentation.is_none());
        tab.apply_events(
            vec![Event::Status {
                automatic: false,
                generation: tab.generation,
                model: Arc::new(TreeModel::new(&files)),
                files,
            }],
            false,
        );
        assert_eq!(
            tab.selected,
            (!empty).then(|| GitPath(b"src/main.rs".to_vec()))
        );
        harness.run_steps(2);
        assert_eq!(
            harness
                .get_all_by_role(egui::accesskit::Role::TextInput)
                .count(),
            2
        );
    }
}

#[test]
fn document_target_git_view_does_not_select_another_files_changes() {
    let mut tab = fixture();
    tab.request.target = OpenTarget::Document;
    tab.request.repository = "/repo/clean.txt".into();
    tab.selected = Some(GitPath(b"clean.txt".to_vec()));
    tab.apply_events(
        vec![Event::Status {
            automatic: false,
            generation: tab.generation,
            files: tab.files.clone(),
            model: tab.tree.model_snapshot(),
        }],
        false,
    );
    assert!(!tab.document_mode);
    assert_eq!(tab.selected, Some(GitPath(b"clean.txt".to_vec())));
    assert!(tab.presentation.is_none());
    assert!(!tab.loading);
}

#[test]
fn automatic_refresh_does_not_open_full_file_after_changes_disappear() {
    let mut tab = fixture();
    tab.apply_events(
        vec![Event::Status {
            automatic: false,
            generation: tab.generation,
            files: Arc::new(Vec::new()),
            model: Arc::new(TreeModel::new(&[])),
        }],
        false,
    );
    assert!(!tab.document_mode);
    assert!(tab.selected.is_none());
    assert!(tab.presentation.is_none());
    assert!(!tab.loading);
}

#[test]
fn restarted_worker_reports_discovery_failure() {
    let directory = tempfile::tempdir().unwrap();
    let mut tab = fixture();
    tab.request.repository = directory
        .path()
        .join("missing")
        .to_string_lossy()
        .into_owned();
    tab.worker = Some(Worker::spawn(tab.request.clone(), tab.context.clone()));
    for restart in [false, true] {
        if restart {
            tab.refresh();
        }
        poll_until(
            format!("discovery worker termination, restart={restart}"),
            WAIT_LIMIT,
            |_| tab.worker.as_ref().unwrap().is_finished().then_some(()),
        )
        .unwrap_or_else(|failure| panic!("{failure}"));
    }
    tab.events(false);
    assert!(tab.error.is_some());
    assert!(!tab.loading);
}

#[test]
fn document_result_cannot_replace_git_view_during_worker_startup() {
    let mut tab = fixture();
    let previous = Arc::clone(&tab.presentation.as_ref().unwrap().document);
    let document = Arc::new(PreparedDocument::from_text(
        "/repo/clean.txt".into(),
        "whole file".into(),
        &CancelToken::default(),
    ));
    tab.apply_events(
        vec![Event::Diff {
            generation: tab.generation,
            document,
        }],
        false,
    );
    assert!(Arc::ptr_eq(
        &previous,
        &tab.presentation.as_ref().unwrap().document
    ));
    assert!(!tab.document_mode);
}

#[test]
fn document_discovery_preserves_selection_when_root_spelling_differs() {
    let mut tab = fixture();
    tab.request.target = OpenTarget::Document;
    tab.request.repository = "/repo/src/main.rs".into();
    let selected = tab.selected.clone();
    tab.apply_events(vec![Event::Ready("/canonical/repo".into(), true)], false);
    assert_eq!(tab.selected, selected);
}

#[test]
fn restarted_document_worker_replays_selected_display_mode() {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(directory.path())
            .status()
            .unwrap()
            .success()
    );
    let path = directory.path().join("notes.txt");
    std::fs::write(&path, "whole file\n").unwrap();
    for document_mode in [false, true] {
        let mut tab = fixture();
        tab.request.target = OpenTarget::Document;
        tab.request.file = None;
        tab.request.repository.clear();
        tab.worker = Some(Worker::spawn(tab.request.clone(), tab.context.clone()));
        let mut shutdown = ProgressGuard::new(
            format!("old document worker termination, document_mode={document_mode}"),
            WAIT_LIMIT,
        );
        while !tab.worker.as_ref().unwrap().is_finished() {
            tab.events(false);
            if tab.error.is_some() {
                shutdown.advance("startup error received");
            }
            shutdown.check(tab.worker.as_ref().unwrap().is_finished());
            std::thread::sleep(Duration::from_millis(10));
        }
        tab.request.repository = path.to_string_lossy().into_owned();
        tab.selected = Some(GitPath(b"notes.txt".to_vec()));
        tab.document_mode = document_mode;
        tab.view = View::Staged;
        tab.presentation = None;
        tab.refresh();
        let initial_files = Arc::clone(&tab.files);
        let mut reload = ProgressGuard::new(
            format!(
                "document reload generation {}, document_mode={document_mode}",
                tab.generation
            ),
            WAIT_LIMIT,
        );
        while tab.loading {
            tab.events(false);
            if !tab.loading {
                break;
            }
            advance_tab_load(&mut reload, &tab, &initial_files);
            reload.check(tab.worker.as_ref().unwrap().is_finished());
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(tab.error.is_none(), "{:?}", tab.error);
        assert_eq!(tab.document_mode, document_mode);
        assert_eq!(
            tab.presentation.as_ref().map(|p| p.document.is_document),
            document_mode.then_some(true)
        );
        if !document_mode {
            assert_eq!(tab.selected, Some(GitPath(b"notes.txt".to_vec())));
        }
    }
}

#[test]
fn copying_path_and_selected_patch_uses_original_metadata() {
    let mut harness = Harness::new_state(|context, app| app.show(context), viewer());
    harness.event(egui::Event::Key {
        key: egui::Key::C,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
    });
    harness.step();
    assert!(harness.output().platform_output.commands.iter().any(
        |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "src/main.rs")
    ));
    harness.event(egui::Event::Key {
        key: egui::Key::C,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND | egui::Modifiers::ALT,
    });
    harness.step();
    assert!(harness.output().platform_output.commands.iter().any(
        |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "Unstaged patch")
    ));
}

#[test]
fn text_fields_keep_editing_shortcuts() {
    let mut harness = Harness::new_state(|context, app| app.show(context), viewer());
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
    harness.run();
    harness.event(egui::Event::Text("main".into()));
    harness.run();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Num3);
    harness.run();
    assert_eq!(harness.state().tabs[0].view, View::All);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::ArrowDown);
    harness.run();
    assert_eq!(
        harness.state().tabs[0]
            .presentation
            .as_ref()
            .unwrap()
            .scroll_y,
        0.0
    );
}

#[test]
fn windows_repository_identity_reuses_existing_tab_for_forwarded_verbatim_path() {
    let mut viewer = viewer();
    viewer.tabs[0].request.repository = r"C:\Users\Dev\repository".into();
    let mut forwarded = viewer.tabs[0].request.clone();
    forwarded.repository = r"\\?\C:\Users\Dev\repository".into();
    assert_eq!(viewer.matching_tab(&forwarded, true), Some(0));
}

#[test]
fn stale_diff_result_does_not_replace_selected_file() {
    let mut tab = fixture();
    let current = Arc::clone(&tab.presentation.as_ref().unwrap().document);
    let stale = Arc::new(PreparedDocument::new(
        DiffDocument::default(),
        &CancelToken::default(),
    ));
    tab.apply_events(
        vec![Event::Diff {
            generation: 6,
            document: stale,
        }],
        false,
    );
    assert!(Arc::ptr_eq(
        &current,
        &tab.presentation.as_ref().unwrap().document
    ));
    assert_eq!(tab.generation, 7);
}

#[test]
fn automatic_refresh_preserves_selected_comparison_and_scroll() {
    let mut tab = fixture();
    tab.presentation.as_mut().unwrap().scroll_y = 80.0;
    tab.search = "main".into();
    tab.search_hits = vec![0, 1];
    tab.search_index = 1;
    let selected = tab.selected.clone();
    let files = Arc::clone(&tab.files);
    let visible = tab.visible_files.as_ptr();
    tab.apply_events(
        vec![Event::Status {
            automatic: true,
            generation: 7,
            files: tab.files.clone(),
            model: tab.tree.model_snapshot(),
        }],
        false,
    );
    assert_eq!(tab.generation, 7);
    assert_eq!(tab.selected, selected);
    assert!(!tab.loading);
    assert!(Arc::ptr_eq(&tab.files, &files));
    assert_eq!(tab.visible_files.as_ptr(), visible);
    assert_eq!(tab.search, "main");
    assert_eq!(tab.search_hits, vec![0, 1]);
    assert_eq!(tab.search_index, 1);
    assert_eq!(tab.comparison, 1);
    let document = Arc::clone(&tab.presentation.as_ref().unwrap().document);
    tab.apply_events(
        vec![Event::Diff {
            generation: 7,
            document,
        }],
        false,
    );
    assert_eq!(tab.presentation.as_ref().unwrap().scroll_y, 80.0);
    assert_eq!(tab.comparison, 1);
    assert!(!tab.loading);
}

#[test]
fn unchanged_automatic_status_without_selected_change_preserves_idle_state() {
    let mut tab = fixture();
    tab.selected = None;
    tab.presentation = None;
    tab.apply_events(
        vec![Event::Status {
            automatic: true,
            generation: tab.generation,
            files: tab.files.clone(),
            model: tab.tree.model_snapshot(),
        }],
        false,
    );
    assert_eq!(tab.generation, 7);
    assert!(tab.selected.is_none());
    assert!(!tab.loading);
}

#[test]
fn explicit_equal_status_refresh_still_reveals_and_completes_loading() {
    let mut tab = fixture();
    tab.refresh();
    tab.reveal_requested = true;
    let generation = tab.generation;
    let files = Arc::clone(&tab.files);
    let document = Arc::clone(&tab.presentation.as_ref().unwrap().document);
    tab.apply_events(
        vec![Event::Status {
            automatic: false,
            generation,
            files: tab.files.clone(),
            model: tab.tree.model_snapshot(),
        }],
        false,
    );
    assert_eq!(tab.generation, generation + 1);
    assert!(!tab.reveal_requested);
    assert!(tab.loading);
    assert!(Arc::ptr_eq(&tab.files, &files));
    tab.apply_events(
        vec![Event::Diff {
            generation: tab.generation,
            document,
        }],
        false,
    );
    assert!(!tab.loading);
}

#[test]
fn shortcuts_cycle_copy_comparison_and_copy_source_without_patch() {
    let mut harness = Harness::new_state(|context, app| app.show(context), viewer());
    harness.key_press_modifiers(
        egui::Modifiers::COMMAND | egui::Modifiers::ALT,
        egui::Key::ArrowLeft,
    );
    harness.run();
    assert_eq!(harness.state().tabs[0].comparison, 0);
    harness.event(egui::Event::Key {
        key: egui::Key::C,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND | egui::Modifiers::ALT | egui::Modifiers::SHIFT,
    });
    harness.step();
    let copied = harness
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|command| {
            if let egui::OutputCommand::CopyText(text) = command {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(copied, ["fn main() {}"]);
    harness.key_press_modifiers(
        egui::Modifiers::COMMAND | egui::Modifiers::ALT,
        egui::Key::ArrowRight,
    );
    harness.run();
    assert_eq!(harness.state().tabs[0].comparison, 1);
}

#[test]
fn shortcut_actions_toggle_layout_zoom_and_refresh() {
    let mut harness = Harness::new_state(|context, app| app.show(context), viewer());
    harness.key_press_modifiers(
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Key::U,
    );
    harness.run();
    assert!(harness.state().preferences.unified);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Minus);
    harness.run();
    assert_eq!(harness.state().preferences.font_size, 12.0);
    harness.key_press_modifiers(
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Key::Equals,
    );
    harness.run();
    assert_eq!(harness.state().preferences.font_size, 13.0);
    let generation = harness.state().tabs[0].generation;
    harness.key_press(egui::Key::F5);
    harness.run_steps(2);
    assert_eq!(harness.state().tabs[0].generation, generation + 1);
    assert!(harness.state().tabs[0].loading);
}

#[test]
fn search_arrow_buttons_move_in_both_directions() {
    let mut app = viewer();
    app.tabs[0].search_hits = vec![0, 1];
    let mut harness = Harness::new_state(|context, app| app.show(context), app);
    harness.get_by_label("Next match").click();
    harness.run();
    assert_eq!(harness.state().tabs[0].search_index, 1);
    harness.get_by_label("Previous match").click();
    harness.run();
    assert_eq!(harness.state().tabs[0].search_index, 0);
}

#[test]
fn search_shortcuts_move_in_both_directions() {
    let mut app = viewer();
    app.tabs[0].search_hits = vec![0, 1];
    let mut harness = Harness::new_state(|context, app| app.show(context), app);
    harness.key_press(egui::Key::F3);
    harness.run();
    assert_eq!(harness.state().tabs[0].search_index, 1);
    harness.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::F3);
    harness.run();
    assert_eq!(harness.state().tabs[0].search_index, 0);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert_eq!(harness.state().tabs[0].search_index, 1);
    harness.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Enter);
    harness.run();
    assert_eq!(harness.state().tabs[0].search_index, 0);
}

#[test]
fn native_copy_events_honor_exact_copy_variant_modifiers() {
    let mut harness = Harness::new_state(|context, app| app.show(context), viewer());
    for (modifiers, expected) in [
        (
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            "src/main.rs",
        ),
        (
            egui::Modifiers::COMMAND | egui::Modifiers::ALT,
            "Unstaged patch",
        ),
        (
            egui::Modifiers::COMMAND | egui::Modifiers::ALT | egui::Modifiers::SHIFT,
            "fn main() {}",
        ),
    ] {
        harness.input_mut().modifiers = modifiers | egui::Modifiers::COMMAND;
        harness.event(egui::Event::Copy);
        harness.step();
        let copied = harness
            .output()
            .platform_output
            .commands
            .iter()
            .filter_map(|command| {
                if let egui::OutputCommand::CopyText(text) = command {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(copied, [expected]);
    }
}

#[test]
fn source_context_menu_takes_copy_focus_from_search_field() {
    let mut harness = Harness::new_state(|context, app| app.show(context), viewer());
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
    harness.run();
    harness.event(egui::Event::Text("search text".into()));
    harness.run();
    harness
        .get_all_by_label("fn main() {}")
        .next()
        .unwrap()
        .click_secondary();
    harness.run();
    harness.get_by_label("Select all").click();
    harness.run();
    harness.input_mut().modifiers = egui::Modifiers::COMMAND;
    harness.event(egui::Event::Copy);
    harness.step();
    assert!(harness.output().platform_output.commands.iter().any(
        |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "fn main() {}")
    ));
    assert_eq!(harness.state().tabs[0].search, "search text");
}
#[test]
fn native_copy_uses_c_release_modifiers_when_the_chord_finishes_in_one_frame() {
    let mut harness = Harness::new_state(|context, app| app.show(context), viewer());
    harness.input_mut().events = vec![
        egui::Event::Copy,
        egui::Event::Key {
            key: egui::Key::C,
            physical_key: Some(egui::Key::C),
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        },
    ];
    harness.input_mut().modifiers = egui::Modifiers::NONE;
    harness.step();
    assert!(harness.output().platform_output.commands.iter().any(
        |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "src/main.rs")
    ));
}

#[test]
fn document_targets_keep_their_own_identity() {
    let mut app = viewer();
    app.tabs[0].request.target = OpenTarget::Document;
    app.tabs[0].request.repository = "/repo/readme.txt".into();
    let same = app.tabs[0].request.clone();
    assert_eq!(app.matching_tab(&same, false), Some(0));
    let mut other = same.clone();
    other.repository = "/repo/other.txt".into();
    assert_eq!(app.matching_tab(&other, false), None);
    other.repository = same.repository;
    other.target = OpenTarget::Repository;
    assert_eq!(app.matching_tab(&other, false), None);
}

#[test]
fn unchanged_requested_file_opens_full_text_without_selecting_other_changes() {
    let mut tab = fixture();
    tab.selected = Some(GitPath(b"clean.txt".to_vec()));
    tab.reveal_requested = true;
    tab.apply_events(
        vec![Event::Status {
            automatic: false,
            generation: 7,
            files: tab.files.clone(),
            model: tab.tree.model_snapshot(),
        }],
        false,
    );
    assert!(tab.document_mode);
    assert!(!tab.reveal_requested);
    assert_eq!(tab.selected, Some(GitPath(b"clean.txt".to_vec())));
    assert_eq!(
        tab.document_path().unwrap(),
        if cfg!(windows) {
            "/repo\\clean.txt"
        } else {
            "/repo/clean.txt"
        }
    );
}

#[test]
fn windows_document_path_uses_one_separator_for_nested_git_paths() {
    assert_eq!(
        join_document_path(
            r"\\?\C:\repo",
            ".agents/skills/speckit-checklist/SKILL.md",
            '\\'
        ),
        r"\\?\C:\repo\.agents\skills\speckit-checklist\SKILL.md"
    );
    assert_eq!(
        join_document_path("/repo/", "src/main.rs", '/'),
        "/repo/src/main.rs"
    );
}

#[test]
fn document_status_refresh_keeps_document_contents_and_selection() {
    let mut tab = fixture();
    tab.document_mode = true;
    let document = Arc::new(PreparedDocument::from_text(
        "/repo/src/main.rs".into(),
        "whole file".into(),
        &CancelToken::default(),
    ));
    tab.presentation = Some(DiffPresentation::new(Arc::clone(&document), false));
    tab.apply_events(
        vec![Event::Status {
            automatic: false,
            generation: 7,
            files: Arc::new(Vec::new()),
            model: Arc::new(TreeModel::new(&[])),
        }],
        false,
    );
    assert!(Arc::ptr_eq(
        &document,
        &tab.presentation.as_ref().unwrap().document
    ));
    assert_eq!(tab.selected, Some(GitPath(b"src/main.rs".to_vec())));
}

#[test]
fn selected_deleted_file_disables_full_text_until_disk_reappears() {
    let mut tab = fixture();
    tab.apply_events(
        vec![Event::Availability {
            generation: 7,
            available: false,
        }],
        false,
    );
    assert!(!tab.full_available);
    tab.apply_events(
        vec![Event::Availability {
            generation: 7,
            available: true,
        }],
        false,
    );
    assert!(tab.full_available);
}

#[test]
fn reflog_panel_preserves_file_filter_and_accepts_diff_events() {
    let mut tab = fixture();
    tab.view = View::Staged;
    tab.filter = "main".into();
    let selected = tab.selected.clone();
    let document = Arc::clone(&tab.presentation.as_ref().unwrap().document);
    let generation = tab.generation;
    tab.toggle_reflog();
    assert!(tab.reflog_open);
    assert_eq!(tab.generation, generation);
    tab.apply_events(
        vec![Event::Diff {
            generation,
            document,
        }],
        false,
    );
    assert_eq!(tab.selected, selected);
    assert!(!tab.loading);
    tab.toggle_reflog();
    assert!(!tab.reflog_open);
    assert!(!tab.document_mode);
    assert_eq!(tab.view, View::Staged);
    assert_eq!(tab.filter, "main");
    assert_eq!(tab.selected, selected);
}

#[test]
fn reflog_panel_preserves_diff_controls_and_supports_shortcut_and_refresh() {
    let mut harness = Harness::new_state(|context, app| app.show(context), viewer());
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Num5);
    harness.run_steps(2);
    assert!(harness.state().tabs[0].reflog_open);
    for label in [
        "Search diff",
        "Previous match",
        "Next match",
        "Unified",
        "All",
        "Copy",
    ] {
        assert!(harness.query_by_label(label).is_some(), "{label}");
    }
    let generation = harness.state().tabs[0].generation;
    harness.key_press(egui::Key::F5);
    harness.run_steps(2);
    assert!(harness.state().tabs[0].generation > generation);
    harness.get_by_label("Reflog").click();
    harness.run_steps(2);
    assert!(!harness.state().tabs[0].reflog_open);
}

fn reflog_list(branch: Option<&str>, count: usize) -> ReflogList {
    ReflogList {
        snapshot: None,
        start: 0,
        has_more: false,
        branch: branch.map(str::to_owned),
        notice: None,
        entries: (0..count)
            .map(|index| fastdiff_core::ReflogEntry {
                id: format!("{index:040x}"),
                short_id: format!("{index:07x}"),
                decorations: branch.unwrap_or_default().into(),
                selector: format!("HEAD@{{{index}}}"),
                action: format!("commit: Commit subject {index} 漢字"),
            })
            .collect(),
    }
}

#[test]
fn reflog_events_use_independent_generations_and_ignore_closed_panel() {
    let mut tab = fixture();
    tab.toggle_reflog();
    let generation = tab.reflog_generation;
    tab.apply_events(
        vec![Event::Reflog {
            generation: generation - 1,
            list: reflog_list(Some("old"), 1),
        }],
        false,
    );
    assert!(tab.reflog.is_none());
    tab.select_file(0, false);
    assert_ne!(tab.generation, generation);
    tab.apply_events(
        vec![Event::Reflog {
            generation,
            list: reflog_list(Some("new"), 1),
        }],
        false,
    );
    assert_eq!(tab.branch.as_deref(), Some("new"));
    assert!(!tab.reflog_loading);
    assert!(
        tab.loading,
        "Reflog results must not finish an active diff load"
    );
    tab.toggle_reflog();
    tab.apply_events(
        vec![
            Event::Reflog {
                generation,
                list: reflog_list(Some("closed"), 1),
            },
            Event::Branch {
                generation: tab.generation,
                branch: None,
            },
        ],
        false,
    );
    assert_eq!(tab.branch.as_deref(), Some("Detached HEAD"));
    assert_eq!(tab.reflog.as_ref().unwrap().branch.as_deref(), Some("new"));
}

#[test]
fn branch_label_is_truncated_with_full_tooltip_and_absent_outside_git() {
    let name = format!("feature/{}", "long-branch-name-".repeat(20));
    let mut app = viewer();
    app.tabs[0].branch = Some(name.clone());
    let mut harness = Harness::builder()
        .with_size(egui::vec2(420.0, 600.0))
        .build_state(|context, app| app.show(context), app);
    harness.run();
    assert!(harness.get_by_label(&name).rect().right() <= 420.0);
    harness.get_by_label(&name).hover();
    harness.run();
    assert_eq!(
        harness.get_all_by_label(&name).count(),
        2,
        "label plus one tooltip"
    );
    harness.state_mut().tabs[0].has_repository = false;
    harness.state_mut().tabs[0].branch = Some("Detached HEAD".into());
    harness.run();
    assert!(harness.query_by_label("Detached HEAD").is_none());
    assert!(harness.query_by_label("Reflog").is_none());
}

#[test]
fn reflog_actions_stay_aligned_when_panel_width_changes() {
    for width in [180.0, 320.0, 500.0] {
        let mut tab = fixture();
        tab.reflog = Some(reflog_list(Some("main"), 3));
        let mut harness = Harness::builder()
            .with_size(egui::vec2(width, 300.0))
            .build_ui_state(|ui, tab| tab.show_reflog(ui, 0, false), tab);
        harness.run();
        let text_x = |label: &str| {
            harness
                .output()
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) if text.galley.job.text == label => {
                        Some(text.pos.x)
                    }
                    _ => None,
                })
                .unwrap()
        };
        for index in 0..3 {
            assert!(
                (text_x("HEAD@{0}: commit: Commit subject 0 漢字")
                    - text_x(&format!(
                        "HEAD@{{{index}}}: commit: Commit subject {index} 漢字"
                    )))
                .abs()
                    < 1.0,
                "Reflog action field at width {width}"
            );
        }
    }
}

#[test]
fn reflog_rows_are_virtualized_informational_and_fit_narrow_windows() {
    for width in [420.0, 640.0, 1100.0] {
        let mut app = viewer();
        app.tabs[0].reflog_open = true;
        app.tabs[0].reflog = Some(reflog_list(Some("main"), 10_000));
        let mut harness = Harness::builder()
            .with_size(egui::vec2(width, 600.0))
            .build_state(|context, app| app.show(context), app);
        harness.run();
        for label in [
            "0000000",
            "(main)",
            "HEAD@{0}: commit: Commit subject 0 漢字",
        ] {
            let nodes: Vec<_> = harness.get_all_by_label(label).collect();
            assert!(!nodes.is_empty(), "{label} must be visible");
            for node in nodes {
                let rect = node.rect();
                assert!(
                    rect.left() >= 0.0 && rect.right() <= width,
                    "{label}: {rect:?}"
                );
            }
        }
        assert!(
            harness
                .query_by_label("HEAD@{9999}: commit: Commit subject 9999 漢字")
                .is_none()
        );
        assert!(
            harness
                .query_by_role_and_label(egui::accesskit::Role::Button, "src/main.rs")
                .is_some()
        );
        assert!(
            harness
                .query_by_role_and_label(
                    egui::accesskit::Role::Button,
                    "HEAD@{0}: commit: Commit subject 0 漢字"
                )
                .is_none()
        );
    }
}

#[test]
fn reflog_panel_stays_open_when_switching_document_and_diff() {
    let mut tab = fixture();
    tab.toggle_reflog();
    tab.toggle_document(true);
    assert!(tab.reflog_open);
    assert!(tab.document_mode);
    tab.toggle_document(false);
    assert!(tab.reflog_open);
    assert!(!tab.document_mode);
    tab.set_view(View::Staged);
    assert!(tab.reflog_open);
    assert_eq!(tab.view, View::Staged);
}

#[test]
fn reflog_panel_resizes_vertically_and_remembers_height_after_reopening() {
    let mut tab = fixture();
    tab.reflog_open = true;
    tab.reflog = Some(reflog_list(Some("main"), 100));
    let mut harness = Harness::builder()
        .with_size(egui::vec2(300.0, 600.0))
        .build_ui_state(|ui, tab| Fastdiff::sidebar(tab, ui, 0, false), tab);
    harness.run();
    let header_rect = |harness: &Harness<ReviewTab>| harness.get_by_label("Reflog").rect();
    let initial = header_rect(&harness);
    let start = initial.center_top() - egui::vec2(0.0, 6.0);
    let end = start - egui::vec2(0.0, 100.0);
    harness.hover_at(start);
    harness.drag_at(start);
    harness.run_steps(2);
    harness.hover_at(end);
    harness.run_steps(2);
    harness.drop_at(end);
    harness.run();
    let resized = header_rect(&harness);
    assert!(resized.top() < initial.top() - 90.0);
    assert!(
        harness
            .query_by_role(egui::accesskit::Role::TextInput)
            .is_some()
    );
    harness.get_by_label("Reflog").click();
    harness.run_steps(2);
    assert!(!harness.state().reflog_open);
    harness.get_by_label("Reflog").click();
    harness.run_steps(2);
    assert!(harness.state().reflog_open);
    assert!((header_rect(&harness).top() - resized.top()).abs() < 1.0);
}

#[test]
fn restarted_worker_restores_reflog_and_file_changes() {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(directory.path())
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(directory.path().join("untracked.txt"), "must not load\n").unwrap();
    let mut tab = fixture();
    tab.request.repository = directory
        .path()
        .join("missing")
        .to_string_lossy()
        .into_owned();
    tab.worker = Some(Worker::spawn(tab.request.clone(), tab.context.clone()));
    poll_until(
        "old worker termination before reflog reload",
        WAIT_LIMIT,
        |_| tab.worker.as_ref().unwrap().is_finished().then_some(()),
    )
    .unwrap_or_else(|failure| panic!("{failure}"));
    tab.request.repository = directory.path().to_string_lossy().into_owned();
    tab.reflog_open = true;
    tab.refresh();
    let initial_files = Arc::clone(&tab.files);
    poll_until("reflog and file changes reload", WAIT_LIMIT, |guard| {
        tab.events(false);
        advance_tab_load(guard, &tab, &initial_files);
        (!tab.loading && !tab.reflog_loading).then_some(())
    })
    .unwrap_or_else(|failure| panic!("{failure}"));
    assert!(tab.error.is_none(), "{:?}", tab.error);
    assert!(tab.reflog_open);
    assert_eq!(tab.branch.as_deref(), Some("main"));
    assert!(tab.reflog.as_ref().unwrap().entries.is_empty());
    assert_eq!(tab.files[0].path.display(), "untracked.txt");
    assert_eq!(tab.selected, Some(GitPath(b"untracked.txt".to_vec())));
}

#[test]
fn git_filter_shortcut_keeps_reflog_panel_open() {
    let mut app = viewer();
    app.tabs[0].reflog_open = true;
    let mut harness = Harness::new_state(|context, app| app.show(context), app);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Num3);
    harness.run_steps(2);
    assert!(harness.state().tabs[0].reflog_open);
    assert_eq!(harness.state().tabs[0].view, View::Staged);
}

#[test]
fn reflog_disclosure_stays_below_tree_and_preserves_selected_diff() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .build_state(|context, app| app.show(context), viewer());
    harness.run();
    let header = harness.get_by_label("Reflog").rect();
    let tree = harness
        .get_by_role_and_label(egui::accesskit::Role::Button, "src/main.rs")
        .rect();
    assert!(
        header.top() > tree.bottom(),
        "Reflog must be below the tree"
    );
    let tab = &harness.state().tabs[0];
    let selected = tab.selected.clone();
    let generation = tab.generation;
    let document = Arc::clone(&tab.presentation.as_ref().unwrap().document);
    harness.get_by_label("Reflog").click();
    harness.run_steps(2);
    let tab = &harness.state().tabs[0];
    assert!(tab.reflog_open);
    assert_eq!(tab.selected, selected);
    assert_eq!(tab.generation, generation);
    assert!(Arc::ptr_eq(
        &tab.presentation.as_ref().unwrap().document,
        &document
    ));
    assert!(
        harness
            .query_by_role_and_label(egui::accesskit::Role::Button, "src/main.rs")
            .is_some()
    );
    assert!(harness.query_by_label("Search diff").is_some());
}

#[test]
fn reflog_panel_error_does_not_replace_diff_or_clear_file_loading() {
    let mut tab = fixture();
    tab.toggle_reflog();
    tab.loading = true;
    let document = Arc::clone(&tab.presentation.as_ref().unwrap().document);
    tab.apply_events(
        vec![Event::ReflogError {
            generation: tab.reflog_generation,
            message: "Could not load reflog".into(),
        }],
        false,
    );
    assert_eq!(tab.reflog_error.as_deref(), Some("Could not load reflog"));
    assert!(!tab.reflog_loading);
    assert!(tab.loading);
    assert!(tab.error.is_none());
    assert!(Arc::ptr_eq(
        &tab.presentation.as_ref().unwrap().document,
        &document
    ));
}

#[test]
fn reflog_panel_on_file_tabs_does_not_expose_other_file_selection() {
    let mut app = viewer();
    app.tabs[0].request.target = OpenTarget::Document;
    app.tabs[0].request.repository = "/repo/src/main.rs".into();
    app.tabs[0].document_mode = true;
    app.tabs[0].reflog_open = true;
    app.tabs[0].reflog = Some(reflog_list(Some("main"), 1));
    let mut harness = Harness::new_state(|context, app| app.show(context), app);
    harness.run();
    assert!(harness.query_by_label("Reflog").is_some());
    assert!(
        harness
            .query_by_label("HEAD@{0}: commit: Commit subject 0 漢字")
            .is_some()
    );
    assert!(
        harness
            .query_by_role_and_label(egui::accesskit::Role::Button, "src/main.rs")
            .is_none()
    );
    assert!(harness.query_by_label("/repo/src/main.rs").is_some());
    harness.get_by_label("Reflog").click();
    harness.run_steps(2);
    assert!(!harness.state().tabs[0].reflog_open);
    assert!(harness.state().tabs[0].document_mode);
}

#[test]
fn reflog_disclosure_fits_zoomed_windows_and_long_file_trees() {
    for (width, zoom) in [(420.0, 1.0), (640.0, 1.5), (1100.0, 2.0)] {
        let mut app = viewer();
        let tab = &mut app.tabs[0];
        tab.files = Arc::new(
            (0..1_000)
                .map(|index| FileChange {
                    path: GitPath(format!("file-{index:04}.rs").into_bytes()),
                    ..tab.files[0].clone()
                })
                .collect(),
        );
        tab.tree.update(Arc::new(TreeModel::new(&tab.files)));
        tab.reflog = Some(reflog_list(Some("main"), 10_000));
        let mut harness = Harness::builder()
            .with_size(egui::vec2(width, 600.0))
            .build_state(|context, app| app.show(context), app);
        harness.ctx.set_zoom_factor(zoom);
        harness.run();
        let collapsed = harness.get_by_label("Reflog").rect();
        assert!(collapsed.bottom() <= 600.0 && collapsed.right() <= width);
        harness.get_by_label("Reflog").click_accesskit();
        harness.run();
        assert!(harness.state().tabs[0].reflog_open);
        let expanded = harness.get_by_label("Reflog").rect();
        assert!(expanded.top() < collapsed.top());
        assert!(
            harness
                .query_by_label("HEAD@{0}: commit: Commit subject 0 漢字")
                .is_some()
        );
        assert!(
            harness
                .query_by_label("HEAD@{9999}: commit: Commit subject 9999 漢字")
                .is_none()
        );
        harness.get_by_label("Reflog").click_accesskit();
        harness.run();
        assert!(!harness.state().tabs[0].reflog_open);
        assert!(harness.get_by_label("Reflog").rect().top() > expanded.top());
    }
}

#[test]
fn theme_requests_enable_the_whole_window_and_remain_enabled() {
    let mut app = viewer();
    let context = egui::Context::default();
    let mut request = app.tabs[0].request.clone();
    request.theme = true;
    app.open(request.clone(), &context);
    assert!(app.preferences.theme);
    assert_eq!(app.tabs.len(), 1);
    request.theme = false;
    app.open(request.clone(), &context);
    assert!(app.preferences.theme);
    request.repository = "/another-repo".into();
    app.open(request, &context);
    assert!(app.preferences.theme);
    assert_eq!(app.tabs.len(), 2);
}

#[test]
fn startup_and_ipc_requests_enable_window_theme() {
    for startup_theme in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let server = IpcServer::bind_at(directory.path()).unwrap().unwrap();
        let context = egui::Context::default();
        let request = OpenRequest {
            theme: startup_theme,
            ..fixture().request
        };
        let mut app = Fastdiff::new(
            request.clone(),
            server,
            &context,
            Benchmark::new(false),
            SettingsStore::memory(),
        );
        assert_eq!(app.preferences.theme, startup_theme);
        let forwarded = OpenRequest {
            theme: true,
            ..request
        };
        assert!(fastdiff_cli::forward_at(directory.path(), &forwarded).unwrap());
        poll_until("forwarded window theme", WAIT_LIMIT, |_| {
            app.poll(&context);
            app.preferences.theme.then_some(())
        })
        .unwrap_or_else(|failure| panic!("{failure}"));
        assert_eq!(app.tabs.len(), 1);
    }
}

#[test]
fn theme_on_a_new_tab_applies_to_existing_tabs() {
    let mut app = viewer();
    let request = OpenRequest {
        repository: "/another-repo".into(),
        theme: true,
        ..fixture().request
    };
    app.open(request, &egui::Context::default());
    assert!(app.preferences.theme);
    assert_eq!(app.tabs.len(), 2);
}

#[test]
fn reflog_colors_match_reference_only_when_theme_is_enabled() {
    assert_eq!(
        crate::theme::ThemeId::Fastdiff.theme().status.reflog_id,
        egui::Color32::from_rgb(190, 154, 74)
    );
    assert_eq!(
        crate::theme::ThemeId::Fastdiff.theme().status.reflog_branch,
        egui::Color32::from_rgb(88, 234, 81)
    );
    for theme in [false, true] {
        let mut tab = fixture();
        tab.reflog = Some(reflog_list(Some("test/theme"), 2));
        let mut harness = Harness::builder()
            .with_size(egui::vec2(320.0, 300.0))
            .build_ui_state(|ui, tab| tab.show_reflog(ui, 0, theme), tab);
        harness.run();
        let expected = if theme {
            crate::theme::ThemeId::Fastdiff.theme().status.reflog_id
        } else {
            harness.ctx.style().visuals.text_color()
        };
        for (id, expected) in [
            ("0000000", expected),
            ("0000001", expected),
            (
                "(test/theme)",
                if theme {
                    crate::theme::ThemeId::Fastdiff.theme().status.reflog_branch
                } else {
                    harness.ctx.style().visuals.text_color()
                },
            ),
        ] {
            let labels: Vec<_> = harness
                .output()
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) if text.galley.job.text == id => Some(text),
                    _ => None,
                })
                .collect();
            assert!(!labels.is_empty(), "{id}");
            assert!(
                labels.iter().all(|text| text
                    .galley
                    .job
                    .sections
                    .iter()
                    .all(|section| { section.format.color == expected })),
                "{id}, theme={theme}"
            );
        }
    }
}

#[test]
fn reflog_panel_omits_duplicate_head_and_branch_row() {
    for branch in [Some("test/branch"), None] {
        for count in [0, 2] {
            let mut tab = fixture();
            let list = reflog_list(branch, count);
            tab.reflog = Some(list);
            let mut harness = Harness::builder()
                .with_size(egui::vec2(320.0, 300.0))
                .build_ui_state(|ui, tab| tab.show_reflog(ui, 0, false), tab);
            harness.run();
            for label in [
                "HEAD",
                "Commit ID",
                "Message",
                "-> test/branch",
                "(detached)",
            ] {
                assert!(harness.query_by_label(label).is_none(), "{label}");
            }
        }
    }
}

#[test]
fn zoom_changes_are_saved_once_and_saved_zoom_is_restored_at_startup() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(900.0, 500.0))
        .build_state(|context, app| app.show(context), viewer());
    harness.run();
    assert_eq!(harness.state().appearance.store.saves(), 0);

    harness.ctx.set_zoom_factor(1.3);
    harness.run();
    harness.run();
    let store = &harness.state().appearance.store;
    assert_eq!(store.saves(), 1);
    assert!((store.settings().zoom - 1.3).abs() < 1e-6);

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    std::fs::write(&path, r#"{"zoom": 1.5}"#).unwrap();
    let ipc_directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(ipc_directory.path(), std::fs::Permissions::from_mode(0o700))
            .unwrap();
    }
    let server = IpcServer::bind_at(ipc_directory.path()).unwrap().unwrap();
    let context = egui::Context::default();
    let app = Fastdiff::new(
        fixture().request,
        server,
        &context,
        Benchmark::new(false),
        SettingsStore::at(path),
    );
    // egui applies a requested zoom factor at the start of the next pass.
    let _ = context.run(egui::RawInput::default(), |_| {});
    assert!((context.zoom_factor() - 1.5).abs() < 1e-6);
    assert_eq!(app.appearance.store.saves(), 0);
}
