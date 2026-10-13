use crate::{CancelToken, Repository};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

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

fn repository() -> (tempfile::TempDir, Repository) {
    let directory = tempfile::tempdir().unwrap();
    git(directory.path(), &["init", "-q"]);
    fs::write(directory.path().join(".gitignore"), "output/\n").unwrap();
    fs::create_dir(directory.path().join("output")).unwrap();
    let repository = Repository::discover(directory.path()).unwrap();
    (directory, repository)
}

/// Resolves the longest existing ancestor so equal locations compare equal when Git and the
/// temp directory spell them differently, such as Windows 8.3 short names (`RUNNER~1`).
fn resolved(path: &Path) -> PathBuf {
    let mut suffix = Vec::new();
    let mut current = path;
    loop {
        if let Ok(canonical) = fs::canonicalize(current) {
            return suffix
                .iter()
                .rev()
                .fold(canonical, |base, part| base.join(part));
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                suffix.push(name.to_owned());
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

fn contains_location(paths: &[PathBuf], expected: &Path) -> bool {
    let expected = resolved(expected);
    paths.iter().any(|path| resolved(path) == expected)
}

#[test]
fn ignored_batches_keep_tracked_files_and_mixed_paths_relevant() {
    let (directory, repository) = repository();
    let cancel = CancelToken::default();
    let ignored = directory.path().join("output/generated.txt");
    fs::write(&ignored, "generated").unwrap();
    assert!(
        repository
            .paths_are_ignored(std::slice::from_ref(&ignored), &cancel)
            .unwrap()
    );
    let source = directory.path().join("source.rs");
    assert!(
        !repository
            .paths_are_ignored(&[ignored.clone(), source], &cancel)
            .unwrap()
    );
    git(directory.path(), &["add", "-f", "output/generated.txt"]);
    assert!(!repository.paths_are_ignored(&[ignored], &cancel).unwrap());
}

#[test]
fn ignored_directories_keep_force_added_descendants_relevant_after_removal() {
    let (directory, repository) = repository();
    let ignored_directory = directory.path().join("output");
    fs::write(ignored_directory.join("tracked.txt"), "tracked").unwrap();
    git(directory.path(), &["add", "-f", "output/tracked.txt"]);
    assert!(
        !repository
            .paths_are_ignored(
                std::slice::from_ref(&ignored_directory),
                &CancelToken::default()
            )
            .unwrap()
    );
    let mut directory_with_separator = ignored_directory.clone();
    directory_with_separator.push("");
    assert!(
        !repository
            .paths_are_ignored(
                std::slice::from_ref(&directory_with_separator),
                &CancelToken::default()
            )
            .unwrap()
    );
    fs::remove_dir_all(&ignored_directory).unwrap();
    assert!(
        !repository
            .paths_are_ignored(&[ignored_directory], &CancelToken::default())
            .unwrap()
    );
    assert!(
        !repository
            .paths_are_ignored(&[directory_with_separator], &CancelToken::default())
            .unwrap()
    );
}

#[test]
fn ignored_names_with_special_characters_count_once() {
    let (directory, repository) = repository();
    let paths = [
        "output/--flag",
        #[cfg(unix)]
        "output/line\nbreak",
        #[cfg(unix)]
        "output/quote\"file",
    ]
    .map(|name| directory.path().join(name));
    for path in &paths {
        fs::write(path, "generated").unwrap();
    }
    assert!(
        repository
            .paths_are_ignored(&paths, &CancelToken::default())
            .unwrap()
    );
}

#[test]
fn watcher_git_queries_honor_cancellation() {
    let (_directory, repository) = repository();
    let cancel = CancelToken::default();
    cancel.cancel();
    assert!(matches!(
        repository.watch_paths(&cancel),
        Err(crate::Error::Cancelled)
    ));
    assert!(matches!(
        repository.paths_are_ignored(&[], &cancel),
        Err(crate::Error::Cancelled)
    ));
}

#[test]
fn watch_paths_follow_a_separate_git_directory() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("worktree");
    let metadata = directory.path().join("metadata");
    fs::create_dir(&root).unwrap();
    let output = Command::new("git")
        .current_dir(&root)
        .args(["init", "-q", "--separate-git-dir"])
        .arg(&metadata)
        .output()
        .unwrap();
    assert!(output.status.success());
    let repository = Repository::discover(&root).unwrap();
    let (paths, _) = repository.watch_paths(&CancelToken::default()).unwrap();
    for expected in [
        metadata.clone(),
        metadata.join("HEAD"),
        metadata.join("index"),
        metadata.join("info/exclude"),
        repository.root().join(".git"),
    ] {
        assert!(
            contains_location(&paths, &expected),
            "{expected:?} missing from {paths:?}"
        );
    }
}

#[test]
fn watch_paths_cover_missing_external_includes_and_ignore_files() {
    let (directory, repository) = repository();
    let outside = tempfile::tempdir().unwrap();
    let include = outside.path().join("missing/settings.ini");
    let excludes = outside.path().join("missing/ignored.txt");
    git(
        directory.path(),
        &["config", "include.path", include.to_str().unwrap()],
    );
    git(
        directory.path(),
        &["config", "core.excludesFile", excludes.to_str().unwrap()],
    );
    let (paths, configurations) = repository.watch_paths(&CancelToken::default()).unwrap();
    assert!(paths.contains(&include));
    assert!(paths.contains(&excludes));
    assert!(configurations.contains(&include));
}

#[test]
fn batched_paths_split_into_exactly_the_expected_lines() {
    use super::split_batched_paths;
    assert_eq!(
        split_batched_paths(b"/a\n/b\n", 2),
        Some(vec![&b"/a"[..], &b"/b"[..]])
    );
    assert_eq!(split_batched_paths(b"/a\n/b", 2), None, "unterminated");
    assert_eq!(
        split_batched_paths(b"/a\nb\n/c\n", 2),
        None,
        "embedded newline"
    );
    assert_eq!(split_batched_paths(b"/a\n", 2), None, "missing line");
    assert_eq!(split_batched_paths(b"/a\n\n", 2), None, "empty line");
}

#[cfg(unix)]
#[test]
fn watch_paths_fall_back_when_a_metadata_path_contains_a_newline() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("worktree");
    let metadata = directory.path().join("meta\ndata");
    fs::create_dir(&root).unwrap();
    let output = Command::new("git")
        .current_dir(&root)
        .args(["init", "-q", "--separate-git-dir"])
        .arg(&metadata)
        .output()
        .unwrap();
    assert!(output.status.success());
    let repository = Repository::discover(&root).unwrap();
    let (paths, _) = repository.watch_paths(&CancelToken::default()).unwrap();
    for expected in [
        metadata.clone(),
        metadata.join("HEAD"),
        metadata.join("info/exclude"),
    ] {
        assert!(
            contains_location(&paths, &expected),
            "{expected:?} missing from {paths:?}"
        );
    }
}

#[test]
fn metadata_paths_are_batched_and_match_the_per_path_queries() {
    let (directory, _repository) = repository();
    git(
        directory.path(),
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-q",
            "-m",
            "base",
        ],
    );
    let linked = directory.path().join("linked");
    git(
        directory.path(),
        &[
            "worktree",
            "add",
            "-q",
            linked.to_str().unwrap(),
            "-b",
            "topic",
        ],
    );
    let cancel = CancelToken::default();
    for root in [directory.path().to_path_buf(), linked] {
        let repository = Repository::discover(&root).unwrap();
        let batched = repository
            .batched_metadata_paths(&cancel)
            .unwrap()
            .expect("plain paths must resolve in one rev-parse");
        assert_eq!(
            batched,
            repository.sequential_metadata_paths(&cancel).unwrap()
        );
    }
}
