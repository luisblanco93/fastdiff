//! Unpushed discovery for a branch without a usable upstream commit.
use super::{Repository, ReviewMetadata, parse_commit_id};
use crate::{
    CancelToken, Error, FileChange, GitPath, Result, View,
    process::{checked, git_command, run},
};
use std::ffi::OsString;

const FALLBACK_NOTICE: &str = "No upstream commit: Unpushed shows commits that are not on another local or remote-tracking branch.";
const WHOLE_HISTORY_UNPUSHED_NOTICE: &str =
    "No upstream commit or other branch: Unpushed shows the whole history of this branch.";
const WHOLE_HISTORY_ALL_NOTICE: &str = "No upstream commit or other branch: open the Unpushed view to review the whole history of this branch.";
const UNRELATED_NOTICE: &str =
    "Unpushed unavailable: no upstream commit, and HEAD shares no history with other branches.";
const TOO_MANY_NOTICE: &str =
    "Unpushed unavailable: no upstream commit, and too many commits are not on another branch.";
const FILE_LIST_LIMIT_NOTICE: &str = "Unpushed unavailable: no upstream commit, and the list of changed files exceeds the 16 MiB limit.";

const SHA1_EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const SHA256_EMPTY_TREE: &str = "6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321";
const HEADS_PREFIX: &[u8] = b"refs/heads/";
const REF_LIST_LIMIT: usize = 64 * 1024;
const BOUNDARY_OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const MAX_BOUNDARIES: usize = 64;

/// The commit the Unpushed comparison starts from when there is no upstream commit.
enum FallbackBase {
    Base(String),
    /// HEAD is already reachable from another local or remote-tracking branch.
    Contained,
    /// No other branch exists, so nothing has been pushed.
    NoOtherRefs,
    Unrelated,
    TooMany,
}

type Unpushed = (Vec<FileChange>, Option<String>);

impl Repository {
    /// `head_ref` is the `symbolic-ref` output of a branch HEAD; `head` is its commit.
    pub(super) fn unpushed_without_upstream(
        &self,
        head_ref: &[u8],
        head: &str,
        view: View,
        metadata: &mut ReviewMetadata,
        cancel: &CancelToken,
    ) -> Result<Unpushed> {
        let head_ref = head_ref.strip_suffix(b"\n").unwrap_or(head_ref);
        let (base, notice) = match self.fallback_base(head_ref, head, metadata, cancel)? {
            FallbackBase::Base(base) => (base, FALLBACK_NOTICE),
            FallbackBase::Contained => return Ok((Vec::new(), Some(FALLBACK_NOTICE.into()))),
            FallbackBase::NoOtherRefs if view == View::All => {
                return Ok((Vec::new(), Some(WHOLE_HISTORY_ALL_NOTICE.into())));
            }
            FallbackBase::NoOtherRefs => (empty_tree(head).into(), WHOLE_HISTORY_UNPUSHED_NOTICE),
            FallbackBase::Unrelated => return Ok((Vec::new(), Some(UNRELATED_NOTICE.into()))),
            FallbackBase::TooMany => return Ok((Vec::new(), Some(TOO_MANY_NOTICE.into()))),
        };
        let files = match self.committed_changes(&base, head, metadata, cancel) {
            Err(Error::OutputLimit) => {
                return Ok((Vec::new(), Some(FILE_LIST_LIMIT_NOTICE.into())));
            }
            result => result?,
        };
        Ok((files, Some(notice.into())))
    }

    fn fallback_base(
        &self,
        head_ref: &[u8],
        head: &str,
        metadata: &mut ReviewMetadata,
        cancel: &CancelToken,
    ) -> Result<FallbackBase> {
        if !self.has_other_refs(head_ref, cancel)? {
            return Ok(FallbackBase::NoOtherRefs);
        }
        // Like merge-base on the upstream path, the work overlaps the filter-override probe.
        self.prefetched(metadata, cancel, |_| {
            self.boundary_base(head_ref, head, cancel)
        })
    }

    /// Whether any local or remote-tracking branch other than the current one exists.
    fn has_other_refs(&self, head_ref: &[u8], cancel: &CancelToken) -> Result<bool> {
        let mut command = git_command(&self.root);
        command.args([
            "for-each-ref",
            "--count=2",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
        ]);
        let bytes = checked(run(command, REF_LIST_LIMIT, cancel)?)?;
        Ok(lines(&bytes).any(|name| name != head_ref))
    }

    fn boundary_base(
        &self,
        head_ref: &[u8],
        head: &str,
        cancel: &CancelToken,
    ) -> Result<FallbackBase> {
        let mut command = git_command(&self.root);
        command.args(["rev-list", "--boundary", head, "--not"]);
        // `--exclude` applies only to the next `--branches`, so remotes stay included.
        if let Some(name) = head_ref.strip_prefix(HEADS_PREFIX) {
            command.arg(exclude_argument(name)?);
        }
        command.args(["--branches", "--remotes"]);
        let bytes = match run(command, BOUNDARY_OUTPUT_LIMIT, cancel) {
            Err(Error::OutputLimit) => return Ok(FallbackBase::TooMany),
            result => checked(result?)?,
        };
        if lines(&bytes).next().is_none() {
            return Ok(FallbackBase::Contained);
        }
        let boundaries = lines(&bytes)
            .filter_map(|line| line.strip_prefix(b"-"))
            .map(parse_commit_id)
            .collect::<Result<Vec<_>>>()?;
        match boundaries.as_slice() {
            [] => Ok(FallbackBase::Unrelated),
            [base] => Ok(FallbackBase::Base(base.clone())),
            many if many.len() > MAX_BOUNDARIES => Ok(FallbackBase::TooMany),
            many => self.common_base(head, many, cancel),
        }
    }

    /// With several boundary commits, `merge-base` returns one best common ancestor of HEAD
    /// and a hypothetical merge of them. Independent boundaries yield one of them, not
    /// necessarily the newest; after merging a remote branch it is that branch's tip.
    fn common_base(
        &self,
        head: &str,
        boundaries: &[String],
        cancel: &CancelToken,
    ) -> Result<FallbackBase> {
        let mut command = git_command(&self.root);
        command.args(["merge-base", head]).args(boundaries);
        let output = run(command, REF_LIST_LIMIT, cancel)?;
        if output.status.code() == Some(1) {
            return Ok(FallbackBase::Unrelated);
        }
        Ok(FallbackBase::Base(parse_commit_id(&checked(output)?)?))
    }
}

fn lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
}

/// Build `--exclude=<name>` from raw bytes so non-UTF-8 branch names survive.
fn exclude_argument(name: &[u8]) -> Result<OsString> {
    Ok(GitPath([b"--exclude=".as_slice(), name].concat())
        .to_path_buf()?
        .into_os_string())
}

/// The empty tree has a fixed ID per object format; `head` is a validated full object ID.
fn empty_tree(head: &str) -> &'static str {
    if head.len() == 64 {
        SHA256_EMPTY_TREE
    } else {
        SHA1_EMPTY_TREE
    }
}
