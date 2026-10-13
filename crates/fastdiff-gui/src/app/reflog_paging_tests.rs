use super::*;
use fastdiff_core::{ReflogEntry, ReflogRequest};

fn page(start: usize, count: usize, snapshot: Option<u64>, has_more: bool) -> ReflogList {
    ReflogList {
        branch: Some("main".into()),
        notice: None,
        snapshot,
        start,
        has_more,
        entries: (start..start + count)
            .map(|index| ReflogEntry {
                id: format!("{:040x}", index / 2),
                short_id: format!("{:07x}", index / 2),
                decorations: String::new(),
                selector: format!("HEAD@{{{index}}}"),
                action: format!("reset: {index}"),
            })
            .collect(),
    }
}

fn loaded() -> ReviewTab {
    let mut tab = super::tests::fixture();
    tab.reflog_open = true;
    tab.reflog = Some(page(0, 15, Some(42), true));
    tab
}

#[test]
fn reflog_paging_appends_only_requested_matching_page_and_preserves_diff() {
    let mut tab = loaded();
    let document = Arc::clone(&tab.presentation.as_ref().unwrap().document);
    tab.load_more_reflog();
    assert!(tab.reflog_loading);
    assert_eq!(tab.reflog_pending.as_ref().unwrap().start, 15);
    assert!(tab.reflog_pending.as_ref().unwrap().boundary.is_some());
    tab.apply_events(
        vec![Event::Reflog {
            generation: 0,
            list: page(15, 15, Some(42), true),
        }],
        false,
    );
    assert_eq!(tab.reflog.as_ref().unwrap().entries.len(), 30);
    assert!(!tab.reflog_loading);
    assert!(tab.reflog_pending.is_none());
    assert!(Arc::ptr_eq(
        &document,
        &tab.presentation.as_ref().unwrap().document
    ));
    assert!(!tab.reflog_scroll_reset);
}

#[test]
fn reflog_paging_rejects_wrong_snapshot_offset_and_duplicate_replies() {
    let mut tab = loaded();
    tab.load_more_reflog();
    for (start, snapshot) in [(15, Some(41)), (14, Some(42)), (30, Some(42))] {
        tab.apply_events(
            vec![Event::Reflog {
                generation: 0,
                list: page(start, 15, snapshot, false),
            }],
            false,
        );
        assert_eq!(tab.reflog.as_ref().unwrap().entries.len(), 15);
        assert!(tab.reflog_loading);
    }
    tab.apply_events(
        vec![Event::Reflog {
            generation: 0,
            list: page(15, 15, Some(42), false),
        }],
        false,
    );
    tab.apply_events(
        vec![Event::Reflog {
            generation: 0,
            list: page(15, 15, Some(42), false),
        }],
        false,
    );
    assert_eq!(tab.reflog.as_ref().unwrap().entries.len(), 30);
    tab.load_more_reflog();
    assert!(!tab.reflog_loading, "exhaustion stops page work");
}

#[test]
fn reflog_paging_reset_replaces_prefix_and_invalidates_pending_page() {
    let mut tab = loaded();
    tab.load_more_reflog();
    tab.apply_events(
        vec![Event::Reflog {
            generation: 0,
            list: page(0, 15, Some(43), true),
        }],
        false,
    );
    assert_eq!(tab.reflog.as_ref().unwrap().snapshot, Some(43));
    assert!(tab.reflog_scroll_reset);
    assert!(tab.reflog_pending.is_none());
    tab.apply_events(
        vec![Event::Reflog {
            generation: 0,
            list: page(15, 15, Some(42), true),
        }],
        false,
    );
    assert_eq!(tab.reflog.as_ref().unwrap().entries.len(), 15);
}

#[test]
fn reflog_paging_error_retains_rows_and_retry_reuses_boundary() {
    let mut tab = loaded();
    tab.load_more_reflog();
    let requested: ReflogRequest = tab.reflog_pending.clone().unwrap();
    tab.apply_events(
        vec![Event::ReflogError {
            generation: 0,
            message: "Temporary error".into(),
        }],
        false,
    );
    assert_eq!(tab.reflog.as_ref().unwrap().entries.len(), 15);
    assert!(!tab.reflog_loading);
    tab.load_more_reflog();
    assert_eq!(tab.reflog_pending.as_ref().unwrap().start, requested.start);
    assert_eq!(
        tab.reflog_pending.as_ref().unwrap().boundary,
        requested.boundary
    );
    assert!(tab.reflog_loading);
    assert!(tab.reflog_error.is_none());
}

#[test]
fn reflog_paging_paused_closed_and_outstanding_requests_do_not_load_more() {
    let mut tab = loaded();
    tab.foreground = false;
    tab.load_more_reflog();
    assert!(tab.reflog_pending.is_none());
    tab.foreground = true;
    tab.reflog_open = false;
    tab.load_more_reflog();
    assert!(tab.reflog_pending.is_none());
    tab.reflog_open = true;
    tab.load_more_reflog();
    tab.load_more_reflog();
    assert_eq!(tab.reflog_pending.as_ref().unwrap().start, 15);
    tab.refresh_reflog();
    assert!(tab.reflog_pending.is_none());
    assert!(tab.reflog_scroll_reset);
}

#[test]
fn reflog_paging_visible_final_rows_start_automatic_loading() {
    let tab = loaded();
    let mut harness = egui_kittest::Harness::new_state(
        |context, tab| {
            egui::CentralPanel::default().show(context, |ui| tab.show_reflog(ui, 0, false));
        },
        tab,
    );
    harness.run_steps(2);
    assert!(harness.state().reflog_loading);
    assert_eq!(harness.state().reflog_pending.as_ref().unwrap().start, 15);
}

#[test]
fn reflog_paging_unreliable_snapshot_replaces_prefix_without_resetting_scroll() {
    let mut tab = loaded();
    tab.reflog.as_mut().unwrap().snapshot = None;
    tab.load_more_reflog();
    tab.apply_events(
        vec![Event::Reflog {
            generation: 0,
            list: page(0, 30, None, true),
        }],
        false,
    );
    assert_eq!(tab.reflog.as_ref().unwrap().entries.len(), 30);
    assert!(!tab.reflog_scroll_reset);
    assert!(!tab.reflog_loading);
    tab.refresh_reflog();
    assert!(tab.reflog_scroll_reset);
    let generation = tab.reflog_generation;
    tab.apply_events(
        vec![Event::Reflog {
            generation: generation - 1,
            list: page(0, 30, None, false),
        }],
        false,
    );
    assert!(tab.reflog_loading);
    tab.apply_events(
        vec![Event::Reflog {
            generation,
            list: page(0, 15, None, true),
        }],
        false,
    );
    assert_eq!(tab.reflog.as_ref().unwrap().entries.len(), 15);
}
