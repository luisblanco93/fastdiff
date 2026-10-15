use super::*;
use crate::{sidebar::TreeModel, worker::PreparedDocument};
use egui_kittest::{Harness, kittest::Queryable};
use fastdiff_core::{CancelToken, DiffDocument};
use std::sync::Arc;

#[test]
fn metadata_snapshot_adoption_preserves_filtered_navigation_storage() {
    let mut tab = super::tests::fixture();
    tab.document_mode = true;
    tab.filter = "main".into();
    let visible = tab.visible_files.as_ptr();
    let model = tab.tree.model_snapshot();
    let files = Arc::new(vec![FileChange {
        status: " M".into(),
        staged: false,
        ..tab.files[0].clone()
    }]);
    tab.apply_events(
        vec![Event::Status {
            generation: tab.generation,
            automatic: true,
            files: Arc::clone(&files),
            model: Arc::clone(&model),
        }],
        false,
    );
    assert!(Arc::ptr_eq(&tab.files, &files));
    assert!(Arc::ptr_eq(&tab.tree.model_snapshot(), &model));
    assert_eq!(tab.files[0].status, " M");
    assert_eq!(tab.visible_files.as_ptr(), visible);
}

#[test]
fn stale_status_does_not_adopt_files_or_tree_model() {
    let mut tab = super::tests::fixture();
    let files = Arc::clone(&tab.files);
    let model = tab.tree.model_snapshot();
    tab.apply_events(
        vec![Event::Status {
            generation: tab.generation - 1,
            automatic: false,
            files: Arc::new(Vec::new()),
            model: Arc::new(TreeModel::new(&[])),
        }],
        false,
    );
    assert!(Arc::ptr_eq(&tab.files, &files));
    assert!(Arc::ptr_eq(&tab.tree.model_snapshot(), &model));
    assert_eq!(tab.visible_files, vec![0]);
    assert_eq!(tab.generation, 7);
    assert!(tab.presentation.is_some());
    assert!(!tab.loading);
}

#[test]
fn search_results_update_highlights_before_render_and_reject_stale_events() {
    let mut tab = super::tests::fixture();
    tab.search = "main".into();
    tab.apply_events(
        vec![Event::Search {
            generation: tab.generation,
            request_id: tab.search_request_id,
            query: tab.search.clone(),
            hits: vec![0, 1],
        }],
        false,
    );
    assert_eq!(tab.search_hits, vec![0, 1]);
    assert_eq!(tab.presentation.as_ref().unwrap().search_hits, vec![0, 1]);
    let jump = tab.presentation.as_ref().unwrap().jump;
    tab.search_index = 1;
    tab.apply_events(
        vec![
            Event::Search {
                generation: tab.generation - 1,
                request_id: tab.search_request_id,
                query: tab.search.clone(),
                hits: vec![9],
            },
            Event::Search {
                generation: tab.generation,
                request_id: tab.search_request_id,
                query: "stale query".into(),
                hits: vec![8],
            },
        ],
        false,
    );
    assert_eq!(tab.search_hits, vec![0, 1]);
    assert_eq!(tab.presentation.as_ref().unwrap().search_hits, vec![0, 1]);
    assert_eq!(tab.presentation.as_ref().unwrap().jump, jump);
    assert_eq!(tab.search_index, 1);
    tab.apply_events(
        vec![Event::Search {
            generation: tab.generation,
            request_id: tab.search_request_id,
            query: tab.search.clone(),
            hits: Vec::new(),
        }],
        false,
    );
    assert!(tab.search_hits.is_empty());
    assert!(tab.presentation.as_ref().unwrap().search_hits.is_empty());
}

#[test]
fn search_request_ids_reject_results_after_a_b_a_edits_and_reset() {
    let mut tab = super::tests::fixture();
    tab.search = "main".into();
    tab.request_search();
    let first = tab.search_request_id;
    tab.search = "other".into();
    tab.request_search();
    tab.search = "main".into();
    tab.request_search();
    let latest = tab.search_request_id;
    assert!(latest > first);
    tab.apply_events(
        vec![Event::Search {
            generation: tab.generation,
            request_id: first,
            query: tab.search.clone(),
            hits: vec![9],
        }],
        false,
    );
    assert!(tab.search_hits.is_empty());
    tab.apply_events(
        vec![Event::Search {
            generation: tab.generation,
            request_id: latest,
            query: tab.search.clone(),
            hits: vec![0],
        }],
        false,
    );
    assert_eq!(tab.search_hits, vec![0]);
    tab.clear_search_results();
    assert!(tab.search_request_id > latest);
    tab.apply_events(
        vec![Event::Search {
            generation: tab.generation,
            request_id: latest,
            query: tab.search.clone(),
            hits: vec![9],
        }],
        false,
    );
    assert!(tab.search_hits.is_empty());
    assert!(tab.presentation.as_ref().unwrap().search_hits.is_empty());
}

#[test]
fn replacing_document_rejects_search_from_previous_document_with_same_generation() {
    let mut tab = super::tests::fixture();
    tab.search = "main".into();
    tab.request_search();
    let old_request = tab.search_request_id;
    tab.apply_events(
        vec![
            Event::Diff {
                generation: tab.generation,
                document: Arc::new(PreparedDocument::new(
                    DiffDocument::default(),
                    &CancelToken::default(),
                )),
            },
            Event::Search {
                generation: tab.generation,
                request_id: old_request,
                query: tab.search.clone(),
                hits: vec![9],
            },
        ],
        false,
    );
    assert!(tab.search_request_id > old_request);
    assert!(tab.search_hits.is_empty());
}

#[test]
fn replacement_diff_inherits_current_search_highlights_before_render() {
    let mut tab = super::tests::fixture();
    tab.search = "main".into();
    tab.search_hits = vec![0, 1];
    let document = Arc::new(PreparedDocument::new(
        DiffDocument::default(),
        &CancelToken::default(),
    ));
    tab.apply_events(
        vec![Event::Diff {
            generation: tab.generation,
            document,
        }],
        false,
    );
    assert_eq!(tab.presentation.as_ref().unwrap().search_hits, vec![0, 1]);
}

#[test]
fn switching_to_document_clears_old_diff_search_results() {
    let mut tab = super::tests::fixture();
    tab.search = "main".into();
    tab.search_hits = vec![0, 1];
    tab.search_index = 1;
    tab.toggle_document(true);
    assert!(tab.search_hits.is_empty());
    assert_eq!(tab.search_index, 0);
}

#[test]
fn editing_search_clears_highlights_until_new_results_arrive() {
    let mut tab = super::tests::fixture();
    tab.search = "main".into();
    tab.search_hits = vec![0, 1];
    tab.presentation.as_mut().unwrap().search_hits = vec![0, 1];
    let app = Fastdiff {
        tabs: vec![tab],
        selected_tab: 0,
        preferences: Preferences::default(),
        server: None,
        benchmark: Benchmark::new(false),
        help_open: false,
        fallback_fonts: Default::default(),
        retired_workers: Vec::new(),
    };
    let mut harness = Harness::new_state(|context, app| app.show(context), app);
    harness.get_by_label("Search diff").click();
    harness.event(egui::Event::Text("x".into()));
    harness.run();
    let tab = &harness.state().tabs[0];
    assert!(tab.search_hits.is_empty());
    assert!(tab.presentation.as_ref().unwrap().search_hits.is_empty());
    assert_eq!(tab.search_index, 0);
}

#[test]
fn changed_forwarded_file_clears_previous_search_results() {
    let tab = super::tests::fixture();
    let mut request = tab.request.clone();
    request.file = Some(GitPath(b"other.rs".to_vec()));
    let mut app = Fastdiff {
        tabs: vec![tab],
        selected_tab: 0,
        preferences: Preferences::default(),
        server: None,
        benchmark: Benchmark::new(false),
        help_open: false,
        fallback_fonts: Default::default(),
        retired_workers: Vec::new(),
    };
    app.tabs[0].search_hits = vec![0, 1];
    app.tabs[0].search_index = 1;
    app.open(request, &egui::Context::default());
    assert!(app.tabs[0].search_hits.is_empty());
    assert_eq!(app.tabs[0].search_index, 0);
}
