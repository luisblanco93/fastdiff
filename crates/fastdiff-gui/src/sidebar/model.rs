use fastdiff_core::FileChange;
use std::collections::{BTreeMap, HashSet};

#[derive(Default)]
struct Folder {
    directories: BTreeMap<Vec<u8>, Vec<u8>>,
    files: BTreeMap<Vec<u8>, usize>,
}

#[derive(Default)]
pub(crate) struct TreeModel {
    pub file_paths: Vec<Vec<u8>>,
    folders: BTreeMap<Vec<u8>, Folder>,
    searchable_paths: Vec<String>,
    ordered_files: Vec<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct TreeRow {
    pub key: Vec<u8>,
    pub label: String,
    pub depth: usize,
    pub file: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NodeIdentity {
    pub path: Vec<u8>,
    pub file: bool,
}

impl NodeIdentity {
    pub fn matches(&self, row: &TreeRow) -> bool {
        self.file == row.file.is_some() && self.path == row.key
    }
    pub fn is_root(&self) -> bool {
        !self.file && self.path.is_empty()
    }
}

impl TreeRow {
    pub fn identity(&self) -> NodeIdentity {
        NodeIdentity {
            path: self.key.clone(),
            file: self.file.is_some(),
        }
    }
    pub fn root(path: &str) -> Self {
        let trimmed = path.trim_end_matches(['/', '\\']);
        let label = trimmed
            .rsplit(['/', '\\'])
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(path);
        Self {
            key: Vec::new(),
            label: label.to_owned(),
            depth: 0,
            file: None,
        }
    }
}

impl TreeModel {
    pub fn new(files: &[FileChange]) -> Self {
        let mut folders = BTreeMap::<Vec<u8>, Folder>::new();
        folders.insert(Vec::new(), Folder::default());
        for (index, file) in files.iter().enumerate() {
            let mut parent = Vec::new();
            let components: Vec<_> = file.path.0.split(|byte| *byte == b'/').collect();
            for component in components.iter().take(components.len().saturating_sub(1)) {
                let path = joined(&parent, component);
                folders
                    .entry(parent)
                    .or_default()
                    .directories
                    .insert(component.to_vec(), path.clone());
                folders.entry(path.clone()).or_default();
                parent = path;
            }
            if let Some(basename) = components.last() {
                folders
                    .entry(parent)
                    .or_default()
                    .files
                    .insert(basename.to_vec(), index);
            }
        }
        let mut model = Self {
            folders,
            file_paths: files.iter().map(|file| file.path.0.clone()).collect(),
            searchable_paths: files
                .iter()
                .map(|file| file.path.display().to_lowercase())
                .collect(),
            ordered_files: Vec::new(),
        };
        model.ordered_files = model
            .rows("", &HashSet::new())
            .iter()
            .filter_map(|row| row.file)
            .collect();
        model
    }

    pub fn filtered_files(&self, filter: &str) -> Vec<usize> {
        let filter = filter.to_lowercase();
        self.ordered_files
            .iter()
            .copied()
            .filter(|index| self.searchable_paths[*index].contains(&filter))
            .collect()
    }

    pub fn file_index(&self, path: &[u8]) -> Option<usize> {
        let (parent, basename) = match path.iter().rposition(|byte| *byte == b'/') {
            Some(index) => (&path[..index], &path[index + 1..]),
            None => (b"".as_slice(), path),
        };
        self.folders.get(parent)?.files.get(basename).copied()
    }

    pub(super) fn has_node(&self, identity: &NodeIdentity) -> bool {
        if identity.file {
            self.file_index(&identity.path).is_some()
        } else {
            identity.is_root() || self.has_folder(&identity.path)
        }
    }

    pub(super) fn rows(&self, filter: &str, collapsed: &HashSet<Vec<u8>>) -> Vec<TreeRow> {
        if filter.is_empty() && collapsed.contains(&Vec::new()) {
            return Vec::new();
        }
        let matching: HashSet<_> = self.filtered_file_candidates(filter).collect();
        let visible_folders: HashSet<Vec<u8>> = matching
            .iter()
            .flat_map(|index| ancestors(&self.file_paths[*index]).map(<[u8]>::to_vec))
            .collect();
        let mut rows = Vec::new();
        let mut pending = self.children(&[], 0, &visible_folders, &matching, filter, collapsed);
        pending.reverse();
        while let Some(row) = pending.pop() {
            if row.file.is_none() && (!filter.is_empty() || !collapsed.contains(&row.key)) {
                let children = self.children(
                    &row.key,
                    row.depth + 1,
                    &visible_folders,
                    &matching,
                    filter,
                    collapsed,
                );
                pending.extend(children.into_iter().rev());
            }
            rows.push(row);
        }
        rows
    }

    pub fn has_folder(&self, path: &[u8]) -> bool {
        self.folders.contains_key(path)
    }

    fn children(
        &self,
        parent: &[u8],
        depth: usize,
        visible_folders: &HashSet<Vec<u8>>,
        matching: &HashSet<usize>,
        filter: &str,
        collapsed: &HashSet<Vec<u8>>,
    ) -> Vec<TreeRow> {
        let Some(folder) = self.folders.get(parent) else {
            return Vec::new();
        };
        let mut children = Vec::new();
        for (name, path) in &folder.directories {
            if !visible_folders.contains(path) {
                continue;
            }
            let (key, label) = self.compact(path, name, filter, collapsed);
            children.push(TreeRow {
                key,
                label,
                depth,
                file: None,
            });
        }
        for (name, index) in &folder.files {
            if matching.contains(index) {
                children.push(TreeRow {
                    key: self.file_paths[*index].clone(),
                    label: display(name),
                    depth,
                    file: Some(*index),
                });
            }
        }
        children
    }

    fn compact(
        &self,
        path: &[u8],
        name: &[u8],
        filter: &str,
        collapsed: &HashSet<Vec<u8>>,
    ) -> (Vec<u8>, String) {
        let mut path = path.to_vec();
        let mut label = display(name);
        while !filter.is_empty() || !collapsed.contains(&path) {
            let folder = &self.folders[&path];
            if !folder.files.is_empty() || folder.directories.len() != 1 {
                break;
            }
            let Some((name, child)) = folder.directories.first_key_value() else {
                break;
            };
            label.push('/');
            label.push_str(&display(name));
            path = child.clone();
        }
        (path, label)
    }

    fn filtered_file_candidates<'a>(&'a self, filter: &str) -> impl Iterator<Item = usize> + 'a {
        let filter = filter.to_lowercase();
        self.searchable_paths
            .iter()
            .enumerate()
            .filter_map(move |(index, path)| path.contains(&filter).then_some(index))
    }
}

pub(super) fn ancestors(path: &[u8]) -> impl Iterator<Item = &[u8]> {
    path.iter()
        .enumerate()
        .filter_map(move |(index, byte)| (*byte == b'/').then_some(&path[..index]))
}

fn joined(parent: &[u8], name: &[u8]) -> Vec<u8> {
    if parent.is_empty() {
        name.to_vec()
    } else {
        [parent, b"/", name].concat()
    }
}

fn display(name: &[u8]) -> String {
    String::from_utf8_lossy(name).into_owned()
}
