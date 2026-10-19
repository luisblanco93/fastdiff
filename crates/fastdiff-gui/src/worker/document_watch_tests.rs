//! A standalone document builds its watcher after the first content, so an edit made in that
//! window must still be shown promptly.
use super::test_support::wait_event;
use super::*;
use std::{fs, process::Command};

/// Shorter than the 2 s poll, so only the re-read after the watcher exists can satisfy it.
const FRESHNESS_BOUND: Duration = Duration::from_millis(1500);

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
    // platforms replay them) are a legitimate reason to poll Git, so let them settle first;
    // two more 2 s polls still fall inside the window.
    thread::sleep(Duration::from_millis(700));
    worker.drain_events();
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
