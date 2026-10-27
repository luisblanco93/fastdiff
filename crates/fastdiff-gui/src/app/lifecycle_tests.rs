use super::*;
use crate::worker::test_support::{expect_signal, poll_until};
use fastdiff_core::CancelToken;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

fn viewer(tabs: usize) -> Fastdiff {
    Fastdiff {
        tabs: (0..tabs).map(|_| super::tests::fixture()).collect(),
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

fn while_cleanup_blocked<T: Send + 'static>(
    mut app: Fastdiff,
    release: Sender<()>,
    check: impl FnOnce(&mut Fastdiff) -> T + Send + 'static,
) -> (Fastdiff, T) {
    let (checked, completion) = mpsc::channel();
    let checker = thread::spawn(move || {
        let outcome = check(&mut app);
        let _ = checked.send(());
        (app, outcome)
    });
    let returned_before_release = expect_signal(
        &completion,
        "UI operation while cleanup is blocked",
        Duration::from_secs(2),
    )
    .is_ok();
    let _ = release.send(());
    let result = checker.join().expect("cleanup check panicked");
    assert!(returned_before_release, "UI operation waited for cleanup");
    result
}

#[test]
fn tab_lifecycle_close_returns_before_cleanup_and_remaining_tab_accepts_events() {
    let mut app = viewer(2);
    let (worker, token, release, completion) = stalled_worker();
    app.tabs[0].worker = Some(worker);
    let (app, (cancelled, cleanup_pending)) = while_cleanup_blocked(app, release, move |app| {
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
        (token.is_cancelled(), completion.try_recv().is_err())
    });
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.selected_tab, 0);
    assert_eq!(app.retired_workers.len(), 1);
    assert_eq!(app.tabs[0].branch.as_deref(), Some("remaining-tab"));
    assert!(cancelled);
    assert!(cleanup_pending);
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
    let (mut app, (collected, output)) = while_cleanup_blocked(app, release, move |app| {
        app.close_tab(1);
        app.close_tab(0);
        let collected = poll_until(
            "finished retired worker collected",
            Duration::from_secs(1),
            |guard| {
                app.collect_finished_workers(&check_context);
                guard.advance(format!("{} retired workers", app.retired_workers.len()));
                (app.retired_workers.len() <= 1).then_some(())
            },
        );
        let output = check_context.run(egui::RawInput::default(), |context| {
            app.collect_finished_workers(context);
        });
        (collected, output)
    });
    collected.unwrap_or_else(|failure| panic!("{failure}"));
    assert_eq!(app.retired_workers.len(), 1);
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay <= Duration::from_millis(50)
    );
    expect_signal(
        &completion,
        "released worker completion",
        Duration::from_secs(2),
    )
    .unwrap_or_else(|failure| panic!("{failure}"));
    poll_until(
        "all retired workers collected",
        Duration::from_secs(2),
        |guard| {
            app.collect_finished_workers(&context);
            guard.advance(format!("{} retired workers", app.retired_workers.len()));
            app.retired_workers.is_empty().then_some(())
        },
    )
    .unwrap_or_else(|failure| panic!("{failure}"));
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
    let cancellation = poll_until(
        "all tab workers cancelled",
        Duration::from_secs(2),
        |guard| {
            let cancelled = tokens.iter().filter(|token| token.is_cancelled()).count();
            guard.advance(format!("{cancelled} cancelled workers"));
            (cancelled == tokens.len()).then_some(())
        },
    );
    let all_cancelled = tokens.iter().all(CancelToken::is_cancelled);
    let waits_for_cleanup = shutdown.try_recv().is_err();
    for release in releases {
        let _ = release.send(());
    }
    let shutdown_completed = expect_signal(
        &shutdown,
        "application shutdown completion",
        Duration::from_secs(2),
    );
    let cleanup = if shutdown_completed.is_ok() {
        Some(shutdown_thread.join())
    } else {
        None
    };
    cancellation.unwrap_or_else(|failure| panic!("{failure}"));
    shutdown_completed.unwrap_or_else(|failure| panic!("{failure}"));
    cleanup.unwrap().unwrap();
    for completion in completions {
        expect_signal(
            &completion,
            "shutdown worker completion",
            Duration::from_secs(2),
        )
        .unwrap_or_else(|failure| panic!("{failure}"));
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
    poll_until(
        "panicked retired worker collected",
        Duration::from_secs(2),
        |guard| {
            app.collect_finished_workers(&context);
            guard.advance(format!("{} retired workers", app.retired_workers.len()));
            app.retired_workers.is_empty().then_some(())
        },
    )
    .unwrap_or_else(|failure| panic!("{failure}"));
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
            let (app, (generation, reflog_generation, selected, document, live, cleanup_pending)) =
                while_cleanup_blocked(app, release, move |app| {
                    if trigger == "activation" {
                        app.selected_tab = 1;
                        app.sync_tab_activity();
                    }
                    let tab = &app.tabs[0];
                    let generation = tab.generation;
                    let reflog_generation = tab.reflog_generation;
                    let selected = tab.selected.clone();
                    let document = tab
                        .presentation
                        .as_ref()
                        .map(|presentation| Arc::clone(&presentation.document));
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
                    let live = tab
                        .worker
                        .as_ref()
                        .is_some_and(|worker| !worker.is_finished());
                    (
                        generation,
                        reflog_generation,
                        selected,
                        document,
                        live,
                        completion.try_recv().is_err(),
                    )
                });
            let tab = &app.tabs[0];
            assert_eq!(tab.generation, generation + 1);
            assert_eq!(tab.reflog_generation, reflog_generation + 1);
            assert_eq!(tab.document_mode, document_mode);
            assert_eq!(tab.view, View::Staged);
            assert_eq!(tab.selected, selected);
            assert!(Arc::ptr_eq(
                &tab.presentation.as_ref().unwrap().document,
                &document.unwrap()
            ));
            assert_eq!(tab.comparison, 1);
            assert!(tab.error.is_none());
            assert!(tab.loading && tab.reflog_loading);
            assert!(live);
            assert!(cleanup_pending);
            assert!(app.retired_workers.is_empty());
            assert_eq!(app.tabs[1].selected, Some(GitPath(b"src/main.rs".to_vec())));
            assert_eq!(app.tabs[1].view, View::All);
            drop(app);
        }
    }
}
