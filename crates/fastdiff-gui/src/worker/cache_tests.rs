use super::*;
use fastdiff_core::{CommittedComparison, Comparison, DiffRow, GitPath, RowKind};

fn file(path: &[u8]) -> FileChange {
    FileChange {
        path: GitPath(path.to_vec()),
        old_path: None,
        staged: false,
        unstaged: true,
        untracked: false,
        conflict: false,
        status: "M".into(),
        committed: None,
    }
}

fn prepared(text: &str) -> Arc<PreparedDocument> {
    Arc::new(PreparedDocument::new(
        DiffDocument {
            comparisons: vec![Comparison {
                label: "Unstaged".into(),
                patch: text.into(),
                summary: Some("summary".into()),
                rows: vec![DiffRow {
                    kind: RowKind::Change,
                    old_number: Some(1),
                    new_number: Some(1),
                    left: "old".into(),
                    right: text.into(),
                }],
            }],
        },
        &CancelToken::default(),
    ))
}

#[test]
fn lru_promotes_hits_replaces_mismatches_and_limits_entries() {
    let token = CancelToken::default();
    let mut cache = PreparedCache::with_limits(2, usize::MAX);
    let a = file(b"a");
    let b = file(b"b");
    let c = file(b"c");
    let first = prepared("first");
    let other = prepared("other");
    cache.insert(View::All, &a, Arc::clone(&first), &token);
    cache.insert(View::All, &b, Arc::clone(&other), &token);
    assert!(Arc::ptr_eq(
        &first,
        &cache.get(View::All, &a, &first.document, &token).unwrap()
    ));
    cache.insert(View::All, &c, Arc::clone(&other), &token);
    assert_eq!(cache.len(), 2);
    assert!(cache.get(View::All, &b, &other.document, &token).is_none());
    let changed = prepared("changed");
    assert!(
        cache
            .get(View::All, &a, &changed.document, &token)
            .is_none()
    );
    cache.insert(View::All, &a, Arc::clone(&changed), &token);
    cache.insert(View::All, &a, Arc::clone(&changed), &token);
    assert_eq!(cache.len(), 2);
    assert!(Arc::ptr_eq(
        &changed,
        &cache.get(View::All, &a, &changed.document, &token).unwrap()
    ));
}

#[test]
fn complete_keys_separate_views_flags_renames_commits_and_raw_paths() {
    let token = CancelToken::default();
    let mut cache = PreparedCache::default();
    let key = file(b"a/\xff");
    let doc = prepared("same");
    cache.insert(View::All, &key, Arc::clone(&doc), &token);
    for view in [View::Staged, View::Unstaged, View::Unpushed] {
        assert!(cache.get(view, &key, &doc.document, &token).is_none());
    }
    let mut variants = Vec::new();
    let mut changed = key.clone();
    changed.path = GitPath(b"a/\xfe".to_vec());
    variants.push(changed);
    let mut changed = key.clone();
    changed.old_path = Some(GitPath(b"old".to_vec()));
    variants.push(changed);
    let mut changed = key.clone();
    changed.staged = true;
    variants.push(changed);
    let mut changed = key.clone();
    changed.unstaged = false;
    variants.push(changed);
    let mut changed = key.clone();
    changed.untracked = true;
    variants.push(changed);
    let mut changed = key.clone();
    changed.conflict = true;
    variants.push(changed);
    let mut changed = key.clone();
    changed.status = "R".into();
    variants.push(changed);
    let mut committed = key.clone();
    committed.committed = Some(CommittedComparison {
        base: "base".into(),
        head: "head".into(),
        path: key.path.clone(),
        old_path: None,
        status: "M".into(),
    });
    variants.push(committed.clone());
    for variant in variants {
        assert!(
            cache
                .get(View::All, &variant, &doc.document, &token)
                .is_none()
        );
    }
    cache.insert(View::All, &committed, Arc::clone(&doc), &token);
    for which in 0..5 {
        let mut changed = committed.clone();
        let comparison = changed.committed.as_mut().unwrap();
        match which {
            0 => comparison.base = "new-base".into(),
            1 => comparison.head = "new-head".into(),
            2 => comparison.path = GitPath(b"other".to_vec()),
            3 => comparison.old_path = Some(GitPath(b"old".to_vec())),
            _ => comparison.status = "R".into(),
        }
        assert!(
            cache
                .get(View::All, &changed, &doc.document, &token)
                .is_none()
        );
    }
}

#[test]
fn cancellation_and_pruning_preserve_other_views() {
    let token = CancelToken::default();
    let mut cache = PreparedCache::default();
    let a = file(b"a");
    let b = file(b"b");
    let doc = prepared("same");
    cache.insert(View::All, &a, Arc::clone(&doc), &token);
    cache.insert(View::Staged, &a, Arc::clone(&doc), &token);
    cache.insert(View::All, &b, Arc::clone(&doc), &token);
    let cancelled = CancelToken::default();
    cancelled.cancel();
    let bytes = cache.retained_bytes();
    assert!(
        cache
            .get(View::All, &a, &doc.document, &cancelled)
            .is_none()
    );
    cache.insert(View::All, &file(b"c"), prepared("new"), &cancelled);
    cache.prune(View::All, &[], &cancelled);
    assert_eq!(cache.len(), 3);
    assert_eq!(cache.retained_bytes(), bytes);
    cache.prune(View::All, &[b], &token);
    assert_eq!(cache.len(), 2);
    assert!(cache.get(View::All, &a, &doc.document, &token).is_none());
    assert!(cache.get(View::Staged, &a, &doc.document, &token).is_some());
}

#[test]
fn reuse_requires_every_document_field_to_match() {
    let token = CancelToken::default();
    let key = file(b"a");
    let doc = prepared("same");
    for field in 0..8 {
        let mut cache = PreparedCache::default();
        cache.insert(View::All, &key, Arc::clone(&doc), &token);
        let mut fresh = doc.document.clone();
        let comparison = &mut fresh.comparisons[0];
        match field {
            0 => comparison.label = "Staged".into(),
            1 => comparison.patch = "new patch".into(),
            2 => comparison.summary = None,
            3 => comparison.rows[0].kind = RowKind::Addition,
            4 => comparison.rows[0].old_number = Some(2),
            5 => comparison.rows[0].new_number = Some(2),
            6 => comparison.rows[0].left = "new".into(),
            _ => fresh.comparisons.clear(),
        }
        assert!(cache.get(View::All, &key, &fresh, &token).is_none());
        assert_eq!(cache.len(), 0);
    }
}

#[test]
fn cancellation_after_accounting_does_not_admit_and_evictions_are_deferred() {
    let token = CancelToken::default();
    let mut cache = PreparedCache::with_limits(1, usize::MAX);
    let old = prepared("old");
    let weak = Arc::downgrade(&old);
    cache.insert(View::All, &file(b"a"), old, &token);
    let cancelled = CancelToken::default();
    let candidate = cache.candidate(View::All, &file(b"b"), prepared("new"), &cancelled);
    cancelled.cancel();
    let retired = cache.admit(candidate, &cancelled);
    assert_eq!(cache.len(), 1);
    assert!(weak.upgrade().is_some());
    drop(retired);
    let candidate = cache.candidate(View::All, &file(b"b"), prepared("new"), &token);
    let retired = cache.admit(candidate, &token);
    assert_eq!(cache.len(), 1);
    assert!(weak.upgrade().is_some());
    drop(retired);
    assert!(weak.upgrade().is_none());
}

#[test]
fn bytes_include_capacities_and_shared_storage_once() {
    let token = CancelToken::default();
    let a = file(b"a");
    let b = file(b"b");
    let small = prepared("small");
    let mut cache = PreparedCache::with_limits(2, usize::MAX);
    cache.insert(View::All, &a, Arc::clone(&small), &token);
    let one = cache.retained_bytes();
    cache.insert(View::All, &b, Arc::clone(&small), &token);
    assert_eq!(cache.retained_bytes() - one, key_bytes(&b));
    let mut bounded = PreparedCache::with_limits(2, one);
    bounded.insert(View::All, &a, Arc::clone(&small), &token);
    assert_eq!(bounded.len(), 1);
    bounded.insert(View::All, &b, prepared("other"), &token);
    assert_eq!(bounded.len(), 1);
    assert!(bounded.retained_bytes() <= one);
    let large = prepared(&"x".repeat(one * 2));
    bounded.insert(View::All, &a, large, &token);
    assert_eq!(bounded.len(), 1);
    let mut disabled = PreparedCache::with_limits(0, 0);
    disabled.insert(View::All, &a, small, &token);
    assert_eq!(disabled.len(), 0);
    assert_eq!(disabled.retained_bytes(), 0);
    let mut inflated = PreparedDocument::new(prepared("same").document.clone(), &token);
    let before = document_bytes(&inflated);
    inflated.document.comparisons[0].patch.reserve(10000);
    inflated.rows[0].left.reserve(1000);
    inflated.context.reserve(1000);
    assert!(document_bytes(&inflated) >= before + 10000);
    let full = PreparedDocument::from_text("a".into(), "line\n".into(), &token);
    let mut split = PreparedDocument::from_text("a".into(), "line\n".into(), &token);
    split.unified_lines = split.side_lines.iter().cloned().collect::<Vec<_>>().into();
    assert_eq!(
        document_bytes(&split) - document_bytes(&full),
        arc_slice_bytes(&full.side_lines)
    );
}

#[test]
fn checkpoint_capacity_is_counted_without_admitting_full_documents() {
    let token = CancelToken::default();
    let mut document = PreparedDocument::from_text("long.txt".into(), "é".repeat(5000), &token);
    document.long_line_checkpoints.reserve(100);
    let checkpoint_bytes = vector_bytes(&document.long_line_checkpoints);
    assert!(checkpoint_bytes > 0);
    let total = document_bytes(&document);
    document.long_line_checkpoints = Vec::new();
    assert_eq!(total - document_bytes(&document), checkpoint_bytes);
    let cache = PreparedCache::default();
    assert!(
        cache
            .candidate(View::All, &file(b"long.txt"), Arc::new(document), &token)
            .is_none()
    );
    assert!(prepared("diff preview").long_line_checkpoints.is_empty());
}
