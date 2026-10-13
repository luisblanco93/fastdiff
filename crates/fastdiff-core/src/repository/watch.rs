use super::Repository;
use crate::{
    CancelToken, Error, GitPath, Result,
    process::{checked, git_command, join_scoped, run},
};
use std::{collections::BTreeSet, path::PathBuf, thread};

const GIT_PATH_NAMES: [&str; 8] = [
    "HEAD",
    "index",
    "logs",
    "refs",
    "packed-refs",
    "config",
    "config.worktree",
    "info/exclude",
];

/// Split newline-terminated `rev-parse` output into exactly `expected` non-empty
/// lines. A path containing a newline adds lines, so any other count is ambiguous.
fn split_batched_paths(bytes: &[u8], expected: usize) -> Option<Vec<&[u8]>> {
    let lines: Vec<_> = bytes
        .strip_suffix(b"\n")?
        .split(|byte| *byte == b'\n')
        .collect();
    (lines.len() == expected && lines.iter().all(|line| !line.is_empty())).then_some(lines)
}

impl Repository {
    pub fn paths_are_ignored(&self, paths: &[PathBuf], cancel: &CancelToken) -> Result<bool> {
        cancel.check()?;
        if paths.is_empty() {
            return Ok(false);
        }
        let mut command = git_command(self.root());
        // check-ignore rejects the literal pathspec mode used by other Git operations.
        command
            .args(["--no-literal-pathspecs", "check-ignore", "--"])
            .args(paths);
        let output = run(command, 64 * 1024, cancel)?;
        if output.status.code() == Some(1) {
            return Ok(false);
        }
        let bytes = checked(output)?;
        // Git C-quotes embedded newlines, so each matching path occupies one record.
        Ok(bytes.ends_with(b"\n")
            && bytes.iter().filter(|&&byte| byte == b'\n').count() == paths.len())
    }

    pub fn watch_paths(&self, cancel: &CancelToken) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
        // The three query groups are independent, so run them concurrently: each Git
        // process costs tens of milliseconds on Windows and they used to run in sequence.
        let (metadata, configurations, excludes) = thread::scope(|scope| {
            let configurations =
                scope.spawn(|| super::reflog_snapshot::config_paths(self.root(), cancel));
            let excludes = scope.spawn(|| self.excludes_file_path(cancel));
            let metadata = self.metadata_paths(cancel);
            (metadata, join_scoped(configurations), join_scoped(excludes))
        });
        let mut paths = BTreeSet::from([self.root().join(".git")]);
        paths.extend(metadata?);
        let configurations = configurations?.0;
        paths.extend(configurations.iter().cloned());
        paths.extend(excludes?);
        if let Some(home) = std::env::var_os("HOME") {
            paths.insert(PathBuf::from(&home).join(".gitconfig"));
            let config = std::env::var_os("XDG_CONFIG_HOME")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(home).join(".config"));
            paths.insert(config.join("git/config"));
            paths.insert(config.join("git/ignore"));
        }
        cancel.check()?;
        Ok((paths.into_iter().collect(), configurations))
    }

    /// Git and repository metadata locations, resolved with one `rev-parse` when its
    /// output is unambiguous and with one query per location otherwise.
    fn metadata_paths(&self, cancel: &CancelToken) -> Result<Vec<PathBuf>> {
        match self.batched_metadata_paths(cancel)? {
            Some(paths) => Ok(paths),
            None => self.sequential_metadata_paths(cancel),
        }
    }

    /// `None` when the output cannot be attributed line by line to the requested paths.
    fn batched_metadata_paths(&self, cancel: &CancelToken) -> Result<Option<Vec<PathBuf>>> {
        let mut command = git_command(self.root());
        command.args([
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ]);
        for name in GIT_PATH_NAMES {
            command.args(["--git-path", name]);
        }
        let limit = (2 + GIT_PATH_NAMES.len()) * 64 * 1024;
        let bytes = checked(run(command, limit, cancel)?)?;
        let Some(lines) = split_batched_paths(&bytes, 2 + GIT_PATH_NAMES.len()) else {
            return Ok(None);
        };
        lines
            .into_iter()
            .map(|line| self.absolute_watch_path(line))
            .collect::<Result<_>>()
            .map(Some)
    }

    fn sequential_metadata_paths(&self, cancel: &CancelToken) -> Result<Vec<PathBuf>> {
        let mut paths = Vec::new();
        for option in ["--git-dir", "--git-common-dir"] {
            paths.push(self.watch_git_path(&[option], cancel)?);
        }
        for name in GIT_PATH_NAMES {
            paths.push(self.watch_git_path(&["--git-path", name], cancel)?);
        }
        Ok(paths)
    }

    fn excludes_file_path(&self, cancel: &CancelToken) -> Result<Option<PathBuf>> {
        let mut command = git_command(self.root());
        command.args(["config", "--null", "--path", "--get", "core.excludesFile"]);
        let output = run(command, 64 * 1024, cancel)?;
        if output.status.success() {
            let bytes = output.stdout.strip_suffix(b"\0").unwrap_or(&output.stdout);
            return self.absolute_watch_path(bytes).map(Some);
        }
        if output.status.code() != Some(1) {
            checked(output)?;
        }
        Ok(None)
    }

    fn watch_git_path(&self, arguments: &[&str], cancel: &CancelToken) -> Result<PathBuf> {
        let mut command = git_command(self.root());
        command
            .args(["rev-parse", "--path-format=absolute"])
            .args(arguments);
        let bytes = checked(run(command, 64 * 1024, cancel)?)?;
        self.absolute_watch_path(bytes.strip_suffix(b"\n").unwrap_or(&bytes))
    }

    fn absolute_watch_path(&self, bytes: &[u8]) -> Result<PathBuf> {
        if bytes.is_empty() {
            return Err(Error::InvalidData(
                "Git returned an empty watch path".into(),
            ));
        }
        let path = GitPath(bytes.to_vec()).to_path_buf()?;
        Ok(if path.is_absolute() {
            path
        } else {
            self.root().join(path)
        })
    }
}

#[cfg(test)]
#[path = "watch_tests.rs"]
mod tests;
