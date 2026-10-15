use super::super::test_support::poll_until;
use super::*;
use notify::event::{DataChange, Flag, ModifyKind, RenameMode};
use std::{fs, process::Command, sync::mpsc, time::Duration};

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repository() -> (tempfile::TempDir, Backend) {
    let directory = tempfile::tempdir().unwrap();
    git(directory.path(), &["init", "-q", "-b", "main"]);
    fs::write(directory.path().join(".gitignore"), "output/\n").unwrap();
    fs::create_dir(directory.path().join("output")).unwrap();
    fs::write(directory.path().join("source.rs"), "base\n").unwrap();
    git(directory.path(), &["add", "."]);
    git(
        directory.path(),
        &[
            "-c",
            "user.name=Watcher test",
            "-c",
            "user.email=watcher@example.invalid",
            "commit",
            "-qm",
            "base",
        ],
    );
    let repository = Repository::discover(directory.path()).unwrap();
    let root = repository.root().to_str().unwrap().to_owned();
    (directory, Backend::Local { repository, root })
}

fn coverage(backend: &Backend, document: Option<&str>) -> (WatchCoverage, mpsc::Receiver<()>) {
    let (wake, receiver) = mpsc::sync_channel(1);
    let mut coverage = WatchCoverage::new(backend, document, wake, &CancelToken::default());
    assert!(coverage.take_diagnostics().is_empty());
    (coverage, receiver)
}

fn inject(coverage: &WatchCoverage, event: notify::Result<notify::Event>) {
    collect_event(&coverage.batch, &coverage.received, &coverage.wake, event);
}

fn event(path: PathBuf) -> notify::Event {
    notify::Event::new(notify::EventKind::Modify(ModifyKind::Data(
        DataChange::Content,
    )))
    .add_path(path)
}

fn notification(coverage: &mut WatchCoverage, receiver: &mpsc::Receiver<()>) -> WatchChange {
    poll_until("native notification", Duration::from_secs(3), |guard| {
        let _ = receiver.recv_timeout(Duration::from_millis(50));
        if coverage.take_received() {
            guard.advance("notification received");
            std::thread::sleep(Duration::from_millis(30));
            coverage.take_received();
            let change = coverage.take_pending(&CancelToken::default());
            if change.relevant {
                return Some(change);
            }
        }
        None
    })
    .unwrap_or_else(|failure| panic!("{failure}"))
}

#[test]
fn ignored_batches_skip_refresh_but_tracked_mixed_and_documents_do_not() {
    let (directory, backend) = repository();
    let ignored = directory.path().join("output/generated.txt");
    fs::write(&ignored, "generated").unwrap();
    let (mut coverage, _receiver) = coverage(&backend, None);
    inject(&coverage, Ok(event(ignored.clone())));
    assert!(coverage.take_received());
    assert!(
        !coverage.take_received(),
        "debounce starts only for fresh callbacks"
    );
    assert!(!coverage.take_pending(&CancelToken::default()).relevant);
    inject(&coverage, Ok(event(ignored.clone())));
    inject(&coverage, Ok(event(directory.path().join("source.rs"))));
    assert!(coverage.take_pending(&CancelToken::default()).relevant);
    coverage.update_document(ignored.to_str(), &CancelToken::default());
    inject(&coverage, Ok(event(ignored.clone())));
    assert!(coverage.take_pending(&CancelToken::default()).relevant);
    coverage.update_document(None, &CancelToken::default());
    git(directory.path(), &["add", "-f", "output/generated.txt"]);
    inject(&coverage, Ok(event(ignored)));
    assert!(coverage.take_pending(&CancelToken::default()).relevant);
}

#[test]
fn rename_endpoints_rescans_and_ambiguous_events_are_conservative() {
    let (directory, backend) = repository();
    let (mut coverage, _receiver) = coverage(&backend, None);
    let rename = notify::Event::new(notify::EventKind::Modify(ModifyKind::Name(
        RenameMode::Both,
    )))
    .add_path(directory.path().join("output/old"))
    .add_path(directory.path().join("source.rs"));
    inject(&coverage, Ok(rename));
    assert_eq!(coverage.batch.lock().unwrap().paths.len(), 2);
    assert!(coverage.take_pending(&CancelToken::default()).relevant);
    let rescan = notify::Event::new(notify::EventKind::Access(notify::event::AccessKind::Any))
        .set_flag(Flag::Rescan);
    inject(&coverage, Ok(rescan));
    let change = coverage.take_pending(&CancelToken::default());
    assert!(change.relevant && change.rebuild);
    inject(&coverage, Ok(notify::Event::new(notify::EventKind::Any)));
    assert!(coverage.take_pending(&CancelToken::default()).relevant);
}

#[test]
fn bounded_batch_deduplicates_paths_and_overflow_forces_refresh() {
    let mut batch = PendingPaths::default();
    for _ in 0..256 {
        batch.record(event(PathBuf::from("output/same")));
    }
    assert_eq!(batch.paths.len(), 1);
    assert!(!batch.conservative);
    for index in 0..129 {
        batch.record(event(PathBuf::from(format!("output/{index}"))));
    }
    assert!(batch.paths.len() <= MAX_EVENT_PATHS);
    assert!(batch.conservative);
    let mut batch = PendingPaths::default();
    batch.record(event(PathBuf::from("x".repeat(MAX_EVENT_BYTES + 1))));
    assert!(batch.paths.is_empty());
    assert!(batch.conservative);
}

#[test]
fn ignore_sources_remain_relevant_inside_ignored_directories() {
    let (directory, backend) = repository();
    let (mut coverage, _receiver) = coverage(&backend, None);
    inject(
        &coverage,
        Ok(event(directory.path().join("output/.gitignore"))),
    );
    assert!(coverage.take_pending(&CancelToken::default()).relevant);
}

#[test]
fn watcher_errors_report_diagnostics_and_request_coverage_rebuild() {
    let (_directory, backend) = repository();
    let (mut coverage, _receiver) = coverage(&backend, None);
    inject(
        &coverage,
        Err(notify::Error::generic("injected watcher failure")),
    );
    let change = coverage.take_pending(&CancelToken::default());
    assert!(change.relevant && change.rebuild);
    assert!(
        change
            .diagnostics
            .iter()
            .any(|message| message.contains("injected watcher failure"))
    );
    assert!(coverage.rebuild(&CancelToken::default()).is_empty());
}

#[test]
fn document_renames_do_not_rebuild_for_parent_metadata_events() {
    let directory = tempfile::tempdir().unwrap();
    let document = directory.path().join("notes.txt");
    fs::write(&document, "before").unwrap();
    let backend = Backend::Document {
        repository: None,
        root: directory.path().to_str().unwrap().into(),
    };
    let (mut coverage, _receiver) = coverage(&backend, document.to_str());
    inject(&coverage, Ok(event(directory.path().to_owned())));
    inject(
        &coverage,
        Ok(
            notify::Event::new(notify::EventKind::Modify(ModifyKind::Name(RenameMode::To)))
                .add_path(document),
        ),
    );
    let change = coverage.take_pending(&CancelToken::default());
    assert!(change.relevant);
    assert!(!change.rebuild);
}

#[test]
fn document_atomic_replacement_is_seen_without_periodic_polling() {
    let directory = tempfile::tempdir().unwrap();
    let document = directory.path().join("notes.txt");
    fs::write(&document, "before").unwrap();
    let backend = Backend::Document {
        repository: None,
        root: directory.path().to_str().unwrap().into(),
    };
    let (mut coverage, receiver) = coverage(&backend, document.to_str());
    let temporary = directory.path().join("save.tmp");
    fs::write(&temporary, "after").unwrap();
    fs::rename(temporary, &document).unwrap();
    let change = notification(&mut coverage, &receiver);
    assert!(change.relevant);
    assert!(
        !change.rebuild,
        "atomic document saves retain parent coverage"
    );
}

#[test]
fn missing_document_parents_are_rebuilt_after_recreation() {
    let directory = tempfile::tempdir().unwrap();
    let document = directory.path().join("missing/nested/notes.txt");
    let backend = Backend::Document {
        repository: None,
        root: directory.path().to_str().unwrap().into(),
    };
    let (mut coverage, receiver) = coverage(&backend, document.to_str());
    fs::create_dir_all(document.parent().unwrap()).unwrap();
    fs::write(&document, "first").unwrap();
    assert!(notification(&mut coverage, &receiver).rebuild);
    assert!(coverage.rebuild(&CancelToken::default()).is_empty());
    fs::write(&document, "second").unwrap();
    assert!(notification(&mut coverage, &receiver).relevant);
    fs::remove_dir_all(directory.path().join("missing")).unwrap();
    assert!(notification(&mut coverage, &receiver).rebuild);
    assert!(coverage.rebuild(&CancelToken::default()).is_empty());
    fs::create_dir_all(document.parent().unwrap()).unwrap();
    fs::write(&document, "third").unwrap();
    assert!(notification(&mut coverage, &receiver).relevant);
}

#[test]
fn repository_root_stays_watched_for_document_tabs() {
    let (directory, backend) = repository();
    let document = directory.path().join("output/notes.txt");
    fs::write(&document, "document").unwrap();
    let (mut coverage, receiver) = coverage(&backend, document.to_str());
    fs::write(directory.path().join("source.rs"), "changed").unwrap();
    assert!(notification(&mut coverage, &receiver).relevant);
}

#[test]
fn separate_git_metadata_changes_are_not_masked_by_polling() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("worktree");
    let metadata = directory.path().join("metadata");
    fs::create_dir(&root).unwrap();
    let status = Command::new("git")
        .current_dir(&root)
        .args(["init", "-q", "--separate-git-dir"])
        .arg(&metadata)
        .status()
        .unwrap();
    assert!(status.success());
    let repository = Repository::discover(&root).unwrap();
    let backend = Backend::Local {
        root: root.to_str().unwrap().into(),
        repository,
    };
    let (mut coverage, receiver) = coverage(&backend, None);
    fs::write(
        metadata.join("config"),
        "[core]\nrepositoryformatversion = 0\n",
    )
    .unwrap();
    assert!(notification(&mut coverage, &receiver).relevant);
}

#[test]
fn linked_worktree_watches_external_common_and_private_metadata() {
    let (directory, backend) = repository();
    let worktree_dir = tempfile::tempdir().unwrap();
    let worktree = worktree_dir.path().join("linked");
    let status = Command::new("git")
        .current_dir(directory.path())
        .args(["worktree", "add", "-q", "-b", "linked"])
        .arg(&worktree)
        .status()
        .unwrap();
    assert!(status.success());
    let repository = Repository::discover(&worktree).unwrap();
    let (metadata, _) = repository.watch_paths(&CancelToken::default()).unwrap();
    let private_head = metadata
        .iter()
        .find(|path| {
            // Match by component: Git and the temp directory can spell the prefix differently
            // (Windows 8.3 short names), so a tempdir-based prefix would not compare equal.
            path.ends_with("HEAD")
                && path
                    .components()
                    .any(|component| component.as_os_str() == "worktrees")
        })
        .unwrap()
        .clone();
    let linked = Backend::Local {
        root: worktree.to_str().unwrap().into(),
        repository,
    };
    let (mut coverage, receiver) = coverage(&linked, None);
    fs::write(
        directory.path().join(".git/refs/heads/new-reference"),
        "0123456789012345678901234567890123456789\n",
    )
    .unwrap();
    assert!(notification(&mut coverage, &receiver).relevant);
    fs::write(&private_head, "ref: refs/heads/linked\n").unwrap();
    assert!(notification(&mut coverage, &receiver).relevant);
    drop(backend);
}

#[test]
fn absent_external_configuration_parents_are_covered_and_rebuilt() {
    let (directory, backend) = repository();
    let outside = tempfile::tempdir().unwrap();
    let include = outside.path().join("missing/settings.ini");
    git(
        directory.path(),
        &["config", "include.path", include.to_str().unwrap()],
    );
    let (mut coverage, receiver) = coverage(&backend, None);
    fs::create_dir(include.parent().unwrap()).unwrap();
    fs::write(&include, "[user]\nname = Watcher\n").unwrap();
    assert!(notification(&mut coverage, &receiver).rebuild);
    assert!(coverage.rebuild(&CancelToken::default()).is_empty());
    fs::write(&include, "[user]\nname = Changed\n").unwrap();
    assert!(notification(&mut coverage, &receiver).rebuild);
}

#[test]
fn deferred_build_cancelled_midway_is_discarded_for_a_retry() {
    let (_directory, backend) = repository();
    let (wake, _receiver) = mpsc::sync_channel(1);
    let cancelled = CancelToken::default();
    cancelled.cancel();
    assert!(
        WatchCoverage::new_unless_cancelled(&backend, None, wake.clone(), &cancelled).is_none()
    );
    let live = CancelToken::default();
    assert!(WatchCoverage::new_unless_cancelled(&backend, None, wake, &live).is_some());
}

// Temporary macOS diagnostic (LIT-30): prints the raw notify events for the atomic-save
// scenario so the CI log shows what FSEvents reports. Remove once the cause is fixed.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "diagnostic"]
fn diag_macos_atomic_save_events() {
    use notify::{RecursiveMode, Watcher};
    let directory = tempfile::tempdir().unwrap();
    let document = directory.path().join("notes.txt");
    fs::write(&document, "before").unwrap();
    let (sender, receiver) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = sender.send(event);
    })
    .unwrap();
    watcher
        .watch(directory.path(), RecursiveMode::NonRecursive)
        .unwrap();
    let temporary = directory.path().join("save.tmp");
    fs::write(&temporary, "after").unwrap();
    fs::rename(temporary, &document).unwrap();
    println!("DIAG tempdir {:?}", directory.path());
    println!("DIAG canonical {:?}", directory.path().canonicalize());
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match receiver.recv_timeout(left) {
            Ok(Ok(event)) => println!(
                "DIAG event {:?} paths={:?} rescan={} flag={:?}",
                event.kind,
                event.paths,
                event.need_rescan(),
                event.flag()
            ),
            Ok(Err(error)) => println!("DIAG error {error}"),
            Err(_) => break,
        }
    }
}
