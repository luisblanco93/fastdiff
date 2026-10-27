use super::*;
use crate::worker::test_support::expect_signal;
use std::sync::mpsc;

fn private_directory() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    directory
}

fn idle(context: &egui::Context) {
    for _ in 0..3 {
        let _ = context.run(egui::RawInput::default(), |_| {});
    }
    assert!(!context.has_requested_repaint());
}

#[test]
fn incoming_ipc_request_wakes_registered_gui_context() {
    let directory = private_directory();
    let server = IpcServer::bind_at(directory.path()).unwrap().unwrap();
    let context = egui::Context::default();
    let request = tests::fixture().request;
    assert!(fastdiff_cli::forward_at(directory.path(), &request).unwrap());
    let mut app = Fastdiff::new(
        request.clone(),
        server,
        &context,
        Benchmark::new(false),
        SettingsStore::memory(),
    );
    drop(app.tabs[0].worker.take());
    app.poll(&context);
    assert_eq!(app.tabs.len(), 1);
    idle(&context);
    let (sender, notifications) = mpsc::channel();
    context.set_request_repaint_callback(move |info| sender.send(info.delay).unwrap());

    assert!(fastdiff_cli::forward_at(directory.path(), &request).unwrap());
    assert_eq!(
        expect_signal(&notifications, "IPC repaint", Duration::from_secs(2))
            .unwrap_or_else(|failure| panic!("{failure}")),
        Duration::ZERO
    );
    assert!(context.has_requested_repaint());
    app.poll(&context);
    assert_eq!(app.tabs.len(), 1);
}

#[cfg(unix)]
#[test]
fn ipc_error_backlog_requests_repaints_until_accepted_request_is_delivered() {
    use std::os::unix::net::UnixStream;

    let directory = private_directory();
    let server = IpcServer::bind_at(directory.path()).unwrap().unwrap();
    let context = egui::Context::default();
    let request = tests::fixture().request;
    let mut app = Fastdiff::new(
        request.clone(),
        server,
        &context,
        Benchmark::new(false),
        SettingsStore::memory(),
    );
    drop(app.tabs[0].worker.take());
    for _ in 0..2 {
        let mut stream = UnixStream::connect(directory.path().join("gui.sock")).unwrap();
        fastdiff_cli::write_frame(
            &mut stream,
            &serde_json::json!({
                "version": fastdiff_cli::PROTOCOL_VERSION + 1,
                "request": request,
            }),
        )
        .unwrap();
        let _: serde_json::Value = fastdiff_cli::read_frame(&mut stream).unwrap().unwrap();
    }
    let forwarded = OpenRequest {
        theme: true,
        ..request
    };
    assert!(fastdiff_cli::forward_at(directory.path(), &forwarded).unwrap());
    for _ in 0..2 {
        idle(&context);
        app.poll(&context);
        assert!(
            app.tabs[0]
                .error
                .as_ref()
                .unwrap()
                .contains("Could not accept")
        );
        assert!(!app.preferences.theme);
        assert!(context.has_requested_repaint());
    }
    app.poll(&context);
    assert!(app.preferences.theme);
    assert_eq!(app.tabs.len(), 1);
}
