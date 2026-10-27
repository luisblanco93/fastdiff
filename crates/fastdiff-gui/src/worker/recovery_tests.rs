use super::test_support::{
    ProgressGuard, expect_signal, poll_until, wait_event, wait_events, wait_reflog,
};
use super::*;
use fastdiff_cli::{HelperClient, WslTarget};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

const WAIT: Duration = Duration::from_secs(5);

fn helper(directory: &Path, name: &str) -> std::path::PathBuf {
    document_helper(directory, name, true)
}

fn document_helper(directory: &Path, name: &str, has_repository: bool) -> std::path::PathBuf {
    let script = directory.join(name);
    let log = serde_json::to_string(&script.with_extension("log")).unwrap();
    let stop = serde_json::to_string(&script.with_extension("stop")).unwrap();
    fs::write(
        &script,
        format!(
            r#"#!/usr/bin/python3
import json, os, struct, sys, threading, time
log = {log}
stop = {stop}
def idle_exit():
    while not os.path.exists(stop + '.idle'):
        time.sleep(0.01)
    with open(log, 'a') as output:
        output.write('idle_exit\n')
    os._exit(0)
threading.Thread(target=idle_exit, daemon=True).start()
while True:
    prefix = sys.stdin.buffer.read(4)
    if not prefix:
        break
    request = json.loads(sys.stdin.buffer.read(struct.unpack('<I', prefix)[0]))
    kind = request['request']['type']
    with open(log, 'a') as output:
        output.write(kind + '\n')
        if kind == 'reflog':
            output.write('page:' + str(request['request']['request']['start']) + '\n')
    if os.path.exists(stop) and kind != 'hello':
        if os.path.exists(stop + '.cleanup'):
            os.close(sys.stdout.fileno())
            while sys.stdin.buffer.read(1):
                pass
            with open(log + '.cleanup', 'w') as output:
                output.write(str(os.getpid()))
            time.sleep(30)
        break
    response = {{'type': kind}}
    if kind == 'hello':
        response.update(root='/original/repository', has_repository={has_repository})
    elif kind == 'current_branch':
        response['branch'] = 'main'
    elif kind == 'status':
        response.update(files=[], notice=None)
    elif kind == 'document':
        response['document'] = {{'path': request['request']['path'], 'text': 'fresh text\n'}}
    elif kind == 'document_exists':
        response['available'] = True
    elif kind == 'reflog_snapshot':
        response['snapshot'] = 11
    elif kind == 'reflog':
        response['list'] = {{'branch': 'main', 'entries': [], 'notice': None,
                           'snapshot': 11, 'start': request['request']['request']['start'],
                           'has_more': False}}
    payload = json.dumps({{'version': {version}, 'id': request['id'], 'response': response}}).encode()
    sys.stdout.buffer.write(struct.pack('<I', len(payload)) + payload)
    sys.stdout.buffer.flush()
"#,
            version = fastdiff_cli::PROTOCOL_VERSION,
            has_repository = if has_repository { "True" } else { "False" },
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    script
}

fn request() -> OpenRequest {
    OpenRequest {
        repository: "/original/repository".into(),
        target: OpenTarget::Repository,
        file: None,
        view: View::All,
        wsl: Some(WslTarget {
            distribution: "original-distro".into(),
            user: "original-user".into(),
            helper: "/original/helper".into(),
        }),
        theme: false,
        color_theme: None,
    }
}

fn spawn(
    discover: impl FnMut(&OpenRequest, &CancelToken) -> fastdiff_cli::Result<Backend> + Send + 'static,
) -> Worker {
    spawn_request(request(), egui::Context::default(), discover)
}

fn spawn_request(
    request: OpenRequest,
    context: egui::Context,
    discover: impl FnMut(&OpenRequest, &CancelToken) -> fastdiff_cli::Result<Backend> + Send + 'static,
) -> Worker {
    let (mut worker, _) = Worker::test_with_thread(thread::spawn(|| {}));
    worker.thread.take().unwrap().join().unwrap();
    let (wake, receiver) = mpsc::sync_channel(1);
    let _ = wake.try_send(());
    worker.wake = wake.clone();
    let shared = Arc::clone(&worker.shared);
    worker.thread = Some(thread::spawn(move || {
        run_with_discovery(request, receiver, wake, context, shared, discover);
    }));
    worker
}

fn status(worker: &Worker, generation: u64) -> Event {
    wait_event(
        worker,
        generation,
        |event| matches!(event, Event::Status { generation: actual, .. } if *actual == generation),
    )
}

fn disconnect(worker: &Worker, script: &Path) {
    fs::write(script.with_extension("stop"), "stop").unwrap();
    worker.refresh(1, View::All);
    wait_event(worker, 1, |event| {
        matches!(
            event,
            Event::Error { generation: 1, .. } | Event::ReflogError { .. }
        )
    });
}

fn log(script: &Path) -> String {
    fs::read_to_string(script.with_extension("log")).unwrap_or_default()
}

fn wait_until(expected: &str, mut condition: impl FnMut(&mut ProgressGuard) -> bool) {
    poll_until(expected, WAIT, |guard| condition(guard).then_some(()))
        .unwrap_or_else(|failure| panic!("{failure}"));
}

#[test]
fn explicit_refresh_reconnects_after_a_terminal_helper_failure() {
    let directory = tempfile::tempdir().unwrap();
    let first = helper(directory.path(), "first");
    let second = helper(directory.path(), "second");
    let first_discovery = first.clone();
    let attempts = Arc::new(AtomicU64::new(0));
    let observed = Arc::clone(&attempts);
    let worker = spawn(move |original, token| {
        assert_eq!(original.repository, "/original/repository");
        assert_eq!(
            original.wsl.as_ref().unwrap().distribution,
            "original-distro"
        );
        assert!(!token.is_cancelled());
        let script = if observed.fetch_add(1, Ordering::AcqRel) == 0 {
            &first_discovery
        } else {
            &second
        };
        HelperClient::spawn(script, &original.repository).map(Backend::Wsl)
    });
    status(&worker, 0);
    disconnect(&worker, &first);
    assert!(!worker.is_finished());
    worker.refresh(2, View::All);
    let event = status(&worker, 2);
    assert!(matches!(
        event,
        Event::Status {
            automatic: false,
            ..
        }
    ));
    assert_eq!(attempts.load(Ordering::Acquire), 2);
}

#[test]
fn idle_disconnect_and_failed_reconnection_wait_for_another_explicit_refresh() {
    let directory = tempfile::tempdir().unwrap();
    let first = helper(directory.path(), "first");
    let second = helper(directory.path(), "second");
    let first_discovery = first.clone();
    let attempts = Arc::new(AtomicU64::new(0));
    let observed = Arc::clone(&attempts);
    let worker = spawn(move |original, _| {
        let script = match observed.fetch_add(1, Ordering::AcqRel) {
            0 => &first_discovery,
            1 => {
                return Err(fastdiff_cli::Error::Backend(
                    "Install a matching WSL helper".into(),
                ));
            }
            _ => &second,
        };
        HelperClient::spawn(script, &original.repository).map(Backend::Wsl)
    });
    status(&worker, 0);
    fs::write(first.with_extension("stop.idle"), "exit").unwrap();
    wait_until("helper idle exit", |guard| {
        let log = log(&first);
        guard.advance(format!("helper log length {}", log.len()));
        log.contains("idle_exit")
    });
    thread::sleep(Duration::from_millis(100));
    worker.refresh(1, View::All);
    let event = wait_event(&worker, 1, |event| {
        matches!(event, Event::Error { generation: 1, .. })
    });
    assert!(
        matches!(event, Event::Error { message, .. } if message.contains("matching WSL helper"))
    );
    assert_eq!(attempts.load(Ordering::Acquire), 2);
    thread::sleep(Duration::from_millis(2200));
    assert!(worker.drain_events().is_empty());
    assert_eq!(attempts.load(Ordering::Acquire), 2);
    assert!(!worker.is_finished());
    worker.refresh(2, View::All);
    status(&worker, 2);
    assert_eq!(attempts.load(Ordering::Acquire), 3);
}

#[test]
fn document_reconnection_resets_prepared_state_and_reloads_the_open_reflog() {
    let directory = tempfile::tempdir().unwrap();
    let first = helper(directory.path(), "first");
    let second = helper(directory.path(), "second");
    let first_discovery = first.clone();
    let second_log = second.clone();
    let mut attempt = 0;
    let worker = spawn(move |original, _| {
        attempt += 1;
        HelperClient::spawn(
            if attempt == 1 {
                &first_discovery
            } else {
                &second
            },
            &original.repository,
        )
        .map(Backend::Wsl)
    });
    let Event::Status {
        files: old_files, ..
    } = status(&worker, 0)
    else {
        unreachable!()
    };
    worker.document(1, "/original/repository/readme.md".into(), View::All);
    let Event::Diff {
        document: old_document,
        ..
    } = wait_event(&worker, 1, |event| {
        matches!(event, Event::Diff { generation: 1, .. })
    })
    else {
        unreachable!()
    };
    worker.reflog(9, true);
    wait_reflog(&worker, 9);
    fs::write(first.with_extension("stop"), "stop").unwrap();
    worker.document(2, "/original/repository/readme.md".into(), View::All);
    wait_event(&worker, 2, |event| {
        matches!(
            event,
            Event::Error { generation: 2, .. } | Event::ReflogError { .. }
        )
    });
    worker.document(3, "/original/repository/readme.md".into(), View::All);
    let events = wait_events(&worker, 3, |event| {
        matches!(event, Event::Status { generation: 3, .. })
    });
    let Event::Status { files, .. } = events
        .iter()
        .find(|event| matches!(event, Event::Status { generation: 3, .. }))
        .unwrap()
    else {
        unreachable!()
    };
    assert!(!Arc::ptr_eq(&old_files, files));
    let Event::Diff { document, .. } = events
        .iter()
        .find(|event| matches!(event, Event::Diff { generation: 3, .. }))
        .unwrap()
    else {
        unreachable!()
    };
    assert!(!Arc::ptr_eq(&old_document, document));
    assert!(log(&second_log).contains("page:0\n"));
    assert!(log(&second_log).contains("document\n"));
}

#[test]
fn original_document_target_reconnects_with_and_without_a_repository() {
    for has_repository in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let first = document_helper(directory.path(), "first", has_repository);
        let second = document_helper(directory.path(), "second", has_repository);
        let first_discovery = first.clone();
        let mut attempt = 0;
        let worker = spawn_request(
            OpenRequest {
                target: OpenTarget::Document,
                repository: "/original/readme.md".into(),
                ..request()
            },
            egui::Context::default(),
            move |original, _| {
                assert_eq!(original.target, OpenTarget::Document);
                assert_eq!(original.repository, "/original/readme.md");
                attempt += 1;
                HelperClient::spawn(
                    if attempt == 1 {
                        &first_discovery
                    } else {
                        &second
                    },
                    &original.repository,
                )
                .map(Backend::Wsl)
            },
        );
        wait_event(
            &worker,
            0,
            |event| matches!(event, Event::Diff { generation: 0, document } if document.is_document),
        );
        fs::write(first.with_extension("stop"), "stop").unwrap();
        worker.document(1, "/original/readme.md".into(), View::All);
        wait_event(&worker, 1, |event| {
            matches!(event, Event::Error { generation: 1, .. })
        });
        worker.document(2, "/original/readme.md".into(), View::All);
        wait_event(
            &worker,
            2,
            |event| matches!(event, Event::Diff { generation: 2, document } if document.is_document),
        );
    }
}

#[test]
fn cancellation_after_branch_success_stops_the_remaining_refresh_operations() {
    let directory = tempfile::tempdir().unwrap();
    let script = helper(directory.path(), "helper");
    let discovered = script.clone();
    let context = egui::Context::default();
    let worker = spawn_request(request(), context.clone(), move |original, _| {
        HelperClient::spawn(&discovered, &original.repository).map(Backend::Wsl)
    });
    status(&worker, 0);
    let shared = Arc::clone(&worker.shared);
    let (cancelled, observed) = mpsc::channel();
    let cancelled_once = AtomicBool::new(false);
    context.set_request_repaint_callback(move |_| {
        let branch = shared
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, Event::Branch { generation: 1, .. }));
        if branch && !cancelled_once.swap(true, Ordering::AcqRel) {
            shared.active.lock().unwrap().cancel();
            let _ = cancelled.send(());
        }
    });
    let _ = context.run(Default::default(), |_| {});
    let _ = context.run(Default::default(), |_| {});
    worker.refresh(1, View::All);
    expect_signal(&observed, "refresh cancellation after branch success", WAIT)
        .unwrap_or_else(|failure| panic!("{failure}"));
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        log(&script)
            .lines()
            .filter(|kind| *kind == "status")
            .count(),
        1
    );
    worker.refresh(2, View::All);
    status(&worker, 2);
}

#[test]
fn newer_refresh_cancels_discovery_without_losing_the_latest_request() {
    let directory = tempfile::tempdir().unwrap();
    let first = helper(directory.path(), "first");
    let second = helper(directory.path(), "second");
    let first_discovery = first.clone();
    let attempts = Arc::new(AtomicU64::new(0));
    let observed = Arc::clone(&attempts);
    let (entered, discovery_started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let worker = spawn(
        move |original, token| match observed.fetch_add(1, Ordering::AcqRel) {
            0 => HelperClient::spawn(&first_discovery, &original.repository).map(Backend::Wsl),
            1 => {
                entered.send(token.clone()).unwrap();
                if gate.recv().is_err() {
                    return Err(fastdiff_cli::Error::Backend(
                        "test release gate dropped".into(),
                    ));
                }
                assert!(token.is_cancelled());
                Err(fastdiff_cli::Error::Backend(
                    "stale discovery failure".into(),
                ))
            }
            _ => HelperClient::spawn(&second, &original.repository).map(Backend::Wsl),
        },
    );
    // Release blocked discovery before Worker::drop joins during a failing assertion.
    let release_on_failure = release;
    status(&worker, 0);
    disconnect(&worker, &first);
    worker.refresh(2, View::All);
    let discovery = expect_signal(&discovery_started, "replacement discovery started", WAIT);
    worker.refresh(3, View::Staged);
    let cancelled_before_release = discovery.as_ref().is_ok_and(CancelToken::is_cancelled);
    let released = release_on_failure.send(());
    drop(release_on_failure);
    if let Err(failure) = discovery {
        drop(worker);
        panic!("{failure}");
    }
    let events = wait_events(&worker, 3, |event| {
        matches!(event, Event::Status { generation: 3, .. })
    });
    drop(worker);
    released.unwrap();
    assert!(cancelled_before_release);
    assert_eq!(attempts.load(Ordering::Acquire), 3);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Error { generation: 2, .. }))
    );
}

#[test]
fn refresh_queued_during_terminal_cleanup_is_preserved() {
    let directory = tempfile::tempdir().unwrap();
    let first = helper(directory.path(), "first");
    let second = helper(directory.path(), "second");
    let first_discovery = first.clone();
    let attempts = Arc::new(AtomicU64::new(0));
    let observed = Arc::clone(&attempts);
    let worker = spawn(move |original, _| {
        HelperClient::spawn(
            if observed.fetch_add(1, Ordering::AcqRel) == 0 {
                &first_discovery
            } else {
                &second
            },
            &original.repository,
        )
        .map(Backend::Wsl)
    });
    status(&worker, 0);
    fs::write(first.with_extension("stop.cleanup"), "cleanup").unwrap();
    fs::write(first.with_extension("stop"), "stop").unwrap();
    worker.refresh(1, View::All);
    let cleanup = first.with_extension("log.cleanup");
    wait_until("helper terminal cleanup", |_| cleanup.exists());
    let pid = fs::read_to_string(&cleanup).unwrap();
    assert!(
        std::process::Command::new("kill")
            .args(["-0", &pid])
            .status()
            .unwrap()
            .success()
    );
    let cleanup_token = worker.shared.active.lock().unwrap().clone();
    worker.refresh(2, View::Staged);
    assert!(cleanup_token.is_cancelled());
    let events = wait_events(&worker, 2, |event| {
        matches!(event, Event::Status { generation: 2, .. })
    });
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Error { generation: 1, .. }))
    );
    assert_eq!(attempts.load(Ordering::Acquire), 2);
}
