use fastdiff_core::{CancelToken, GitPath, ReflogRequest, Repository, RowKind, View};
use std::{fs, path::Path, process::Command};
use tempfile::TempDir;

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

fn repo() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Diff test"]);
    git(
        dir.path(),
        &["config", "user.email", "diff@example.invalid"],
    );
    dir
}

fn write(root: &Path, path: &str, content: &str) {
    if let Some(parent) = root.join(path).parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(root.join(path), content).unwrap();
}

fn commit(root: &Path) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "fixture"]);
}

#[test]
fn discovers_repository_from_nested_directory_and_rejects_non_repository() {
    let dir = repo();
    fs::create_dir_all(dir.path().join("nested/deep")).unwrap();
    let repository = Repository::discover(dir.path().join("nested/deep")).unwrap();
    assert_eq!(repository.root(), dir.path().canonicalize().unwrap());
    let plain = tempfile::tempdir().unwrap();
    assert!(Repository::discover(plain.path()).is_err());
}

#[test]
fn all_lists_mixed_changes_once_and_labels_comparisons() {
    let dir = repo();
    write(dir.path(), "main.rs", "old\n");
    commit(dir.path());
    write(dir.path(), "main.rs", "staged\n");
    git(dir.path(), &["add", "main.rs"]);
    write(dir.path(), "main.rs", "working\n");
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let all = repository.status(View::All, &cancel).unwrap();
    assert_eq!(all.len(), 1);
    assert!(all[0].staged && all[0].unstaged);
    let document = repository.diff(&all[0], View::All, &cancel).unwrap();
    assert_eq!(document.comparisons.len(), 2);
    assert_eq!(document.comparisons[0].label, "Staged");
    assert!(document.comparisons[0].patch.contains("+staged"));
    assert_eq!(document.comparisons[1].label, "Unstaged");
    assert!(document.comparisons[1].patch.contains("-staged"));
    assert!(document.comparisons[1].patch.contains("+working"));
    for view in [View::Staged, View::Unstaged] {
        let files = repository.status(view, &cancel).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            repository
                .diff(&files[0], view, &cancel)
                .unwrap()
                .comparisons
                .len(),
            1
        );
    }
}

#[test]
fn staged_and_unstaged_filters_exclude_other_changes() {
    let dir = repo();
    write(dir.path(), "tracked", "old\n");
    commit(dir.path());
    write(dir.path(), "tracked", "new\n");
    write(dir.path(), "added", "addition\n");
    git(dir.path(), &["add", "added"]);
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    assert_eq!(
        repository.status(View::Staged, &cancel).unwrap()[0]
            .path
            .display(),
        "added"
    );
    assert_eq!(
        repository.status(View::Unstaged, &cancel).unwrap()[0]
            .path
            .display(),
        "tracked"
    );
}

#[test]
fn unborn_repository_supports_staged_and_nonignored_untracked_additions() {
    let dir = repo();
    write(dir.path(), ".gitignore", "ignored\n");
    git(dir.path(), &["add", ".gitignore"]);
    write(dir.path(), "ignored", "hidden\n");
    let unusual_name = if cfg!(windows) {
        "new file λ.rs"
    } else {
        "new file\nλ.rs"
    };
    write(dir.path(), unusual_name, "one\ntwo\n");
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let files = repository.status(View::All, &cancel).unwrap();
    assert_eq!(files.len(), 2);
    let untracked = files.iter().find(|f| f.untracked).unwrap();
    let document = repository.diff(untracked, View::All, &cancel).unwrap();
    assert_eq!(document.comparisons[0].label, "Untracked");
    assert_eq!(
        document.comparisons[0]
            .rows
            .iter()
            .filter(|r| r.kind == RowKind::Addition)
            .count(),
        2
    );
    let staged = repository.status(View::Staged, &cancel).unwrap();
    assert_eq!(staged.len(), 1);
    assert!(
        repository
            .diff(&staged[0], View::Staged, &cancel)
            .unwrap()
            .comparisons[0]
            .patch
            .contains("+ignored")
    );
}

#[test]
fn rename_preserves_old_path_and_deletion_has_old_line_numbers() {
    let dir = repo();
    write(dir.path(), "before", "same\n");
    write(dir.path(), "deleted", "gone\n");
    commit(dir.path());
    git(dir.path(), &["mv", "before", "after"]);
    fs::remove_file(dir.path().join("deleted")).unwrap();
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let files = repository.status(View::All, &cancel).unwrap();
    let rename = files.iter().find(|f| f.path.display() == "after").unwrap();
    assert_eq!(rename.old_path.as_ref().unwrap().display(), "before");
    assert!(
        repository
            .diff(rename, View::All, &cancel)
            .unwrap()
            .comparisons[0]
            .patch
            .contains("rename from")
    );
    let deletion = files
        .iter()
        .find(|f| f.path.display() == "deleted")
        .unwrap();
    let rows = &repository
        .diff(deletion, View::All, &cancel)
        .unwrap()
        .comparisons[0]
        .rows;
    assert!(
        rows.iter()
            .any(|r| r.kind == RowKind::Deletion && r.old_number == Some(1) && r.left == "gone")
    );
}

#[test]
fn replaced_lines_are_aligned_with_correct_numbers() {
    let dir = repo();
    write(dir.path(), "file", "first\nold\nlast\n");
    commit(dir.path());
    write(dir.path(), "file", "first\nnew\nextra\nlast\n");
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let file = repository.status(View::All, &cancel).unwrap().remove(0);
    let document = repository.diff(&file, View::All, &cancel).unwrap();
    let rows = &document.comparisons[0].rows;
    assert!(rows.iter().any(|r| r.kind == RowKind::Change
        && r.left == "old"
        && r.right == "new"
        && r.old_number == Some(2)
        && r.new_number == Some(2)));
    assert!(
        rows.iter()
            .any(|r| r.kind == RowKind::Addition && r.right == "extra" && r.new_number == Some(3))
    );
    assert!(
        rows.iter().any(|r| r.kind == RowKind::Context
            && r.old_number == Some(3)
            && r.new_number == Some(4))
    );
}

#[test]
fn loads_enough_context_for_viewer_to_collapse_and_expand() {
    let dir = repo();
    let before = (1..=100)
        .map(|line| format!("line {line}\n"))
        .collect::<String>();
    write(dir.path(), "file", &before);
    commit(dir.path());
    write(
        dir.path(),
        "file",
        &before.replace("line 50\n", "changed 50\n"),
    );
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let file = repository.status(View::All, &cancel).unwrap().remove(0);
    let document = repository.diff(&file, View::All, &cancel).unwrap();
    let context = document.comparisons[0]
        .rows
        .iter()
        .filter(|row| row.kind == RowKind::Context)
        .count();
    assert!(
        context >= 8,
        "viewer folding requires at least eight context rows, got {context}"
    );
    assert!(
        document.comparisons[0]
            .rows
            .iter()
            .any(|row| row.old_number == Some(20))
    );
}

#[test]
fn binary_and_conflicts_have_clear_summaries() {
    let dir = repo();
    fs::write(dir.path().join("binary"), [0, 1, 2]).unwrap();
    write(dir.path(), "conflicted", "base\n");
    commit(dir.path());
    fs::write(dir.path().join("binary"), [0, 9, 8]).unwrap();
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let file = repository.status(View::All, &cancel).unwrap().remove(0);
    assert!(
        repository
            .diff(&file, View::All, &cancel)
            .unwrap()
            .comparisons[0]
            .summary
            .as_ref()
            .unwrap()
            .to_lowercase()
            .contains("binary")
    );
    git(dir.path(), &["checkout", "-b", "other"]);
    write(dir.path(), "conflicted", "other\n");
    git(dir.path(), &["add", "conflicted"]);
    git(dir.path(), &["commit", "-m", "other"]);
    git(dir.path(), &["checkout", "main"]);
    write(dir.path(), "conflicted", "main\n");
    git(dir.path(), &["add", "conflicted"]);
    git(dir.path(), &["commit", "-m", "main"]);
    let output = Command::new("git")
        .current_dir(dir.path())
        .args(["merge", "other"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let conflict = repository
        .status(View::All, &cancel)
        .unwrap()
        .into_iter()
        .find(|f| f.conflict)
        .unwrap();
    assert!(
        repository
            .diff(&conflict, View::All, &cancel)
            .unwrap()
            .comparisons[0]
            .summary
            .as_ref()
            .unwrap()
            .to_lowercase()
            .contains("conflict")
    );
}

#[test]
fn source_and_rendering_limits_produce_summaries() {
    let dir = repo();
    fs::write(dir.path().join("large"), vec![b'x'; 10 * 1024 * 1024 + 1]).unwrap();
    write(dir.path(), "rows", &"line\n".repeat(100_001));
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    for file in repository.status(View::All, &cancel).unwrap() {
        let document = repository.diff(&file, View::All, &cancel).unwrap();
        assert!(document.comparisons[0].summary.is_some());
        assert!(document.comparisons[0].rows.len() <= 100_000);
    }
}

#[test]
fn oversized_index_and_head_blobs_are_not_loaded_as_text() {
    let dir = repo();
    fs::write(dir.path().join("large"), vec![b'x'; 10 * 1024 * 1024 + 1]).unwrap();
    git(dir.path(), &["add", "large"]);
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let staged = repository.status(View::Staged, &cancel).unwrap().remove(0);
    assert!(
        repository
            .diff(&staged, View::Staged, &cancel)
            .unwrap()
            .comparisons[0]
            .summary
            .as_ref()
            .unwrap()
            .contains("10 MiB")
    );
    commit(dir.path());
    write(dir.path(), "large", "small\n");
    git(dir.path(), &["add", "large"]);
    let staged = repository.status(View::Staged, &cancel).unwrap().remove(0);
    assert!(
        repository
            .diff(&staged, View::Staged, &cancel)
            .unwrap()
            .comparisons[0]
            .summary
            .as_ref()
            .unwrap()
            .contains("10 MiB")
    );
}

#[test]
fn oversized_patch_produces_summary_without_losing_connection() {
    let dir = repo();
    fs::write(dir.path().join("patch"), vec![b'x'; 2 * 1024 * 1024 + 1]).unwrap();
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let file = repository.status(View::All, &cancel).unwrap().remove(0);
    let comparison = repository
        .diff(&file, View::All, &cancel)
        .unwrap()
        .comparisons
        .remove(0);
    assert!(comparison.summary.unwrap().contains("2 MiB"));
    assert!(comparison.patch.is_empty());
    assert!(comparison.rows.is_empty());
    assert_eq!(repository.status(View::All, &cancel).unwrap().len(), 1);
}

#[test]
fn deletion_of_parent_directory_still_loads_deleted_source() {
    let dir = repo();
    write(dir.path(), "nested/deep/file", "deleted\n");
    commit(dir.path());
    fs::remove_dir_all(dir.path().join("nested")).unwrap();
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let file = repository.status(View::All, &cancel).unwrap().remove(0);
    assert!(
        repository
            .diff(&file, View::All, &cancel)
            .unwrap()
            .comparisons[0]
            .patch
            .contains("-deleted")
    );
}

#[test]
fn cancellation_is_shared_and_prevents_work() {
    let dir = repo();
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let clone = cancel.clone();
    clone.cancel();
    assert!(cancel.is_cancelled());
    assert!(repository.status(View::All, &cancel).is_err());
    assert!(Repository::discover_with_cancel(dir.path(), &cancel).is_err());
}

#[test]
fn reading_during_concurrent_writes_remains_bounded_and_refreshable() {
    let dir = repo();
    write(dir.path(), "file", "old\n");
    commit(dir.path());
    let path = dir.path().join("file");
    let writer = std::thread::spawn(move || {
        for i in 0..25 {
            fs::write(&path, format!("revision {i}\n")).unwrap();
        }
    });
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    for _ in 0..3 {
        for file in repository.status(View::All, &cancel).unwrap() {
            assert!(
                repository
                    .diff(&file, View::All, &cancel)
                    .unwrap()
                    .comparisons[0]
                    .rows
                    .len()
                    < 10
            );
        }
    }
    writer.join().unwrap();
    let file = repository.status(View::All, &cancel).unwrap().remove(0);
    assert!(
        repository
            .diff(&file, View::All, &cancel)
            .unwrap()
            .comparisons[0]
            .patch
            .contains("revision 24")
    );
}

#[cfg(unix)]
#[test]
fn filename_pathspec_magic_is_treated_literally() {
    let dir = repo();
    write(dir.path(), ":(glob)*", "literal\n");
    write(dir.path(), "other", "other\n");
    commit(dir.path());
    write(dir.path(), ":(glob)*", "changed\n");
    write(dir.path(), "other", "excluded\n");
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let file = repository
        .status(View::All, &cancel)
        .unwrap()
        .into_iter()
        .find(|f| f.path.display().starts_with(':'))
        .unwrap();
    let document = repository.diff(&file, View::All, &cancel).unwrap();
    assert!(document.comparisons[0].patch.contains("+changed"));
    assert!(!document.comparisons[0].patch.contains("excluded"));
}

#[test]
fn unified_rendering_budget_is_shared_by_staged_and_unstaged_comparisons() {
    let dir = repo();
    write(dir.path(), "file", &"a\n".repeat(80_000));
    commit(dir.path());
    write(dir.path(), "file", &"b\n".repeat(80_000));
    git(dir.path(), &["add", "file"]);
    write(dir.path(), "file", &"c\n".repeat(80_000));
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let file = repository.status(View::All, &cancel).unwrap().remove(0);
    let document = repository.diff(&file, View::All, &cancel).unwrap();
    let rendered: usize = document
        .comparisons
        .iter()
        .flat_map(|comparison| &comparison.rows)
        .map(|row| if row.kind == RowKind::Change { 2 } else { 1 })
        .sum();
    assert!(rendered <= 100_000);
    assert_eq!(document.comparisons.len(), 2);
    assert!(
        document
            .comparisons
            .iter()
            .all(|comparison| comparison.summary.is_some())
    );
}

#[test]
fn helper_supplied_paths_cannot_escape_repository() {
    let dir = repo();
    write(dir.path(), "file", "content\n");
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let file = repository.status(View::All, &cancel).unwrap().remove(0);
    for invalid in ["../outside", "/etc/passwd", ".", "file\0suffix"] {
        let forged = fastdiff_core::FileChange {
            path: GitPath(invalid.as_bytes().to_vec()),
            ..file.clone()
        };
        assert!(repository.diff(&forged, View::All, &cancel).is_err());
    }
}

#[cfg(unix)]
#[test]
fn untracked_symlink_shows_link_target_without_reading_destination() {
    use std::os::unix::fs::symlink;
    let dir = repo();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret"), "destination content").unwrap();
    symlink(outside.path().join("secret"), dir.path().join("link")).unwrap();
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let file = repository.status(View::All, &cancel).unwrap().remove(0);
    let patch = repository
        .diff(&file, View::All, &cancel)
        .unwrap()
        .comparisons
        .remove(0)
        .patch;
    assert!(patch.contains("secret"));
    assert!(!patch.contains("destination content"));
}

#[cfg(unix)]
#[test]
fn helper_supplied_path_cannot_follow_external_parent_symlink() {
    use std::os::unix::fs::symlink;
    let dir = repo();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret"), "external text").unwrap();
    symlink(outside.path(), dir.path().join("external")).unwrap();
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let file = repository.status(View::All, &cancel).unwrap().remove(0);
    let forged = fastdiff_core::FileChange {
        path: GitPath(b"external/secret".to_vec()),
        ..file
    };
    assert!(repository.diff(&forged, View::All, &cancel).is_err());
}

#[test]
fn rename_detection_is_independent_of_user_git_configuration() {
    let dir = repo();
    write(dir.path(), "before", "same\n");
    commit(dir.path());
    git(dir.path(), &["config", "status.renames", "false"]);
    git(dir.path(), &["mv", "before", "after"]);
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let files = repository.status(View::All, &cancel).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].old_path.as_ref().unwrap().display(), "before");
}

#[cfg(unix)]
#[test]
fn review_does_not_execute_repository_clean_or_process_filters() {
    let dir = repo();
    write(
        dir.path(),
        ".gitattributes",
        "*.txt filter=review diff=review\n",
    );
    write(dir.path(), "file.txt", "old\n");
    commit(dir.path());
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    assert!(repository.status(View::All, &cancel).unwrap().is_empty());
    git(
        dir.path(),
        &[
            "config",
            "filter.review.clean",
            "touch FILTER_EXECUTED; cat",
        ],
    );
    git(
        dir.path(),
        &[
            "config",
            "filter.review.process",
            "touch PROCESS_EXECUTED; cat",
        ],
    );
    git(dir.path(), &["config", "filter.review.required", "true"]);
    git(
        dir.path(),
        &["config", "diff.external", ": > EXTERNAL_DIFF_EXECUTED"],
    );
    git(
        dir.path(),
        &["config", "diff.review.textconv", ": > TEXTCONV_EXECUTED"],
    );
    write(dir.path(), "file.txt", "new\n");
    let files = repository.status(View::All, &cancel).unwrap();
    assert!(!dir.path().join("FILTER_EXECUTED").exists());
    assert!(!dir.path().join("PROCESS_EXECUTED").exists());
    let file = files
        .into_iter()
        .find(|file| file.path.display() == "file.txt")
        .unwrap();
    assert!(
        repository
            .diff(&file, View::All, &cancel)
            .unwrap()
            .comparisons[0]
            .patch
            .contains("+new")
    );
    assert!(!dir.path().join("FILTER_EXECUTED").exists());
    assert!(!dir.path().join("PROCESS_EXECUTED").exists());
    assert!(!dir.path().join("EXTERNAL_DIFF_EXECUTED").exists());
    assert!(!dir.path().join("TEXTCONV_EXECUTED").exists());
}

#[test]
fn operation_metadata_does_not_retain_index_or_head_sizes() {
    let dir = repo();
    write(dir.path(), "file", "head\n");
    commit(dir.path());
    write(dir.path(), "file", "staged\n");
    git(dir.path(), &["add", "file"]);
    write(dir.path(), "file", "working\n");
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let change = repository.status(View::All, &cancel).unwrap().remove(0);
    let first = repository.diff(&change, View::All, &cancel).unwrap();
    assert!(first.comparisons[0].patch.contains("+staged"));
    assert!(first.comparisons[1].patch.contains("+working"));

    fs::write(dir.path().join("file"), vec![b'x'; 10 * 1024 * 1024 + 1]).unwrap();
    git(dir.path(), &["add", "file"]);
    write(dir.path(), "file", "working\n");
    let large_index = repository.diff(&change, View::All, &cancel).unwrap();
    assert!(
        large_index.comparisons.iter().all(|comparison| comparison
            .summary
            .as_ref()
            .unwrap()
            .contains("10 MiB"))
    );

    git(dir.path(), &["commit", "-m", "large HEAD"]);
    write(dir.path(), "file", "small index\n");
    git(dir.path(), &["add", "file"]);
    write(dir.path(), "file", "small working\n");
    let large_head = repository.diff(&change, View::All, &cancel).unwrap();
    assert!(
        large_head.comparisons[0]
            .summary
            .as_ref()
            .unwrap()
            .contains("10 MiB")
    );
    assert!(large_head.comparisons[1].summary.is_none());
    assert!(large_head.comparisons[1].patch.contains("-small index"));
    assert!(large_head.comparisons[1].patch.contains("+small working"));
}

#[test]
fn diff_is_identical_with_and_without_cached_filter_overrides() {
    let dir = repo();
    write(dir.path(), "file", "head\n");
    commit(dir.path());
    write(dir.path(), "file", "staged\n");
    git(dir.path(), &["add", "file"]);
    write(dir.path(), "file", "working\n");
    git(dir.path(), &["config", "filter.review.clean", "cat"]);
    let cancel = CancelToken::default();
    let cold = Repository::discover(dir.path()).unwrap();
    let change = cold.status(View::All, &cancel).unwrap().remove(0);

    let warm = Repository::discover(dir.path()).unwrap();
    warm.status(View::All, &cancel).unwrap();
    let without_cache = Repository::discover(dir.path()).unwrap();

    let expected = without_cache.diff(&change, View::All, &cancel).unwrap();
    let cached = warm.diff(&change, View::All, &cancel).unwrap();
    assert_eq!(expected.comparisons.len(), 2);
    for (left, right) in expected.comparisons.iter().zip(&cached.comparisons) {
        assert_eq!(left.label, right.label);
        assert_eq!(left.patch, right.patch);
    }
}

// APFS rejects non-UTF-8 file names (EILSEQ), so the fixture cannot be created on macOS.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn non_utf8_paths_roundtrip_without_using_patch_headers() {
    use std::os::unix::ffi::OsStringExt;
    let dir = repo();
    let path = std::ffi::OsString::from_vec(b"nonutf\xff\nname".to_vec());
    fs::write(dir.path().join(&path), "content\n").unwrap();
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let file = repository.status(View::All, &cancel).unwrap().remove(0);
    assert_eq!(file.path.0, b"nonutf\xff\nname");
    assert_eq!(GitPath::from_path(Path::new(&path)), file.path);
    assert_eq!(file.path.to_path_buf().unwrap(), Path::new(&path));
    assert!(
        repository
            .diff(&file, View::All, &cancel)
            .unwrap()
            .comparisons[0]
            .patch
            .contains("+content")
    );
}

fn upstream(root: &Path) {
    git(root, &["branch", "upstream"]);
    git(root, &["branch", "--set-upstream-to=upstream", "main"]);
}

#[test]
fn unpushed_is_cumulative_pinned_and_combines_with_working_comparisons() {
    let dir = repo();
    write(dir.path(), "file", "base\n");
    commit(dir.path());
    upstream(dir.path());
    write(dir.path(), "file", "first\n");
    commit(dir.path());
    write(dir.path(), "file", "local\n");
    commit(dir.path());
    write(dir.path(), "file", "staged\n");
    git(dir.path(), &["add", "file"]);
    write(dir.path(), "file", "working\n");
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let (files, notice) = repository.status_with_notice(View::All, &cancel).unwrap();
    assert!(notice.is_none());
    assert_eq!(files.len(), 1);
    let document = repository.diff(&files[0], View::All, &cancel).unwrap();
    assert_eq!(
        document
            .comparisons
            .iter()
            .map(|comparison| comparison.label.as_str())
            .collect::<Vec<_>>(),
        ["Unpushed", "Staged", "Unstaged"]
    );
    assert!(document.comparisons[0].patch.contains("-base"));
    assert!(document.comparisons[0].patch.contains("+local"));
    commit(dir.path());
    git(dir.path(), &["branch", "-f", "upstream", "HEAD"]);
    let pinned = repository.diff(&files[0], View::Unpushed, &cancel).unwrap();
    assert_eq!(pinned.comparisons[0].patch, document.comparisons[0].patch);
    assert!(
        repository
            .status(View::Unpushed, &cancel)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn diverged_upstream_excludes_remote_changes_and_behind_is_empty() {
    let dir = repo();
    write(dir.path(), "base", "base\n");
    commit(dir.path());
    upstream(dir.path());
    git(dir.path(), &["checkout", "upstream"]);
    write(dir.path(), "remote", "remote\n");
    commit(dir.path());
    git(dir.path(), &["checkout", "main"]);
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    assert!(
        repository
            .status(View::Unpushed, &cancel)
            .unwrap()
            .is_empty()
    );
    write(dir.path(), "local", "local\n");
    commit(dir.path());
    let files = repository.status(View::Unpushed, &cancel).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path.display(), "local");
}

#[test]
fn committed_rename_deletion_binary_and_independent_working_rename_are_retained() {
    let dir = repo();
    write(dir.path(), "before", "rename\n");
    write(dir.path(), "deleted", "gone\n");
    fs::write(dir.path().join("binary"), [0, 1]).unwrap();
    commit(dir.path());
    upstream(dir.path());
    git(dir.path(), &["mv", "before", "after"]);
    fs::remove_file(dir.path().join("deleted")).unwrap();
    fs::write(dir.path().join("binary"), [0, 2]).unwrap();
    commit(dir.path());
    // A staged rename must not overwrite the committed comparison's paths.
    git(dir.path(), &["mv", "after", "working"]);
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let files = repository.status(View::All, &cancel).unwrap();
    let after = files.iter().find(|f| f.path.display() == "after").unwrap();
    assert_eq!(
        after
            .committed
            .as_ref()
            .unwrap()
            .old_path
            .as_ref()
            .unwrap()
            .display(),
        "before"
    );
    assert!(
        repository
            .diff(after, View::Unpushed, &cancel)
            .unwrap()
            .comparisons[0]
            .patch
            .contains("rename from before")
    );
    let deleted = files
        .iter()
        .find(|f| f.path.display() == "deleted")
        .unwrap();
    assert!(
        repository
            .diff(deleted, View::Unpushed, &cancel)
            .unwrap()
            .comparisons[0]
            .patch
            .contains("-gone")
    );
    let binary = files.iter().find(|f| f.path.display() == "binary").unwrap();
    assert!(
        repository
            .diff(binary, View::Unpushed, &cancel)
            .unwrap()
            .comparisons[0]
            .summary
            .as_ref()
            .unwrap()
            .contains("Binary")
    );
}

#[test]
fn unavailable_unpushed_preserves_working_changes_and_explains_state() {
    let dir = repo();
    write(dir.path(), "file", "base\n");
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let (files, notice) = repository.status_with_notice(View::All, &cancel).unwrap();
    assert_eq!(files.len(), 1);
    assert!(notice.unwrap().contains("no commit"));
    commit(dir.path());
    assert!(
        repository
            .status_with_notice(View::Unpushed, &cancel)
            .unwrap()
            .1
            .unwrap()
            .contains("upstream")
    );
    git(dir.path(), &["checkout", "--detach"]);
    assert!(
        repository
            .status_with_notice(View::Unpushed, &cancel)
            .unwrap()
            .1
            .unwrap()
            .contains("detached")
    );
}

#[test]
fn unpushed_checks_both_committed_blob_limits() {
    let dir = repo();
    fs::write(dir.path().join("large"), vec![b'x'; 10 * 1024 * 1024 + 1]).unwrap();
    commit(dir.path());
    upstream(dir.path());
    write(dir.path(), "large", "small\n");
    commit(dir.path());
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let files = repository.status(View::Unpushed, &cancel).unwrap();
    assert!(
        repository
            .diff(&files[0], View::Unpushed, &cancel)
            .unwrap()
            .comparisons[0]
            .summary
            .as_ref()
            .unwrap()
            .contains("10 MiB")
    );
}

#[test]
fn unrelated_history_is_explained_and_net_reverted_commits_are_empty() {
    let dir = repo();
    write(dir.path(), "file", "base\n");
    commit(dir.path());
    upstream(dir.path());
    write(dir.path(), "file", "temporary\n");
    commit(dir.path());
    write(dir.path(), "file", "base\n");
    commit(dir.path());
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    assert!(
        repository
            .status(View::Unpushed, &cancel)
            .unwrap()
            .is_empty()
    );
    git(dir.path(), &["checkout", "--orphan", "unrelated"]);
    write(dir.path(), "file", "unrelated\n");
    commit(dir.path());
    git(dir.path(), &["checkout", "main"]);
    git(
        dir.path(),
        &["branch", "--set-upstream-to=unrelated", "main"],
    );
    write(dir.path(), "file", "working\n");
    let (files, notice) = repository.status_with_notice(View::All, &cancel).unwrap();
    assert_eq!(files.len(), 1);
    assert!(notice.unwrap().contains("no common local history"));
}

#[test]
fn staged_deletion_and_untracked_replacement_share_one_destination() {
    let dir = repo();
    write(dir.path(), "file", "base\n");
    commit(dir.path());
    upstream(dir.path());
    write(dir.path(), "file", "committed\n");
    commit(dir.path());
    git(dir.path(), &["rm", "file"]);
    write(dir.path(), "file", "replacement\n");
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let files = repository.status(View::All, &cancel).unwrap();
    assert_eq!(files.len(), 1);
    assert!(files[0].staged && files[0].unstaged && files[0].untracked);
    let document = repository.diff(&files[0], View::All, &cancel).unwrap();
    assert_eq!(
        document
            .comparisons
            .iter()
            .map(|comparison| comparison.label.as_str())
            .collect::<Vec<_>>(),
        ["Unpushed", "Staged", "Untracked"]
    );
    assert!(document.comparisons[0].patch.contains("+committed"));
    assert!(document.comparisons[1].patch.contains("-committed"));
    assert!(!document.comparisons[1].patch.contains("replacement"));
    assert!(document.comparisons[2].patch.contains("+replacement"));
}

#[test]
fn reflog_preserves_actions_order_and_repeated_commit_ids() {
    let dir = repo();
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let unborn = repository.reflog(&cancel).unwrap();
    assert_eq!(unborn.branch.as_deref(), Some("main"));
    assert_eq!(unborn.notice.as_deref(), Some("No reflog entries"));
    assert!(unborn.entries.is_empty());
    git(
        dir.path(),
        &["commit", "--allow-empty", "-m", "Unicode λ subject"],
    );
    let local = repository.reflog(&cancel).unwrap();
    assert_eq!(
        local.entries[0].action,
        "commit (initial): Unicode λ subject"
    );
    assert_eq!(local.entries[0].selector, "HEAD@{0}");
    assert!(local.entries[0].id.starts_with(&local.entries[0].short_id));
    assert!(local.entries[0].decorations.contains("main"));
    git(dir.path(), &["branch", "upstream"]);
    git(dir.path(), &["branch", "--set-upstream-to=upstream"]);
    assert_eq!(repository.reflog(&cancel).unwrap().entries.len(), 1);
    git(dir.path(), &["checkout", "-b", "topic"]);
    git(dir.path(), &["branch", "-m", "renamed"]);
    git(dir.path(), &["reset", "--soft", "HEAD"]);
    let list = repository.reflog(&cancel).unwrap();
    assert_eq!(list.branch.as_deref(), Some("renamed"));
    assert_eq!(list.entries.len(), 4);
    assert_eq!(list.entries[0].action, "reset: moving to HEAD");
    assert_eq!(
        list.entries[1].action,
        "Branch: renamed refs/heads/topic to refs/heads/renamed"
    );
    assert_eq!(
        list.entries[2].action,
        "checkout: moving from main to topic"
    );
    // Git skips the zero-id entry written during branch rename, leaving a selector gap.
    for (entry, selector) in list
        .entries
        .iter()
        .zip(["HEAD@{0}", "HEAD@{1}", "HEAD@{3}", "HEAD@{4}"])
    {
        assert_eq!(entry.selector, selector);
        assert_eq!(entry.id, local.entries[0].id);
    }
    git(dir.path(), &["checkout", "--detach"]);
    let detached = repository.reflog(&cancel).unwrap();
    assert!(detached.branch.is_none());
    assert_eq!(detached.entries.len(), 5);
    assert!(detached.entries[0].action.starts_with("checkout:"));
    cancel.cancel();
    assert!(matches!(
        repository.reflog(&cancel),
        Err(fastdiff_core::Error::Cancelled)
    ));
}

#[test]
fn reflog_without_entries_is_an_explanatory_state() {
    let dir = repo();
    git(dir.path(), &["commit", "--allow-empty", "-m", "base"]);
    git(dir.path(), &["reflog", "expire", "--expire=all", "--all"]);
    let repository = Repository::discover(dir.path()).unwrap();
    let list = repository.reflog(&CancelToken::default()).unwrap();
    assert!(list.entries.is_empty());
    assert_eq!(list.notice.as_deref(), Some("No reflog entries"));
}

#[test]
fn reflog_pages_count_entries_preserve_repeated_ids_and_stop_at_exhaustion() {
    let dir = repo();
    git(dir.path(), &["commit", "--allow-empty", "-m", "base"]);
    git(dir.path(), &["checkout", "-b", "topic"]);
    git(dir.path(), &["branch", "-m", "renamed"]);
    for _ in 0..31 {
        git(dir.path(), &["reset", "--soft", "HEAD"]);
    }
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let first = repository
        .reflog_page(&ReflogRequest::default(), &cancel)
        .unwrap();
    assert_eq!(first.start, 0);
    assert_eq!(first.entries.len(), 15);
    assert!(first.has_more && first.snapshot.is_some());
    let next = |list: &fastdiff_core::ReflogList, start| ReflogRequest {
        start,
        snapshot: list.snapshot,
        boundary: list.entries.last().cloned(),
    };
    let second = repository.reflog_page(&next(&first, 15), &cancel).unwrap();
    assert_eq!(second.start, 15);
    assert_eq!(second.entries.len(), 15);
    assert!(second.has_more);
    let third = repository.reflog_page(&next(&second, 30), &cancel).unwrap();
    assert_eq!(third.start, 30);
    assert_eq!(third.entries.len(), 4);
    assert!(!third.has_more);
    let entries: Vec<_> = first
        .entries
        .iter()
        .chain(&second.entries)
        .chain(&third.entries)
        .collect();
    assert!(entries.iter().all(|entry| entry.id == entries[0].id));
    assert_eq!(entries[33].selector, "HEAD@{34}");
    let end = repository.reflog_page(&next(&third, 34), &cancel).unwrap();
    assert_eq!(end.start, 34);
    assert!(end.entries.is_empty() && !end.has_more);
}

#[test]
fn reflog_page_boundaries_and_stale_snapshot_or_overlap_reset_to_newest() {
    let dir = repo();
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    for count in 1..=16 {
        git(dir.path(), &["commit", "--allow-empty", "-m", "entry"]);
        if count >= 15 {
            let page = repository.reflog(&cancel).unwrap();
            assert_eq!(page.entries.len(), 15);
            assert_eq!(page.has_more, count == 16);
        }
    }
    let first = repository.reflog(&cancel).unwrap();
    let request = ReflogRequest {
        start: 15,
        snapshot: first.snapshot,
        boundary: first.entries.last().cloned(),
    };
    let mut wrong = request.clone();
    wrong.boundary.as_mut().unwrap().action.push_str(" wrong");
    assert_eq!(repository.reflog_page(&wrong, &cancel).unwrap().start, 0);
    git(dir.path(), &["reset", "--soft", "HEAD"]);
    let newest = repository.reflog_page(&request, &cancel).unwrap();
    assert_eq!(newest.start, 0);
    assert_ne!(newest.snapshot, first.snapshot);
    assert_eq!(newest.entries[0].action, "reset: moving to HEAD");
    git(dir.path(), &["reflog", "expire", "--expire=all", "--all"]);
    let expired = repository.reflog_page(&request, &cancel).unwrap();
    assert!(expired.entries.is_empty() && !expired.has_more);
    assert_eq!(expired.start, 0);
    cancel.cancel();
    assert!(matches!(
        repository.reflog_snapshot(&cancel),
        Err(fastdiff_core::Error::Cancelled)
    ));
}

#[test]
fn reflog_snapshot_tracks_nested_refs_packing_config_and_log_replacement() {
    let dir = repo();
    git(dir.path(), &["commit", "--allow-empty", "-m", "base"]);
    git(dir.path(), &["branch", "nested/topic"]);
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let mut snapshot = repository.reflog_snapshot(&cancel).unwrap();
    assert!(snapshot.is_some());
    assert_eq!(repository.reflog_snapshot(&cancel).unwrap(), snapshot);
    for args in [
        vec!["branch", "nested/other"],
        vec!["pack-refs", "--all", "--prune"],
        vec!["config", "core.abbrev", "12"],
    ] {
        git(dir.path(), &args);
        let changed = repository.reflog_snapshot(&cancel).unwrap();
        assert_ne!(changed, snapshot);
        snapshot = changed;
    }
    let log = dir.path().join(".git/logs/HEAD");
    let replacement = log.with_extension("replacement");
    fs::copy(&log, &replacement).unwrap();
    fs::rename(&replacement, &log).unwrap();
    assert_ne!(repository.reflog_snapshot(&cancel).unwrap(), snapshot);
    snapshot = repository.reflog_snapshot(&cancel).unwrap();
    fs::remove_file(&log).unwrap();
    assert_ne!(repository.reflog_snapshot(&cancel).unwrap(), snapshot);
}

#[test]
fn reflog_snapshot_supports_linked_worktree_and_separate_git_directory() {
    let dir = repo();
    git(dir.path(), &["commit", "--allow-empty", "-m", "base"]);
    let linked = tempfile::tempdir().unwrap();
    let worktree = linked.path().join("linked");
    git(
        dir.path(),
        &[
            "worktree",
            "add",
            "-b",
            "linked",
            worktree.to_str().unwrap(),
        ],
    );
    let repository = Repository::discover(&worktree).unwrap();
    let cancel = CancelToken::default();
    let before = repository.reflog_snapshot(&cancel).unwrap();
    assert!(before.is_some());
    git(dir.path(), &["branch", "common-decoration"]);
    assert_ne!(repository.reflog_snapshot(&cancel).unwrap(), before);
    let separate = tempfile::tempdir().unwrap();
    let root = separate.path().join("root");
    let metadata = separate.path().join("metadata");
    fs::create_dir(&root).unwrap();
    git(
        &root,
        &[
            "init",
            "-b",
            "main",
            "--separate-git-dir",
            metadata.to_str().unwrap(),
        ],
    );
    assert!(
        Repository::discover(&root)
            .unwrap()
            .reflog_snapshot(&cancel)
            .unwrap()
            .is_some()
    );
}

#[test]
fn reflog_snapshot_tracks_included_config_creation_and_updated_sources() {
    let dir = repo();
    git(dir.path(), &["commit", "--allow-empty", "-m", "base"]);
    let first_config = dir.path().join("first-config");
    git(
        dir.path(),
        &["config", "include.path", first_config.to_str().unwrap()],
    );
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    let absent = repository.reflog_snapshot(&cancel).unwrap();
    assert!(absent.is_some());
    fs::write(&first_config, "[core]\n\tabbrev = 9\n").unwrap();
    let created = repository.reflog_snapshot(&cancel).unwrap();
    assert_ne!(created, absent);
    let second_config = dir.path().join("second-config");
    git(
        dir.path(),
        &[
            "config",
            "--file",
            first_config.to_str().unwrap(),
            "include.path",
            second_config.to_str().unwrap(),
        ],
    );
    let source_changed = repository.reflog_snapshot(&cancel).unwrap();
    assert_ne!(source_changed, created);
    fs::write(&second_config, "[core]\n\tabbrev = 12\n").unwrap();
    assert_ne!(repository.reflog_snapshot(&cancel).unwrap(), source_changed);
}

#[cfg(unix)]
#[test]
fn reflog_symlinked_metadata_falls_back_to_loaded_prefix_instead_of_constant_snapshot() {
    let dir = repo();
    git(dir.path(), &["commit", "--allow-empty", "-m", "base"]);
    for _ in 0..20 {
        git(dir.path(), &["reset", "--soft", "HEAD"]);
    }
    let log = dir.path().join(".git/logs/HEAD");
    let target = dir.path().join("actual-log");
    fs::rename(&log, &target).unwrap();
    std::os::unix::fs::symlink(&target, &log).unwrap();
    let repository = Repository::discover(dir.path()).unwrap();
    let cancel = CancelToken::default();
    assert_eq!(repository.reflog_snapshot(&cancel).unwrap(), None);
    let first = repository.reflog(&cancel).unwrap();
    assert_eq!(first.entries.len(), 15);
    assert!(first.has_more && first.snapshot.is_none());
    let older = repository
        .reflog_page(
            &ReflogRequest {
                start: 15,
                snapshot: None,
                boundary: first.entries.last().cloned(),
            },
            &cancel,
        )
        .unwrap();
    assert_eq!(older.start, 0);
    assert_eq!(older.entries.len(), 21);
    assert!(!older.has_more);
}

#[test]
fn reflog_never_runs_repository_signature_verifiers() {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let dir = repo();
    git(dir.path(), &["commit", "--allow-empty", "-m", "base"]);
    let tree = Command::new("git")
        .current_dir(dir.path())
        .args(["rev-parse", "HEAD^{tree}"])
        .output()
        .unwrap();
    assert!(tree.status.success());
    let tree = String::from_utf8(tree.stdout).unwrap();
    let raw = format!(
        "tree {}\nauthor A <a@a> 1700000000 +0000\ncommitter A <a@a> 1700000000 +0000\ngpgsig -----BEGIN PGP SIGNATURE-----\n fake\n -----END PGP SIGNATURE-----\n\nsigned subject\n",
        tree.trim()
    );
    write(dir.path(), "signed-commit", &raw);
    let object = Command::new("git")
        .current_dir(dir.path())
        .args(["hash-object", "-t", "commit", "-w", "signed-commit"])
        .output()
        .unwrap();
    assert!(object.status.success());
    let object = String::from_utf8(object.stdout).unwrap();
    git(
        dir.path(),
        &["update-ref", "-m", "signed subject", "HEAD", object.trim()],
    );
    let verifier = dir.path().join("signature-verifier");
    fs::write(&verifier, "#!/bin/sh\n: > signature-invoked\nexit 1\n").unwrap();
    #[cfg(unix)]
    fs::set_permissions(&verifier, fs::Permissions::from_mode(0o755)).unwrap();
    git(dir.path(), &["config", "log.showSignature", "true"]);
    git(
        dir.path(),
        &["config", "gpg.program", verifier.to_str().unwrap()],
    );
    git(dir.path(), &["log", "--no-patch", "--format=%s"]);
    let marker = dir.path().join("signature-invoked");
    assert!(
        marker.exists(),
        "Signed fixture must invoke the verifier by default"
    );
    fs::remove_file(&marker).unwrap();

    let repository = Repository::discover(dir.path()).unwrap();
    let list = repository.reflog(&CancelToken::default()).unwrap();
    assert_eq!(list.entries[0].action, "signed subject");
    assert!(
        !marker.exists(),
        "Metadata query executed repository gpg.program"
    );
}

#[test]
fn reflog_preserves_branch_names_that_look_like_ref_prefixes() {
    let dir = repo();
    let cancel = CancelToken::default();
    let repository = Repository::discover(dir.path()).unwrap();
    git(
        dir.path(),
        &["symbolic-ref", "HEAD", "refs/heads/refs/heads/topic"],
    );
    assert_eq!(
        repository.current_branch(&cancel).unwrap().as_deref(),
        Some("refs/heads/topic")
    );
    git(dir.path(), &["commit", "--allow-empty", "-m", "empty"]);
    git(dir.path(), &["branch", "refs/remotes/upstream"]);
    git(
        dir.path(),
        &["branch", "--set-upstream-to=refs/remotes/upstream"],
    );
    assert_eq!(
        repository.reflog(&cancel).unwrap().branch.as_deref(),
        Some("refs/heads/topic")
    );
}

fn notice(root: &Path) -> (usize, Option<String>) {
    let repository = Repository::discover(root).unwrap();
    let (files, notice) = repository
        .status_with_notice(View::Unpushed, &CancelToken::default())
        .unwrap();
    (files.len(), notice)
}

#[test]
fn unpushed_reports_detached_head_before_looking_at_commits() {
    let dir = repo();
    write(dir.path(), "a.txt", "one\n");
    commit(dir.path());
    git(dir.path(), &["checkout", "--detach", "-q"]);
    let (count, notice) = notice(dir.path());
    assert_eq!(count, 0);
    assert_eq!(
        notice.as_deref(),
        Some("Unpushed unavailable: HEAD is detached.")
    );
}

#[test]
fn unpushed_reports_a_branch_without_commits() {
    let dir = repo();
    let (count, notice) = notice(dir.path());
    assert_eq!(count, 0);
    assert_eq!(
        notice.as_deref(),
        Some("Unpushed unavailable: HEAD has no commit.")
    );
}

#[test]
fn unpushed_reports_a_missing_upstream() {
    let dir = repo();
    write(dir.path(), "a.txt", "one\n");
    commit(dir.path());
    let (count, notice) = notice(dir.path());
    assert_eq!(count, 0);
    assert!(
        notice
            .as_deref()
            .is_some_and(|text| text.contains("no configured upstream commit")),
        "{notice:?}"
    );
}

#[test]
fn unpushed_lists_local_commits_ahead_of_the_upstream() {
    let dir = repo();
    write(dir.path(), "a.txt", "one\n");
    commit(dir.path());
    git(dir.path(), &["branch", "base"]);
    git(dir.path(), &["branch", "--set-upstream-to=base", "main"]);
    write(dir.path(), "ahead.txt", "local\n");
    commit(dir.path());
    let (count, notice) = notice(dir.path());
    assert_eq!((count, notice), (1, None));
}

/// Two replace refs that point at each other make `HEAD^{commit}` fail with exit 128
/// ("replace depth too high") while `symbolic-ref` and `rev-parse --show-toplevel` still work.
fn break_commit_resolution(root: &Path) {
    let head = |revision: &str| {
        let output = Command::new("git")
            .current_dir(root)
            .args(["rev-parse", revision])
            .output()
            .unwrap();
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    let (first, second) = (head("HEAD~1"), head("HEAD"));
    git(
        root,
        &["update-ref", &format!("refs/replace/{first}"), &second],
    );
    git(
        root,
        &["update-ref", &format!("refs/replace/{second}"), &first],
    );
}

fn two_commits() -> TempDir {
    let dir = repo();
    write(dir.path(), "a.txt", "one\n");
    commit(dir.path());
    write(dir.path(), "a.txt", "two\n");
    commit(dir.path());
    dir
}

#[test]
fn unpushed_detached_notice_wins_over_a_failing_head_query() {
    let dir = two_commits();
    git(dir.path(), &["checkout", "--detach", "-q"]);
    break_commit_resolution(dir.path());
    let (count, notice) = notice(dir.path());
    assert_eq!(count, 0);
    assert_eq!(
        notice.as_deref(),
        Some("Unpushed unavailable: HEAD is detached.")
    );
}

#[test]
fn unpushed_surfaces_a_failing_head_query_on_a_branch() {
    let dir = two_commits();
    break_commit_resolution(dir.path());
    let repository = Repository::discover(dir.path()).unwrap();
    assert!(
        repository
            .status_with_notice(View::Unpushed, &CancelToken::default())
            .is_err()
    );
}
