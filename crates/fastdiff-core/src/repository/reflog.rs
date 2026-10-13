use super::{Repository, parse_reflog_entries};
use crate::{
    CancelToken, Error, REFLOG_PAGE_SIZE, ReflogEntry, ReflogList, ReflogRequest, Result,
    process::{checked, git_command, run},
};

impl Repository {
    pub fn reflog(&self, cancel: &CancelToken) -> Result<ReflogList> {
        self.reflog_page(&ReflogRequest::default(), cancel)
    }

    pub fn reflog_page(&self, request: &ReflogRequest, cancel: &CancelToken) -> Result<ReflogList> {
        self.reflog_page_with_snapshot(request, cancel, || self.reflog_snapshot(cancel))
    }

    fn reflog_page_with_snapshot(
        &self,
        request: &ReflogRequest,
        cancel: &CancelToken,
        mut snapshot: impl FnMut() -> Result<Option<u64>>,
    ) -> Result<ReflogList> {
        let mut reset = false;
        for _ in 0..2 {
            cancel.check()?;
            let before = snapshot()?;
            let continuation = !reset
                && request.start > 0
                && before.is_some()
                && request.snapshot == before
                && request.boundary.is_some();
            // shortcut: unavailable metadata rereads the loaded prefix; retain the output cap.
            let limit = if before.is_none() && !reset {
                request
                    .start
                    .checked_add(REFLOG_PAGE_SIZE)
                    .ok_or(Error::OutputLimit)?
            } else {
                REFLOG_PAGE_SIZE
            };
            let start = if continuation { request.start } else { 0 };
            let (branch, mut entries) = self.read_reflog(
                start.saturating_sub(1),
                limit
                    .checked_add(1 + usize::from(continuation))
                    .ok_or(Error::OutputLimit)?,
                cancel,
            )?;
            let after = snapshot()?;
            if before != after {
                reset = true;
                continue;
            }
            if continuation {
                if entries.first() != request.boundary.as_ref() {
                    reset = true;
                    continue;
                }
                entries.remove(0);
            }
            let has_more = entries.len() > limit;
            entries.truncate(limit);
            return Ok(ReflogList {
                notice: entries.is_empty().then(|| "No reflog entries".into()),
                branch,
                entries,
                snapshot: after,
                start,
                has_more,
            });
        }
        Err(Error::Git(
            "Reflog changed while loading. Retry to load the newest history.".into(),
        ))
    }

    fn read_reflog(
        &self,
        skip: usize,
        count: usize,
        cancel: &CancelToken,
    ) -> Result<(Option<String>, Vec<ReflogEntry>)> {
        let branch = self.current_branch(cancel)?;
        if self.commit_id("HEAD", cancel)?.is_none() {
            return Ok((branch, Vec::new()));
        }
        let mut command = git_command(&self.root);
        command.args([
            "reflog",
            "show",
            "--no-patch",
            "--no-show-signature",
            "--decorate=short",
            "--encoding=UTF-8",
            "-z",
            "--format=%H%x00%h%x00%D%x00%gD%x00%gs",
        ]);
        command
            .arg(format!("--skip={skip}"))
            .arg(format!("--max-count={count}"));
        command.args(["HEAD", "--"]);
        let bytes = checked(run(command, 16 * 1024 * 1024, cancel)?)?;
        Ok((branch, parse_reflog_entries(&bytes, cancel)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, process::Command};

    fn repository() -> (tempfile::TempDir, Repository) {
        let directory = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.invalid"],
            vec!["commit", "--allow-empty", "-m", "base"],
        ] {
            assert!(
                Command::new("git")
                    .current_dir(directory.path())
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        let repository = Repository::discover(directory.path()).unwrap();
        (directory, repository)
    }

    #[test]
    fn reflog_retries_one_snapshot_race_and_rejects_continued_races() {
        let (_dir, repository) = repository();
        let calls = Cell::new(0);
        let list = repository
            .reflog_page_with_snapshot(&ReflogRequest::default(), &CancelToken::default(), || {
                let call = calls.get();
                calls.set(call + 1);
                Ok(Some(if call == 0 { 1 } else { 2 }))
            })
            .unwrap();
        assert_eq!(calls.get(), 4);
        assert_eq!(list.snapshot, Some(2));
        let error = repository
            .reflog_page_with_snapshot(&ReflogRequest::default(), &CancelToken::default(), || {
                calls.set(calls.get() + 1);
                Ok(Some(calls.get()))
            })
            .unwrap_err();
        assert!(error.to_string().contains("changed while loading"));
    }

    #[test]
    fn reflog_unavailable_snapshot_replaces_prefix_and_preserves_older_access() {
        let (dir, repository) = repository();
        for _ in 0..34 {
            assert!(
                Command::new("git")
                    .current_dir(dir.path())
                    .args(["reset", "--soft", "HEAD"])
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        let list = repository
            .reflog_page_with_snapshot(
                &ReflogRequest {
                    start: 15,
                    ..ReflogRequest::default()
                },
                &CancelToken::default(),
                || Ok(None),
            )
            .unwrap();
        assert_eq!(list.start, 0);
        assert_eq!(list.entries.len(), 30);
        assert!(list.has_more && list.snapshot.is_none());
        let end = repository
            .reflog_page_with_snapshot(
                &ReflogRequest {
                    start: 30,
                    ..ReflogRequest::default()
                },
                &CancelToken::default(),
                || Ok(None),
            )
            .unwrap();
        assert_eq!(end.entries.len(), 35);
        assert!(!end.has_more);
    }
}
