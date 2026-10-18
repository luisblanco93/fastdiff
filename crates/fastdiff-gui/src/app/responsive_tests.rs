use super::*;
use egui::accesskit::Role;
use egui_kittest::{Harness, kittest::Queryable};

fn viewer() -> Fastdiff {
    Fastdiff {
        tabs: vec![super::tests::fixture()],
        selected_tab: 0,
        preferences: Preferences::default(),
        server: None,
        benchmark: Benchmark::new(false),
        help_open: false,
        fallback_fonts: Default::default(),
        retired_workers: Vec::new(),
    }
}

#[test]
fn resizing_restores_split_view_without_changing_manual_preference() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1400.0, 720.0))
        .build_state(|context, app| app.show(context), viewer());
    harness.run();
    assert!(
        !harness.state().tabs[0]
            .presentation
            .as_ref()
            .unwrap()
            .unified
    );
    harness.set_size(egui::vec2(640.0, 480.0));
    harness.run();
    assert!(
        harness.state().tabs[0]
            .presentation
            .as_ref()
            .unwrap()
            .unified
    );
    assert!(!harness.state().preferences.unified);
    harness.set_size(egui::vec2(1400.0, 720.0));
    harness.run();
    assert!(
        !harness.state().tabs[0]
            .presentation
            .as_ref()
            .unwrap()
            .unified
    );
}

#[test]
fn explicit_unified_stays_unified_after_widening() {
    let mut app = viewer();
    app.preferences.unified = true;
    let mut harness = Harness::builder()
        .with_size(egui::vec2(640.0, 480.0))
        .build_state(|context, app| app.show(context), app);
    harness.set_size(egui::vec2(1400.0, 720.0));
    harness.run();
    assert!(
        harness.state().tabs[0]
            .presentation
            .as_ref()
            .unwrap()
            .unified
    );
    assert!(harness.state().preferences.unified);
}

#[test]
fn branch_name_is_at_the_left_of_the_footer() {
    let mut app = viewer();
    app.tabs[0].branch = Some("main".into());
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .build_state(|context, app| app.show(context), app);
    harness.run();
    let branch = harness.get_by_label("main").rect();
    let nodes = harness
        .output()
        .shapes
        .iter()
        .filter(|shape| {
            matches!(&shape.shape, egui::epaint::Shape::Circle(circle)
            if circle.center.x < branch.left()
                && circle.center.y >= branch.top()
                && circle.center.y <= branch.bottom())
        })
        .count();
    assert_eq!(nodes, 3, "Branch icon must precede the branch name");
    assert!(
        branch.top() > 680.0,
        "Branch must be in the footer: {branch:?}"
    );
    assert!(
        branch.left() < 40.0,
        "Branch must be left aligned: {branch:?}"
    );
}

#[test]
fn long_repository_status_fits_the_window() {
    let mut app = viewer();
    app.tabs[0].root = format!("/repo/{}/project", "nested/".repeat(80));
    let label = format!("1 changed files · {}", app.tabs[0].root);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(640.0, 480.0))
        .build_state(|context, app| app.show(context), app);
    harness.run();
    assert!(harness.get_by_label(&label).rect().right() <= 640.0);
}

#[test]
fn narrow_toolbar_keeps_controls_and_filter_inside_window() {
    for width in [420.0, 640.0, 1100.0] {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(width, 720.0))
            .build_state(|context, app| app.show(context), viewer());
        harness.run();
        for label in [
            "Help",
            "Previous match",
            "Next match",
            "Search diff",
            "Full file",
            "Git Diff",
            "All",
            "Unstaged",
            "Staged",
            "Unpushed",
            "Side by side",
            "Unified",
            "Copy",
            "100%",
            "UI −",
            "UI +",
        ] {
            let rect = harness
                .query_all_by_label(label)
                .map(|node| node.rect())
                .next()
                .unwrap_or_else(|| panic!("{label}"));
            assert!(
                rect.left() >= 0.0 && rect.right() <= width,
                "{label}: {rect:?}"
            );
        }
    }
}

#[test]
fn filtering_reveals_matching_file_and_restores_collapsed_folder() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .build_state(|context, app| app.show(context), viewer());
    harness.get_by_label("Collapse folder src").click();
    harness.run();
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "src/main.rs")
            .is_none()
    );
    harness.key_press_modifiers(
        egui::Modifiers::COMMAND | egui::Modifiers::ALT,
        egui::Key::F,
    );
    harness.run();
    harness.event(egui::Event::Text("main".into()));
    harness.run();
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "src/main.rs")
            .is_some()
    );
    harness.state_mut().tabs[0].filter.clear();
    harness.run();
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "src/main.rs")
            .is_none()
    );
    assert_eq!(
        harness.state().tabs[0].selected,
        Some(GitPath(b"src/main.rs".to_vec()))
    );
}

#[test]
fn clicking_files_preserves_tree_scroll_and_focus() {
    let mut app = viewer();
    let tab = &mut app.tabs[0];
    tab.files = Arc::new(
        (0..100)
            .map(|index| FileChange {
                path: GitPath(format!("file{index:03}.rs").into_bytes()),
                ..tab.files[0].clone()
            })
            .collect(),
    );
    tab.tree
        .update(Arc::new(crate::sidebar::TreeModel::new(&tab.files)));
    tab.visible_files = tab.tree.filtered_files("");
    tab.tree.reveal(30);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .build_state(|context, app| app.show(context), app);
    harness.run();

    for index in [35, 35, 37] {
        let label = format!("file{index:03}.rs");
        let before = harness.get_by_role_and_label(Role::Button, &label).rect();
        harness.get_by_role_and_label(Role::Button, &label).click();
        harness.run_steps(1);
        let focus = harness.ctx.memory(|memory| memory.focused());
        // This worker-free fixture intentionally remains loading after selection.
        harness.run_steps(3);
        assert_eq!(
            harness.state().tabs[0].selected,
            Some(GitPath(label.as_bytes().to_vec()))
        );
        assert_eq!(
            harness.get_by_role_and_label(Role::Button, &label).rect(),
            before,
            "Clicking a file must not move the tree"
        );
        assert!(focus.is_some());
        assert_eq!(harness.ctx.memory(|memory| memory.focused()), focus);
    }
}

#[test]
fn arrow_navigation_keeps_visible_rows_in_place() {
    for modifiers in [egui::Modifiers::NONE, egui::Modifiers::ALT] {
        let mut app = viewer();
        let tab = &mut app.tabs[0];
        tab.files = Arc::new(
            (0..100)
                .map(|index| FileChange {
                    path: GitPath(format!("file{index:03}.rs").into_bytes()),
                    ..tab.files[0].clone()
                })
                .collect(),
        );
        tab.tree
            .update(Arc::new(crate::sidebar::TreeModel::new(&tab.files)));
        tab.visible_files = tab.tree.filtered_files("");
        tab.tree.reveal(30);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1100.0, 720.0))
            .build_state(|context, app| app.show(context), app);
        harness.run();
        harness
            .get_by_role_and_label(Role::Button, "file035.rs")
            .click();
        harness.run_steps(3);
        harness.state_mut().tabs[0].presentation = super::tests::fixture().presentation;
        let document = harness.state().tabs[0]
            .presentation
            .as_ref()
            .unwrap()
            .document
            .clone();
        let before = harness
            .get_by_role_and_label(Role::Button, "file035.rs")
            .rect();
        for key in [
            egui::Key::ArrowDown,
            egui::Key::ArrowDown,
            egui::Key::ArrowUp,
        ] {
            harness.key_press_modifiers(modifiers, key);
            harness.run_steps(3);
            assert_eq!(
                harness
                    .get_by_role_and_label(Role::Button, "file035.rs")
                    .rect(),
                before
            );
            assert!(Arc::ptr_eq(
                &harness.state().tabs[0]
                    .presentation
                    .as_ref()
                    .unwrap()
                    .document,
                &document
            ));
        }
        for (key, count, label) in [
            (egui::Key::ArrowDown, 30, "file066.rs"),
            (egui::Key::ArrowUp, 40, "file026.rs"),
        ] {
            for _ in 0..count {
                harness.key_press_modifiers(modifiers, key);
                harness.run_steps(2);
            }
            let rect = harness.get_by_role_and_label(Role::Button, label).rect();
            assert!(rect.top() >= 0.0);
            assert!(rect.bottom() <= harness.get_by_label("Reflog").rect().top());
            assert!(Arc::ptr_eq(
                &harness.state().tabs[0]
                    .presentation
                    .as_ref()
                    .unwrap()
                    .document,
                &document
            ));
        }
    }
}

#[test]
fn navigation_reveals_a_selected_file_under_collapsed_parent() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .build_state(|context, app| app.show(context), viewer());
    harness.get_by_label("Collapse folder src").click();
    harness.run();
    harness.key_press_modifiers(egui::Modifiers::ALT, egui::Key::ArrowDown);
    // This worker-free fixture intentionally remains loading after navigation.
    harness.run_steps(2);
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "src/main.rs")
            .is_some()
    );
    assert_eq!(
        harness.state().tabs[0].selected,
        Some(GitPath(b"src/main.rs".to_vec()))
    );
}

#[test]
fn accepted_refresh_preserves_collapsed_folder_and_file_selection() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .build_state(|context, app| app.show(context), viewer());
    harness.get_by_label("Collapse folder src").click();
    harness.run();
    let tab = &mut harness.state_mut().tabs[0];
    let generation = tab.generation;
    let document = std::sync::Arc::clone(&tab.presentation.as_ref().unwrap().document);
    tab.apply_events(
        vec![Event::Status {
            automatic: true,
            generation: tab.generation,
            files: tab.files.clone(),
            model: tab.tree.model_snapshot(),
        }],
        false,
    );
    assert_eq!(tab.generation, generation);
    assert!(!tab.loading);
    tab.apply_events(
        vec![Event::Diff {
            generation: tab.generation,
            document,
        }],
        false,
    );
    harness.run();
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "src/main.rs")
            .is_none()
    );
    assert!(harness.query_by_label("Expand folder src").is_some());
    assert_eq!(
        harness.state().tabs[0].selected,
        Some(GitPath(b"src/main.rs".to_vec()))
    );
    assert_eq!(harness.state().tabs[0].comparison, 1);
}

#[test]
fn forwarded_file_request_reveals_collapsed_parent() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .build_state(|context, app| app.show(context), viewer());
    harness.get_by_label("Collapse folder src").click();
    harness.run();
    let context = harness.ctx.clone();
    let request = harness.state().tabs[0].request.clone();
    harness.state_mut().open(request, &context);
    let tab = &mut harness.state_mut().tabs[0];
    tab.apply_events(
        vec![Event::Status {
            automatic: false,
            generation: tab.generation,
            files: tab.files.clone(),
            model: tab.tree.model_snapshot(),
        }],
        false,
    );
    harness.run_steps(2);
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "src/main.rs")
            .is_some()
    );
}

#[test]
fn repository_tabs_have_tab_semantics_and_named_close_controls() {
    let mut app = viewer();
    let mut second = super::tests::fixture();
    second.root = "/repo/another-project".into();
    app.tabs.push(second);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .build_state(|context, app| app.show(context), app);
    harness.run();
    harness
        .get_by_role_and_label(Role::Tab, "another-project")
        .click();
    harness.run_steps(2);
    assert_eq!(harness.state().selected_tab, 1);
    assert!(harness.state().tabs[1].loading);
    assert!(!harness.state().tabs[0].foreground);
    harness
        .get_by_role_and_label(Role::Button, "Close another-project tab")
        .click();
    harness.run_steps(2);
    assert_eq!(harness.state().tabs.len(), 1);
    assert_eq!(harness.state().selected_tab, 0);
    assert!(harness.state().tabs[0].loading);
}

#[test]
fn closing_an_earlier_tab_keeps_the_selected_repository() {
    let mut app = viewer();
    for name in ["second-project", "third-project"] {
        let mut tab = super::tests::fixture();
        tab.root = format!("/repo/{name}");
        app.tabs.push(tab);
    }
    app.selected_tab = 1;
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .build_state(|context, app| app.show(context), app);
    harness.run();
    let generation = harness.state().tabs[1].generation;
    harness.get_by_label("Close repo tab").click();
    harness.run();
    assert_eq!(
        harness.state().tabs[harness.state().selected_tab].root,
        "/repo/second-project"
    );
    assert_eq!(harness.state().tabs[0].generation, generation);
    assert!(!harness.state().tabs[0].loading);
}

#[test]
fn long_repository_tabs_scroll_instead_of_wrapping_above_tools() {
    let mut app = viewer();
    for index in 0..8 {
        let mut tab = super::tests::fixture();
        tab.root = format!("/repo/long-repository-name-with-many-characters-{index}");
        app.tabs.push(tab);
    }
    let mut harness = Harness::builder()
        .with_size(egui::vec2(420.0, 720.0))
        .build_state(|context, app| app.show(context), app);
    harness.run();
    let search = harness.get_by_label("Search diff").rect();
    assert!(search.bottom() < 140.0, "Tabs must not wrap: {search:?}");
    assert!(
        harness.get_by_label("Close repo tab").rect().right()
            <= harness.get_by_label("UI −").rect().left()
    );
}

#[test]
fn newly_selected_overflow_tab_and_its_close_control_scroll_into_view() {
    let mut app = viewer();
    for index in 0..8 {
        let mut tab = super::tests::fixture();
        tab.root = format!("/repo/long-repository-name-{index}");
        app.tabs.push(tab);
    }
    let mut harness = Harness::builder()
        .with_size(egui::vec2(420.0, 720.0))
        .build_state(|context, app| app.show(context), app);
    harness.run();
    harness.state_mut().selected_tab = 8;
    harness.run_steps(4);
    let strip_end = harness.get_by_label("UI −").rect().left();
    for label in ["long-repository-name-7", "Close long-repository-name-7 tab"] {
        let rect = harness.get_by_label(label).rect();
        assert!(
            rect.left() >= 0.0 && rect.right() <= strip_end,
            "{label}: {rect:?}"
        );
    }
}

#[test]
fn toolbar_breaks_into_two_rows_below_the_threshold_and_keeps_search_first() {
    for (width, two_rows) in [(420.0, true), (640.0, false), (1100.0, false)] {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(width, 720.0))
            .build_state(|context, app| app.show(context), viewer());
        harness.run();
        let search = harness.get_by_label("Search diff").rect();
        let full = harness.get_by_label("Full file").rect();
        let unstaged = harness
            .get_by_role_and_label(Role::Button, "Unstaged")
            .rect();
        assert!(
            search.left() < full.left(),
            "{width}: search must come first"
        );
        assert_eq!(
            unstaged.top() > search.bottom(),
            two_rows,
            "{width}: {search:?} {unstaged:?}"
        );
        if two_rows {
            let all = harness.get_by_role_and_label(Role::Button, "All").rect();
            assert!(all.left() <= search.left() + 16.0, "{width}: {all:?}");
        }
    }
}

#[test]
fn global_controls_sit_beside_the_tabs_not_in_the_toolbar() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .build_state(|context, app| app.show(context), viewer());
    harness.run();
    let tab = harness.get_by_role_and_label(Role::Tab, "repo").rect();
    let search = harness.get_by_label("Search diff").rect();
    assert!(harness.get_by_label("Help").rect().right() >= 1100.0 - 24.0);
    for label in ["Help", "UI −", "100%", "UI +"] {
        let rect = harness.get_by_label(label).rect();
        assert!(
            rect.bottom() <= search.top(),
            "{label}: {rect:?} {search:?}"
        );
        assert!(rect.left() > tab.right(), "{label}: {rect:?}");
    }
    assert!(
        harness
            .query_by_label("Current Git changes, including pre-existing edits")
            .is_none()
    );
    assert!(harness.query_by_label("Layout: Side by side").is_none());
}

#[test]
fn tabs_with_the_same_name_show_their_parent_folder() {
    let mut app = viewer();
    app.tabs[0].root = "/home/work/web".into();
    let mut second = super::tests::fixture();
    second.root = "/home/oss/web".into();
    let mut third = super::tests::fixture();
    third.root = "/home/luis/unique".into();
    app.tabs.extend([second, third]);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .build_state(|context, app| app.show(context), app);
    harness.run();
    assert!(harness.query_by_label("work").is_some());
    assert!(harness.query_by_label("oss").is_some());
    assert!(harness.query_by_label("luis").is_none());
    // The tab keeps its bare name and the full path as hover text.
    assert_eq!(
        harness
            .query_all_by_role_and_label(Role::Tab, "web")
            .count(),
        2
    );
}

#[test]
fn path_row_keeps_copy_and_layout_choice_without_a_selected_file() {
    let mut app = viewer();
    app.tabs[0].selected = None;
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .build_state(|context, app| app.show(context), app);
    harness.run();
    for label in ["Copy", "Side by side", "Unified"] {
        assert!(harness.query_by_label(label).is_some(), "{label}");
    }
    harness.get_by_label("Unified").click();
    harness.run();
    assert!(harness.state().preferences.unified);
}

#[test]
fn tab_names_and_suffixes_follow_either_separator() {
    assert_eq!(tab_name("/home/work/web/"), "web");
    assert_eq!(tab_name("C:\\Users\\luis\\notes.md"), "notes.md");
    let suffixes = tab_suffixes(&["/home/work/web/", "C:\\Users\\work\\web", "/solo"]);
    assert_eq!(suffixes[0].as_deref(), Some("home/work"));
    assert_eq!(suffixes[1].as_deref(), Some("Users/work"));
    assert_eq!(suffixes[2], None);
}

#[test]
fn duplicate_tab_suffix_grows_only_until_the_tabs_differ() {
    let suffixes = tab_suffixes(&["/a/work/web", "/b/work/web", "/b/other/web", "/b/api"]);
    assert_eq!(suffixes[0].as_deref(), Some("a/work"));
    assert_eq!(suffixes[1].as_deref(), Some("b/work"));
    assert_eq!(suffixes[2].as_deref(), Some("other"));
    assert_eq!(suffixes[3], None);
}

#[test]
fn identical_tab_paths_still_get_a_bounded_suffix() {
    let suffixes = tab_suffixes(&["/x/web", "/x/web"]);
    assert_eq!(suffixes[0].as_deref(), Some("x"));
    assert_eq!(suffixes[1].as_deref(), Some("x"));
}

#[test]
fn toolbar_stays_inside_the_window_while_searching() {
    for width in [640.0, 680.0, 720.0] {
        let mut app = viewer();
        app.tabs[0].search = "x".into();
        app.tabs[0].search_hits = vec![0; 99_999];
        let mut harness = Harness::builder()
            .with_size(egui::vec2(width, 720.0))
            .build_state(|context, app| app.show(context), app);
        harness.run();
        for label in ["99999 matches", "Unpushed", "Search diff"] {
            let rect = harness.get_by_label(label).rect();
            assert!(rect.right() <= width, "{width}: {label} {rect:?}");
        }
    }
}

#[test]
fn toolbar_row_threshold_reserves_room_for_the_match_count() {
    assert!(!toolbar_needs_two_rows(TWO_ROW_TOOLBAR_WIDTH, false));
    assert!(toolbar_needs_two_rows(TWO_ROW_TOOLBAR_WIDTH - 1.0, false));
    assert!(toolbar_needs_two_rows(TWO_ROW_TOOLBAR_WIDTH, true));
    assert!(!toolbar_needs_two_rows(
        TWO_ROW_TOOLBAR_WIDTH + MATCH_COUNT_RESERVE,
        true
    ));
}

fn focus_stops(roots: &[&str]) -> usize {
    let mut app = viewer();
    app.tabs[0].root = roots[0].into();
    for root in &roots[1..] {
        let mut tab = super::tests::fixture();
        tab.root = (*root).into();
        app.tabs.push(tab);
    }
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .build_state(|context, app| app.show(context), app);
    harness.run();
    let mut seen = Vec::new();
    for _ in 0..80 {
        harness.key_press(egui::Key::Tab);
        harness.run();
        let focused = harness.ctx.memory(|memory| memory.focused());
        if seen.contains(&focused) {
            break;
        }
        seen.push(focused);
    }
    seen.len()
}

#[test]
fn tab_icons_and_folder_suffixes_are_not_keyboard_stops() {
    let one = focus_stops(&["/home/work/web"]);
    let two = focus_stops(&["/home/work/web", "/home/luis/api"]);
    let duplicate = focus_stops(&["/home/work/web", "/home/oss/web"]);
    assert_eq!(two - one, 2, "a tab adds its name and close control only");
    assert_eq!(duplicate, two, "the folder suffix adds no focus stop");
}
