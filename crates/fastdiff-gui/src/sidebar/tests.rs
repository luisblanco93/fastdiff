use super::*;
use fastdiff_core::{FileChange, GitPath};

fn change(path: &[u8]) -> FileChange {
    FileChange {
        path: GitPath(path.to_vec()),
        old_path: None,
        committed: None,
        staged: false,
        unstaged: true,
        untracked: false,
        conflict: false,
        status: " M".into(),
    }
}

#[test]
fn model_lookup_uses_raw_git_paths_and_distinguishes_folder_names() {
    let files = vec![
        change(b"raw/\xff.rs"),
        change(b"raw/\xfe.rs"),
        change(b"a\\b.rs"),
        change(b"foo"),
        change(b"foo/bar"),
    ];
    let model = TreeModel::new(&files);
    for (index, file) in files.iter().enumerate() {
        assert_eq!(model.file_index(&file.path.0), Some(index));
    }
    assert_eq!(model.file_index(b"raw"), None);
    assert_eq!(model.file_index(b"raw/missing"), None);
    assert_eq!(model.file_index(b"a/b.rs"), None);
}

#[test]
fn reusing_a_model_preserves_cached_rows_and_pending_navigation() {
    let model = Arc::new(TreeModel::new(&[change(b"src/main.rs")]));
    let mut tree = FileTree::default();
    assert!(tree.update(model.clone()));
    tree.reveal(0);
    tree.rows("");
    let rows = tree.cache.as_ptr();
    assert!(!tree.update(model.clone()));
    assert!(Arc::ptr_eq(&tree.model, &model));
    assert_eq!(tree.rows("").as_ptr(), rows);
    assert_eq!(tree.scroll_target(), Some(2));
}

#[test]
fn repeated_reveals_preserve_visible_rows_and_still_scroll() {
    for filter in ["", ".RS"] {
        let mut tree = FileTree::default();
        tree.update(Arc::new(TreeModel::new(&[
            change(b"src/a.rs"),
            change(b"src/b.rs"),
        ])));
        if !filter.is_empty() {
            tree.toggle(b"");
            tree.toggle(b"src");
        }
        let rows = tree.rows(filter).as_ptr();
        let collapsed = tree.collapsed.clone();
        for index in [0, 1, 0] {
            tree.reveal(index);
            assert!(!tree.dirty, "Revealing visible rows should reuse the cache");
            assert_eq!(tree.rows(filter).as_ptr(), rows);
            assert_eq!(tree.scroll_target(), Some(index + 2));
            assert_eq!(tree.collapsed, collapsed);
        }
    }
}

#[test]
fn reveal_expands_every_collapsed_ancestor_and_preserves_other_folders() {
    let mut tree = FileTree::default();
    tree.update(Arc::new(TreeModel::new(&[
        change(b"src/deep/main.rs"),
        change(b"elsewhere/hidden.rs"),
    ])));
    for path in [b"".as_slice(), b"src", b"src/deep", b"elsewhere"] {
        tree.toggle(path);
    }
    assert!(tree.rows("").is_empty());
    tree.reveal(0);
    assert!(tree.dirty);
    assert_eq!(tree.collapsed, HashSet::from([b"elsewhere".to_vec()]));
    assert_eq!(tree.rows("").len(), 3);
    assert_eq!(tree.scroll_target(), Some(3));
}

#[test]
fn reveal_preserves_pending_row_invalidation() {
    let mut tree = FileTree::default();
    tree.update(Arc::new(TreeModel::new(&[
        change(b"src/main.rs"),
        change(b"elsewhere/hidden.rs"),
    ])));
    assert_eq!(tree.rows("").len(), 4);
    tree.toggle(b"elsewhere");
    tree.reveal(0);
    assert!(tree.dirty);
    assert_eq!(tree.rows("").len(), 3);
    assert_eq!(tree.scroll_target(), Some(3));
}

#[test]
fn folders_first_sorting_and_compact_folder_chains() {
    let files = vec![
        change(b"z.rs"),
        change(b"tests/z.rs"),
        change(b"tests/a.rs"),
        change(b"src/deep/main.rs"),
    ];
    let mut tree = FileTree::default();
    tree.update(Arc::new(TreeModel::new(&files)));
    assert_eq!(tree.filtered_files(""), vec![3, 2, 1, 0]);
    assert_eq!(
        tree.rows("")
            .iter()
            .map(|r| r.label.as_str())
            .collect::<Vec<_>>(),
        vec!["src/deep", "main.rs", "tests", "a.rs", "z.rs", "z.rs"]
    );
}

#[test]
fn filtering_reveals_ancestors_without_overwriting_collapsed_state() {
    let files = vec![change(b"src/deep/main.rs"), change(b"readme")];
    let mut tree = FileTree::default();
    tree.update(Arc::new(TreeModel::new(&files)));
    tree.toggle(b"src/deep");
    assert_eq!(tree.rows("").len(), 2);
    assert_eq!(tree.rows("MAIN").len(), 2);
    assert_eq!(tree.filtered_files("MAIN"), vec![0]);
    assert_eq!(tree.rows("").len(), 2);
    tree.reveal(0);
    assert_eq!(tree.rows("").len(), 3);
}

#[test]
fn collapse_and_raw_selection_survive_refresh_and_duplicate_basenames() {
    let mut tree = FileTree::default();
    tree.update(Arc::new(TreeModel::new(&[
        change(b"a/file"),
        change(b"b/file"),
    ])));
    tree.toggle(b"a");
    tree.update(Arc::new(TreeModel::new(&[
        change(b"b/file"),
        change(b"a/file"),
        change(b"a/new"),
    ])));
    assert_eq!(
        tree.rows("")
            .iter()
            .filter_map(|r| r.file)
            .collect::<Vec<_>>(),
        vec![0]
    );
    tree.reveal(1);
    assert_eq!(
        tree.rows("")
            .iter()
            .filter_map(|r| r.file)
            .collect::<Vec<_>>(),
        vec![1, 2, 0]
    );
}

#[test]
fn pending_reveal_follows_raw_path_when_file_indexes_change() {
    let mut tree = FileTree::default();
    tree.update(Arc::new(TreeModel::new(&[change(b"a"), change(b"z")])));
    tree.reveal(0);
    tree.update(Arc::new(TreeModel::new(&[change(b"z"), change(b"a")])));
    tree.rows("");
    assert_eq!(tree.scroll_target(), Some(1));
}

#[test]
fn pending_reveal_discards_removed_or_replaced_files() {
    for replacement in [b"other".as_slice(), b"foo/bar".as_slice()] {
        let mut tree = FileTree::default();
        tree.update(Arc::new(TreeModel::new(&[change(b"foo")])));
        tree.reveal(0);
        tree.update(Arc::new(TreeModel::new(&[change(replacement)])));
        tree.rows("");
        assert_eq!(tree.scroll_target(), None);
    }
}

#[test]
fn refresh_preserves_surviving_focus_and_discards_changed_node_kinds() {
    let mut tree = FileTree::default();
    tree.update(Arc::new(TreeModel::new(&[
        change(b"foo"),
        change(b"keep/file"),
    ])));
    let file = NodeIdentity {
        path: b"foo".to_vec(),
        file: true,
    };
    tree.focused = Some((file.clone(), Id::new("file-focus")));
    tree.focus_target = Some(file);
    tree.update(Arc::new(TreeModel::new(&[
        change(b"foo/child"),
        change(b"keep/file"),
    ])));
    assert!(tree.focused.is_none());
    assert!(tree.focus_target.is_none());

    let folder = NodeIdentity {
        path: b"foo".to_vec(),
        file: false,
    };
    tree.focused = Some((folder.clone(), Id::new("folder-focus")));
    tree.focus_target = Some(folder);
    tree.toggle(b"foo");
    tree.update(Arc::new(TreeModel::new(&[
        change(b"foo"),
        change(b"keep/file"),
    ])));
    assert!(tree.focused.is_none());
    assert!(tree.focus_target.is_none());
    assert!(!tree.collapsed.contains(b"foo".as_slice()));

    let survivor = NodeIdentity {
        path: b"keep/file".to_vec(),
        file: true,
    };
    let focus = (survivor.clone(), Id::new("surviving-focus"));
    tree.focused = Some(focus.clone());
    tree.focus_target = Some(survivor.clone());
    tree.toggle(b"keep");
    tree.update(Arc::new(TreeModel::new(&[
        change(b"keep/file"),
        change(b"new"),
    ])));
    assert_eq!(tree.focused, Some(focus));
    assert_eq!(tree.focus_target, Some(survivor));
    assert!(tree.collapsed.contains(b"keep".as_slice()));
}

#[test]
fn filenames_use_raw_identity_and_only_git_slashes_are_separators() {
    let files = vec![
        change(b"raw/\xff.rs"),
        change(b"raw/\xfe.rs"),
        change(b"a\\b.rs"),
    ];
    let mut tree = FileTree::default();
    tree.update(Arc::new(TreeModel::new(&files)));
    assert_eq!(tree.filtered_files(""), vec![1, 0, 2]);
    assert_eq!(
        tree.rows("")
            .iter()
            .filter_map(|r| r.file)
            .collect::<Vec<_>>(),
        vec![1, 0, 2]
    );
    assert_eq!(tree.rows("").last().unwrap().label, "a\\b.rs");
}

#[test]
fn status_badges_show_staged_conflicted_and_untracked_states() {
    let mut file = change(b"src/new.rs");
    file.staged = true;
    assert_eq!(status_badge(&file), "S+U");
    file.conflict = true;
    assert_eq!(status_badge(&file), "!");
    file.conflict = false;
    file.untracked = true;
    assert_eq!(status_badge(&file), "+");
}

#[test]
fn virtual_tree_only_builds_widgets_for_visible_rows() {
    use egui_kittest::Harness;
    let files = (0..10_000)
        .map(|n| change(format!("src/file-{n:05}.rs").as_bytes()))
        .collect::<Vec<_>>();
    let mut tree = FileTree::default();
    tree.update(Arc::new(TreeModel::new(&files)));
    let mut filter = String::new();
    let harness = Harness::new_ui(|ui| {
        egui_extras::install_image_loaders(ui.ctx());
        tree.show(ui, "repository", &files, &mut filter, None, true);
    });
    drop(harness);
    assert!(
        tree.rendered_rows < 100,
        "{} rows rendered",
        tree.rendered_rows
    );
    assert_eq!(tree.filtered_files("").len(), 10_000);
}

#[test]
fn clicking_forced_open_folder_during_filter_preserves_saved_collapse() {
    use egui_kittest::{Harness, kittest::Queryable};
    let files = vec![change(b"src/main.rs")];
    let mut tree = FileTree::default();
    tree.update(Arc::new(TreeModel::new(&files)));
    tree.toggle(b"src");
    let mut filter = "main".to_string();
    let mut harness = Harness::new_ui(|ui| {
        egui_extras::install_image_loaders(ui.ctx());
        tree.show(ui, "repository", &files, &mut filter, None, true);
    });
    harness.get_by_label("Collapse folder src").click();
    harness.run();
    drop(harness);
    assert!(tree.collapsed.contains(b"src".as_slice()));
    assert_eq!(tree.rows("").len(), 1);
}

#[test]
fn file_directory_replacement_keeps_distinct_keyboard_targets() {
    use egui_kittest::{Harness, kittest::Queryable};
    let files = vec![change(b"foo"), change(b"foo/bar")];
    let mut tree = FileTree::default();
    tree.update(Arc::new(TreeModel::new(&files)));
    let mut filter = String::new();
    let mut choice = None;
    let mut harness = Harness::new_ui(|ui| {
        egui_extras::install_image_loaders(ui.ctx());
        if let Some(index) = tree.show(ui, "repository", &files, &mut filter, None, true) {
            choice = Some(index);
        }
    });
    use egui_kittest::kittest::NodeT;
    assert_ne!(
        harness.get_by_label("foo").accesskit_node().id(),
        harness
            .get_by_label("Collapse folder foo")
            .accesskit_node()
            .id()
    );
    harness.get_by_label("foo").click();
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    drop(harness);
    assert_eq!(
        choice,
        Some(0),
        "Enter must select deleted foo, not toggle its replacement folder"
    );
    assert!(!tree.collapsed.contains(b"foo".as_slice()));
}

#[test]
fn revealing_filtered_selection_preserves_saved_expansion() {
    let mut tree = FileTree::default();
    tree.update(Arc::new(TreeModel::new(&[change(b"src/main.rs")])));
    tree.toggle(b"src");
    tree.rows("main");
    tree.reveal(0);
    assert_eq!(
        tree.rows("").len(),
        1,
        "clearing a filter restores collapse even after file navigation"
    );
}

#[test]
fn keyboard_moves_focus_folds_folders_and_selects_files() {
    use egui_kittest::{Harness, kittest::Queryable};
    let files = vec![change(b"src/a.rs"), change(b"src/b.rs"), change(b"readme")];
    let mut tree = FileTree::default();
    tree.update(Arc::new(TreeModel::new(&files)));
    let mut filter = String::new();
    let mut choice = None;
    let mut harness = Harness::new_ui(|ui| {
        egui_extras::install_image_loaders(ui.ctx());
        if let Some(index) = tree.show(ui, "repository", &files, &mut filter, None, true) {
            choice = Some(index);
        }
    });
    harness.get_by_label("Collapse folder src").click();
    harness.run();
    harness.key_press(egui::Key::ArrowRight);
    harness.run();
    harness.key_press(egui::Key::ArrowDown);
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    drop(harness);
    assert_eq!(choice, Some(0));
    assert!(!tree.collapsed.contains(b"src".as_slice()));
}

#[test]
fn selected_file_has_visible_selection_background() {
    use egui_kittest::Harness;
    let files = vec![change(b"src/main.rs")];
    let mut tree = FileTree::default();
    tree.update(Arc::new(TreeModel::new(&files)));
    let mut filter = String::new();
    let harness = Harness::new_ui(|ui| {
        egui_extras::install_image_loaders(ui.ctx());
        tree.show(
            ui,
            "repository",
            &files,
            &mut filter,
            Some(&files[0].path),
            true,
        );
    });
    let selection_color = harness.ctx.style().visuals.selection.bg_fill;
    assert!(harness.output().shapes.iter().any(|shape| matches!(&shape.shape,
        egui::Shape::Rect(rect) if rect.fill == selection_color && rect.rect.width() > 100.0 && (rect.rect.height() - 24.0).abs() < 0.1)),
        "selected file must have a visible full-width highlight");
}
