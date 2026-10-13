use super::*;
use fastdiff_core::CancelToken;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Instant;

fn viewer(tabs: usize) -> Fastdiff {
    Fastdiff {
        tabs: (0..tabs).map(|_| super::tests::fixture()).collect(),
        selected_tab: 0,
        preferences: Preferences::default(),
        server: None,
        benchmark: Benchmark::new(false),
        help_open: false,
        retired_workers: Vec::new(),
    }
}

fn stalled_worker() -> (Worker, CancelToken, Sender<()>, Receiver<()>) {
    let (release, gate) = mpsc::channel();
    let (completed, completion) = mpsc::channel();
    let thread = thread::spawn(move || {
        let _ = gate.recv();
        let _ = completed.send(());
    });
    let (worker, token) = Worker::test_with_thread(thread);
    (worker, token, release, completion)
}

fn while_cleanup_blocked(
    mut app: Fastdiff,
    release: Sender<()>,
    check: impl FnOnce(&mut Fastdiff) + Send + 'static,
) -> Fastdiff {
    let (checked, completion) = mpsc::channel();
    let checker = thread::spawn(move || {
        check(&mut app);
        let _ = checked.send(());
        app
    });
    let returned_before_release = completion.recv_timeout(Duration::from_secs(2)).is_ok();
    let _ = release.send(());
    let app = checker.join().expect("cleanup check panicked");
    assert!(returned_before_release, "UI operation waited for cleanup");
    app
}

#[test]
fn tab_lifecycle_close_returns_before_cleanup_and_remaining_tab_accepts_events() {
    let mut app = viewer(2);
    let (worker, token, release, completion) = stalled_worker();
    app.tabs[0].worker = Some(worker);
    let app = while_cleanup_blocked(app, release, move |app| {
        app.close_tab(0);
        let generation = app.tabs[0].generation;
        app.tabs[0].apply_events(
            vec![Event::Branch {
                generation,
                branch: Some("remaining-tab".into()),
            }],
            false,
        );
        app.collect_finished_workers(&egui::Context::default());
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.selected_tab, 0);
        assert_eq!(app.retired_workers.len(), 1);
        assert_eq!(app.tabs[0].branch.as_deref(), Some("remaining-tab"));
        assert!(token.is_cancelled());
        assert!(completion.try_recv().is_err());
    });
    drop(app);
}

#[test]
fn tab_lifecycle_close_preserves_selection_and_final_tab_requests_window_close() {
    for (selected, closed, expected) in [(2, 0, 1), (1, 1, 1), (0, 2, 0)] {
        let mut app = viewer(3);
        app.selected_tab = selected;
        app.sync_tab_activity();
        app.close_tab(closed);
        assert_eq!(app.selected_tab, expected);
        assert_eq!(app.tabs.len(), 2);
        assert!(app.tabs[expected].foreground);
        assert_eq!(app.tabs.iter().filter(|tab| tab.foreground).count(), 1);
    }
    let mut app = viewer(1);
    app.close_tab(0);
    let output = egui::Context::default().run(egui::RawInput::default(), |context| {
        app.show(context);
    });
    assert!(app.tabs.is_empty());
    assert_eq!(app.selected_tab, 0);
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::Close)
    );
}

#[test]
fn tab_lifecycle_collects_only_finished_handles_once_and_stops_repainting() {
    let mut app = viewer(2);
    let (blocked, _, release, completion) = stalled_worker();
    let (finished, _) = Worker::test_with_thread(thread::spawn(|| {}));
    app.tabs[0].worker = Some(blocked);
    app.tabs[1].worker = Some(finished);
    let context = egui::Context::default();
    let check_context = context.clone();
    let mut app = while_cleanup_blocked(app, release, move |app| {
        app.close_tab(1);
        app.close_tab(0);
        let deadline = Instant::now() + Duration::from_secs(1);
        while app.retired_workers.len() > 1 && Instant::now() < deadline {
            app.collect_finished_workers(&check_context);
            thread::yield_now();
        }
        assert_eq!(app.retired_workers.len(), 1);
        let output = check_context.run(egui::RawInput::default(), |context| {
            app.collect_finished_workers(context);
        });
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT].repaint_delay
                <= Duration::from_millis(50)
        );
    });
    completion.recv_timeout(Duration::from_secs(2)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !app.retired_workers.is_empty() && Instant::now() < deadline {
        app.collect_finished_workers(&context);
        thread::yield_now();
    }
    assert!(app.retired_workers.is_empty());
    let _ = context.run(egui::RawInput::default(), |context| {
        app.collect_finished_workers(context);
    });
    let output = context.run(egui::RawInput::default(), |context| {
        app.collect_finished_workers(context);
    });
    assert_eq!(
        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay,
        Duration::MAX
    );
}

#[test]
fn tab_lifecycle_shutdown_cancels_all_before_waiting_and_drains_retired_handles() {
    let mut app = viewer(3);
    let mut releases = Vec::new();
    let mut tokens = Vec::new();
    let mut completions = Vec::new();
    for tab in &mut app.tabs {
        let (worker, token, release, completion) = stalled_worker();
        tab.worker = Some(worker);
        tokens.push(token);
        releases.push(release);
        completions.push(completion);
    }
    let (done, shutdown) = mpsc::channel();
    let shutdown_thread = thread::spawn(move || {
        app.close_tab(0);
        drop(app);
        done.send(()).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while tokens.iter().any(|token| !token.is_cancelled()) && Instant::now() < deadline {
        thread::yield_now();
    }
    let all_cancelled = tokens.iter().all(CancelToken::is_cancelled);
    let waits_for_cleanup = shutdown.try_recv().is_err();
    for release in releases {
        release.send(()).unwrap();
    }
    shutdown.recv_timeout(Duration::from_secs(2)).unwrap();
    shutdown_thread.join().unwrap();
    for completion in completions {
        completion.recv_timeout(Duration::from_secs(2)).unwrap();
    }
    assert!(all_cancelled);
    assert!(waits_for_cleanup);
}

#[test]
fn tab_lifecycle_worker_panic_does_not_interrupt_cleanup() {
    let mut app = viewer(1);
    let (worker, _) = Worker::test_with_thread(thread::spawn(|| panic!("test worker panic")));
    app.tabs[0].worker = Some(worker);
    app.close_tab(0);
    let context = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !app.retired_workers.is_empty() && Instant::now() < deadline {
        app.collect_finished_workers(&context);
        thread::yield_now();
    }
    assert!(app.retired_workers.is_empty());
}

#[test]
fn recovery_refresh_triggers_retain_live_worker_and_latest_tab_state() {
    for document_mode in [false, true] {
        for trigger in ["F5", "dff", "activation"] {
            let mut app = viewer(2);
            app.tabs[1].request.repository = "/other-repo".into();
            app.sync_tab_activity();
            let tab = &mut app.tabs[0];
            tab.document_mode = document_mode;
            tab.view = View::Staged;
            tab.request.view = tab.view;
            tab.request.target = if document_mode {
                OpenTarget::Document
            } else {
                OpenTarget::Repository
            };
            tab.reflog_open = true;
            tab.error = Some("helper disconnected; press F5".into());
            let (worker, _, release, completion) = stalled_worker();
            tab.worker = Some(worker);
            let app = while_cleanup_blocked(app, release, move |app| {
                if trigger == "activation" {
                    app.selected_tab = 1;
                    app.sync_tab_activity();
                }
                let tab = &app.tabs[0];
                let generation = tab.generation;
                let reflog_generation = tab.reflog_generation;
                let selected = tab.selected.clone();
                let document = Arc::clone(&tab.presentation.as_ref().unwrap().document);
                let context = egui::Context::default();
                match trigger {
                    "F5" => {
                        let input = egui::RawInput {
                            events: vec![egui::Event::Key {
                                key: egui::Key::F5,
                                physical_key: None,
                                pressed: true,
                                repeat: false,
                                modifiers: egui::Modifiers::NONE,
                            }],
                            ..Default::default()
                        };
                        let _ = context.run(input, |context| app.shortcuts(context));
                    }
                    "dff" => app.open(app.tabs[0].request.clone(), &context),
                    _ => {
                        app.selected_tab = 0;
                        app.sync_tab_activity();
                    }
                }
                let tab = &mut app.tabs[0];
                tab.apply_events(
                    vec![Event::Error {
                        generation,
                        message: "obsolete failure".into(),
                    }],
                    false,
                );
                assert_eq!(tab.generation, generation + 1);
                assert_eq!(tab.reflog_generation, reflog_generation + 1);
                assert_eq!(tab.document_mode, document_mode);
                assert_eq!(tab.view, View::Staged);
                assert_eq!(tab.selected, selected);
                assert!(Arc::ptr_eq(
                    &tab.presentation.as_ref().unwrap().document,
                    &document
                ));
                assert_eq!(tab.comparison, 1);
                assert!(tab.error.is_none());
                assert!(tab.loading && tab.reflog_loading);
                assert!(!tab.worker.as_ref().unwrap().is_finished());
                assert!(completion.try_recv().is_err());
                assert!(app.retired_workers.is_empty());
                assert_eq!(app.tabs[1].selected, Some(GitPath(b"src/main.rs".to_vec())));
                assert_eq!(app.tabs[1].view, View::All);
            });
            drop(app);
        }
    }
}
