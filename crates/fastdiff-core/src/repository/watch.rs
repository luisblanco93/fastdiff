use super::Repository;
use crate::{
    CancelToken, Error, GitPath, Result,
    process::{checked, git_command, run},
};
use std::{collections::BTreeSet, path::PathBuf};

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
        let mut paths = BTreeSet::from([self.root().join(".git")]);
        for option in ["--git-dir", "--git-common-dir"] {
            paths.insert(self.watch_git_path(&[option], cancel)?);
        }
        for name in [
            "HEAD",
            "index",
            "logs",
            "refs",
            "packed-refs",
            "config",
            "config.worktree",
            "info/exclude",
        ] {
            paths.insert(self.watch_git_path(&["--git-path", name], cancel)?);
        }
        let configurations = super::reflog_snapshot::config_paths(self.root(), cancel)?.0;
        paths.extend(configurations.iter().cloned());
        let mut command = git_command(self.root());
        command.args(["config", "--null", "--path", "--get", "core.excludesFile"]);
        let output = run(command, 64 * 1024, cancel)?;
        if output.status.success() {
            let bytes = output.stdout.strip_suffix(b"\0").unwrap_or(&output.stdout);
            paths.insert(self.absolute_watch_path(bytes)?);
        } else if output.status.code() != Some(1) {
            checked(output)?;
        }
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
