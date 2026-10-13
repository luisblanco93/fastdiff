use crate::worker::PreparedDocument;
use fastdiff_cli::{OpenRequest, OpenTarget};
use fastdiff_core::{GitPath, View};
use std::{cell::RefCell, collections::HashMap, path::PathBuf, sync::Arc};

/// Where a tab points: enough to decide whether it is the one a request asked for.
pub(crate) struct TabLocus<'a> {
    root: &'a str,
    requested: &'a str,
    target: OpenTarget,
    file: Option<&'a GitPath>,
    view: View,
}

impl<'a> TabLocus<'a> {
    /// `root` is the repository root reported by the worker; the request keeps the path the
    /// tab was opened with, which may differ (subdirectory, symlink, short Windows name).
    pub(crate) fn new(
        request: &'a OpenRequest,
        root: &'a str,
        file: Option<&'a GitPath>,
        view: View,
    ) -> Self {
        Self {
            root,
            requested: &request.repository,
            target: request.target,
            file,
            view,
        }
    }

    pub(super) fn view(&self) -> View {
        self.view
    }
}

/// A selected tab's locus plus what the tab currently presents.
pub(crate) struct TabState<'a> {
    locus: TabLocus<'a>,
    document_mode: bool,
    loading: bool,
    failed: bool,
    document: Option<&'a Arc<PreparedDocument>>,
}

impl<'a> TabState<'a> {
    pub(crate) fn new(
        locus: TabLocus<'a>,
        document_mode: bool,
        loading: bool,
        failed: bool,
        document: Option<&'a Arc<PreparedDocument>>,
    ) -> Self {
        Self {
            locus,
            document_mode,
            loading,
            failed,
            document,
        }
    }

    pub(super) fn locus(&self) -> &TabLocus<'a> {
        &self.locus
    }

    pub(super) fn document(&self) -> Option<&'a Arc<PreparedDocument>> {
        self.document
    }
}

/// The review state a measurement waits to see: repository, file, and view.
#[derive(Clone)]
pub(super) struct Identity {
    target: OpenTarget,
    repository: String,
    canonical: Option<PathBuf>,
    file: Option<String>,
    view: Option<View>,
    /// Whether a candidate path resolves to `canonical`, remembered per candidate. A sample
    /// asks on the UI thread every frame it waits; the filesystem answers once.
    resolved: RefCell<HashMap<String, bool>>,
}

impl Identity {
    pub(super) fn new(
        target: OpenTarget,
        repository: &str,
        file: Option<&str>,
        view: Option<View>,
    ) -> Self {
        Self {
            target,
            repository: repository.to_owned(),
            canonical: std::fs::canonicalize(repository).ok(),
            file: file.map(str::to_owned),
            view,
            resolved: RefCell::default(),
        }
    }

    pub(super) fn from_request(request: &OpenRequest) -> Self {
        let file = request.file.as_ref().map(GitPath::display);
        Self::new(
            request.target,
            &request.repository,
            file.as_deref(),
            Some(request.view),
        )
    }

    pub(super) fn repository(&self) -> &str {
        &self.repository
    }

    pub(super) fn file(&self) -> Option<&str> {
        self.file.as_deref()
    }

    pub(super) fn view(&self) -> Option<View> {
        self.view
    }

    pub(super) fn with_view(self, view: View) -> Self {
        Self {
            view: Some(view),
            ..self
        }
    }

    fn same_path(&self, candidate: &str) -> bool {
        if self.repository == candidate {
            return true;
        }
        let Some(expected) = &self.canonical else {
            return false;
        };
        if let Some(&known) = self.resolved.borrow().get(candidate) {
            return known;
        }
        let same = std::fs::canonicalize(candidate).is_ok_and(|actual| &actual == expected);
        self.resolved
            .borrow_mut()
            .insert(candidate.to_owned(), same);
        same
    }

    /// Whether an incoming open request asks for this identity's repository and file.
    /// An unset view matches any request view.
    pub(super) fn is_requested_by(&self, request: &OpenRequest) -> bool {
        self.target == request.target
            && self.same_path(&request.repository)
            && self.file.as_deref() == request.file.as_ref().map(GitPath::display).as_deref()
            && self.view.is_none_or(|view| view == request.view)
    }

    /// Whether `tab` is the place this identity asks for, regardless of what it presents.
    pub(super) fn selects(&self, tab: &TabLocus<'_>) -> bool {
        self.target == tab.target
            && (self.same_path(tab.root) || self.same_path(tab.requested))
            && self
                .file
                .as_ref()
                .is_none_or(|file| tab.file.is_some_and(|selected| selected.display() == *file))
            && self.view.is_none_or(|view| view == tab.view)
    }

    /// Whether `tab` presents settled diff content for this identity. Loading and error
    /// states, empty states, and full-document fallbacks never count.
    pub(super) fn is_presented(&self, tab: &TabState<'_>) -> bool {
        let documents = self.target == OpenTarget::Document;
        self.selects(&tab.locus)
            && !tab.loading
            && !tab.failed
            && tab.document.is_some()
            && tab.document_mode == documents
            && (documents || tab.locus.file.is_some())
    }
}
