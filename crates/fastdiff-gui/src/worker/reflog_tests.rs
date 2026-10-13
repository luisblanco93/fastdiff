use super::*;
use std::{path::Path, process::Command as ProcessCommand};

fn page(start: usize, snapshot: Option<u64>, count: usize, has_more: bool) -> ReflogList {
    ReflogList {
        branch: None,
        entries: (start..start + count)
            .map(|index| fastdiff_core::ReflogEntry {
                id: "a".repeat(40),
                short_id: "aaaaaaa".into(),
                selector: format!("HEAD@{{{}}}", index * 2),
                action: "commit".into(),
                decorations: String::new(),
            })
            .collect(),
        notice: None,
        snapshot,
        start,
        has_more,
    }
}

#[test]
fn reflog_pages_and_errors_survive_ordinary_event_queue_overflow() {
    for (start, failed) in [(0, false), (15, false), (15, true)] {
        let shared = Shared {
            pending: Mutex::new(Pending::default()),
            events: Mutex::new(VecDeque::new()),
            active: Mutex::new(CancelToken::default()),
            latest_search_id: AtomicU64::new(0),
            foreground: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            initialized: AtomicBool::new(true),
        };
        let context = egui::Context::default();
        let mut state = ReflogState::default();
        state.open(3, true);
        if start != 0 {
            state.accept(&page(0, Some(9), 15, true));
        }
        let result = if failed {
            Err(fastdiff_cli::Error::Backend("unavailable".into()))
        } else {
            Ok(Some(page(start, Some(9), 15, true)))
        };
        assert!(state.publish(&shared, &context, &CancelToken::default(), result));
        for generation in 0..16 {
            emit(
                &shared,
                &context,
                Event::Branch {
                    generation,
                    branch: None,
                },
            );
        }
        let events: Vec<_> = shared.events.lock().unwrap().drain(..).collect();
        assert_eq!(events.len(), 16);
        assert!(
            events.iter().any(|event| match event {
                Event::Reflog {
                    generation: 3,
                    list,
                } => !failed && list.start == start,
                Event::ReflogError { generation: 3, .. } => failed,
                _ => false,
            }),
            "An undrained reflog page or error was evicted"
        );
    }
}

#[test]
fn repeated_reflog_errors_do_not_displace_an_undrained_page() {
    let shared = Shared {
        pending: Mutex::new(Pending::default()),
        events: Mutex::new(VecDeque::new()),
        active: Mutex::new(CancelToken::default()),
        latest_search_id: AtomicU64::new(0),
        foreground: AtomicBool::new(true),
        stopped: AtomicBool::new(false),
        dirty: AtomicBool::new(false),
        initialized: AtomicBool::new(true),
    };
    let context = egui::Context::default();
    let token = CancelToken::default();
    let mut state = ReflogState::default();
    state.open(3, true);
    state.accept(&page(0, Some(9), 15, true));
    assert!(state.publish(
        &shared,
        &context,
        &token,
        Ok(Some(page(15, Some(9), 15, true)))
    ));
    for attempt in 0..32 {
        assert!(state.publish(
            &shared,
            &context,
            &token,
            Err(fastdiff_cli::Error::Backend(format!(
                "unavailable {attempt}"
            )))
        ));
    }
    let events: Vec<_> = shared.events.lock().unwrap().drain(..).collect();
    assert_eq!(events.len(), 2);
    assert!(matches!(&events[0], Event::Reflog { generation: 3, list } if list.start == 15));
    assert!(
        matches!(&events[1], Event::ReflogError { generation: 3, message } if message.contains("unavailable 31"))
    );
}

#[test]
fn reflog_scheduler_rejects_stale_continuations_and_skips_unchanged_snapshots() {
    let mut state = ReflogState::default();
    state.open(3, true);
    assert!(state.request.take().is_some());
    let page = page(0, Some(9), 15, true);
    state.accept(&page);
    assert!(state.periodic_request(Some(9)).is_none());
    assert_eq!(state.periodic_request(Some(10)).unwrap().start, 0);
    let request = fastdiff_core::ReflogRequest {
        start: 15,
        snapshot: Some(9),
        boundary: page.entries.last().cloned(),
    };
    assert!(!state.more(2, request.clone()));
    assert!(!state.more(
        3,
        fastdiff_core::ReflogRequest {
            start: 14,
            ..request.clone()
        }
    ));
    assert!(!state.more(
        3,
        fastdiff_core::ReflogRequest {
            snapshot: Some(10),
            ..request.clone()
        }
    ));
    assert!(!state.more(
        3,
        fastdiff_core::ReflogRequest {
            boundary: None,
            ..request.clone()
        }
    ));
    let mut changed_boundary = request.clone();
    changed_boundary.boundary.as_mut().unwrap().action = "changed".into();
    assert!(!state.more(3, changed_boundary));
    assert!(state.more(3, request.clone()));
    assert!(
        !state.more(3, request.clone()),
        "Queued continuation was replaced"
    );
    state.open(4, true);
    assert!(!state.more(3, request.clone()));
    state.open(5, false);
    assert!(!state.more(5, request));
    assert!(state.generation.is_none());
    assert!(state.request.is_none());
}

#[test]
fn unreliable_reflog_snapshot_reloads_the_loaded_prefix_and_exhaustion_stops_more() {
    let mut state = ReflogState::default();
    state.open(3, true);
    state.accept(&page(0, None, 15, true));
    assert_eq!(state.periodic_request(None).unwrap().start, 0);
    state.accept(&page(15, None, 15, true));
    let prefix = state.periodic_request(None).unwrap();
    assert_eq!(prefix.start, 15);
    assert_eq!(prefix.snapshot, None);
    assert!(prefix.boundary.is_some());
    assert_eq!(state.periodic_request(Some(9)).unwrap().start, 0);
    let exhausted = page(0, None, 30, false);
    state.accept(&exhausted);
    assert!(!state.more(
        3,
        fastdiff_core::ReflogRequest {
            start: 30,
            snapshot: None,
            boundary: exhausted.entries.last().cloned(),
        }
    ));
}

#[test]
fn new_reset_close_or_cancellation_blocks_old_reflog_publication() {
    for scenario in 0..5 {
        let shared = Shared {
            pending: Mutex::new(Pending::default()),
            events: Mutex::new(VecDeque::new()),
            active: Mutex::new(CancelToken::default()),
            latest_search_id: AtomicU64::new(0),
            foreground: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            initialized: AtomicBool::new(true),
        };
        let token = CancelToken::default();
        let mut state = ReflogState::default();
        state.open(3, true);
        match scenario {
            0 => {
                shared.pending.lock().unwrap().reflog = Some(Command::Reflog {
                    generation: 4,
                    enabled: true,
                })
            }
            1 => {
                shared.pending.lock().unwrap().reflog = Some(Command::Reflog {
                    generation: 4,
                    enabled: false,
                })
            }
            2 => token.cancel(),
            3 => shared.foreground.store(false, Ordering::Release),
            _ => shared.stopped.store(true, Ordering::Release),
        }
        assert!(!state.publish(
            &shared,
            &egui::Context::default(),
            &token,
            Ok(Some(page(0, Some(9), 15, true)))
        ));
        assert!(shared.events.lock().unwrap().is_empty());
        assert!(
            state.request.is_some(),
            "Cancelled page request was consumed"
        );
    }
}

#[test]
fn reflog_error_retains_the_prefix_and_allows_continuation_retry() {
    let shared = Shared {
        pending: Mutex::new(Pending::default()),
        events: Mutex::new(VecDeque::new()),
        active: Mutex::new(CancelToken::default()),
        latest_search_id: AtomicU64::new(0),
        foreground: AtomicBool::new(true),
        stopped: AtomicBool::new(false),
        dirty: AtomicBool::new(false),
        initialized: AtomicBool::new(true),
    };
    let mut state = ReflogState::default();
    state.open(3, true);
    let first = page(0, Some(9), 15, true);
    state.accept(&first);
    let request = ReflogRequest {
        start: 15,
        snapshot: first.snapshot,
        boundary: first.entries.last().cloned(),
    };
    assert!(state.more(3, request.clone()));
    assert!(state.publish(
        &shared,
        &egui::Context::default(),
        &CancelToken::default(),
        Err(fastdiff_cli::Error::Backend("unavailable".into())),
    ));
    assert!(
        matches!(shared.events.lock().unwrap().pop_front(), Some(Event::ReflogError { generation: 3, message }) if message.contains("unavailable"))
    );
    assert!(state.more(3, request));
    assert!(state.periodic_request(Some(9)).is_none());
}

fn repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    git(directory.path(), &["init", "-q", "-b", "main"]);
    directory
}

fn git(root: &Path, arguments: &[&str]) {
    let output = ProcessCommand::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn spawn(root: &Path) -> Worker {
    Worker::spawn(
        OpenRequest {
            target: OpenTarget::Repository,
            repository: root.to_string_lossy().into_owned(),
            file: None,
            view: View::All,
            wsl: None,
            theme: false,
        },
        egui::Context::default(),
    )
}

fn wait_reflog(worker: &Worker, generation: u64) -> ReflogList {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        for event in worker.drain_events() {
            match event {
                Event::Reflog {
                    generation: actual,
                    list,
                } if actual == generation => return list,
                Event::ReflogError {
                    generation: actual,
                    message,
                } if actual == generation => {
                    panic!("Reflog failed: {message}");
                }
                _ => {}
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("Worker did not deliver reflog generation {generation}");
}

#[test]
fn unchanged_reflog_poll_does_not_republish() {
    let directory = repository();
    let worker = spawn(directory.path());
    worker.reflog(1, true);
    assert!(wait_reflog(&worker, 1).entries.is_empty());
    thread::sleep(Duration::from_millis(2300));
    assert!(
        !worker
            .drain_events()
            .iter()
            .any(|event| matches!(event, Event::Reflog { .. } | Event::ReflogError { .. })),
        "Unchanged history was loaded again"
    );
}

#[test]
fn worker_loads_repeated_history_in_pages_and_resets_after_history_changes() {
    let directory = repository();
    git(directory.path(), &["config", "user.name", "Test"]);
    git(
        directory.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    git(directory.path(), &["commit", "--allow-empty", "-m", "base"]);
    for _ in 0..35 {
        git(directory.path(), &["reset", "--soft", "HEAD"]);
    }
    let worker = spawn(directory.path());
    worker.reflog(1, true);
    let first = wait_reflog(&worker, 1);
    assert_eq!(first.entries.len(), 15);
    assert!(first.has_more);
    let snapshot = first.snapshot;
    assert!(snapshot.is_some());
    worker.reflog_more(
        1,
        ReflogRequest {
            start: 15,
            snapshot,
            boundary: first.entries.last().cloned(),
        },
    );
    let second = wait_reflog(&worker, 1);
    assert_eq!(second.start, 15);
    assert_eq!(second.entries.len(), 15);
    assert!(second.has_more);
    assert_eq!(second.snapshot, snapshot);
    assert_eq!(second.entries[0].id, first.entries[0].id);
    assert_ne!(second.entries[0].selector, first.entries[0].selector);
    worker.reflog_more(
        1,
        ReflogRequest {
            start: 30,
            snapshot,
            boundary: second.entries.last().cloned(),
        },
    );
    let last = wait_reflog(&worker, 1);
    assert_eq!(last.start, 30);
    assert_eq!(last.entries.len(), 6);
    assert!(!last.has_more);
    git(directory.path(), &["reset", "--soft", "HEAD"]);
    let updated = wait_reflog(&worker, 1);
    assert_eq!(updated.start, 0);
    assert_eq!(updated.entries.len(), 15);
    assert_ne!(updated.snapshot, snapshot);
    worker.reflog(2, false);
}

#[test]
fn continuation_submission_preserves_pending_reset_close_and_other_work() {
    let (wake, _) = mpsc::sync_channel(1);
    let worker = Worker {
        wake,
        shared: Arc::new(Shared {
            pending: Mutex::new(Pending::default()),
            events: Mutex::new(VecDeque::new()),
            active: Mutex::new(CancelToken::default()),
            latest_search_id: AtomicU64::new(0),
            foreground: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            initialized: AtomicBool::new(true),
        }),
        thread: None,
    };
    worker.refresh(8, View::All);
    let token = activate(&worker.shared).unwrap();
    worker.search(8, 1, "needle".into());
    worker.reflog(1, true);
    worker.reflog_more(1, ReflogRequest::default());
    assert!(!token.is_cancelled());
    assert!(matches!(
        worker.shared.pending.lock().unwrap().reflog,
        Some(Command::Reflog {
            generation: 1,
            enabled: true
        })
    ));
    worker.reflog(2, false);
    worker.reflog_more(1, ReflogRequest::default());
    let pending = worker.shared.pending.lock().unwrap();
    assert!(matches!(
        pending.reflog,
        Some(Command::Reflog {
            generation: 2,
            enabled: false
        })
    ));
    assert!(matches!(
        pending.work,
        Some(Command::Refresh { generation: 8, .. })
    ));
    assert!(matches!(
        pending.search,
        Some(Command::Search { generation: 8, .. })
    ));
}
