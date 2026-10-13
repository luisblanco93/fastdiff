use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    #[default]
    All,
    Unstaged,
    Staged,
    Unpushed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReflogEntry {
    pub id: String,
    pub short_id: String,
    pub decorations: String,
    pub selector: String,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReflogList {
    pub branch: Option<String>,
    pub entries: Vec<ReflogEntry>,
    pub notice: Option<String>,
    #[serde(default)]
    pub snapshot: Option<u64>,
    #[serde(default)]
    pub start: usize,
    #[serde(default)]
    pub has_more: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflogRequest {
    pub start: usize,
    pub snapshot: Option<u64>,
    pub boundary: Option<ReflogEntry>,
}

/// Git names are bytes, not UTF-8. Display text must never be used for file access.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GitPath(pub Vec<u8>);

impl GitPath {
    pub fn display(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }

    pub fn from_path(path: &Path) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Self(path.as_os_str().as_bytes().to_vec())
        }
        #[cfg(not(unix))]
        {
            Self(path.to_string_lossy().replace('\\', "/").into_bytes())
        }
    }

    pub fn to_path_buf(&self) -> Result<PathBuf> {
        if self.0.contains(&0) {
            return Err(Error::InvalidPath);
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            Ok(std::ffi::OsString::from_vec(self.0.clone()).into())
        }
        #[cfg(not(unix))]
        {
            let name = std::str::from_utf8(&self.0).map_err(|_| Error::InvalidPath)?;
            Ok(PathBuf::from(name))
        }
    }

    pub(crate) fn relative_path(&self) -> Result<PathBuf> {
        let path = self.to_path_buf()?;
        if path.as_os_str().is_empty()
            || !path
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        {
            return Err(Error::InvalidPath);
        }
        Ok(path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: GitPath,
    pub old_path: Option<GitPath>,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    pub conflict: bool,
    pub status: String,
    #[serde(default)]
    pub committed: Option<CommittedComparison>,
}

/// Commit IDs and paths captured together during refresh, independent of working-tree renames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommittedComparison {
    pub base: String,
    pub head: String,
    pub path: GitPath,
    pub old_path: Option<GitPath>,
    pub status: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffDocument {
    pub comparisons: Vec<Comparison>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comparison {
    pub label: String,
    pub patch: String,
    pub rows: Vec<DiffRow>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RowKind {
    Context,
    Addition,
    Deletion,
    Header,
    Change,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffRow {
    pub kind: RowKind,
    pub old_number: Option<usize>,
    pub new_number: Option<usize>,
    pub left: String,
    pub right: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_validate_boundaries_but_preserve_display_characters() {
        assert_eq!(View::default(), View::All);
        assert_eq!(GitPath(b"line\n\tname".to_vec()).display(), "line\n\tname");
        for bytes in [
            b"".as_slice(),
            b"..",
            b"nested/../outside",
            b"/absolute",
            b"bad\0",
        ] {
            assert!(GitPath(bytes.to_vec()).relative_path().is_err());
        }
        assert_eq!(
            GitPath(b"nested/valid".to_vec()).relative_path().unwrap(),
            Path::new("nested/valid")
        );
    }
}
