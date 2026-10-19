//! A standalone document builds its watcher after the first content, so an edit made in that
//! window must still be shown promptly.
use super::test_support::wait_event;
use super::*;
use std::{fs, process::Command};

/// Shorter than the 2 s poll, so only the re-read after the watcher exists can satisfy it.
const FRESHNESS_BOUND: Duration = Duration::from_millis(1500);
/// Longer than the 2 s poll, so a branch reported on every tick can never look settled.
const SETTLE_QUIET: Duration = Duration::from_millis(2500);
/// How long replayed events may keep the branch being reported before the test calls it polling.
const SETTLE_BOUND: Duration = Duration::from_secs(12);

fn row_is(event: &Event, text: &str) -> bool {
    // A re-read can land between the truncate and the write of `fs::write`, so no row is possible.
    matches!(event, Event::Diff { document, .. }
        if document.is_document
            && document.document.comparisons[0]
                .rows
                .first()
                .is_some_and(|row| row.right == text))
}

#[test]
fn edit_after_the_first_content_in_a_repository_is_shown_before_the_next_poll() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new("git")
        .current_dir(directory.path())
        .args(["init", "-q"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let path = directory.path().join("notes.txt");
    fs::write(&path, "first\n").unwrap();
    let worker = Worker::spawn(
        OpenRequest {
            target: OpenTarget::Document,
            repository: path.to_string_lossy().into_owned(),
            file: None,
            view: View::All,
            wsl: None,
            theme: false,
        },
        egui::Context::default(),
    );
    wait_event(&worker, 0, |event| row_is(event, "first"));
    fs::write(&path, "second\n").unwrap();
    let edited = Instant::now();
    wait_event(&worker, 0, |event| row_is(event, "second"));
    assert!(
        edited.elapsed() < FRESHNESS_BOUND,
        "the edit took {:?} to show",
        edited.elapsed()
    );
}

#[test]
fn an_idle_document_does_not_ask_git_for_the_branch_on_every_poll() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new("git")
        .current_dir(directory.path())
        .args(["init", "-q"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let path = directory.path().join("notes.txt");
    fs::write(&path, "first\n").unwrap();
    let worker = Worker::spawn(
        OpenRequest {
            target: OpenTarget::Document,
            repository: path.to_string_lossy().into_owned(),
            file: None,
            view: View::All,
            wsl: None,
            theme: false,
        },
        egui::Context::default(),
    );
    wait_event(&worker, 0, |event| row_is(event, "first"));
    // The first poll reported the branch. Events raised while the watcher registers (some
    // platforms replay them, FSEvents on a loaded macOS runner more than a second late) are a
    // legitimate reason to poll Git. Settle on quiet instead of a fixed delay: wait until no
    // branch was reported for longer than the 2 s poll, so a replay burst can end but polling on
    // every tick (a report every 2 s) never goes quiet and fails here.
    let settled_from = Instant::now();
    let mut quiet_since = settled_from;
    while quiet_since.elapsed() < SETTLE_QUIET {
        assert!(
            settled_from.elapsed() < SETTLE_BOUND,
            "the branch was still being reported {:?} after the first content",
            settled_from.elapsed()
        );
        if worker
            .drain_events()
            .iter()
            .any(|event| matches!(event, Event::Branch { .. }))
        {
            quiet_since = Instant::now();
        }
        thread::sleep(Duration::from_millis(20));
    }
    let window = Instant::now() + Duration::from_millis(4500);
    let mut later_branches = 0;
    while Instant::now() < window {
        later_branches += worker
            .drain_events()
            .iter()
            .filter(|event| matches!(event, Event::Branch { .. }))
            .count();
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(later_branches, 0);
}
