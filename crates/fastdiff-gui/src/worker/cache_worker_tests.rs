use super::{
    tests::{git, repository_worker, wait_event, wait_status},
    *,
};

fn load(worker: &Worker, generation: u64, view: View, file: &FileChange) -> Arc<PreparedDocument> {
    worker.diff(generation, view, file.clone());
    let Event::Diff { document, .. } = wait_event(
        worker,
        |event| matches!(event, Event::Diff { generation: actual, .. } if *actual == generation),
    ) else {
        panic!("Expected prepared diff");
    };
    document
}

fn file<'a>(files: &'a [FileChange], path: &[u8]) -> &'a FileChange {
    files.iter().find(|file| file.path.0 == path).unwrap()
}

fn search(worker: &Worker, generation: u64, query: &str) -> Vec<usize> {
    let request_id = worker.shared.latest_search_id.load(Ordering::Acquire) + 1;
    worker.search(generation, request_id, query.into());
    let Event::Search { hits, .. } = wait_event(worker, |event| {
        matches!(event, Event::Search { generation: actual, request_id: actual_id, query: actual_query, .. }
            if *actual == generation && *actual_id == request_id && actual_query == query)
    }) else {
        panic!("Expected search result");
    };
    hits
}

fn backend(root: &std::path::Path) -> Backend {
    Backend::discover_with_cancel(
        &OpenRequest {
            target: OpenTarget::Repository,
            repository: root.to_string_lossy().into_owned(),
            file: None,
            view: View::All,
            wsl: None,
            theme: false,
        },
        &CancelToken::default(),
    )
    .unwrap()
}

fn assert_fresh(
    root: &std::path::Path,
    view: View,
    file: &FileChange,
    document: &PreparedDocument,
) {
    assert_eq!(
        document.document,
        backend(root)
            .diff(file, view, &CancelToken::default())
            .unwrap()
    );
}

fn commit(root: &std::path::Path) {
    git(root, &["add", "-A"]);
    git(
        root,
        &[
            "-c",
            "user.name=Worker test",
            "-c",
            "user.email=worker@example.invalid",
            "commit",
            "-qm",
            "cache fixture",
        ],
    );
}

#[test]
fn revisiting_cached_file_reuses_original_arc_and_searches_that_file() {
    let (directory, worker) = repository_worker();
    wait_status(&worker, 0, false);
    std::fs::write(directory.path().join("notes.txt"), "needle-a\n").unwrap();
    std::fs::write(directory.path().join("other.txt"), "needle-b\n").unwrap();
    worker.refresh(1, View::All);
    let files = wait_status(&worker, 1, false);
    let first = load(&worker, 2, View::All, file(&files, b"notes.txt"));
    let other = load(&worker, 3, View::All, file(&files, b"other.txt"));
    assert!(!Arc::ptr_eq(&first, &other));
    let revisited = load(&worker, 4, View::All, file(&files, b"notes.txt"));
    assert!(Arc::ptr_eq(&first, &revisited));
    assert!(!search(&worker, 4, "needle-a").is_empty());
    assert!(search(&worker, 4, "needle-b").is_empty());
}

#[test]
fn cached_file_reads_same_size_edits_with_restored_timestamp() {
    let (directory, worker) = repository_worker();
    wait_status(&worker, 0, false);
    let path = directory.path().join("notes.txt");
    std::fs::write(&path, "aaaa\n").unwrap();
    std::fs::write(directory.path().join("other.txt"), "other\n").unwrap();
    worker.refresh(1, View::All);
    let files = wait_status(&worker, 1, false);
    let first = load(&worker, 2, View::All, file(&files, b"notes.txt"));
    load(&worker, 3, View::All, file(&files, b"other.txt"));
    worker.set_foreground(false);
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::fs::write(&path, "bbbb\n").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    worker.refresh(4, View::All);
    worker.set_foreground(true);
    let refreshed = wait_status(&worker, 4, false);
    assert_eq!(files, refreshed);
    let changed = load(&worker, 5, View::All, file(&refreshed, b"notes.txt"));
    assert!(!Arc::ptr_eq(&first, &changed));
    assert_fresh(
        directory.path(),
        View::All,
        file(&refreshed, b"notes.txt"),
        &changed,
    );
    assert!(!search(&worker, 5, "bbbb").is_empty());
    assert!(search(&worker, 5, "aaaa").is_empty());
}

#[test]
fn cached_diff_tracks_stage_unstage_and_requested_view() {
    let (directory, worker) = repository_worker();
    wait_status(&worker, 0, false);
    std::fs::write(directory.path().join("notes.txt"), "staged-value\n").unwrap();
    std::fs::write(directory.path().join("other.txt"), "other\n").unwrap();
    worker.refresh(1, View::All);
    let files = wait_status(&worker, 1, false);
    let unstaged = load(&worker, 2, View::All, file(&files, b"notes.txt"));
    load(&worker, 3, View::All, file(&files, b"other.txt"));
    git(directory.path(), &["add", "notes.txt"]);
    std::fs::write(directory.path().join("notes.txt"), "worktree-value\n").unwrap();
    worker.refresh(4, View::All);
    let staged_files = wait_status(&worker, 4, false);
    let note = file(&staged_files, b"notes.txt");
    assert!(note.staged && note.unstaged);
    let all = load(&worker, 5, View::All, note);
    assert!(!Arc::ptr_eq(&unstaged, &all));
    for (generation, view) in [(6, View::Staged), (7, View::Unstaged), (8, View::All)] {
        let document = load(&worker, generation, view, note);
        assert_fresh(directory.path(), view, note, &document);
        if view == View::All {
            assert!(Arc::ptr_eq(&all, &document));
        } else {
            assert_ne!(document.document, all.document);
        }
    }
    load(&worker, 9, View::All, file(&staged_files, b"other.txt"));
    git(
        directory.path(),
        &["reset", "-q", "HEAD", "--", "notes.txt"],
    );
    worker.refresh(10, View::All);
    let unstaged_files = wait_status(&worker, 10, false);
    let note = file(&unstaged_files, b"notes.txt");
    assert!(note.unstaged && !note.staged);
    let unstaged_again = load(&worker, 11, View::All, note);
    assert_fresh(directory.path(), View::All, note, &unstaged_again);
    assert!(!Arc::ptr_eq(&all, &unstaged_again));
}

#[test]
fn cached_unpushed_diff_tracks_head_and_upstream_movement() {
    let (directory, worker) = repository_worker();
    wait_status(&worker, 0, false);
    std::fs::write(directory.path().join("notes.txt"), "first-commit\n").unwrap();
    commit(directory.path());
    std::fs::write(directory.path().join("other.txt"), "other\n").unwrap();
    worker.refresh(1, View::Unpushed);
    let first_files = wait_status(&worker, 1, false);
    let first = load(&worker, 2, View::Unpushed, file(&first_files, b"notes.txt"));
    worker.refresh(3, View::All);
    let all_files = wait_status(&worker, 3, false);
    load(&worker, 4, View::All, file(&all_files, b"other.txt"));
    std::fs::write(directory.path().join("notes.txt"), "second-commit\n").unwrap();
    commit(directory.path());
    worker.refresh(5, View::Unpushed);
    let second_files = wait_status(&worker, 5, false);
    let second = load(
        &worker,
        6,
        View::Unpushed,
        file(&second_files, b"notes.txt"),
    );
    assert!(!Arc::ptr_eq(&first, &second));
    assert_fresh(
        directory.path(),
        View::Unpushed,
        file(&second_files, b"notes.txt"),
        &second,
    );
    assert!(!search(&worker, 6, "second-commit").is_empty());
    git(
        directory.path(),
        &["update-ref", "refs/heads/upstream", "HEAD~1"],
    );
    worker.refresh(7, View::Unpushed);
    let moved_files = wait_status(&worker, 7, false);
    let moved = load(&worker, 8, View::Unpushed, file(&moved_files, b"notes.txt"));
    assert_fresh(
        directory.path(),
        View::Unpushed,
        file(&moved_files, b"notes.txt"),
        &moved,
    );
    assert_ne!(second.document, moved.document);
    git(
        directory.path(),
        &["update-ref", "refs/heads/upstream", "HEAD"],
    );
    worker.refresh(9, View::Unpushed);
    assert!(wait_status(&worker, 9, false).is_empty());
}

#[test]
fn cached_diff_tracks_rename_and_deletion_without_serving_old_content() {
    let (directory, worker) = repository_worker();
    wait_status(&worker, 0, false);
    let baseline = "base-line\n".repeat(20);
    std::fs::write(directory.path().join("notes.txt"), &baseline).unwrap();
    commit(directory.path());
    git(
        directory.path(),
        &["update-ref", "refs/heads/upstream", "HEAD"],
    );
    std::fs::write(
        directory.path().join("notes.txt"),
        format!("{baseline}new-line\n"),
    )
    .unwrap();
    std::fs::write(directory.path().join("other.txt"), "other\n").unwrap();
    worker.refresh(1, View::All);
    let files = wait_status(&worker, 1, false);
    let original = load(&worker, 2, View::All, file(&files, b"notes.txt"));
    load(&worker, 3, View::All, file(&files, b"other.txt"));
    git(directory.path(), &["mv", "notes.txt", "renamed.txt"]);
    worker.refresh(4, View::All);
    let renamed_files = wait_status(&worker, 4, false);
    let renamed_file = file(&renamed_files, b"renamed.txt");
    assert_eq!(renamed_file.old_path.as_ref().unwrap().0, b"notes.txt");
    let renamed = load(&worker, 5, View::All, renamed_file);
    assert_fresh(directory.path(), View::All, renamed_file, &renamed);
    assert!(!Arc::ptr_eq(&original, &renamed));
    load(&worker, 6, View::All, file(&renamed_files, b"other.txt"));
    std::fs::remove_file(directory.path().join("renamed.txt")).unwrap();
    worker.refresh(7, View::All);
    let deleted_files = wait_status(&worker, 7, false);
    let deleted_file = file(&deleted_files, b"renamed.txt");
    let deleted = load(&worker, 8, View::All, deleted_file);
    assert_fresh(directory.path(), View::All, deleted_file, &deleted);
    assert!(!Arc::ptr_eq(&renamed, &deleted));
    assert_ne!(renamed.document, deleted.document);
}

#[cfg(unix)]
#[test]
fn cached_diff_keeps_raw_unix_paths_distinct() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    let (directory, worker) = repository_worker();
    wait_status(&worker, 0, false);
    for (path, content) in [
        (b"raw-\xff.txt".as_slice(), "first-raw\n"),
        (b"raw-\xfe.txt", "second-raw\n"),
    ] {
        std::fs::write(directory.path().join(OsStr::from_bytes(path)), content).unwrap();
    }
    worker.refresh(1, View::All);
    let files = wait_status(&worker, 1, false);
    let first = load(&worker, 2, View::All, file(&files, b"raw-\xff.txt"));
    let second = load(&worker, 3, View::All, file(&files, b"raw-\xfe.txt"));
    assert_ne!(first.document, second.document);
    let revisited = load(&worker, 4, View::All, file(&files, b"raw-\xff.txt"));
    assert!(Arc::ptr_eq(&first, &revisited));
    assert!(!search(&worker, 4, "first-raw").is_empty());
    assert!(search(&worker, 4, "second-raw").is_empty());
}

#[test]
fn cached_diff_does_not_hide_backend_errors() {
    let (directory, worker) = repository_worker();
    wait_status(&worker, 0, false);
    std::fs::write(directory.path().join("notes.txt"), "changed\n").unwrap();
    std::fs::write(directory.path().join("other.txt"), "other\n").unwrap();
    worker.refresh(1, View::All);
    let files = wait_status(&worker, 1, false);
    let original = load(&worker, 2, View::All, file(&files, b"notes.txt"));
    load(&worker, 3, View::All, file(&files, b"other.txt"));
    worker.set_foreground(false);
    let index = directory.path().join(".git/index");
    let contents = std::fs::read(&index).unwrap();
    std::fs::write(&index, "invalid index").unwrap();
    worker.diff(4, View::All, file(&files, b"notes.txt").clone());
    worker.set_foreground(true);
    let Event::Error { message, .. } = wait_event(&worker, |event| {
        if matches!(event, Event::Diff { generation: 4, .. }) {
            panic!("Cache hid backend failure");
        }
        matches!(event, Event::Error { generation: 4, .. })
    }) else {
        panic!("Expected backend error");
    };
    assert!(!message.is_empty());
    worker.set_foreground(false);
    std::fs::write(&index, contents).unwrap();
    worker.diff(5, View::All, file(&files, b"notes.txt").clone());
    worker.set_foreground(true);
    let Event::Diff { document, .. } = wait_event(&worker, |event| {
        matches!(event, Event::Diff { generation: 5, .. })
    }) else {
        panic!("Expected recovered diff");
    };
    assert!(Arc::ptr_eq(&original, &document));
}

fn measurement_source(lines: usize, value: &str) -> String {
    (0..lines)
        .map(|index| format!("let item_{index} = \"{value}-{index}\"; // source line\n"))
        .collect()
}

fn measurement_fixture(
    lines: usize,
    count: usize,
) -> (tempfile::TempDir, Backend, Vec<FileChange>) {
    let (directory, worker) = repository_worker();
    wait_status(&worker, 0, false);
    drop(worker);
    for index in 0..count {
        std::fs::write(
            directory.path().join(format!("source-{index}.rs")),
            measurement_source(lines, "before"),
        )
        .unwrap();
    }
    commit(directory.path());
    git(
        directory.path(),
        &["update-ref", "refs/heads/upstream", "HEAD"],
    );
    for index in 0..count {
        std::fs::write(
            directory.path().join(format!("source-{index}.rs")),
            measurement_source(lines, &format!("after-{index}")),
        )
        .unwrap();
    }
    let backend = backend(directory.path());
    let (files, _) = backend
        .status_with_notice(View::All, &CancelToken::default())
        .unwrap();
    assert_eq!(files.len(), count);
    (directory, backend, files)
}

fn rss_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/status")
            .ok()?
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()?
            .checked_mul(1024)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn timing_summary(values: &[f64]) -> serde_json::Value {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    let median = if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    };
    serde_json::json!({
        "median_ms": median,
        "p95_ms": sorted[(sorted.len() * 95).div_ceil(100) - 1],
    })
}

#[test]
fn measurement_summary_reports_median_and_nearest_rank_p95() {
    let samples: Vec<f64> = (1..=20).rev().map(f64::from).collect();
    assert_eq!(
        timing_summary(&samples),
        serde_json::json!({"median_ms": 10.5, "p95_ms": 19.0}),
    );
    assert_eq!(
        timing_summary(&[4.0, 1.0, 2.0]),
        serde_json::json!({"median_ms": 2.0, "p95_ms": 4.0}),
    );
}

fn measure_navigation(
    backend: &Backend,
    files: &[FileChange],
    order: &[usize],
    entry_limit: usize,
    byte_limit: usize,
) -> serde_json::Value {
    use super::cache::PreparedCache;
    let mut cache = PreparedCache::with_limits(entry_limit, byte_limit);
    let token = CancelToken::default();
    cache.prune(View::All, files, &token);
    let mut current: Option<Arc<PreparedDocument>> = None;
    let mut previous: Vec<_> = (0..files.len()).map(|_| std::sync::Weak::new()).collect();
    let mut navigation_ms = Vec::with_capacity(order.len());
    let mut backend_ms = Vec::with_capacity(order.len());
    let mut preparation_ms = Vec::with_capacity(order.len());
    let mut reuse_count = 0;
    let mut cache_reuses = 0;
    let mut current_reuses = 0;
    let mut max_retained_bytes = cache.retained_bytes();
    let rss_before = rss_bytes();
    let mut rss_peak = rss_before;
    for &index in order {
        let started = Instant::now();
        let file = &files[index];
        assert!(backend.document_exists(&file.path, &token).unwrap());
        let fresh = backend.diff(file, View::All, &token).unwrap();
        let read_ms = started.elapsed().as_secs_f64() * 1000.0;
        let prepare_started = Instant::now();
        let prepared = if let Some(current) = &current
            && !current.is_document
            && current.document == fresh
        {
            current_reuses += 1;
            Arc::clone(current)
        } else if let Some(cached) = cache.get(View::All, file, &fresh, &token) {
            cache_reuses += 1;
            cached
        } else {
            Arc::new(PreparedDocument::new(fresh, &token))
        };
        if previous[index]
            .upgrade()
            .is_some_and(|last| Arc::ptr_eq(&last, &prepared))
        {
            reuse_count += 1;
        }
        previous[index] = Arc::downgrade(&prepared);
        cache.insert(View::All, file, Arc::clone(&prepared), &token);
        current = Some(prepared);
        preparation_ms.push(prepare_started.elapsed().as_secs_f64() * 1000.0);
        backend_ms.push(read_ms);
        navigation_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        max_retained_bytes = max_retained_bytes.max(cache.retained_bytes());
        rss_peak = rss_peak.into_iter().chain(rss_bytes()).max();
        assert!(cache.len() <= entry_limit);
        assert!(cache.retained_bytes() <= byte_limit);
    }
    serde_json::json!({
        "entry_limit": entry_limit,
        "byte_limit": byte_limit,
        "raw_navigation_ms": navigation_ms,
        "raw_backend_ms": backend_ms,
        "raw_preparation_ms": preparation_ms,
        "navigation": timing_summary(&navigation_ms),
        "backend": timing_summary(&backend_ms),
        "preparation": timing_summary(&preparation_ms),
        "reuse_count": reuse_count,
        "cache_reuses": cache_reuses,
        "current_reuses": current_reuses,
        "cached_entries": cache.len(),
        "retained_bytes": cache.retained_bytes(),
        "max_retained_bytes": max_retained_bytes,
        "rss_before_bytes": rss_before,
        "rss_after_bytes": rss_bytes(),
        "rss_peak_bytes": rss_peak,
    })
}

#[test]
#[ignore = "Worker-level measurements require explicit benchmark authorization"]
fn measure_prepared_diff_cache_navigation() {
    let mut workloads = Vec::new();
    for (name, lines, count, byte_limit) in [
        ("medium_a_b_a", 400, 2, 16 * 1024 * 1024),
        ("large_a_b_a", 4_000, 2, 16 * 1024 * 1024),
        ("eight_file_churn", 400, 8, 16 * 1024 * 1024),
        ("oversized", 400, 2, 1024),
    ] {
        let (_directory, backend, files) = measurement_fixture(lines, count);
        let order: Vec<usize> = (0..30)
            .flat_map(|_| {
                if count == 8 {
                    (0..8).collect()
                } else {
                    vec![0, 1, 0]
                }
            })
            .collect();
        let disabled = measure_navigation(&backend, &files, &order, 0, 0);
        let enabled = measure_navigation(&backend, &files, &order, 4, byte_limit);
        assert_eq!(disabled["cache_reuses"], 0);
        assert_eq!(disabled["cached_entries"], 0);
        if name == "medium_a_b_a" || name == "large_a_b_a" {
            assert!(enabled["cache_reuses"].as_u64().unwrap() > 0);
        }
        if count == 8 {
            assert!(enabled["cached_entries"].as_u64().unwrap() <= 4);
        }
        if name == "oversized" {
            assert_eq!(enabled["cached_entries"], 0);
        }
        workloads.push(serde_json::json!({
            "name": name,
            "lines_per_file": lines,
            "file_count": count,
            "cycles": 30,
            "navigation_count": order.len(),
            "disabled": disabled,
            "enabled": enabled,
        }));
    }
    println!(
        "{}",
        serde_json::json!({
            "schema": "prepared_diff_cache_navigation_v1",
            "scope": "worker backend reads, content verification, and prepared-document reuse; excludes UI and worker scheduling",
            "mode_order": "disabled then enabled; filesystem cache warming can affect timing comparisons",
            "rss_scope": "whole process; allocator retention and preceding workloads can affect RSS",
            "workloads": workloads,
        })
    );
}
