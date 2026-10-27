use fastdiff_cli::{IpcServer, OpenRequest, forward_at};
use fastdiff_core::View;
use std::{
    sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

fn request() -> OpenRequest {
    OpenRequest {
        target: fastdiff_cli::OpenTarget::Repository,
        repository: "/some/repository".into(),
        file: None,
        view: View::All,
        wsl: None,
        theme: false,
        color_theme: None,
    }
}
fn private_directory() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    directory
}

#[test]
fn acknowledged_launch_is_queued_and_owner_blocks_second_server() {
    let directory = private_directory();
    let server = IpcServer::bind_at(directory.path()).unwrap().unwrap();
    assert!(IpcServer::bind_at(directory.path()).unwrap().is_none());
    assert!(forward_at(directory.path(), &request()).unwrap());
    assert_eq!(server.poll().unwrap()[0].repository, "/some/repository");
    drop(server);
    assert!(!forward_at(directory.path(), &request()).unwrap());
    assert!(IpcServer::bind_at(directory.path()).unwrap().is_some());
}

#[test]
fn launch_race_has_exactly_one_owner() {
    let directory = private_directory();
    let barrier = Arc::new(Barrier::new(2));
    let handles = (0..2)
        .map(|_| {
            let path = directory.path().to_path_buf();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                let server = IpcServer::bind_at(&path).unwrap();
                if server.is_none() {
                    assert!(forward_at(&path, &request()).unwrap());
                }
                barrier.wait();
                (
                    server.is_some(),
                    server
                        .as_ref()
                        .map_or(0, |server| server.poll().unwrap().len()),
                )
            })
        })
        .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|(owned, _)| *owned).count(), 1);
    assert_eq!(results.iter().map(|(_, queued)| queued).sum::<usize>(), 1);
}

#[test]
fn repeated_forwarding_waits_for_acknowledgement_and_preserves_every_request() {
    let directory = private_directory();
    let server = IpcServer::bind_at(directory.path()).unwrap().unwrap();
    for index in 0..10 {
        let next = OpenRequest {
            repository: format!("/repository-{index}"),
            theme: index % 2 == 0,
            ..request()
        };
        assert!(forward_at(directory.path(), &next).unwrap());
        let queued = server.poll().unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].repository, next.repository);
        assert_eq!(queued[0].theme, next.theme);
    }
    drop(server);
    assert!(!forward_at(directory.path(), &request()).unwrap());
}

#[test]
fn registering_wake_callback_preserves_previously_queued_requests() {
    let directory = private_directory();
    let server = IpcServer::bind_at(directory.path()).unwrap().unwrap();
    for index in 0..2 {
        let next = OpenRequest {
            repository: format!("/before-registration-{index}"),
            ..request()
        };
        assert!(forward_at(directory.path(), &next).unwrap());
    }
    let wakes = Arc::new(AtomicUsize::new(0));
    let callback_wakes = wakes.clone();
    server.set_wake_callback(move || {
        callback_wakes.fetch_add(1, Ordering::SeqCst);
    });
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    let queued = server.poll().unwrap();
    assert_eq!(queued.len(), 2);
    assert_eq!(queued[0].repository, "/before-registration-0");
    assert_eq!(queued[1].repository, "/before-registration-1");
}

#[test]
fn replacing_wake_callback_preserves_concurrent_exactly_once_delivery() {
    const CLIENTS: usize = 8;
    let directory = private_directory();
    let server = IpcServer::bind_at(directory.path()).unwrap().unwrap();
    let first_wakes = Arc::new(AtomicUsize::new(0));
    let first_callback_wakes = first_wakes.clone();
    server.set_wake_callback(move || {
        first_callback_wakes.fetch_add(1, Ordering::SeqCst);
    });
    let barrier = Arc::new(Barrier::new(CLIENTS + 1));
    let handles = (0..CLIENTS)
        .map(|index| {
            let path = directory.path().to_path_buf();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let next = OpenRequest {
                    repository: format!("/concurrent-{index}"),
                    ..request()
                };
                barrier.wait();
                assert!(forward_at(&path, &next).unwrap());
            })
        })
        .collect::<Vec<_>>();
    let second_wakes = Arc::new(AtomicUsize::new(0));
    let second_callback_wakes = second_wakes.clone();
    barrier.wait();
    server.set_wake_callback(move || {
        second_callback_wakes.fetch_add(1, Ordering::SeqCst);
    });
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(
        first_wakes.load(Ordering::SeqCst) + second_wakes.load(Ordering::SeqCst),
        CLIENTS + 2
    );
    let mut queued = server
        .poll()
        .unwrap()
        .into_iter()
        .map(|request| request.repository)
        .collect::<Vec<_>>();
    queued.sort();
    assert_eq!(
        queued,
        (0..CLIENTS)
            .map(|index| format!("/concurrent-{index}"))
            .collect::<Vec<_>>()
    );
    let first_count = first_wakes.load(Ordering::SeqCst);
    assert!(forward_at(directory.path(), &request()).unwrap());
    assert_eq!(first_wakes.load(Ordering::SeqCst), first_count);
    assert_eq!(server.poll().unwrap().len(), 1);
}

#[cfg(unix)]
#[test]
fn stale_socket_is_reclaimed_only_after_ownership_lock_is_acquired() {
    let directory = private_directory();
    let listener =
        std::os::unix::net::UnixListener::bind(directory.path().join("gui.sock")).unwrap();
    drop(listener);
    let server = IpcServer::bind_at(directory.path()).unwrap().unwrap();
    assert!(forward_at(directory.path(), &request()).unwrap());
    assert_eq!(server.poll().unwrap().len(), 1);
}

#[cfg(unix)]
#[test]
fn world_readable_runtime_directory_is_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(IpcServer::bind_at(directory.path()).is_err());
}
