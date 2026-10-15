use super::test_support::expect_signal;
use super::*;
use fastdiff_core::{Comparison, DiffRow, RowKind};

fn document(rows: &[(&str, &str)]) -> PreparedDocument {
    PreparedDocument::new(
        DiffDocument {
            comparisons: vec![Comparison {
                label: "Search".into(),
                patch: String::new(),
                summary: None,
                rows: rows
                    .iter()
                    .map(|(left, right)| DiffRow {
                        kind: RowKind::Context,
                        old_number: None,
                        new_number: None,
                        left: (*left).into(),
                        right: (*right).into(),
                    })
                    .collect(),
            }],
        },
        &CancelToken::default(),
    )
}

fn worker() -> Worker {
    let (wake, _) = mpsc::sync_channel(1);
    Worker {
        wake,
        shared: Arc::new(Shared {
            pending: Mutex::new(Pending::default()),
            events: Mutex::new(VecDeque::new()),
            active: Mutex::new(CancelToken::default()),
            latest_search_id: std::sync::atomic::AtomicU64::new(0),
            foreground: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            initialized: AtomicBool::new(true),
        }),
        thread: None,
    }
}

#[test]
fn superseded_search_stops_traversal_and_discards_partial_hits() {
    let document = document(&[("", "needle"); 100]);
    let mut checks = 0;
    let hits = search_document(Some(&document), "needle", || {
        checks += 1;
        checks <= 2
    });
    assert!(hits.is_none());
    assert_eq!(checks, 3, "Search continued checking later rows");
}

#[test]
fn superseded_search_stops_between_left_and_right_matching() {
    let document = document(&[("unmatched", "needle")]);
    let mut checks = 0;
    let hits = search_document(Some(&document), "needle", || {
        checks += 1;
        checks == 1
    });
    assert!(hits.is_none());
    assert_eq!(checks, 2);
}

#[test]
fn search_preserves_full_line_unicode_case_matching_and_row_order() {
    let long = format!("{}Needle", "x".repeat(20_000));
    let document = document(&[("İΣ", ""), ("", &long), ("nEeDlE", "Needle")]);
    assert_eq!(
        search_document(Some(&document), "NEEDLE", || true),
        Some(vec![1, 2])
    );
    assert_eq!(
        search_document(Some(&document), "i\u{307}ς", || true),
        Some(vec![0])
    );
    assert_eq!(search_document(Some(&document), "", || true), Some(vec![]));
    assert_eq!(search_document(None, "needle", || true), Some(vec![]));
    assert!(search_document(Some(&document), "needle", || false).is_none());
}

#[test]
fn new_search_replaces_pending_request_without_cancelling_repository_work() {
    let worker = worker();
    let active = activate(&worker.shared).unwrap();
    worker.reflog(4, true);
    worker.search(7, 1, "a".into());
    worker.search(7, 2, "".into());
    assert!(!active.is_cancelled());
    assert!(!search_current(&worker.shared, &active, 1));
    assert!(search_current(&worker.shared, &active, 2));
    let pending = worker.shared.pending.lock().unwrap();
    assert!(matches!(
        pending.reflog,
        Some(Command::Reflog { generation: 4, .. })
    ));
    assert!(matches!(
        &pending.search,
        Some(Command::Search { generation: 7, request_id: 2, query }) if query.is_empty()
    ));
}

#[test]
fn new_search_after_traversal_before_publication_discards_completed_result() {
    let worker = worker();
    worker.search(7, 1, "needle".into());
    let token = activate(&worker.shared).unwrap();
    let shared = Arc::clone(&worker.shared);
    let (ready, finished) = mpsc::channel();
    let (resume, proceed) = mpsc::channel();
    let search = thread::spawn(move || -> Result<(), String> {
        let document = document(&[("", "needle")]);
        let hits = search_document(Some(&document), "needle", || {
            search_current(&shared, &token, 1)
        })
        .ok_or_else(|| "search traversal was superseded".to_owned())?;
        ready
            .send(())
            .map_err(|_| "search traversal receiver disconnected".to_owned())?;
        expect_signal(
            &proceed,
            "search publication release",
            Duration::from_secs(5),
        )?;
        publish_search(
            &shared,
            &egui::Context::default(),
            &token,
            7,
            1,
            "needle".into(),
            hits,
        );
        Ok(())
    });
    let traversal_finished = expect_signal(
        &finished,
        "search traversal finished",
        Duration::from_secs(5),
    );
    worker.search(7, 2, "new".into());
    let resumed = resume.send(());
    drop(resume);
    let joined = search.join();
    traversal_finished.unwrap_or_else(|failure| panic!("{failure}"));
    resumed.unwrap();
    joined
        .unwrap()
        .unwrap_or_else(|failure| panic!("{failure}"));
    assert!(worker.drain_events().is_empty());
}

#[test]
fn cancelled_background_and_stopped_searches_never_publish_partial_results() {
    for state in 0..3 {
        let worker = worker();
        worker.search(7, 1, "needle".into());
        let token = activate(&worker.shared).unwrap();
        match state {
            0 => token.cancel(),
            1 => worker.set_foreground(false),
            _ => worker.shared.stopped.store(true, Ordering::Release),
        }
        publish_search(
            &worker.shared,
            &egui::Context::default(),
            &token,
            7,
            1,
            "needle".into(),
            vec![0],
        );
        assert!(worker.drain_events().is_empty());
    }
}

#[test]
fn current_search_publishes_request_id_and_keeps_the_event_queue_bounded() {
    let worker = worker();
    let token = activate(&worker.shared).unwrap();
    for request_id in 1..=32 {
        worker.search(7, request_id, "needle".into());
        publish_search(
            &worker.shared,
            &egui::Context::default(),
            &token,
            7,
            request_id,
            "needle".into(),
            vec![0],
        );
    }
    let events = worker.drain_events();
    assert_eq!(events.len(), 16);
    for (offset, event) in events.into_iter().enumerate() {
        assert!(matches!(event, Event::Search { request_id, hits, .. }
            if request_id == 17 + offset as u64 && hits == vec![0]));
    }
}
