use super::Repository;
use crate::{
    CancelToken, Error, GitPath, Result,
    process::{checked, git_command, run},
};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(super) struct ReflogLayout {
    files: Vec<PathBuf>,
    refs: Vec<PathBuf>,
    guards: Vec<PathBuf>,
    guard_fingerprint: u64,
    reliable: bool,
}

impl Repository {
    /// None means metadata cannot reliably detect changes; callers must reload history.
    pub fn reflog_snapshot(&self, cancel: &CancelToken) -> Result<Option<u64>> {
        cancel.check()?;
        let result = self.read_reflog_snapshot(cancel);
        cancel.check()?;
        match result {
            Ok(snapshot) => Ok(snapshot),
            Err(Error::Cancelled) => Err(Error::Cancelled),
            Err(_) => Ok(None),
        }
    }

    fn read_reflog_snapshot(&self, cancel: &CancelToken) -> Result<Option<u64>> {
        let mut cache = self
            .reflog_layout
            .lock()
            .map_err(|_| Error::InvalidData("Reflog layout lock poisoned".into()))?;
        let stale = match cache.as_ref() {
            Some(layout) => fingerprint_files(&layout.guards, cancel)? != layout.guard_fingerprint,
            None => true,
        };
        if stale {
            *cache = Some(resolve_layout(&self.root, cancel)?);
        }
        let layout = cache
            .as_ref()
            .ok_or_else(|| Error::InvalidData("Missing reflog layout".into()))?;
        if !layout.reliable {
            return Ok(None);
        }
        let mut hash = DefaultHasher::new();
        fingerprint_files(&layout.files, cancel)?.hash(&mut hash);
        for refs in &layout.refs {
            refs.hash(&mut hash);
            fingerprint_refs(refs, refs, &mut hash, cancel)?;
        }
        cancel.check()?;
        Ok(Some(hash.finish()))
    }
}

fn resolve_layout(root: &Path, cancel: &CancelToken) -> Result<ReflogLayout> {
    let mut command = git_command(root);
    command.args([
        "rev-parse",
        "--path-format=absolute",
        "--git-dir",
        "--git-common-dir",
        "--git-path",
        "HEAD",
        "--git-path",
        "logs/HEAD",
        "--git-path",
        "refs",
        "--git-path",
        "packed-refs",
        "--git-path",
        "config",
        "--git-path",
        "config.worktree",
    ]);
    let resolved = checked(run(command, 64 * 1024, cancel)?)?;
    let paths = parse_paths(&resolved)?;
    if paths.len() != 8 {
        return Err(Error::InvalidData("Ambiguous Git metadata paths".into()));
    }
    let private = &paths[0];
    let common = &paths[1];
    let (config, reliable) = config_paths(root, cancel)?;
    let mut config = config;
    config.sort();
    config.dedup();
    let mut guards = config.clone();
    guards.extend([
        root.join(".git"),
        private.join("commondir"),
        private.join("gitdir"),
        paths[2].clone(),
        private.join("HEAD"),
        paths[6].clone(),
        paths[7].clone(),
        common.join("config"),
    ]);
    guards.sort();
    guards.dedup();
    // Re-read sources between guard checks so a racing include change cannot stay cached.
    let guard_fingerprint = fingerprint_files(&guards, cancel)?;
    let (mut confirmed, confirmed_reliable) = config_paths(root, cancel)?;
    confirmed.sort();
    confirmed.dedup();
    if config != confirmed
        || reliable != confirmed_reliable
        || fingerprint_files(&guards, cancel)? != guard_fingerprint
    {
        return Err(Error::InvalidData(
            "Git configuration changed while resolving reflog paths".into(),
        ));
    }
    let mut files = guards.clone();
    files.extend([
        paths[3].clone(),
        private.join("logs/HEAD"),
        paths[5].clone(),
        private.join("packed-refs"),
        common.join("packed-refs"),
        private.join("shallow"),
        common.join("shallow"),
    ]);
    files.sort();
    files.dedup();
    let mut refs = vec![paths[4].clone(), private.join("refs"), common.join("refs")];
    refs.sort();
    refs.dedup();
    Ok(ReflogLayout {
        guard_fingerprint,
        files,
        refs,
        guards,
        reliable,
    })
}

fn parse_paths(bytes: &[u8]) -> Result<Vec<PathBuf>> {
    let bytes = bytes
        .strip_suffix(b"\n")
        .ok_or_else(|| Error::InvalidData("Unterminated Git metadata path".into()))?;
    bytes
        .split(|byte| *byte == b'\n')
        .map(|path| {
            if path.is_empty() {
                return Err(Error::InvalidPath);
            }
            GitPath(path.to_vec()).to_path_buf()
        })
        .collect()
}

pub(super) fn config_paths(root: &Path, cancel: &CancelToken) -> Result<(Vec<PathBuf>, bool)> {
    let mut paths = Vec::new();
    for variable in ["GIT_CONFIG_SYSTEM", "GIT_CONFIG_GLOBAL"] {
        let mut command = git_command(root);
        command.args(["var", variable]);
        paths.extend(parse_paths(&checked(run(command, 64 * 1024, cancel)?)?)?);
    }
    let mut command = git_command(root);
    command.args(["config", "--includes", "--show-origin", "--null", "--list"]);
    let bytes = checked(run(command, 1024 * 1024, cancel)?)?;
    let fields: Vec<_> = bytes
        .strip_suffix(&[0])
        .ok_or_else(|| Error::InvalidData("Unterminated Git config origins".into()))?
        .split(|byte| *byte == 0)
        .collect();
    if fields.len() % 2 != 0 {
        return Err(Error::InvalidData("Incomplete Git config origins".into()));
    }
    let mut reliable = true;
    for pair in fields.chunks_exact(2) {
        cancel.check()?;
        let origin = pair[0].strip_prefix(b"file:");
        let path = origin
            .map(|bytes| GitPath(bytes.to_vec()).to_path_buf())
            .transpose()?;
        let path = path.map(|path| {
            if path.is_absolute() {
                path
            } else {
                root.join(path)
            }
        });
        if let Some(path) = &path {
            if !path.is_file() {
                reliable = false;
            }
            paths.push(path.clone());
        }
        let Some(newline) = pair[1].iter().position(|byte| *byte == b'\n') else {
            continue;
        };
        let (key, value) = (&pair[1][..newline], &pair[1][newline + 1..]);
        if key == b"extensions.refstorage" && value != b"files" {
            reliable = false;
        }
        if key == b"include.path" || (key.starts_with(b"includeif.") && key.ends_with(b".path")) {
            match included_path(value, path.as_deref()) {
                Ok(included) => paths.push(included),
                Err(_) => reliable = false,
            }
        }
    }
    Ok((paths, reliable))
}

fn included_path(value: &[u8], origin: Option<&Path>) -> Result<PathBuf> {
    let path = if let Some(relative) = value.strip_prefix(b"~/") {
        let home = std::env::var_os("HOME").ok_or(Error::InvalidPath)?;
        PathBuf::from(home).join(GitPath(relative.to_vec()).to_path_buf()?)
    } else if value.starts_with(b"~") || value.starts_with(b"%(") {
        return Err(Error::InvalidPath);
    } else {
        GitPath(value.to_vec()).to_path_buf()?
    };
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(origin
        .and_then(Path::parent)
        .ok_or(Error::InvalidPath)?
        .join(path))
}

fn fingerprint_files(paths: &[PathBuf], cancel: &CancelToken) -> Result<u64> {
    let mut hash = DefaultHasher::new();
    for path in paths {
        cancel.check()?;
        path.hash(&mut hash);
        metadata(path, &mut hash)?;
    }
    Ok(hash.finish())
}

fn metadata(path: &Path, hash: &mut DefaultHasher) -> Result<Option<fs::Metadata>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            false.hash(hash);
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.is_symlink() {
        return Err(Error::InvalidData(
            "Symlinked Git metadata requires conservative reloads".into(),
        ));
    }
    true.hash(hash);
    metadata.is_dir().hash(hash);
    metadata.len().hash(hash);
    metadata.modified()?.hash(hash);
    metadata.created().ok().hash(hash);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.dev().hash(hash);
        metadata.ino().hash(hash);
        metadata.ctime().hash(hash);
        metadata.ctime_nsec().hash(hash);
    }
    Ok(Some(metadata))
}

fn fingerprint_refs(
    root: &Path,
    path: &Path,
    hash: &mut DefaultHasher,
    cancel: &CancelToken,
) -> Result<()> {
    cancel.check()?;
    path.strip_prefix(root)
        .map_err(|_| Error::InvalidPath)?
        .as_os_str()
        .hash(hash);
    let Some(metadata) = metadata(path, hash)? else {
        return Ok(());
    };
    if metadata.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<_>>()?;
        entries.sort();
        for entry in entries {
            fingerprint_refs(root, &entry, hash, cancel)?;
        }
    } else if !metadata.is_file() {
        return Err(Error::InvalidData("Unsupported loose ref metadata".into()));
    }
    Ok(())
}
