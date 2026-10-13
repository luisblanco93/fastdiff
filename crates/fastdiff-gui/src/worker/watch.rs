use fastdiff_cli::Backend;
use fastdiff_core::{CancelToken, Repository};
use notify::{
    RecommendedWatcher, RecursiveMode, Watcher,
    event::{ModifyKind, RenameMode},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
    },
};

const MAX_EVENT_PATHS: usize = 128;
const MAX_EVENT_BYTES: usize = 8 * 1024;

#[derive(Default)]
struct PendingPaths {
    paths: BTreeMap<PathBuf, bool>,
    bytes: usize,
    conservative: bool,
    rescan: bool,
    diagnostic: Option<String>,
}

impl PendingPaths {
    fn record(&mut self, event: notify::Event) {
        if event.need_rescan() {
            self.conservative = true;
            self.rescan = true;
        }
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return;
        }
        let layout = matches!(
            event.kind,
            notify::EventKind::Create(_)
                | notify::EventKind::Remove(_)
                | notify::EventKind::Modify(ModifyKind::Name(_))
        );
        self.rescan |= event.paths.is_empty()
            || matches!(
                event.kind,
                notify::EventKind::Any | notify::EventKind::Other
            );
        self.conservative |= self.rescan
            || matches!(event.kind, notify::EventKind::Modify(ModifyKind::Name(mode))
                if mode != RenameMode::Both || event.paths.len() != 2);
        for path in event.paths {
            if let Some(path_layout) = self.paths.get_mut(&path) {
                *path_layout |= layout;
                continue;
            }
            let bytes = path.as_os_str().as_encoded_bytes().len().saturating_add(1);
            if self.paths.len() == MAX_EVENT_PATHS
                || bytes > MAX_EVENT_BYTES.saturating_sub(self.bytes)
            {
                self.conservative = true;
                self.rescan = true;
                break;
            }
            self.bytes += bytes;
            self.paths.insert(path, layout);
        }
    }
}

pub(super) struct WatchChange {
    pub(super) relevant: bool,
    pub(super) rebuild: bool,
    pub(super) diagnostics: Vec<String>,
}

pub(super) struct WatchCoverage {
    root: PathBuf,
    repository: Option<Repository>,
    document: Option<PathBuf>,
    metadata: Vec<PathBuf>,
    configurations: Vec<PathBuf>,
    directories: BTreeSet<PathBuf>,
    watcher: Option<RecommendedWatcher>,
    batch: Arc<Mutex<PendingPaths>>,
    received: Arc<AtomicBool>,
    wake: SyncSender<()>,
    diagnostics: Vec<String>,
    failed: bool,
}

impl WatchCoverage {
    pub(super) fn new(
        backend: &Backend,
        document: Option<&str>,
        wake: SyncSender<()>,
        cancel: &CancelToken,
    ) -> Self {
        let repository = match backend {
            Backend::Local { repository, .. }
            | Backend::Document {
                repository: Some(repository),
                ..
            } => Some(repository.clone()),
            _ => None,
        };
        let mut coverage = Self {
            root: absolute_path(Path::new(backend.root())),
            repository,
            document: document.map(Path::new).map(absolute_path),
            metadata: Vec::new(),
            configurations: Vec::new(),
            directories: BTreeSet::new(),
            watcher: None,
            batch: Arc::new(Mutex::new(PendingPaths::default())),
            received: Arc::new(AtomicBool::new(false)),
            wake,
            diagnostics: Vec::new(),
            failed: false,
        };
        coverage.diagnostics = coverage.rebuild(cancel);
        coverage
    }

    /// Coverage for a deferred start. A build cancelled midway is inert (`rebuild` returns
    /// early and reports nothing), so it is discarded instead of replacing a retry.
    pub(super) fn new_unless_cancelled(
        backend: &Backend,
        document: Option<&str>,
        wake: SyncSender<()>,
        cancel: &CancelToken,
    ) -> Option<Self> {
        let coverage = Self::new(backend, document, wake, cancel);
        (!cancel.is_cancelled()).then_some(coverage)
    }

    pub(super) fn take_received(&self) -> bool {
        self.received.swap(false, Ordering::AcqRel)
    }

    pub(super) fn take_diagnostics(&mut self) -> Vec<String> {
        std::mem::take(&mut self.diagnostics)
    }

    pub(super) fn update_document(
        &mut self,
        document: Option<&str>,
        cancel: &CancelToken,
    ) -> Vec<String> {
        let document = document.map(Path::new).map(absolute_path);
        if document == self.document && !self.failed {
            return Vec::new();
        }
        self.document = document;
        self.rebuild(cancel)
    }

    pub(super) fn rebuild(&mut self, cancel: &CancelToken) -> Vec<String> {
        if cancel.is_cancelled() {
            return Vec::new();
        }
        let mut diagnostics = Vec::new();
        self.failed = false;
        if let Some(repository) = &self.repository {
            match repository.watch_paths(cancel) {
                Ok((paths, configurations)) => {
                    self.metadata = paths.into_iter().map(|path| absolute_path(&path)).collect();
                    self.configurations = configurations
                        .into_iter()
                        .map(|path| absolute_path(&path))
                        .collect();
                }
                Err(error) if !cancel.is_cancelled() => {
                    self.failed = true;
                    diagnostics.push(format!(
                        "Cannot resolve Git watch paths: {error}; using periodic refresh"
                    ));
                }
                _ => return Vec::new(),
            }
        }
        let batch = Arc::clone(&self.batch);
        let received = Arc::clone(&self.received);
        let wake = self.wake.clone();
        let mut watcher = match notify::recommended_watcher(move |event| {
            collect_event(&batch, &received, &wake, event)
        }) {
            Ok(watcher) => watcher,
            Err(error) => {
                self.failed = true;
                diagnostics.push(format!(
                    "Cannot create filesystem watcher: {error}; using periodic refresh"
                ));
                return diagnostics;
            }
        };
        let targets = self.targets();
        for (path, mode) in &targets {
            if cancel.is_cancelled() {
                return Vec::new();
            }
            if let Err(error) = watcher.watch(path, *mode) {
                self.failed = true;
                diagnostics.push(format!(
                    "Cannot watch {}: {error}; using periodic refresh",
                    path.display()
                ));
            }
        }
        self.directories = targets.into_keys().collect();
        self.watcher = Some(watcher);
        diagnostics
    }

    fn targets(&self) -> BTreeMap<PathBuf, RecursiveMode> {
        let mut targets = BTreeMap::new();
        if self.repository.is_some() {
            if self.root.is_dir() {
                targets.insert(self.root.clone(), RecursiveMode::Recursive);
            }
            add_parent(&mut targets, &self.root);
        }
        if let Some(document) = &self.document {
            add_parent(&mut targets, document);
            if let Some(parent) = document.parent() {
                add_parent(&mut targets, parent);
            }
        }
        for path in &self.metadata {
            if path.is_dir() {
                targets.insert(path.clone(), RecursiveMode::Recursive);
            }
            add_parent(&mut targets, path);
        }
        let recursive: Vec<_> = targets
            .iter()
            .filter(|(_, mode)| **mode == RecursiveMode::Recursive)
            .map(|(path, _)| path.clone())
            .collect();
        targets.retain(|path, _| {
            !recursive
                .iter()
                .any(|ancestor| path != ancestor && path.starts_with(ancestor))
        });
        targets
    }

    pub(super) fn take_pending(&mut self, cancel: &CancelToken) -> WatchChange {
        let batch = match self.batch.lock() {
            Ok(mut batch) => std::mem::take(&mut *batch),
            Err(_) => {
                return WatchChange {
                    relevant: true,
                    rebuild: true,
                    diagnostics: vec![
                        "Filesystem watcher queue failed; using periodic refresh".into(),
                    ],
                };
            }
        };
        let mut diagnostics: Vec<String> = batch.diagnostic.into_iter().collect();
        if !diagnostics.is_empty() {
            self.failed = true;
        }
        let mut relevant = batch.conservative;
        let mut rebuild = batch.rescan;
        let mut candidates = Vec::new();
        for (path, layout) in batch.paths {
            let path = absolute_path(&path);
            let document = self
                .document
                .as_ref()
                .is_some_and(|document| document == &path || document.starts_with(&path));
            let metadata = self
                .metadata
                .iter()
                .any(|metadata| path.starts_with(metadata) || metadata.starts_with(&path));
            if document || metadata {
                relevant = true;
                rebuild |=
                    layout
                        && (self.document.as_ref().is_some_and(|document| {
                            document != &path && document.starts_with(&path)
                        }) || self.directories.contains(&path)
                            || self
                                .metadata
                                .iter()
                                .any(|metadata| metadata != &path && metadata.starts_with(&path)));
                rebuild |= path == self.root.join(".git") || self.configurations.contains(&path);
            } else if self.repository.is_some() && path.starts_with(&self.root) {
                if path
                    .file_name()
                    .is_some_and(|name| name == ".gitignore" || name == ".gitattributes")
                {
                    relevant = true;
                } else {
                    candidates.push(path);
                }
            }
        }
        if !relevant && !candidates.is_empty() {
            relevant = match self
                .repository
                .as_ref()
                .map(|repository| repository.paths_are_ignored(&candidates, cancel))
            {
                Some(Ok(ignored)) => !ignored,
                Some(Err(error)) => {
                    if !cancel.is_cancelled() {
                        diagnostics.push(format!(
                            "Cannot classify filesystem changes: {error}; refreshing conservatively"
                        ));
                    }
                    true
                }
                None => true,
            };
        }
        WatchChange {
            relevant,
            rebuild,
            diagnostics,
        }
    }
}

fn collect_event(
    batch: &Mutex<PendingPaths>,
    received: &AtomicBool,
    wake: &SyncSender<()>,
    event: notify::Result<notify::Event>,
) {
    if let Ok(event) = &event
        && matches!(event.kind, notify::EventKind::Access(_))
        && !event.need_rescan()
    {
        return;
    }
    if let Ok(mut batch) = batch.lock() {
        match event {
            Ok(event) => batch.record(event),
            Err(error) => {
                batch.conservative = true;
                batch.rescan = true;
                batch.diagnostic.get_or_insert_with(|| {
                    format!("Filesystem watcher failed: {error}; using periodic refresh")
                        .chars()
                        .take(1024)
                        .collect()
                });
            }
        }
    }
    received.store(true, Ordering::Release);
    let _ = wake.try_send(());
}

fn add_parent(targets: &mut BTreeMap<PathBuf, RecursiveMode>, path: &Path) {
    if let Some(parent) = path
        .parent()
        .and_then(|parent| parent.ancestors().find(|ancestor| ancestor.is_dir()))
    {
        targets
            .entry(parent.to_owned())
            .or_insert(RecursiveMode::NonRecursive);
    }
}

fn absolute_path(path: &Path) -> PathBuf {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    let Some(parent) = path
        .parent()
        .and_then(|parent| parent.ancestors().find(|ancestor| ancestor.is_dir()))
    else {
        return path;
    };
    match (parent.canonicalize(), path.strip_prefix(parent)) {
        (Ok(parent), Ok(suffix)) => parent.join(suffix),
        _ => path,
    }
}

#[cfg(test)]
#[path = "watch_tests.rs"]
mod tests;
