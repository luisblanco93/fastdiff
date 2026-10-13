use super::*;

const WAIT: Duration = Duration::from_secs(5);

fn blocked_worker() -> (Worker, CancelToken, mpsc::Sender<()>, Receiver<()>) {
    let (wake, receiver) = mpsc::sync_channel(1);
    let (started, cleanup_started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let worker_thread = thread::spawn(move || {
        if receiver.recv().is_ok() {
            let _ = started.send(());
            let _ = gate.recv();
        }
    });
    let (mut worker, token) = Worker::test_with_thread(worker_thread);
    worker.wake = wake;
    (worker, token, release, cleanup_started)
}

#[test]
fn retirement_returns_while_worker_cleanup_is_blocked() {
    let (worker, token, release, cleanup_started) = blocked_worker();
    let wake = worker.wake.clone();
    let (returned, retirement_returned) = mpsc::channel();
    let retirement = thread::spawn(move || {
        let handle = worker.retire();
        let _ = returned.send(());
        handle
    });
    let cleanup_blocked = cleanup_started.recv_timeout(WAIT).is_ok();
    let returned_before_release = retirement_returned.recv_timeout(WAIT).is_ok();
    let cancelled_before_release = token.is_cancelled();
    let _ = wake.try_send(());
    let _ = release.send(());
    let handle = retirement.join().expect("retirement thread panicked");
    let owned_handle = handle.is_some();
    if let Some(handle) = handle {
        handle.join().expect("worker cleanup panicked");
    }
    assert!(cleanup_blocked);
    assert!(returned_before_release);
    assert!(cancelled_before_release);
    assert!(owned_handle);
}

#[test]
fn retirement_prevents_startup_activation() {
    let (worker, token) = Worker::test_with_thread(thread::spawn(|| {}));
    let shared = Arc::clone(&worker.shared);
    let handle = worker.retire();
    if let Some(handle) = handle {
        handle.join().expect("worker thread panicked");
    }
    assert!(token.is_cancelled());
    assert!(activate(&shared).is_none());
}

#[test]
fn retirement_cancels_discovery_and_active_jobs() {
    for initialized in [false, true] {
        let (worker, _) = Worker::test_with_thread(thread::spawn(|| {}));
        let token = activate(&worker.shared).expect("job should activate");
        worker
            .shared
            .initialized
            .store(initialized, Ordering::Release);
        let handle = worker.retire();
        if let Some(handle) = handle {
            handle.join().expect("worker thread panicked");
        }
        assert!(token.is_cancelled(), "initialized: {initialized}");
    }
}

#[test]
fn retirement_wakes_inactive_worker_waiting_for_a_request() {
    let (worker, _, release, cleanup_started) = blocked_worker();
    worker.shared.foreground.store(false, Ordering::Release);
    worker.shared.initialized.store(true, Ordering::Release);
    let wake = worker.wake.clone();
    let retirement = thread::spawn(move || worker.retire());
    let woke = cleanup_started.recv_timeout(WAIT).is_ok();
    let _ = wake.try_send(());
    let _ = release.send(());
    let handle = retirement.join().expect("retirement thread panicked");
    if let Some(handle) = handle {
        handle.join().expect("worker cleanup panicked");
    }
    assert!(woke);
}

#[test]
fn retirement_without_a_thread_still_cancels_and_stops() {
    let (mut worker, token) = Worker::test_with_thread(thread::spawn(|| {}));
    worker.thread.take().unwrap().join().unwrap();
    let shared = Arc::clone(&worker.shared);
    assert!(worker.retire().is_none());
    assert!(token.is_cancelled());
    assert!(shared.stopped.load(Ordering::Acquire));
}

#[test]
fn drop_waits_until_worker_cleanup_is_released() {
    let (worker, token, release, cleanup_started) = blocked_worker();
    let wake = worker.wake.clone();
    let (finished, dropped) = mpsc::channel();
    let drop_thread = thread::spawn(move || {
        drop(worker);
        let _ = finished.send(());
    });
    let cleanup_blocked = cleanup_started.recv_timeout(WAIT).is_ok();
    let waited_for_cleanup = dropped.recv_timeout(Duration::from_millis(50)).is_err();
    let cancelled_before_release = token.is_cancelled();
    let _ = wake.try_send(());
    let _ = release.send(());
    drop_thread.join().expect("worker drop panicked");
    let dropped_after_release = dropped.recv_timeout(WAIT).is_ok();
    assert!(cleanup_blocked);
    assert!(waited_for_cleanup);
    assert!(cancelled_before_release);
    assert!(dropped_after_release);
}

#[test]
fn drop_joins_a_panicked_worker_without_panicking() {
    let (worker, _) = Worker::test_with_thread(thread::spawn(|| panic!("worker failure")));
    drop(worker);
}
