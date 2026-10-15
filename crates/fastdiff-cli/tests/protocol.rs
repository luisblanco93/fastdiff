use fastdiff_cli::{
    HelperClient, HelperRequest, HelperResponse, PROTOCOL_VERSION, RequestEnvelope,
    ResponseEnvelope, read_frame, write_frame,
};
use fastdiff_core::{CancelToken, View};
use std::{
    fs,
    io::Write,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn git(path: &std::path::Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .unwrap()
            .status
            .success()
    );
}
fn repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    git(directory.path(), &["init", "-q"]);
    fs::write(directory.path().join("untracked.rs"), "fn main() {}\n").unwrap();
    directory
}
fn helper(path: &std::path::Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_fastdiff"))
        .arg("--helper")
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}
fn send(child: &mut Child, id: u64, request: HelperRequest) {
    write_frame(
        child.stdin.as_mut().unwrap(),
        &RequestEnvelope {
            version: PROTOCOL_VERSION,
            id,
            request,
        },
    )
    .unwrap();
}
fn response(child: &mut Child) -> ResponseEnvelope {
    read_frame(child.stdout.as_mut().unwrap()).unwrap().unwrap()
}
fn wait_for_exit(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("helper did not exit on EOF/shutdown");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn recording_helper(path: &std::path::Path, commands: &std::path::Path) -> Child {
    use std::os::unix::fs::PermissionsExt;
    let executable = commands.join("git");
    fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$FASTDIFF_GIT_LOG\"\nexec /usr/bin/git \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_fastdiff"))
        .arg("--helper")
        .arg(path)
        .env("PATH", commands)
        .env("FASTDIFF_GIT_LOG", commands.join("commands.log"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    send(&mut child, 1, HelperRequest::Hello);
    assert!(matches!(
        response(&mut child).response,
        HelperResponse::Hello { .. }
    ));
    child
}

#[cfg(unix)]
fn recorded_count(commands: &std::path::Path, suffix: &str) -> usize {
    fs::read_to_string(commands.join("commands.log"))
        .unwrap()
        .lines()
        .filter(|line| line.ends_with(suffix))
        .count()
}

#[cfg(unix)]
fn mixed_repository() -> tempfile::TempDir {
    let directory = repository();
    git(directory.path(), &["config", "user.name", "Diff test"]);
    git(
        directory.path(),
        &["config", "user.email", "diff@example.invalid"],
    );
    fs::write(directory.path().join("untracked.rs"), "base\n").unwrap();
    git(directory.path(), &["add", "-A"]);
    git(directory.path(), &["commit", "-qm", "base"]);
    git(directory.path(), &["branch", "upstream"]);
    git(directory.path(), &["branch", "--set-upstream-to=upstream"]);
    fs::write(directory.path().join("untracked.rs"), "local\n").unwrap();
    git(directory.path(), &["commit", "-qam", "local"]);
    fs::write(directory.path().join("untracked.rs"), "staged\n").unwrap();
    git(directory.path(), &["add", "untracked.rs"]);
    fs::write(directory.path().join("untracked.rs"), "working\n").unwrap();
    directory
}

#[cfg(unix)]
#[test]
fn helper_operation_metadata_is_shared_for_mixed_all() {
    let directory = mixed_repository();
    let commands = tempfile::tempdir().unwrap();
    let mut child = recording_helper(directory.path(), commands.path());
    let filter_query =
        r"config --null --name-only --get-regexp ^filter\..*\.(clean|process|required)$";
    send(&mut child, 2, HelperRequest::Status { view: View::All });
    let HelperResponse::Status { files, notice } = response(&mut child).response else {
        panic!("expected status")
    };
    assert!(notice.is_none());
    assert_eq!(files.len(), 1);
    assert!(files[0].staged && files[0].unstaged && files[0].committed.is_some());
    let status_configs = recorded_count(commands.path(), filter_query);
    send(
        &mut child,
        3,
        HelperRequest::Diff {
            change: files[0].clone(),
            view: View::All,
        },
    );
    let HelperResponse::Diff { document } = response(&mut child).response else {
        panic!("expected diff")
    };
    let counts = (
        status_configs,
        recorded_count(commands.path(), filter_query) - status_configs,
        recorded_count(commands.path(), "cat-file -s :untracked.rs"),
    );
    git(directory.path(), &["branch", "-f", "upstream", "HEAD"]);
    send(
        &mut child,
        4,
        HelperRequest::Status {
            view: View::Unpushed,
        },
    );
    let HelperResponse::Status {
        files: refreshed,
        notice,
    } = response(&mut child).response
    else {
        panic!("expected refreshed upstream")
    };
    assert!(refreshed.is_empty() && notice.is_none());
    git(directory.path(), &["commit", "-qam", "next head"]);
    send(
        &mut child,
        5,
        HelperRequest::Status {
            view: View::Unpushed,
        },
    );
    let HelperResponse::Status {
        files: refreshed,
        notice,
    } = response(&mut child).response
    else {
        panic!("expected refreshed HEAD")
    };
    assert!(notice.is_none());
    assert_eq!(refreshed.len(), 1);
    let original = files[0].committed.as_ref().unwrap();
    let updated = refreshed[0].committed.as_ref().unwrap();
    assert_eq!(updated.base, original.head);
    assert_ne!(updated.head, original.head);
    send(
        &mut child,
        6,
        HelperRequest::Diff {
            change: files[0].clone(),
            view: View::Unpushed,
        },
    );
    let HelperResponse::Diff { document: pinned } = response(&mut child).response else {
        panic!("expected pinned diff")
    };
    assert_eq!(pinned.comparisons[0].patch, document.comparisons[0].patch);
    child.stdin.take();
    assert!(wait_for_exit(&mut child).success());
    assert_eq!(
        document
            .comparisons
            .iter()
            .map(|comparison| comparison.label.as_str())
            .collect::<Vec<_>>(),
        ["Unpushed", "Staged", "Unstaged"]
    );
    for (comparison, before, after) in [
        (&document.comparisons[0], "-base", "+local"),
        (&document.comparisons[1], "-local", "+staged"),
        (&document.comparisons[2], "-staged", "+working"),
    ] {
        assert!(comparison.patch.contains(before));
        assert!(comparison.patch.contains(after));
    }
    assert_eq!(
        counts,
        (1, 0, 1),
        "status config, diff reuses the status filter probe, selected index size"
    );
}

#[cfg(unix)]
#[test]
fn helper_operation_metadata_caches_missing_index_blob() {
    let directory = mixed_repository();
    let commands = tempfile::tempdir().unwrap();
    let mut child = recording_helper(directory.path(), commands.path());
    send(&mut child, 2, HelperRequest::Status { view: View::All });
    let HelperResponse::Status { mut files, .. } = response(&mut child).response else {
        panic!("expected status")
    };
    let change = files.remove(0);
    git(directory.path(), &["rm", "--cached", "-f", "untracked.rs"]);
    send(
        &mut child,
        3,
        HelperRequest::Diff {
            change: change.clone(),
            view: View::All,
        },
    );
    let HelperResponse::Diff { document } = response(&mut child).response else {
        panic!("expected diff")
    };
    let missing_queries = recorded_count(commands.path(), "cat-file -s :untracked.rs");
    assert!(document.comparisons[1].patch.contains("-local"));
    assert!(document.comparisons[2].patch.is_empty());
    git(directory.path(), &["add", "untracked.rs"]);
    send(
        &mut child,
        4,
        HelperRequest::Diff {
            change,
            view: View::All,
        },
    );
    let HelperResponse::Diff { document } = response(&mut child).response else {
        panic!("expected refreshed diff")
    };
    let restored_queries =
        recorded_count(commands.path(), "cat-file -s :untracked.rs") - missing_queries;
    child.stdin.take();
    assert!(wait_for_exit(&mut child).success());
    assert!(document.comparisons[1].patch.contains("+working"));
    assert_eq!((missing_queries, restored_queries), (1, 1));
}

#[cfg(unix)]
#[test]
fn helper_operation_metadata_stays_lazy_for_unavailable_and_summaries() {
    let directory = repository();
    let commands = tempfile::tempdir().unwrap();
    let mut child = recording_helper(directory.path(), commands.path());
    let filter_query =
        r"config --null --name-only --get-regexp ^filter\..*\.(clean|process|required)$";
    send(
        &mut child,
        2,
        HelperRequest::Status {
            view: View::Unpushed,
        },
    );
    let HelperResponse::Status { files, notice } = response(&mut child).response else {
        panic!("expected unborn status")
    };
    assert!(files.is_empty() && notice.unwrap().contains("no commit"));
    git(directory.path(), &["config", "user.name", "Diff test"]);
    git(
        directory.path(),
        &["config", "user.email", "diff@example.invalid"],
    );
    git(directory.path(), &["add", "-A"]);
    git(directory.path(), &["commit", "-qm", "initial"]);
    send(
        &mut child,
        3,
        HelperRequest::Status {
            view: View::Unpushed,
        },
    );
    let HelperResponse::Status { files, notice } = response(&mut child).response else {
        panic!("expected no-upstream status")
    };
    assert!(files.is_empty() && notice.unwrap().contains("no configured upstream"));
    git(directory.path(), &["checkout", "--detach", "-q"]);
    send(
        &mut child,
        4,
        HelperRequest::Status {
            view: View::Unpushed,
        },
    );
    let HelperResponse::Status { files, notice } = response(&mut child).response else {
        panic!("expected detached status")
    };
    assert!(files.is_empty() && notice.unwrap().contains("detached"));
    assert_eq!(recorded_count(commands.path(), filter_query), 0);

    fs::write(directory.path().join("untracked.rs"), "changed\n").unwrap();
    send(
        &mut child,
        5,
        HelperRequest::Status {
            view: View::Unstaged,
        },
    );
    let HelperResponse::Status { mut files, .. } = response(&mut child).response else {
        panic!("expected working status")
    };
    let change = files.remove(0);
    fs::write(
        directory.path().join("untracked.rs"),
        vec![b'x'; 10 * 1024 * 1024 + 1],
    )
    .unwrap();
    send(
        &mut child,
        6,
        HelperRequest::Diff {
            change: change.clone(),
            view: View::All,
        },
    );
    let HelperResponse::Diff { document } = response(&mut child).response else {
        panic!("expected oversized summary")
    };
    assert!(
        document.comparisons[0]
            .summary
            .as_ref()
            .unwrap()
            .contains("10 MiB")
    );
    send(
        &mut child,
        7,
        HelperRequest::Diff {
            change: fastdiff_core::FileChange {
                conflict: true,
                ..change
            },
            view: View::All,
        },
    );
    let HelperResponse::Diff { document } = response(&mut child).response else {
        panic!("expected conflict summary")
    };
    child.stdin.take();
    assert!(wait_for_exit(&mut child).success());
    assert!(
        document.comparisons[0]
            .summary
            .as_ref()
            .unwrap()
            .contains("Merge conflict")
    );
    assert_eq!(recorded_count(commands.path(), filter_query), 1);
    assert_eq!(
        recorded_count(commands.path(), "cat-file -s :untracked.rs"),
        0
    );
}

#[test]
fn real_helper_discovers_status_diff_and_reaps_on_client_drop() {
    let directory = repository();
    let client = HelperClient::spawn(
        std::path::Path::new(env!("CARGO_BIN_EXE_fastdiff")),
        directory.path().to_str().unwrap(),
    )
    .unwrap();
    assert_eq!(
        std::path::Path::new(client.root()),
        directory.path().canonicalize().unwrap()
    );
    let files = client.status(View::All, &CancelToken::default()).unwrap();
    assert_eq!(files.len(), 1);
    let document = client
        .diff(&files[0], View::All, &CancelToken::default())
        .unwrap();
    assert!(!document.comparisons.is_empty());
    let cancelled = CancelToken::default();
    cancelled.cancel();
    assert!(
        client
            .status(View::All, &cancelled)
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    drop(client);
}

#[test]
fn native_backend_handles_discovery_changes_and_invalid_repositories() {
    let directory = repository();
    let request = fastdiff_cli::OpenRequest {
        target: fastdiff_cli::OpenTarget::Repository,
        repository: directory.path().to_str().unwrap().into(),
        file: None,
        view: View::All,
        wsl: None,
        theme: false,
    };
    let backend = fastdiff_cli::Backend::discover(&request).unwrap();
    assert_eq!(
        std::path::Path::new(backend.root()),
        directory.path().canonicalize().unwrap()
    );
    let changes = backend.status(View::All, &CancelToken::default()).unwrap();
    assert!(
        !backend
            .diff(&changes[0], View::All, &CancelToken::default())
            .unwrap()
            .comparisons
            .is_empty()
    );
    let request = fastdiff_cli::OpenRequest {
        target: fastdiff_cli::OpenTarget::Repository,
        repository: directory.path().join("missing").to_str().unwrap().into(),
        ..request
    };
    assert!(fastdiff_cli::Backend::discover(&request).is_err());
    assert!(HelperClient::spawn(&directory.path().join("missing-helper"), "/repo").is_err());
}

#[test]
fn helper_responds_with_ids_and_stops_at_stdin_eof() {
    let directory = repository();
    let mut child = helper(directory.path());
    send(&mut child, 42, HelperRequest::Hello);
    let result = response(&mut child);
    assert_eq!(result.id, 42);
    assert!(matches!(result.response, HelperResponse::Hello { .. }));
    child.stdin.take();
    assert!(wait_for_exit(&mut child).success());
}

#[test]
fn helper_rejects_incompatible_versions_with_installation_guidance() {
    let directory = repository();
    let mut child = helper(directory.path());
    write_frame(
        child.stdin.as_mut().unwrap(),
        &RequestEnvelope {
            version: 99,
            id: 1,
            request: HelperRequest::Hello,
        },
    )
    .unwrap();
    let HelperResponse::Error { code, message } = response(&mut child).response else {
        panic!("expected version error")
    };
    assert_eq!(code, "incompatible_version");
    assert!(message.contains("matching Windows GUI and WSL helper"));
    assert!(wait_for_exit(&mut child).success());
}

#[test]
fn helper_shutdown_and_unknown_cancel_are_acknowledged() {
    let directory = repository();
    let mut child = helper(directory.path());
    send(&mut child, 1, HelperRequest::Cancel { request_id: 999 });
    assert!(matches!(
        response(&mut child).response,
        HelperResponse::Cancelled
    ));
    send(&mut child, 2, HelperRequest::Shutdown);
    assert!(matches!(
        response(&mut child).response,
        HelperResponse::Shutdown
    ));
    assert!(wait_for_exit(&mut child).success());
}

#[test]
fn helper_exits_on_oversized_broken_frame() {
    let directory = repository();
    let mut child = helper(directory.path());
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&u32::MAX.to_le_bytes())
        .unwrap();
    child.stdin.take();
    assert!(!wait_for_exit(&mut child).success());
}

#[test]
fn unavailable_git_is_a_structured_error() {
    let directory = repository();
    let empty_path = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_fastdiff"))
        .arg("--helper")
        .arg(directory.path())
        .env("PATH", empty_path.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    send(&mut child, 1, HelperRequest::Hello);
    assert!(matches!(
        response(&mut child).response,
        HelperResponse::Error { .. }
    ));
    child.stdin.take();
    assert!(wait_for_exit(&mut child).success());
}

#[cfg(unix)]
#[test]
fn eof_cancels_and_reaps_inflight_git_subprocess() {
    use std::os::unix::fs::PermissionsExt;
    let directory = repository();
    let commands = tempfile::tempdir().unwrap();
    let executable = commands.path().join("git");
    fs::write(&executable, "#!/bin/sh\ncase \"$*\" in *rev-parse*) exec /usr/bin/git \"$@\" ;; *) exec /bin/sleep 30 ;; esac\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_fastdiff"))
        .arg("--helper")
        .arg(directory.path())
        .env("PATH", commands.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    send(&mut child, 1, HelperRequest::Hello);
    assert!(matches!(
        response(&mut child).response,
        HelperResponse::Hello { .. }
    ));
    send(&mut child, 2, HelperRequest::Status { view: View::All });
    thread::sleep(Duration::from_millis(100));
    child.stdin.take();
    assert!(wait_for_exit(&mut child).success());
}

#[test]
fn eof_stops_helper_even_if_client_never_drains_large_diff_response() {
    let directory = repository();
    fs::write(
        directory.path().join("untracked.rs"),
        "fn code() {}\n".repeat(15_000),
    )
    .unwrap();
    let repo = fastdiff_core::Repository::discover(directory.path()).unwrap();
    let change = repo
        .status(View::All, &CancelToken::default())
        .unwrap()
        .remove(0);
    let mut child = helper(directory.path());
    send(&mut child, 1, HelperRequest::Hello);
    assert!(matches!(
        response(&mut child).response,
        HelperResponse::Hello { .. }
    ));
    send(
        &mut child,
        2,
        HelperRequest::Diff {
            change,
            view: View::All,
        },
    );
    thread::sleep(Duration::from_millis(500));
    child.stdin.take();
    assert!(wait_for_exit(&mut child).success());
}

#[cfg(unix)]
#[test]
fn cancellation_acknowledges_and_stops_a_running_git_request() {
    use std::os::unix::fs::PermissionsExt;
    let directory = repository();
    let commands = tempfile::tempdir().unwrap();
    let executable = commands.path().join("git");
    fs::write(&executable, "#!/bin/sh\ncase \"$*\" in *rev-parse*) exec /usr/bin/git \"$@\" ;; *) exec /bin/sleep 30 ;; esac\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_fastdiff"))
        .arg("--helper")
        .arg(directory.path())
        .env("PATH", commands.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    send(&mut child, 1, HelperRequest::Hello);
    assert!(matches!(
        response(&mut child).response,
        HelperResponse::Hello { .. }
    ));
    send(&mut child, 2, HelperRequest::Status { view: View::All });
    thread::sleep(Duration::from_millis(100));
    send(&mut child, 3, HelperRequest::Cancel { request_id: 2 });
    let first = response(&mut child);
    let second = response(&mut child);
    assert!([first.id, second.id].contains(&2));
    assert!([first.id, second.id].contains(&3));
    assert!(matches!(first.response, HelperResponse::Cancelled));
    assert!(matches!(second.response, HelperResponse::Cancelled));
    child.stdin.take();
    assert!(wait_for_exit(&mut child).success());
}

#[test]
fn document_backend_opens_standalone_and_ignored_files() {
    use fastdiff_cli::{Backend, OpenRequest, OpenTarget};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("document.txt");
    fs::write(&path, "hello\n").unwrap();
    let request = OpenRequest {
        target: OpenTarget::Document,
        repository: path.to_string_lossy().into_owned(),
        file: None,
        view: View::All,
        wsl: None,
        theme: false,
    };
    let backend = Backend::discover(&request).unwrap();
    assert!(!backend.has_repository());
    assert!(
        backend
            .document_exists(
                &fastdiff_core::GitPath::from_path(&path),
                &CancelToken::default()
            )
            .unwrap()
    );
    assert!(
        !backend
            .document_exists(
                &fastdiff_core::GitPath(b"missing.txt".to_vec()),
                &CancelToken::default()
            )
            .unwrap()
    );
    assert_eq!(
        backend
            .document(&request.repository, &CancelToken::default())
            .unwrap()
            .text,
        "hello\n"
    );
    git(directory.path(), &["init", "-q"]);
    fs::write(directory.path().join(".gitignore"), "document.txt\n").unwrap();
    let backend = Backend::discover(&request).unwrap();
    assert!(backend.has_repository());
    assert_eq!(
        backend
            .document(&request.repository, &CancelToken::default())
            .unwrap()
            .text,
        "hello\n"
    );
}

#[test]
fn document_helper_transports_text_without_git_and_reports_disappearance() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("document.txt");
    fs::write(&path, "full text\n").unwrap();
    let empty_path = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_fastdiff"))
        .args(["--helper", "--document"])
        .arg(&path)
        .env("PATH", empty_path.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    send(&mut child, 1, HelperRequest::Hello);
    assert!(matches!(
        response(&mut child).response,
        HelperResponse::Hello {
            has_repository: false,
            ..
        }
    ));
    send(
        &mut child,
        2,
        HelperRequest::Document {
            path: path.to_string_lossy().into_owned(),
        },
    );
    let HelperResponse::Document { document } = response(&mut child).response else {
        panic!("expected text")
    };
    assert_eq!(document.text, "full text\n");
    send(
        &mut child,
        4,
        HelperRequest::DocumentExists {
            path: fastdiff_core::GitPath::from_path(&path),
        },
    );
    assert!(matches!(
        response(&mut child).response,
        HelperResponse::DocumentExists { available: true }
    ));
    fs::remove_file(&path).unwrap();
    send(
        &mut child,
        3,
        HelperRequest::Document {
            path: path.to_string_lossy().into_owned(),
        },
    );
    assert!(matches!(
        response(&mut child).response,
        HelperResponse::Error { .. }
    ));
    send(
        &mut child,
        5,
        HelperRequest::DocumentExists {
            path: fastdiff_core::GitPath::from_path(&path),
        },
    );
    assert!(matches!(
        response(&mut child).response,
        HelperResponse::DocumentExists { available: false }
    ));
    child.stdin.take();
    assert!(wait_for_exit(&mut child).success());
}

#[test]
fn native_document_backend_does_not_require_git() {
    const MARKER: &str = "FASTDIFF_DOCUMENT_NO_GIT_TEST";
    if std::env::var_os(MARKER).is_none() {
        let empty_path = tempfile::tempdir().unwrap();
        assert!(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "native_document_backend_does_not_require_git"])
                .env(MARKER, "1")
                .env("PATH", empty_path.path())
                .status()
                .unwrap()
                .success()
        );
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("document.txt");
    fs::write(&path, "text without Git").unwrap();
    let request = fastdiff_cli::OpenRequest {
        target: fastdiff_cli::OpenTarget::Document,
        repository: path.to_string_lossy().into_owned(),
        file: None,
        view: View::All,
        wsl: None,
        theme: false,
    };
    let backend = fastdiff_cli::Backend::discover(&request).unwrap();
    assert!(!backend.has_repository());
    assert_eq!(
        backend
            .document(&request.repository, &CancelToken::default())
            .unwrap()
            .text,
        "text without Git"
    );
}

#[test]
fn real_helper_round_trips_branch_and_reflog() {
    let directory = repository();
    git(directory.path(), &["config", "user.name", "Test"]);
    git(
        directory.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    git(directory.path(), &["checkout", "-b", "metadata"]);
    git(
        directory.path(),
        &[
            "commit",
            "--allow-empty",
            "-m",
            "subject λ",
            "-m",
            "excluded body",
        ],
    );
    let client = HelperClient::spawn(
        std::path::Path::new(env!("CARGO_BIN_EXE_fastdiff")),
        directory.path().to_str().unwrap(),
    )
    .unwrap();
    let cancel = CancelToken::default();
    assert_eq!(
        client.current_branch(&cancel).unwrap().as_deref(),
        Some("metadata")
    );
    let list = client.reflog(&cancel).unwrap();
    assert_eq!(list.branch.as_deref(), Some("metadata"));
    assert!(list.notice.is_none());
    assert_eq!(list.entries[0].action, "commit (initial): subject λ");
    assert_eq!(list.entries[0].selector, "HEAD@{0}");
    assert!(list.entries[0].decorations.contains("metadata"));
    assert!(list.entries[0].id.starts_with(&list.entries[0].short_id));
    git(directory.path(), &["branch", "upstream"]);
    git(directory.path(), &["branch", "--set-upstream-to=upstream"]);
    let synchronized = client.reflog(&cancel).unwrap();
    assert_eq!(synchronized.entries.len(), list.entries.len());
    assert_eq!(synchronized.entries[0].id, list.entries[0].id);
    assert_eq!(synchronized.entries[0].action, list.entries[0].action);
    git(directory.path(), &["checkout", "--detach"]);
    let detached = client.reflog(&cancel).unwrap();
    assert!(detached.branch.is_none());
    assert_eq!(detached.entries.len(), list.entries.len() + 1);
    assert_eq!(detached.entries[0].id, list.entries[0].id);
    cancel.cancel();
    assert!(matches!(
        client.reflog(&cancel),
        Err(fastdiff_cli::Error::Cancelled)
    ));
}

#[test]
fn reflog_metadata_round_trips_empty_state() {
    let list: fastdiff_core::ReflogList =
        serde_json::from_str(r#"{"branch":"main","entries":[],"notice":"No reflog entries"}"#)
            .unwrap();
    assert_eq!(list.branch.as_deref(), Some("main"));
    assert!(list.entries.is_empty());
    assert_eq!(
        serde_json::from_str::<fastdiff_core::ReflogList>(&serde_json::to_string(&list).unwrap())
            .unwrap(),
        list
    );
}

#[test]
fn real_helper_round_trips_reflog_pages_and_snapshots() {
    let directory = repository();
    git(directory.path(), &["config", "user.name", "Test"]);
    git(
        directory.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    git(directory.path(), &["commit", "--allow-empty", "-m", "base"]);
    for _ in 0..16 {
        git(directory.path(), &["reset", "--soft", "HEAD"]);
    }
    let client = HelperClient::spawn(
        std::path::Path::new(env!("CARGO_BIN_EXE_fastdiff")),
        directory.path().to_str().unwrap(),
    )
    .unwrap();
    let cancel = CancelToken::default();
    let first = client
        .reflog_page(&fastdiff_core::ReflogRequest::default(), &cancel)
        .unwrap();
    assert_eq!(first.entries.len(), 15);
    assert!(first.has_more && first.snapshot.is_some());
    assert_eq!(client.reflog_snapshot(&cancel).unwrap(), first.snapshot);
    let second = client
        .reflog_page(
            &fastdiff_core::ReflogRequest {
                start: 15,
                snapshot: first.snapshot,
                boundary: first.entries.last().cloned(),
            },
            &cancel,
        )
        .unwrap();
    assert_eq!(second.start, 15);
    assert_eq!(second.entries.len(), 2);
    assert!(!second.has_more);
    git(directory.path(), &["branch", "new-decoration"]);
    assert_ne!(client.reflog_snapshot(&cancel).unwrap(), first.snapshot);
}

#[cfg(unix)]
#[test]
fn helper_reflog_queries_are_bounded_and_unchanged_snapshot_checks_run_no_git() {
    let directory = repository();
    git(directory.path(), &["config", "user.name", "Test"]);
    git(
        directory.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    git(directory.path(), &["commit", "--allow-empty", "-m", "base"]);
    for _ in 0..16 {
        git(directory.path(), &["reset", "--soft", "HEAD"]);
    }
    let commands = tempfile::tempdir().unwrap();
    let mut child = recording_helper(directory.path(), commands.path());
    send(
        &mut child,
        2,
        HelperRequest::Reflog {
            request: fastdiff_core::ReflogRequest::default(),
        },
    );
    let HelperResponse::Reflog { list } = response(&mut child).response else {
        panic!("expected reflog");
    };
    let before = fs::read_to_string(commands.path().join("commands.log")).unwrap();
    assert!(before.contains("--skip=0 --max-count=16 HEAD --"));
    send(&mut child, 3, HelperRequest::ReflogSnapshot);
    assert!(
        matches!(response(&mut child).response, HelperResponse::ReflogSnapshot { snapshot } if snapshot == list.snapshot && snapshot.is_some())
    );
    assert_eq!(
        fs::read_to_string(commands.path().join("commands.log")).unwrap(),
        before
    );
    send(
        &mut child,
        4,
        HelperRequest::Reflog {
            request: fastdiff_core::ReflogRequest {
                start: 15,
                snapshot: list.snapshot,
                boundary: list.entries.last().cloned(),
            },
        },
    );
    assert!(
        matches!(response(&mut child).response, HelperResponse::Reflog { list } if list.start == 15 && list.entries.len() == 2)
    );
    assert!(
        fs::read_to_string(commands.path().join("commands.log"))
            .unwrap()
            .contains("--skip=14 --max-count=17 HEAD --")
    );
    child.stdin.take();
    assert!(wait_for_exit(&mut child).success());
}

#[test]
fn reflog_paging_protocol_serializes_full_boundary_and_rejects_extra_request_fields() {
    let boundary = fastdiff_core::ReflogEntry {
        id: "a".repeat(40),
        short_id: "aaaaaaa".into(),
        decorations: "main".into(),
        selector: "HEAD@{14}".into(),
        action: "reset: moving to HEAD".into(),
    };
    let request = HelperRequest::Reflog {
        request: fastdiff_core::ReflogRequest {
            start: 15,
            snapshot: Some(99),
            boundary: Some(boundary.clone()),
        },
    };
    let json = serde_json::to_string(&request).unwrap();
    assert!(
        matches!(serde_json::from_str::<HelperRequest>(&json).unwrap(), HelperRequest::Reflog { request } if request.start == 15 && request.snapshot == Some(99) && request.boundary == Some(boundary))
    );
    assert!(serde_json::from_str::<HelperRequest>(r#"{"type":"reflog","request":{"start":15,"snapshot":99,"boundary":null,"unexpected":true}}"#).is_err());
    assert!(matches!(
        serde_json::from_str::<HelperRequest>(r#"{"type":"reflog_snapshot"}"#).unwrap(),
        HelperRequest::ReflogSnapshot
    ));
    assert_eq!(PROTOCOL_VERSION, 5);
}
