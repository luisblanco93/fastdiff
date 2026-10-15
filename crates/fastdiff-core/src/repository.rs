use crate::{
    CancelToken, CommittedComparison, Comparison, DiffDocument, Error, FileChange, GitPath,
    MAX_DIFF_ROWS, MAX_PATCH_BYTES, MAX_SOURCE_BYTES, ReflogEntry, Result, View, patch,
    process::{GitOutput, checked, git_command, join_scoped, run},
};
use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    thread,
};

mod reflog;
mod reflog_snapshot;
mod unpushed;
mod watch;

#[derive(Debug, Clone)]
pub struct Repository {
    root: PathBuf,
    reflog_layout: Arc<Mutex<Option<reflog_snapshot::ReflogLayout>>>,
    // Refreshed by every status; a config change is picked up at the next refresh.
    filter_cache: Arc<Mutex<Option<Vec<OsString>>>>,
}

#[derive(Default)]
struct ReviewMetadata {
    filter_overrides: Option<Vec<OsString>>,
}

#[derive(Clone, Copy)]
struct WorkingTarget<'a> {
    label: &'a str,
    staged: bool,
}

struct BlobSizes {
    index: Option<u64>,
    head: Option<u64>,
}

enum Fetched {
    Done(Comparison),
    Patch { label: String, bytes: Vec<u8> },
}

impl Fetched {
    fn finish(self, rows: usize) -> Comparison {
        match self {
            Self::Done(comparison) => comparison,
            Self::Patch { label, bytes } => {
                patch::parse(&label, String::from_utf8_lossy(&bytes).into_owned(), rows)
            }
        }
    }
}

fn exceeds_source_limit(size: Option<u64>) -> bool {
    size.is_some_and(|size| size > MAX_SOURCE_BYTES as u64)
}

impl Repository {
    pub fn discover(path: impl AsRef<Path>) -> Result<Self> {
        Self::discover_with_cancel(path, &CancelToken::default())
    }

    pub fn discover_with_cancel(path: impl AsRef<Path>, cancel: &CancelToken) -> Result<Self> {
        cancel.check()?;
        let path = path.as_ref().canonicalize()?;
        let mut command = git_command(&path);
        command.args(["rev-parse", "--show-toplevel"]);
        let bytes = checked(run(command, 64 * 1024, cancel)?)?;
        let bytes = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
        let root = GitPath(bytes.to_vec()).to_path_buf()?.canonicalize()?;
        Ok(Self {
            root,
            reflog_layout: Arc::default(),
            filter_cache: Arc::default(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn current_branch(&self, cancel: &CancelToken) -> Result<Option<String>> {
        let output = self.symbolic_head(cancel)?;
        if output.status.code() == Some(1) {
            return Ok(None);
        }
        let bytes = checked(output)?;
        let name = String::from_utf8_lossy(&bytes);
        let name = name.trim_end_matches('\n');
        Ok(Some(
            name.strip_prefix("refs/heads/").unwrap_or(name).into(),
        ))
    }

    pub fn status(&self, view: View, cancel: &CancelToken) -> Result<Vec<FileChange>> {
        self.status_with_notice(view, cancel)
            .map(|(files, _)| files)
    }

    /// Missing upstream/history is an explanatory state, while other Git failures remain errors.
    pub fn status_with_notice(
        &self,
        view: View,
        cancel: &CancelToken,
    ) -> Result<(Vec<FileChange>, Option<String>)> {
        let mut metadata = ReviewMetadata::default();
        let (committed, notice) = if matches!(view, View::All | View::Unpushed) {
            self.unpushed(view, &mut metadata, cancel)?
        } else {
            (Vec::new(), None)
        };
        if view == View::Unpushed {
            self.remember_filter_overrides(&metadata);
            return Ok((committed, notice));
        }
        let mut command = self.review_command(&mut metadata, cancel)?;
        command.args(["-c", "status.renames=true"]);
        command.args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ]);
        let working = parse_status(&checked(run(command, 16 * 1024 * 1024, cancel)?)?, view)?;
        self.remember_filter_overrides(&metadata);
        Ok((merge_changes(working, committed), notice))
    }

    pub fn diff(
        &self,
        change: &FileChange,
        view: View,
        cancel: &CancelToken,
    ) -> Result<DiffDocument> {
        cancel.check()?;
        change.path.relative_path()?;
        if let Some(old) = &change.old_path {
            old.relative_path()?;
        }
        if view != View::Unpushed && (change.staged || change.unstaged) {
            self.validate_path(&change.path)?;
            if let Some(old) = &change.old_path {
                self.validate_path(old)?;
            }
        }
        let mut metadata = ReviewMetadata {
            filter_overrides: self.cached_filter_overrides(),
        };
        let mut comparisons = Vec::new();
        if matches!(view, View::All | View::Unpushed)
            && let Some(committed) = &change.committed
        {
            comparisons.push(self.committed_comparison(committed, &mut metadata, cancel)?);
        }
        if change.conflict && view != View::Unpushed {
            comparisons.push(patch::summary(
                "Conflict",
                "Merge conflict: resolve the index conflict before a normal comparison is available.",
            ));
            return Ok(DiffDocument { comparisons });
        }
        let mut targets = Vec::new();
        if change.staged && matches!(view, View::All | View::Staged) {
            targets.push(WorkingTarget {
                label: "Staged",
                staged: true,
            });
        }
        if change.unstaged && matches!(view, View::All | View::Unstaged) {
            targets.push(WorkingTarget {
                label: if change.untracked {
                    "Untracked"
                } else {
                    "Unstaged"
                },
                staged: false,
            });
        }
        // The Git processes for every comparison run together; each row budget is applied
        // afterwards, in display order, so the document matches the sequential result.
        for fetched in self.fetch_working(change, &targets, &mut metadata, cancel)? {
            let rows = remaining_rows(&comparisons);
            comparisons.push(fetched.finish(rows));
        }
        Ok(DiffDocument { comparisons })
    }

    fn unpushed(
        &self,
        view: View,
        metadata: &mut ReviewMetadata,
        cancel: &CancelToken,
    ) -> Result<(Vec<FileChange>, Option<String>)> {
        // HEAD, its commit, and the upstream commit do not depend on each other. Run them
        // together but consume the results in the original order, so a failure is reported
        // only where the sequential code would have reached it. The filter probe is not
        // started here, so detached and unborn results never run it.
        let (branch, head, upstream) = thread::scope(|scope| {
            let head = scope.spawn(|| self.commit_id("HEAD", cancel));
            let upstream = scope.spawn(|| self.commit_id("@{upstream}", cancel));
            (
                self.symbolic_head(cancel),
                join_scoped(head),
                join_scoped(upstream),
            )
        });
        let output = branch?;
        if output.status.code() == Some(1) {
            return Ok((
                Vec::new(),
                Some("Unpushed unavailable: HEAD is detached.".into()),
            ));
        }
        let head_ref = checked(output)?;
        let Some(head) = head? else {
            return Ok((
                Vec::new(),
                Some("Unpushed unavailable: HEAD has no commit.".into()),
            ));
        };
        let Some(upstream) = upstream? else {
            return self.unpushed_without_upstream(&head_ref, &head, view, metadata, cancel);
        };
        // Both refs resolved, so the probe overlaps merge-base; it is only wasted when the
        // histories are unrelated.
        let output = self.prefetched(metadata, cancel, |_| {
            let mut command = git_command(&self.root);
            command.args(["merge-base", &upstream, &head]);
            run(command, 64 * 1024, cancel)
        })?;
        if output.status.code() == Some(1) {
            return Ok((
                Vec::new(),
                Some(
                    "Unpushed unavailable: upstream and HEAD have no common local history.".into(),
                ),
            ));
        }
        let base = parse_commit_id(&checked(output)?)?;
        Ok((
            self.committed_changes(&base, &head, metadata, cancel)?,
            None,
        ))
    }

    fn committed_changes(
        &self,
        base: &str,
        head: &str,
        metadata: &mut ReviewMetadata,
        cancel: &CancelToken,
    ) -> Result<Vec<FileChange>> {
        let mut command = self.review_command(metadata, cancel)?;
        command.args([
            "diff",
            "--name-status",
            "-z",
            "--find-renames",
            "--no-ext-diff",
            "--no-textconv",
            base,
            head,
            "--",
        ]);
        let bytes = checked(run(command, 16 * 1024 * 1024, cancel)?)?;
        parse_committed(&bytes, base, head)
    }

    fn commit_id(&self, revision: &str, cancel: &CancelToken) -> Result<Option<String>> {
        let mut command = git_command(&self.root);
        command.env("LC_ALL", "C");
        command.args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{revision}^{{commit}}"),
        ]);
        let output = run(command, 64 * 1024, cancel)?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.code() == Some(1)
            || (revision == "@{upstream}"
                && output.status.code() == Some(128)
                && (stderr.contains("no upstream configured")
                    || stderr.contains("does not point to a branch")
                    || stderr.contains("upstream branch")
                    || stderr.contains("no such branch")))
        {
            return Ok(None);
        }
        Ok(Some(parse_commit_id(&checked(output)?)?))
    }

    fn committed_comparison(
        &self,
        committed: &CommittedComparison,
        metadata: &mut ReviewMetadata,
        cancel: &CancelToken,
    ) -> Result<Comparison> {
        // IDs cross the helper boundary: accept only full object IDs, never Git revision syntax.
        for id in [&committed.base, &committed.head] {
            validate_commit_id(id)?;
        }
        committed.path.relative_path()?;
        if let Some(old) = &committed.old_path {
            old.relative_path()?;
        }
        let (oversized, patch) = self.speculative(
            metadata,
            cancel,
            || -> Result<bool> {
                let base_path = committed.old_path.as_ref().unwrap_or(&committed.path);
                let (base, head) = thread::scope(|scope| {
                    let base = scope.spawn(|| {
                        self.blob_size(format!("{}:", committed.base).as_bytes(), base_path, cancel)
                    });
                    let head = self.blob_size(
                        format!("{}:", committed.head).as_bytes(),
                        &committed.path,
                        cancel,
                    );
                    (join_scoped(base), head)
                });
                Ok(exceeds_source_limit(base?) || exceeds_source_limit(head?))
            },
            |overrides| -> Result<GitOutput> {
                let mut command = self.diff_command(overrides);
                command.args([
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-color",
                    "--unified=30",
                    "--find-renames",
                    "--no-relative",
                    &committed.base,
                    &committed.head,
                    "--",
                ]);
                command.arg(committed.path.relative_path()?);
                if let Some(old) = &committed.old_path {
                    command.arg(old.relative_path()?);
                }
                run(command, MAX_PATCH_BYTES, cancel)
            },
        );
        if oversized? {
            return Ok(patch::summary(
                "Unpushed",
                "File exceeds the 10 MiB automatic text loading limit.",
            ));
        }
        let bytes = match patch? {
            Err(Error::OutputLimit) => {
                return Ok(patch::summary(
                    "Unpushed",
                    "Patch exceeds the 2 MiB automatic preview limit.",
                ));
            }
            result => checked(result?)?,
        };
        Ok(patch::parse(
            "Unpushed",
            String::from_utf8_lossy(&bytes).into_owned(),
            MAX_DIFF_ROWS,
        ))
    }

    /// Fetch the staged and unstaged comparisons of one file with a single round of Git
    /// processes: blob sizes and every diff start together. Results are interpreted in
    /// display order so oversize summaries and errors match the sequential behavior.
    fn fetch_working(
        &self,
        change: &FileChange,
        targets: &[WorkingTarget<'_>],
        metadata: &mut ReviewMetadata,
        cancel: &CancelToken,
    ) -> Result<Vec<Fetched>> {
        let mut fetched: Vec<Option<Fetched>> = targets.iter().map(|_| None).collect();
        let mut pending = Vec::new();
        for (index, target) in targets.iter().enumerate() {
            if self.oversized_on_disk(change, target.staged, cancel)? {
                fetched[index] = Some(Fetched::Done(patch::summary(
                    target.label,
                    "File exceeds the 10 MiB automatic text loading limit.",
                )));
            } else {
                pending.push(index);
            }
        }
        if !pending.is_empty() {
            let needs_index = |target: &WorkingTarget<'_>| !change.untracked || target.staged;
            let need_index = pending.iter().any(|&index| needs_index(&targets[index]));
            let need_head = pending.iter().any(|&index| targets[index].staged);
            let (sizes, outputs) = self.speculative(
                metadata,
                cancel,
                || self.blob_sizes(change, need_index, need_head, cancel),
                |overrides| {
                    thread::scope(|scope| {
                        let handles: Vec<_> = pending
                            .iter()
                            .map(|&index| {
                                let staged = targets[index].staged;
                                scope.spawn(move || {
                                    self.working_diff(change, staged, overrides, cancel)
                                })
                            })
                            .collect();
                        handles.into_iter().map(join_scoped).collect::<Vec<_>>()
                    })
                },
            );
            let sizes = sizes?;
            // A probe failure is reported only where a diff would have needed it, so an
            // oversize summary still wins, as in the sequential code.
            let mut outputs = match outputs {
                Ok(outputs) => outputs
                    .into_iter()
                    .map(Some)
                    .collect::<Vec<_>>()
                    .into_iter(),
                Err(error) => {
                    let all_oversized = pending.iter().all(|&index| {
                        let target = targets[index];
                        (needs_index(&target) && exceeds_source_limit(sizes.index))
                            || (target.staged && exceeds_source_limit(sizes.head))
                    });
                    if !all_oversized {
                        return Err(error);
                    }
                    Vec::new().into_iter()
                }
            };
            for &index in &pending {
                let target = targets[index];
                let output = outputs.next().flatten();
                let oversized = (needs_index(&target) && exceeds_source_limit(sizes.index))
                    || (target.staged && exceeds_source_limit(sizes.head));
                if oversized {
                    fetched[index] = Some(Fetched::Done(patch::summary(
                        target.label,
                        "File exceeds the 10 MiB automatic text loading limit.",
                    )));
                    continue;
                }
                let output = output
                    .ok_or_else(|| Error::InvalidData("missing speculative diff result".into()))?;
                let output = match output {
                    Err(Error::OutputLimit) => {
                        fetched[index] = Some(Fetched::Done(patch::summary(
                            target.label,
                            "Patch exceeds the 2 MiB automatic preview limit.",
                        )));
                        continue;
                    }
                    other => other?,
                };
                let bytes = if change.untracked && !target.staged && output.status.code() == Some(1)
                {
                    output.stdout
                } else {
                    checked(output)?
                };
                fetched[index] = Some(Fetched::Patch {
                    label: target.label.into(),
                    bytes,
                });
            }
        }
        fetched
            .into_iter()
            .map(|entry| entry.ok_or_else(|| Error::InvalidData("missing comparison".into())))
            .collect()
    }

    fn working_diff(
        &self,
        change: &FileChange,
        staged: bool,
        overrides: &[OsString],
        cancel: &CancelToken,
    ) -> Result<GitOutput> {
        let mut command = self.diff_command(overrides);
        command.args([
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--unified=30",
            "--find-renames",
            "--no-relative",
        ]);
        if change.untracked && !staged {
            command.args(["--no-index", "--"]);
            command.arg(if cfg!(windows) { "NUL" } else { "/dev/null" });
            command.arg(change.path.relative_path()?);
        } else {
            if staged {
                command.arg("--cached");
            }
            command.arg("--").arg(change.path.relative_path()?);
            if let Some(old) = &change.old_path {
                command.arg(old.relative_path()?);
            }
        }
        run(command, MAX_PATCH_BYTES, cancel)
    }

    /// Index and HEAD blob sizes are independent, so they are read together.
    fn blob_sizes(
        &self,
        change: &FileChange,
        need_index: bool,
        need_head: bool,
        cancel: &CancelToken,
    ) -> Result<BlobSizes> {
        thread::scope(|scope| {
            let head = need_head.then(|| {
                scope.spawn(|| {
                    let old = change.old_path.as_ref().unwrap_or(&change.path);
                    self.blob_size(b"HEAD:", old, cancel)
                })
            });
            let index = if need_index {
                self.blob_size(b":", &change.path, cancel)
            } else {
                Ok(None)
            };
            let head = head.map(join_scoped).transpose()?.flatten();
            Ok(BlobSizes {
                index: index?,
                head,
            })
        })
    }

    /// Run `diff` beside `check`. With known filter overrides both start together; otherwise
    /// the probe overlaps `check` and `diff` follows it. Callers consume `check` first and
    /// `diff` second, so errors surface in the order the sequential code reported them.
    fn speculative<S, D: Send>(
        &self,
        metadata: &mut ReviewMetadata,
        cancel: &CancelToken,
        check: impl FnOnce() -> S,
        diff: impl FnOnce(&[OsString]) -> D + Send,
    ) -> (S, Result<D>) {
        if let Some(overrides) = metadata.filter_overrides.clone() {
            return thread::scope(|scope| {
                let diff = scope.spawn(|| diff(&overrides));
                let checked = check();
                (checked, Ok(join_scoped(diff)))
            });
        }
        let (checked, probed) = thread::scope(|scope| {
            let probe = scope.spawn(|| self.filter_overrides(cancel));
            (check(), join_scoped(probe))
        });
        match probed {
            Ok(overrides) => {
                let result = diff(&overrides);
                metadata.filter_overrides = Some(overrides);
                (checked, Ok(result))
            }
            Err(error) => (checked, Err(error)),
        }
    }

    fn cached_filter_overrides(&self) -> Option<Vec<OsString>> {
        self.filter_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Status refreshes run the probe anyway; keeping its result lets later diffs skip it.
    fn remember_filter_overrides(&self, metadata: &ReviewMetadata) {
        if let Some(overrides) = &metadata.filter_overrides {
            *self
                .filter_cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(overrides.clone());
        }
    }

    fn diff_command(&self, overrides: &[OsString]) -> Command {
        let mut command = self.command_with_overrides(overrides);
        // An oversized blob that reaches a speculative diff is treated as binary, not loaded.
        command
            .arg("-c")
            .arg(format!("core.bigFileThreshold={MAX_SOURCE_BYTES}"));
        command.arg("diff");
        command
    }

    fn command_with_overrides(&self, overrides: &[OsString]) -> Command {
        let mut command = git_command(&self.root);
        for value in overrides {
            command.arg("-c").arg(value);
        }
        command
    }

    /// The working-tree size check needs no Git process, so it runs before any probe starts.
    fn oversized_on_disk(
        &self,
        change: &FileChange,
        staged: bool,
        cancel: &CancelToken,
    ) -> Result<bool> {
        cancel.check()?;
        if staged {
            return Ok(false);
        }
        let path = self.root.join(change.path.relative_path()?);
        match fs::symlink_metadata(path) {
            Ok(meta) => Ok(meta.is_file() && meta.len() > MAX_SOURCE_BYTES as u64),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn blob_size(
        &self,
        prefix: &[u8],
        path: &GitPath,
        cancel: &CancelToken,
    ) -> Result<Option<u64>> {
        let spec = GitPath([prefix, path.0.as_slice()].concat())
            .to_path_buf()?
            .into_os_string();
        let mut command = git_command(&self.root);
        command.args(["cat-file", "-s"]).arg(spec);
        let output = run(command, 64 * 1024, cancel)?;
        // Missing index paths, deleted files, and unborn HEAD are normal.
        if !output.status.success() {
            return Ok(None);
        }
        let text = std::str::from_utf8(&output.stdout)
            .map_err(|_| Error::InvalidData("invalid blob size".into()))?;
        Ok(Some(text.trim().parse().map_err(|_| {
            Error::InvalidData("invalid blob size".into())
        })?))
    }

    fn validate_path(&self, name: &GitPath) -> Result<()> {
        let full = self.root.join(name.relative_path()?);
        let mut parent = full.parent().ok_or(Error::InvalidPath)?;
        loop {
            match parent.canonicalize() {
                Ok(resolved) if resolved.starts_with(&self.root) => return Ok(()),
                Ok(_) => return Err(Error::InvalidPath),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    parent = parent.parent().ok_or(Error::InvalidPath)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// Run `work` while the filter-override probe runs concurrently. The probe result is
    /// cached only on success, so a probe failure still surfaces in `review_command`.
    fn prefetched<T>(
        &self,
        metadata: &mut ReviewMetadata,
        cancel: &CancelToken,
        work: impl FnOnce(&mut ReviewMetadata) -> T,
    ) -> T {
        if metadata.filter_overrides.is_some() {
            return work(metadata);
        }
        let (result, overrides) = thread::scope(|scope| {
            let probe = scope.spawn(|| self.filter_overrides(cancel));
            (work(metadata), join_scoped(probe))
        });
        if let Ok(overrides) = overrides {
            metadata.filter_overrides = Some(overrides);
        }
        result
    }

    fn symbolic_head(&self, cancel: &CancelToken) -> Result<GitOutput> {
        let mut command = git_command(&self.root);
        command.args(["symbolic-ref", "--quiet", "HEAD"]);
        run(command, 64 * 1024, cancel)
    }

    fn filter_overrides(&self, cancel: &CancelToken) -> Result<Vec<OsString>> {
        let mut config = git_command(&self.root);
        config.args([
            "config",
            "--null",
            "--name-only",
            "--get-regexp",
            r"^filter\..*\.(clean|process|required)$",
        ]);
        let output = run(config, 1024 * 1024, cancel)?;
        let keys = if output.status.code() == Some(1) {
            Vec::new()
        } else {
            checked(output)?
        };
        let mut overrides = Vec::new();
        for key in keys.split(|byte| *byte == 0).filter(|key| !key.is_empty()) {
            let dot = key
                .iter()
                .rposition(|byte| *byte == b'.')
                .ok_or_else(|| Error::InvalidData("invalid filter key".into()))?;
            for suffix in [b".clean=".as_slice(), b".process=", b".required=false"] {
                let value: OsString = GitPath([&key[..dot], suffix].concat())
                    .to_path_buf()?
                    .into();
                overrides.push(value);
            }
        }
        Ok(overrides)
    }

    fn review_command(
        &self,
        metadata: &mut ReviewMetadata,
        cancel: &CancelToken,
    ) -> Result<Command> {
        cancel.check()?;
        if metadata.filter_overrides.is_none() {
            metadata.filter_overrides = Some(self.filter_overrides(cancel)?);
        }
        let command =
            self.command_with_overrides(metadata.filter_overrides.as_deref().unwrap_or_default());
        Ok(command)
    }
}

fn parse_reflog_entries(bytes: &[u8], cancel: &CancelToken) -> Result<Vec<ReflogEntry>> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let bytes = bytes
        .strip_suffix(&[0])
        .ok_or_else(|| Error::InvalidData("reflog metadata lacks terminating NUL".into()))?;
    let fields: Vec<_> = bytes.split(|byte| *byte == 0).collect();
    if fields.len() % 5 != 0 {
        return Err(Error::InvalidData("incomplete reflog metadata".into()));
    }
    fields
        .chunks_exact(5)
        .map(|fields| {
            cancel.check()?;
            let id = parse_commit_id(fields[0])?;
            let short_id = std::str::from_utf8(fields[1])
                .map_err(|_| Error::InvalidData("invalid short commit ID".into()))?;
            if short_id.is_empty() || !id.starts_with(short_id) {
                return Err(Error::InvalidData("invalid short commit ID".into()));
            }
            Ok(ReflogEntry {
                id,
                short_id: short_id.into(),
                decorations: String::from_utf8_lossy(fields[2]).into_owned(),
                selector: String::from_utf8_lossy(fields[3]).into_owned(),
                action: String::from_utf8_lossy(fields[4]).into_owned(),
            })
        })
        .collect()
}

fn validate_commit_id(id: &str) -> Result<()> {
    if !matches!(id.len(), 40 | 64) || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::InvalidData("invalid pinned commit ID".into()));
    }
    Ok(())
}

fn parse_commit_id(bytes: &[u8]) -> Result<String> {
    let id = std::str::from_utf8(bytes)
        .map_err(|_| Error::InvalidData("invalid commit ID".into()))?
        .trim();
    validate_commit_id(id)?;
    Ok(id.into())
}

fn remaining_rows(comparisons: &[Comparison]) -> usize {
    MAX_DIFF_ROWS.saturating_sub(
        comparisons
            .iter()
            .flat_map(|comparison| &comparison.rows)
            .map(patch::rendered_rows)
            .sum(),
    )
}

fn merge_changes(working: Vec<FileChange>, committed: Vec<FileChange>) -> Vec<FileChange> {
    let mut files: Vec<FileChange> = Vec::new();
    let mut indices: HashMap<GitPath, usize> = HashMap::new();
    for change in working.into_iter().chain(committed) {
        if let Some(&index) = indices.get(&change.path) {
            let existing = &files[index];
            let staged_metadata = if change.staged && !existing.staged {
                &change
            } else {
                existing
            };
            files[index] = FileChange {
                path: existing.path.clone(),
                old_path: staged_metadata.old_path.clone(),
                staged: existing.staged || change.staged,
                unstaged: existing.unstaged || change.unstaged,
                untracked: existing.untracked || change.untracked,
                conflict: existing.conflict || change.conflict,
                status: staged_metadata.status.clone(),
                committed: change.committed.or_else(|| existing.committed.clone()),
            };
        } else {
            indices.insert(change.path.clone(), files.len());
            files.push(change);
        }
    }
    files
}

fn parse_committed(bytes: &[u8], base: &str, head: &str) -> Result<Vec<FileChange>> {
    if !bytes.is_empty() && bytes.last() != Some(&0) {
        return Err(Error::InvalidData(
            "committed status lacks terminating NUL".into(),
        ));
    }
    let mut entries = bytes.split(|byte| *byte == 0);
    let mut files = Vec::new();
    while let Some(status) = entries.next().filter(|entry| !entry.is_empty()) {
        if !b"ACDMRTUXB".contains(&status[0]) {
            return Err(Error::InvalidData("invalid committed status".into()));
        }
        let first = entries
            .next()
            .filter(|path| !path.is_empty())
            .ok_or_else(|| Error::InvalidData("committed status lacks path".into()))?;
        let (old_path, path) = if matches!(status[0], b'R' | b'C') {
            let destination = entries
                .next()
                .filter(|path| !path.is_empty())
                .ok_or_else(|| Error::InvalidData("committed rename lacks destination".into()))?;
            (Some(GitPath(first.to_vec())), GitPath(destination.to_vec()))
        } else {
            (None, GitPath(first.to_vec()))
        };
        path.relative_path()?;
        if let Some(old) = &old_path {
            old.relative_path()?;
        }
        let status = String::from_utf8_lossy(status).into_owned();
        let committed = CommittedComparison {
            base: base.into(),
            head: head.into(),
            path: path.clone(),
            old_path: old_path.clone(),
            status: status.clone(),
        };
        files.push(FileChange {
            path,
            old_path,
            staged: false,
            unstaged: false,
            untracked: false,
            conflict: false,
            status,
            committed: Some(committed),
        });
    }
    Ok(files)
}

fn parse_status(bytes: &[u8], view: View) -> Result<Vec<FileChange>> {
    if !bytes.is_empty() && bytes.last() != Some(&0) {
        return Err(Error::InvalidData("status lacks terminating NUL".into()));
    }
    let mut entries = bytes.split(|b| *b == 0);
    let mut files = Vec::new();
    while let Some(entry) = entries.next() {
        if entry.is_empty() {
            continue;
        }
        if entry.len() < 4 || entry[2] != b' ' {
            return Err(Error::InvalidData("malformed NUL-delimited status".into()));
        }
        let x = entry[0];
        let y = entry[1];
        if ![x, y].iter().all(|code| b" MADRCUT?".contains(code)) {
            return Err(Error::InvalidData("unrecognized status code".into()));
        }
        let untracked = x == b'?' && y == b'?';
        let conflict =
            x == b'U' || y == b'U' || (x == b'A' && y == b'A') || (x == b'D' && y == b'D');
        let old_path = if x == b'R' || x == b'C' || y == b'R' || y == b'C' {
            Some(GitPath(
                entries
                    .next()
                    .filter(|old| !old.is_empty())
                    .ok_or_else(|| Error::InvalidData("rename lacks old path".into()))?
                    .to_vec(),
            ))
        } else {
            None
        };
        let staged = !untracked && x != b' ';
        let unstaged = untracked || y != b' ';
        if view == View::Staged && !staged || view == View::Unstaged && !unstaged {
            continue;
        }
        let path = GitPath(entry[3..].to_vec());
        path.relative_path()?;
        if let Some(old) = &old_path {
            old.relative_path()?;
        }
        files.push(FileChange {
            path,
            old_path,
            staged,
            unstaged,
            untracked,
            conflict,
            status: String::from_utf8_lossy(&entry[..2]).into_owned(),
            committed: None,
        });
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_changes_handles_empty_and_one_sided_inputs() {
        assert!(merge_changes(Vec::new(), Vec::new()).is_empty());
        let working = parse_status(b" M z\0R  a\0old\0", View::All).unwrap();
        assert_eq!(merge_changes(working.clone(), Vec::new()), working);
        let committed =
            parse_committed(b"M\0z\0R100\0old\0a\0", &"1".repeat(40), &"2".repeat(40)).unwrap();
        assert_eq!(merge_changes(Vec::new(), committed.clone()), committed);
    }

    #[test]
    fn merge_changes_preserves_first_occurrence_order_across_inputs() {
        let working = parse_status(b" M z\0 M a\0 M z\0", View::All).unwrap();
        let committed = parse_committed(
            b"M\0a\0M\0b\0M\0z\0M\0c\0M\0b\0",
            &"1".repeat(40),
            &"2".repeat(40),
        )
        .unwrap();
        let expected = vec![
            FileChange {
                committed: committed[2].committed.clone(),
                ..working[0].clone()
            },
            FileChange {
                committed: committed[0].committed.clone(),
                ..working[1].clone()
            },
            committed[1].clone(),
            committed[3].clone(),
        ];
        assert_eq!(merge_changes(working, committed), expected);
    }

    #[test]
    fn merge_changes_keeps_first_staged_metadata_and_combines_flags() {
        let changes = parse_status(
            b"?? destination\0R  destination\0first-old\0UU destination\0",
            View::All,
        )
        .unwrap();
        let expected = FileChange {
            unstaged: true,
            untracked: true,
            ..changes[1].clone()
        };
        assert_eq!(
            merge_changes(vec![changes[0].clone()], vec![changes[1].clone()]),
            vec![expected.clone()]
        );
        assert_eq!(
            merge_changes(changes[..2].to_vec(), vec![changes[2].clone()]),
            vec![FileChange {
                conflict: true,
                ..expected
            }]
        );
    }

    #[test]
    fn merge_changes_replaces_committed_comparison_and_retains_it_on_none() {
        let first = parse_committed(
            b"R100\0old-a\0destination\0",
            &"1".repeat(40),
            &"2".repeat(40),
        )
        .unwrap();
        let later = parse_committed(
            b"R100\0old-b\0destination\0",
            &"3".repeat(40),
            &"4".repeat(40),
        )
        .unwrap();
        let expected = FileChange {
            committed: later[0].committed.clone(),
            ..first[0].clone()
        };
        assert_eq!(
            merge_changes(first.clone(), later.clone()),
            vec![expected.clone()]
        );
        let without_committed = parse_status(b" M destination\0", View::All).unwrap();
        assert_eq!(
            merge_changes(first, [later, without_committed].concat()),
            vec![FileChange {
                unstaged: true,
                ..expected
            }]
        );
    }

    #[test]
    fn merge_changes_distinguishes_byte_paths_with_identical_lossy_display() {
        let template = parse_status(b" M file\0", View::All).unwrap().remove(0);
        let first = FileChange {
            path: GitPath(b"file-\xff".to_vec()),
            ..template.clone()
        };
        let second = FileChange {
            path: GitPath(b"file-\xfe".to_vec()),
            ..template
        };
        assert_ne!(first.path, second.path);
        assert_eq!(first.path.display(), second.path.display());
        assert_eq!(
            merge_changes(
                vec![first.clone()],
                vec![second.clone(), first.clone(), second.clone()]
            ),
            vec![first, second]
        );
    }

    #[test]
    fn reflog_metadata_parser_rejects_truncation_and_invalid_ids() {
        let cancel = CancelToken::default();
        let id = "a".repeat(40);
        let valid = format!("{id}\0aaaaaaa\0topic\0HEAD@{{0}}\0commit: λ\t subject\0");
        assert_eq!(
            parse_reflog_entries(valid.as_bytes(), &cancel).unwrap()[0].action,
            "commit: λ\t subject"
        );
        for invalid in [
            format!("{id}\0aaaaaaa\0topic\0HEAD@{{0}}\0subject"),
            format!("{id}\0aaaaaaa\0"),
            format!("{id}\0bbbbbbb\0topic\0HEAD@{{0}}\0subject\0"),
            "invalid\0aaaaaaa\0topic\0HEAD@{0}\0subject\0".into(),
        ] {
            assert!(parse_reflog_entries(invalid.as_bytes(), &cancel).is_err());
        }
        cancel.cancel();
        assert!(matches!(
            parse_reflog_entries(valid.as_bytes(), &cancel),
            Err(Error::Cancelled)
        ));
    }

    #[test]
    fn committed_records_preserve_rename_order_and_merge_independently() {
        let base = "1".repeat(40);
        let head = "2".repeat(40);
        let committed = parse_committed(
            b"R100\0committed-old\0destination\0D\0deleted\0",
            &base,
            &head,
        )
        .unwrap();
        let working = parse_status(b"R  destination\0working-old\0", View::All).unwrap();
        let merged = merge_changes(working, committed);
        assert_eq!(merged.len(), 2);
        assert_eq!(
            merged[0].old_path.as_ref().unwrap().display(),
            "working-old"
        );
        assert_eq!(
            merged[0]
                .committed
                .as_ref()
                .unwrap()
                .old_path
                .as_ref()
                .unwrap()
                .display(),
            "committed-old"
        );
        for invalid in [
            b"M\0missing".as_slice(),
            b"R100\0old\0",
            b"Z\0file\0",
            b"M\0../escape\0",
        ] {
            assert!(parse_committed(invalid, &base, &head).is_err());
        }
        assert!(validate_commit_id("HEAD").is_err());
        assert!(validate_commit_id(&"z".repeat(40)).is_err());
    }

    #[test]
    fn parses_status_types_rename_order_and_conflicts() {
        let bytes = b" M working\0A  added\0R  new\0old\0?? untracked\0UU conflict\0 T type\0";
        let all = parse_status(bytes, View::All).unwrap();
        assert_eq!(all.len(), 6);
        assert_eq!(all[2].path.0, b"new");
        assert_eq!(all[2].old_path.as_ref().unwrap().0, b"old");
        assert!(all[4].conflict);
        assert_eq!(parse_status(bytes, View::Staged).unwrap().len(), 3);
        assert_eq!(parse_status(bytes, View::Unstaged).unwrap().len(), 4);
        for code in ["AA", "DD", "AU", "UA", "DU", "UD", "UU"] {
            let entry = format!("{code} file\0");
            assert!(parse_status(entry.as_bytes(), View::All).unwrap()[0].conflict);
        }
    }

    #[test]
    fn rejects_truncated_and_malformed_status() {
        for bytes in [
            b" M file".as_slice(),
            b"M file\0",
            b"R  new\0",
            b"Z  file\0",
            b"?? \0",
        ] {
            assert!(parse_status(bytes, View::All).is_err(), "{bytes:?}");
        }
        assert!(parse_status(b"", View::All).unwrap().is_empty());
    }
}
