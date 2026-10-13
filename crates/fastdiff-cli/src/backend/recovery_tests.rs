use super::{tests::process, *};

fn backend(process: ClientProcess) -> Backend {
    Backend::Wsl(HelperClient {
        has_repository: true,
        root: "/repo".into(),
        inner: Mutex::new(process),
    })
}

#[test]
fn backend_reconnect_state_tracks_closed_transport_and_finished_reader() {
    let document = Backend::Document {
        repository: None,
        root: "/repo".into(),
    };
    assert!(!document.needs_reconnect());

    let (process, _sender) = process();
    let backend = backend(process);
    assert!(!backend.needs_reconnect());
    let Backend::Wsl(helper) = &backend else {
        unreachable!();
    };
    helper.inner.lock().unwrap().terminate();
    assert!(backend.needs_reconnect());

    let (mut process, _sender) = super::tests::process();
    let reader = thread::spawn(|| ());
    while !reader.is_finished() {
        thread::yield_now();
    }
    process.reader = Some(reader);
    let backend = self::backend(process);
    assert!(backend.needs_reconnect());
    let Backend::Wsl(helper) = &backend else {
        unreachable!();
    };
    {
        let mut process = helper.inner.lock().unwrap();
        assert!(process.input.is_none());
        assert!(process.child.try_wait().unwrap().is_some());
    }
    assert!(
        helper
            .status(View::All, &CancelToken::default())
            .unwrap_err()
            .to_string()
            .contains("Helper is closed")
    );
    assert!(backend.needs_reconnect());
}

#[test]
fn poisoned_client_detection_reaps_helper_and_rejects_request() {
    let (process, _sender) = process();
    let backend = backend(process);
    let Backend::Wsl(helper) = &backend else {
        unreachable!();
    };
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _process = helper.inner.lock().unwrap();
            panic!("poison helper client");
        }))
        .is_err()
    );
    assert!(backend.needs_reconnect());
    {
        let mut process = match helper.inner.lock() {
            Ok(_) => panic!("helper client lock must remain poisoned"),
            Err(poisoned) => poisoned.into_inner(),
        };
        assert!(process.input.is_none());
        assert!(process.child.try_wait().unwrap().is_some());
    }
    assert!(
        helper
            .status(View::All, &CancelToken::default())
            .unwrap_err()
            .to_string()
            .contains("lock poisoned")
    );
    assert!(backend.needs_reconnect());
}
