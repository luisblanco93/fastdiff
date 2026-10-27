use crate::{
    benchmark::{Benchmark, TabLocus, TabState},
    render::DiffPresentation,
    settings::SettingsStore,
    sidebar::FileTree,
    state::Preferences,
    theme::ThemeId,
    worker::{Event, Worker},
};
use fastdiff_cli::{IpcServer, OpenRequest, OpenTarget};
use fastdiff_core::{FileChange, GitPath, ReflogList, ReflogRequest, RowKind, View};
use std::{sync::Arc, thread::JoinHandle, time::Duration};

const TAB_ROW_HEIGHT: f32 = 32.0;
const TAB_ICON_SIZE: f32 = 11.0;
/// Space between the tab name and its close control; with the control's own padding the ✕ sits
/// as far from the name as from the tab's right edge.
const TAB_CLOSE_GAP: f32 = 8.0;
/// Breathing room above and below the toolbar rows, as in the approved design.
const TOOLBAR_TOP_GAP: f32 = 3.0;
const TOOLBAR_PANEL_BOTTOM_MARGIN: i8 = 6;
const SIDEBAR_TOP_GAP: f32 = 6.0;
const BRANCH_LABEL_MAX_WIDTH: f32 = 360.0;
/// Below this width the toolbar breaks into two rows; egui cannot wrap widget groups.
const TWO_ROW_TOOLBAR_WIDTH: f32 = 620.0;
/// Width of the "N matches" label and its spacing, which only an active search adds to row one.
const MATCH_COUNT_RESERVE: f32 = 80.0;
/// A folder suffix wider than this is truncated so it cannot push the close control away.
const TAB_SUFFIX_MAX_WIDTH: f32 = 140.0;

fn toolbar_needs_two_rows(width: f32, searching: bool) -> bool {
    let reserve = if searching { MATCH_COUNT_RESERVE } else { 0.0 };
    width < TWO_ROW_TOOLBAR_WIDTH + reserve
}

struct TabView<'a> {
    title: &'a str,
    name: &'a str,
    folder: Option<&'a str>,
    selected: bool,
    is_document: bool,
    reveal: bool,
}

enum TabAction {
    Select,
    Close,
}

fn tab_name(title: &str) -> &str {
    title
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(title)
}

fn parent_segments(title: &str) -> Vec<&str> {
    let mut segments: Vec<&str> = title
        .trim_end_matches(['/', '\\'])
        .split(['/', '\\'])
        .filter(|segment| !segment.is_empty())
        .collect();
    segments.pop();
    segments
}

fn parent_tail(parents: &[&str], depth: usize) -> Option<String> {
    let tail = &parents[parents.len().saturating_sub(depth)..];
    (!tail.is_empty()).then(|| tail.join("/"))
}

/// For tabs that share a name, the shortest run of parent folders that tells them apart.
fn tab_suffixes(titles: &[&str]) -> Vec<Option<String>> {
    titles
        .iter()
        .enumerate()
        .map(|(index, title)| {
            let name = tab_name(title);
            let rivals: Vec<Vec<&str>> = titles
                .iter()
                .enumerate()
                .filter(|&(other, other_title)| other != index && tab_name(other_title) == name)
                .map(|(_, other_title)| parent_segments(other_title))
                .collect();
            if rivals.is_empty() {
                return None;
            }
            let own = parent_segments(title);
            let longest = rivals.iter().map(Vec::len).fold(own.len(), usize::max);
            let depth = (1..=longest.max(1))
                .find(|&depth| {
                    rivals
                        .iter()
                        .all(|rival| parent_tail(rival, depth) != parent_tail(&own, depth))
                })
                .unwrap_or(longest);
            parent_tail(&own, depth)
        })
        .collect()
}

mod reflog;
#[cfg(test)]
mod reflog_paging_tests;
mod repository_identity;
mod shortcuts;
mod theme_menu;

#[cfg(test)]
mod responsive_tests;

#[cfg(test)]
mod snapshot_tests;

#[cfg(test)]
mod ipc_tests;

// Reads /proc/self/stat, which only Linux provides.
#[cfg(all(test, target_os = "linux"))]
mod refresh_measurement;

#[cfg(test)]
mod benchmark_tests;

#[cfg(test)]
mod lifecycle_tests;

pub struct ReviewTab {
    request: OpenRequest,
    root: String,
    has_repository: bool,
    document_mode: bool,
    reflog_open: bool,
    reflog: Option<ReflogList>,
    reflog_generation: u64,
    reflog_loading: bool,
    reflog_error: Option<String>,
    reflog_pending: Option<ReflogRequest>,
    reflog_scroll_reset: bool,
    branch: Option<String>,
    full_available: bool,
    notice: Option<String>,
    worker: Option<Worker>,
    foreground: bool,
    generation: u64,
    files: Arc<Vec<FileChange>>,
    filter: String,
    visible_files: Vec<usize>,
    tree: FileTree,
    reveal_requested: bool,
    selected: Option<GitPath>,
    view: View,
    presentation: Option<DiffPresentation>,
    loading: bool,
    // The presentation is cached content that the verified diff has not confirmed yet.
    provisional: bool,
    error: Option<String>,
    comparison: usize,
    search: String,
    search_request_id: u64,
    search_hits: Vec<usize>,
    search_index: usize,
    context: egui::Context,
}

impl ReviewTab {
    fn new(request: OpenRequest, context: &egui::Context) -> Self {
        let worker = Worker::spawn(request.clone(), context.clone());
        let reveal_requested = request.file.is_some();
        Self {
            root: request.repository.clone(),
            has_repository: request.target == OpenTarget::Repository,
            document_mode: request.target == OpenTarget::Document,
            reflog_open: false,
            reflog: None,
            reflog_generation: 0,
            reflog_loading: false,
            reflog_error: None,
            reflog_pending: None,
            reflog_scroll_reset: false,
            branch: None,
            full_available: true,
            notice: None,
            selected: request.file.clone(),
            view: request.view,
            request,
            worker: Some(worker),
            foreground: true,
            generation: 0,
            files: Arc::new(Vec::new()),
            filter: String::new(),
            visible_files: Vec::new(),
            tree: FileTree::default(),
            reveal_requested,
            presentation: None,
            loading: true,
            provisional: false,
            error: None,
            comparison: 0,
            search: String::new(),
            search_request_id: 0,
            search_hits: Vec::new(),
            search_index: 0,
            context: context.clone(),
        }
    }

    fn refresh(&mut self) {
        if self.worker.as_ref().is_some_and(Worker::is_finished) {
            self.request.view = self.view;
            self.worker = Some(Worker::spawn(self.request.clone(), self.context.clone()));
            self.generation = 0;
        } else {
            self.generation += 1;
        }
        self.loading = true;
        self.error = None;
        if self.document_mode {
            self.load_document();
        } else if let Some(worker) = &self.worker {
            worker.refresh(self.generation, self.view);
        }
        if self.reflog_open {
            self.refresh_reflog();
        }
    }

    fn set_view(&mut self, view: View) {
        if !self.has_repository || self.document_mode || self.view == view {
            return;
        }
        self.document_mode = false;
        self.view = view;
        self.reveal_requested = false;
        self.presentation = None;
        self.comparison = 0;
        self.clear_search_results();
        self.refresh();
    }

    fn select_file(&mut self, index: usize, preserve: bool) {
        let Some(file) = self.files.get(index).cloned() else {
            return;
        };
        self.generation += 1;
        self.selected = Some(file.path.clone());
        self.document_mode = false;
        self.loading = true;
        self.provisional = false;
        self.error = None;
        if !preserve {
            self.presentation = None;
            self.comparison = 0;
            self.clear_search_results();
        }
        if let Some(worker) = &self.worker {
            worker.diff(self.generation, self.view, file);
        }
    }

    fn events(&mut self, unified: bool) -> bool {
        let events: Vec<Event> = self
            .worker
            .as_ref()
            .map(Worker::drain_events)
            .unwrap_or_default();
        self.apply_events(events, unified)
    }

    fn apply_events(&mut self, events: Vec<Event>, unified: bool) -> bool {
        let mut ready = false;
        for event in events {
            match event {
                Event::Ready(root, has_repository) => {
                    self.root = root;
                    self.has_repository = has_repository;
                    if self.request.target == OpenTarget::Document && has_repository {
                        let separator = if self.request.wsl.is_some() || !cfg!(windows) {
                            '/'
                        } else {
                            '\\'
                        };
                        let prefix =
                            format!("{}{}", self.root.trim_end_matches(separator), separator);
                        self.selected = self
                            .request
                            .repository
                            .strip_prefix(&prefix)
                            .map(|path| {
                                if self.request.wsl.is_some() {
                                    GitPath(path.as_bytes().to_vec())
                                } else {
                                    GitPath::from_path(std::path::Path::new(path))
                                }
                            })
                            .or_else(|| self.selected.clone());
                    }
                }
                Event::Branch { generation, branch } if generation == self.generation => {
                    self.branch = Some(branch.unwrap_or_else(|| "Detached HEAD".into()));
                }
                Event::Reflog { generation, list }
                    if generation == self.reflog_generation && self.reflog_open =>
                {
                    ready |= self.accept_reflog(list);
                }
                Event::ReflogError {
                    generation,
                    message,
                } if generation == self.reflog_generation && self.reflog_open => {
                    self.reflog_error = Some(message);
                    self.reflog_loading = false;
                }
                Event::Notice {
                    generation,
                    message,
                } if generation == self.generation => {
                    self.notice = message;
                }
                Event::Availability {
                    generation,
                    available,
                } if generation == self.generation => {
                    self.full_available = available;
                }
                Event::DocumentPath { generation, path } if generation == self.generation => {
                    if self.request.target == OpenTarget::Document && self.has_repository {
                        self.selected = if self.request.wsl.is_some() {
                            path.strip_prefix(&format!("{}/", self.root.trim_end_matches('/')))
                                .map(|relative| GitPath(relative.as_bytes().to_vec()))
                        } else {
                            std::path::Path::new(&path)
                                .strip_prefix(&self.root)
                                .ok()
                                .map(GitPath::from_path)
                        };
                    }
                }
                Event::Status {
                    generation,
                    automatic,
                    files,
                    model,
                } if generation == self.generation => {
                    let changed = !Arc::ptr_eq(&files, &self.files);
                    self.files = files;
                    if self.tree.update(model) {
                        self.visible_files = self.tree.filtered_files(&self.filter);
                    }
                    if self.document_mode {
                        continue;
                    }
                    let selected_index = self
                        .selected
                        .as_ref()
                        .and_then(|path| self.tree.file_index(&path.0));
                    if automatic && !changed {
                        // Status equality cannot detect edits to an already-modified file.
                        if let Some(file) = selected_index.and_then(|index| self.files.get(index))
                            && let Some(worker) = &self.worker
                        {
                            worker.diff(self.generation, self.view, file.clone());
                        }
                        continue;
                    }
                    let requested = std::mem::take(&mut self.reveal_requested);
                    let index = selected_index.or_else(|| {
                        (self.request.target == OpenTarget::Repository
                            && (!requested || self.selected.is_none())
                            && !self.files.is_empty())
                        .then_some(0)
                    });
                    if let Some(index) = index {
                        if requested || Some(&self.files[index].path) != self.selected.as_ref() {
                            self.tree.reveal(index);
                        }
                        self.select_file(
                            index,
                            Some(&self.files[index].path) == self.selected.as_ref(),
                        );
                    } else if requested && self.selected.is_some() {
                        self.toggle_document(true);
                    } else {
                        if self.request.target == OpenTarget::Repository {
                            self.selected = None;
                        }
                        self.presentation = None;
                        self.comparison = 0;
                        self.clear_search_results();
                        self.loading = false;
                        ready = true;
                    }
                }
                Event::Diff {
                    generation,
                    document,
                } if generation == self.generation
                    && document.is_document == self.document_mode =>
                {
                    if self.presentation.as_ref().is_some_and(|presentation| {
                        std::sync::Arc::ptr_eq(&presentation.document, &document)
                    }) {
                        self.loading = false;
                        self.error = None;
                        // Hits were cleared with the old presentation; confirm-in-place must
                        // search again, as the full path below does.
                        if std::mem::take(&mut self.provisional) && !self.search.is_empty() {
                            self.request_search();
                        }
                        ready = true;
                        continue;
                    }
                    self.provisional = false;
                    let mut presentation = DiffPresentation::new(document, unified);
                    if let Some(previous) = &self.presentation {
                        presentation.preserve_state_from(previous);
                    }
                    presentation.search_hits.clone_from(&self.search_hits);
                    self.comparison = self.comparison.min(
                        presentation
                            .document
                            .document
                            .comparisons
                            .len()
                            .saturating_sub(1),
                    );
                    self.presentation = Some(presentation);
                    self.loading = false;
                    self.error = None;
                    if !self.search.is_empty() {
                        self.request_search();
                    }
                    ready = true;
                }
                Event::Provisional {
                    generation,
                    document,
                } if generation == self.generation
                    && document.is_document == self.document_mode =>
                {
                    // Cached content stays flagged as loading until the verified diff arrives.
                    // Skip it when it is already on screen, so selection and drag survive.
                    if !self.loading
                        || self.presentation.as_ref().is_some_and(|presentation| {
                            std::sync::Arc::ptr_eq(&presentation.document, &document)
                        })
                    {
                        continue;
                    }
                    self.provisional = true;
                    let mut presentation = DiffPresentation::new(document, unified);
                    if let Some(previous) = &self.presentation {
                        presentation.preserve_state_from(previous);
                    }
                    presentation.search_hits.clone_from(&self.search_hits);
                    self.comparison = self.comparison.min(
                        presentation
                            .document
                            .document
                            .comparisons
                            .len()
                            .saturating_sub(1),
                    );
                    self.presentation = Some(presentation);
                    self.error = None;
                }
                Event::Search {
                    generation,
                    request_id,
                    query,
                    hits,
                } if generation == self.generation
                    && request_id == self.search_request_id
                    && query == self.search =>
                {
                    self.search_hits = hits;
                    self.search_index = 0;
                    if let Some(presentation) = &mut self.presentation {
                        presentation.search_hits.clone_from(&self.search_hits);
                        if let Some(hit) = self.search_hits.first() {
                            presentation.jump_to_match(*hit, &self.search);
                        }
                    }
                }
                Event::Error {
                    generation,
                    message,
                } if generation == self.generation => {
                    self.error = Some(message);
                    if self.document_mode || self.provisional {
                        self.provisional = false;
                        self.presentation = None;
                        self.clear_search_results();
                    }
                    self.loading = false;
                }
                _ => {}
            }
        }
        ready
    }

    fn document_path(&self) -> Result<String, String> {
        if self.request.target == OpenTarget::Document {
            return Ok(self.request.repository.clone());
        }
        let selected = self.selected.as_ref().ok_or("No file selected")?;
        let path = selected.to_path_buf().map_err(|error| error.to_string())?;
        let path = path
            .to_str()
            .ok_or("Full-file viewing requires a UTF-8 file path")?;
        let separator = if self.request.wsl.is_some() || !cfg!(windows) {
            '/'
        } else {
            '\\'
        };
        Ok(join_document_path(&self.root, path, separator))
    }

    fn load_document(&mut self) {
        match self.document_path() {
            Ok(path) => {
                if let Some(worker) = &self.worker {
                    worker.document(self.generation, path, self.view);
                }
            }
            Err(message) => {
                self.error = Some(message);
                self.loading = false;
                self.presentation = None;
                self.clear_search_results();
            }
        }
    }

    fn toggle_document(&mut self, document_mode: bool) {
        if self.document_mode == document_mode {
            return;
        }
        if !document_mode {
            if !self.has_repository {
                return;
            }
            self.document_mode = false;
            self.reveal_requested = false;
            self.presentation = None;
            self.comparison = 0;
            self.clear_search_results();
            self.refresh();
            return;
        }
        self.document_mode = true;
        self.generation += 1;
        self.loading = true;
        self.error = None;
        self.presentation = None;
        self.clear_search_results();
        self.load_document();
    }

    fn navigate_file(&mut self, next: bool) {
        if self.request.target == OpenTarget::Document || self.visible_files.is_empty() {
            return;
        }
        let selected_index = self
            .selected
            .as_ref()
            .and_then(|path| self.tree.file_index(&path.0));
        let position = self
            .visible_files
            .iter()
            .position(|index| Some(*index) == selected_index)
            .unwrap_or(0);
        let target = if next {
            (position + 1) % self.visible_files.len()
        } else {
            (position + self.visible_files.len() - 1) % self.visible_files.len()
        };
        let index = self.visible_files[target];
        self.tree.reveal(index);
        self.selected = Some(self.files[index].path.clone());
    }

    fn clear_search_results(&mut self) {
        self.search_request_id += 1;
        self.search_hits.clear();
        self.search_index = 0;
        if let Some(presentation) = &mut self.presentation {
            presentation.search_hits.clear();
        }
    }

    fn request_search(&mut self) {
        self.search_request_id += 1;
        if let Some(worker) = &self.worker {
            worker.search(self.generation, self.search_request_id, self.search.clone());
        }
    }

    fn navigate_search(&mut self, next: bool) {
        if self.search_hits.is_empty() {
            return;
        }
        self.search_index = if next {
            (self.search_index + 1) % self.search_hits.len()
        } else {
            (self.search_index + self.search_hits.len() - 1) % self.search_hits.len()
        };
        if let Some(presentation) = &mut self.presentation {
            presentation.jump_to_match(self.search_hits[self.search_index], &self.search);
        }
    }
}

pub struct Fastdiff {
    tabs: Vec<ReviewTab>,
    retired_workers: Vec<JoinHandle<()>>,
    selected_tab: usize,
    preferences: Preferences,
    server: Option<IpcServer>,
    benchmark: Benchmark,
    help_open: bool,
    fallback_fonts: crate::fallback_fonts::Watch,
    appearance: theme_menu::Appearance,
}

/// fastdiff has one dark theme. `set_visuals` only writes the active theme, so pin the theme
/// first; otherwise a light system appearance shows egui's light chrome around dark diff panes.
fn apply_theme(context: &egui::Context) {
    context.set_theme(egui::Theme::Dark);
    context.set_visuals(ThemeId::Fastdiff.theme().visuals());
    crate::theme::install(context, crate::theme::Active::default());
    // The native title bar is outside egui's visuals and follows the system appearance.
    context.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark));
}

impl Fastdiff {
    pub fn new(
        request: OpenRequest,
        server: IpcServer,
        context: &egui::Context,
        mut benchmark: Benchmark,
        store: SettingsStore,
    ) -> Self {
        apply_theme(context);
        context.set_zoom_factor(store.settings().zoom);
        egui_extras::install_image_loaders(context);
        let wake_context = context.clone();
        server.set_wake_callback(move || wake_context.request_repaint());
        let preferences = Preferences {
            theme: request.theme,
            ..Preferences::default()
        };
        benchmark.accept_request(&request);
        Self {
            tabs: vec![ReviewTab::new(request, context)],
            retired_workers: Vec::new(),
            selected_tab: 0,
            preferences,
            server: Some(server),
            benchmark,
            help_open: false,
            fallback_fonts: crate::fallback_fonts::Watch::default(),
            appearance: theme_menu::Appearance { store },
        }
    }

    /// Installs a system CJK font the first time a path or diff row needs one.
    fn ensure_fallback_fonts(&mut self, context: &egui::Context) {
        if self.fallback_fonts.is_done() {
            return;
        }
        for (index, tab) in self.tabs.iter().enumerate() {
            let files = Arc::as_ptr(&tab.files) as usize;
            let document = tab.presentation.as_ref().map_or(0, |presentation| {
                Arc::as_ptr(&presentation.document) as usize
            });
            if !self.fallback_fonts.changed(index, files, document) {
                continue;
            }
            // Valid UTF-8 names are borrowed; only a lossy conversion allocates.
            let names: Vec<_> = tab
                .files
                .iter()
                .map(|file| String::from_utf8_lossy(&file.path.0))
                .collect();
            let mut texts: Vec<&str> = names.iter().map(|name| name.as_ref()).collect();
            texts.push(&tab.root);
            if let Some(presentation) = &tab.presentation {
                for comparison in &presentation.document.document.comparisons {
                    texts.push(&comparison.label);
                    for row in &comparison.rows {
                        texts.push(&row.left);
                        texts.push(&row.right);
                    }
                }
            }
            if crate::fallback_fonts::missing_glyphs(context, texts) {
                crate::fallback_fonts::install(context);
                self.fallback_fonts.finish();
                context.request_repaint();
                return;
            }
        }
    }

    fn open(&mut self, request: OpenRequest, context: &egui::Context) {
        self.benchmark.accept_request(&request);
        self.preferences.theme |= request.theme;
        if let Some(index) = self.matching_tab(&request, cfg!(windows)) {
            self.selected_tab = index;
            let tab = &mut self.tabs[index];
            tab.reveal_requested = request.file.is_some();
            let document_mode = request.target == OpenTarget::Document;
            if tab.document_mode != document_mode
                || tab.view != request.view
                || request
                    .file
                    .as_ref()
                    .is_some_and(|file| Some(file) != tab.selected.as_ref())
            {
                tab.presentation = None;
                tab.clear_search_results();
            }
            tab.document_mode = document_mode;
            tab.selected = request.file.or_else(|| tab.selected.clone());
            tab.view = request.view;
            if tab.foreground {
                tab.refresh();
            }
        } else {
            self.tabs.push(ReviewTab::new(request, context));
            self.selected_tab = self.tabs.len() - 1;
        }
        self.sync_tab_activity();
        context.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn matching_tab(&self, request: &OpenRequest, native_windows: bool) -> Option<usize> {
        self.tabs.iter().position(|tab| {
            repository_identity::same_repository(&tab.request, request, native_windows)
                && tab.request.target == request.target
                && same_wsl(&tab.request, request)
        })
    }

    fn sync_tab_activity(&mut self) {
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let foreground = index == self.selected_tab;
            if foreground == tab.foreground {
                continue;
            }
            tab.foreground = foreground;
            if foreground {
                tab.refresh();
            } else {
                tab.generation += 1;
                tab.reflog_generation += 1;
                tab.reflog_pending = None;
                tab.reflog_loading = false;
            }
            if let Some(worker) = &tab.worker {
                worker.set_foreground(foreground);
            }
        }
    }

    fn close_tab(&mut self, index: usize) {
        if let Some(worker) = self.tabs[index].worker.take()
            && let Some(thread) = worker.retire()
        {
            self.retired_workers.push(thread);
        }
        self.tabs.remove(index);
        if index < self.selected_tab {
            self.selected_tab -= 1;
        }
        self.selected_tab = self.selected_tab.min(self.tabs.len().saturating_sub(1));
        self.sync_tab_activity();
    }

    fn collect_finished_workers(&mut self, context: &egui::Context) {
        let mut index = 0;
        while index < self.retired_workers.len() {
            if self.retired_workers[index].is_finished() {
                join_worker(self.retired_workers.swap_remove(index));
            } else {
                index += 1;
            }
        }
        if !self.retired_workers.is_empty() {
            context.request_repaint_after(Duration::from_millis(50));
        }
    }

    fn poll(&mut self, context: &egui::Context) {
        if let Some(server) = &self.server {
            match server.poll() {
                Ok(requests) => {
                    for request in requests {
                        self.open(request, context);
                    }
                }
                Err(error) => {
                    context.request_repaint();
                    if let Some(tab) = self.tabs.get_mut(self.selected_tab) {
                        tab.error = Some(format!("Could not accept another repository: {error}"));
                    }
                }
            }
        }
        self.sync_tab_activity();
        if let Some(tab) = self.tabs.get_mut(self.selected_tab) {
            tab.events(self.preferences.unified);
        }
        self.observe_benchmark();
    }

    /// Report the selected tab's settled state to the benchmark; a no-op without one.
    fn observe_benchmark(&mut self) {
        if !self.benchmark.is_measuring() {
            return;
        }
        let Some(tab) = self.tabs.get(self.selected_tab) else {
            return;
        };
        self.benchmark.observe(&TabState::new(
            TabLocus::new(&tab.request, &tab.root, tab.selected.as_ref(), tab.view),
            tab.document_mode,
            tab.loading,
            tab.error.is_some(),
            tab.presentation
                .as_ref()
                .map(|presentation| &presentation.document),
        ));
    }

    pub fn show(&mut self, context: &egui::Context) {
        self.benchmark.rendered(false);
        self.sync_zoom(context);
        self.shortcuts(context);
        let top_frame = egui::Frame::side_top_panel(&context.style()).inner_margin(egui::Margin {
            left: 8,
            right: 8,
            top: 2,
            bottom: TOOLBAR_PANEL_BOTTOM_MARGIN,
        });
        egui::TopBottomPanel::top("toolbar")
            .frame(top_frame)
            .show(context, |ui| {
                self.tab_bar(ui);
                if let Some(tab) = self.tabs.get_mut(self.selected_tab) {
                    Self::toolbar(tab, ui);
                }
            });
        if self.tabs.is_empty() {
            context.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let tab = &mut self.tabs[self.selected_tab];
        egui::TopBottomPanel::bottom("status").show(context, |ui| {
            ui.horizontal(|ui| {
                if tab.has_repository
                    && let Some(branch) = &tab.branch
                {
                    ui.scope(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(10.0, 18.0), egui::Sense::hover());
                        let stroke = egui::Stroke::new(1.0, ui.visuals().text_color());
                        let top = rect.min + egui::vec2(2.0, 5.0);
                        let bottom = rect.min + egui::vec2(2.0, 13.0);
                        let fork = rect.min + egui::vec2(8.0, 5.0);
                        ui.painter().line_segment([top, bottom], stroke);
                        ui.painter().add(egui::Shape::line(
                            vec![
                                fork,
                                rect.min + egui::vec2(8.0, 8.0),
                                rect.min + egui::vec2(2.0, 11.0),
                            ],
                            stroke,
                        ));
                        for center in [top, bottom, fork] {
                            ui.painter()
                                .circle(center, 1.4, ui.visuals().panel_fill, stroke);
                        }
                        // A truncated label already shows the full name on hover.
                        // add_sized would center the text; a left-to-right child keeps it at the glyph.
                        ui.allocate_ui_with_layout(
                            egui::vec2(
                                BRANCH_LABEL_MAX_WIDTH.min(ui.available_width() * 0.5),
                                18.0,
                            ),
                            // Main-justified: the label's hit area spans the whole box, so a drag
                            // can start anywhere beside the name instead of exactly on the text.
                            egui::Layout::left_to_right(egui::Align::Center)
                                .with_main_align(egui::Align::Min)
                                .with_main_justify(true),
                            |ui| {
                                // Selectable so the branch name can be copied.
                                ui.add(
                                    egui::Label::new(egui::RichText::new(branch))
                                        .truncate()
                                        .halign(egui::Align::Min),
                                )
                            },
                        );
                    });
                }
                if tab.loading {
                    ui.spinner();
                }
                let hints = shortcuts::binding_label("Alt+Up/Down files · Ctrl+F search · F1 help");
                let hint_width = if ui.available_width() >= 900.0 {
                    ui.painter()
                        .layout_no_wrap(
                            hints.to_owned(),
                            egui::TextStyle::Small.resolve(ui.style()),
                            ui.visuals().text_color(),
                        )
                        .size()
                        .x
                        + ui.spacing().item_spacing.x
                } else {
                    0.0
                };
                ui.add_sized(
                    [(ui.available_width() - hint_width).max(1.0), 18.0],
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "{} changed files · {}",
                            tab.files.len(),
                            tab.root
                        ))
                        .small(),
                    )
                    .truncate()
                    .selectable(false),
                )
                .on_hover_text(format!("{}\n{hints}", tab.root));
                if hint_width > 0.0 {
                    ui.small(hints);
                }
            });
        });
        let maximum_sidebar = (context.content_rect().width() * 0.4).max(1.0);
        if tab.has_repository && (!tab.document_mode || tab.reflog_open) {
            egui::SidePanel::left("files")
                .resizable(true)
                .default_width(260.0_f32.min(maximum_sidebar))
                .width_range(140.0_f32.min(maximum_sidebar)..=maximum_sidebar)
                .show(context, |ui| {
                    Self::sidebar(tab, ui, self.selected_tab, self.preferences.theme)
                });
        }
        egui::CentralPanel::default().show(context, |ui| {
            Self::path_row(tab, ui, &mut self.preferences.unified);
            if let Some(notice) = &tab.notice {
                ui.label(notice);
            }
            if let Some(error) = &tab.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if let Some(presentation) = &mut tab.presentation {
                presentation.set_layout(
                    self.preferences.unified,
                    ui.available_width(),
                    self.preferences.font_size,
                );
                if presentation.unified && !self.preferences.unified && !tab.document_mode {
                    ui.small("Unified · narrow window");
                }
                for comparison in &presentation.document.document.comparisons {
                    if let Some(summary) = &comparison.summary {
                        ui.label(format!("{}: {summary}", comparison.label));
                    }
                }
                if let Some(crate::render::DisplayLine::Limit { omitted }) =
                    presentation.lines.last()
                {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("Rendering limited to 100,000 rows; {omitted} rows omitted"),
                    );
                }
                let marker_visible = presentation.show_tracked(
                    ui,
                    egui::Id::new((
                        "diff",
                        self.selected_tab,
                        Arc::as_ptr(&presentation.document),
                        tab.view as u8,
                    )),
                    self.preferences.font_size,
                    self.benchmark.marker(&TabLocus::new(
                        &tab.request,
                        &tab.root,
                        tab.selected.as_ref(),
                        tab.view,
                    )),
                );
                self.benchmark.rendered(marker_visible);
            } else if tab.loading {
                ui.centered_and_justified(|ui| {
                    ui.label(if tab.document_mode {
                        "Loading document…"
                    } else {
                        "Loading current Git changes…"
                    });
                });
            } else if tab.error.is_none() {
                ui.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.label("No changes in this view");
                        ui.label(if tab.view == View::All {
                            "Check that this is the right repository, or refresh (F5)."
                        } else {
                            "Try All to see other changes, or refresh (F5)."
                        });
                    });
                });
            }
        });
        self.help(context);
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        let selection_id = ui.id().with("selected-repository-tab");
        let selection = (self.selected_tab, self.tabs.len());
        let reveal_selected = ui.data_mut(|data| {
            let previous = data.get_temp::<(usize, usize)>(selection_id);
            data.insert_temp(selection_id, selection);
            previous != Some(selection)
        });
        let titles: Vec<&str> = self
            .tabs
            .iter()
            .map(|tab| {
                if tab.request.target == OpenTarget::Document {
                    tab.request.repository.as_str()
                } else {
                    tab.root.as_str()
                }
            })
            .collect();
        let suffixes = tab_suffixes(&titles);
        let mut close = None;
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), TAB_ROW_HEIGHT),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                Self::global_controls(ui, &mut self.help_open);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    egui::ScrollArea::horizontal()
                        .id_salt("repository-tabs")
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                for (index, tab) in self.tabs.iter().enumerate() {
                                    let title = titles[index];
                                    let name = tab_name(title);
                                    let folder = suffixes[index].as_deref();
                                    let selected = index == self.selected_tab;
                                    let is_document = tab.request.target == OpenTarget::Document;
                                    Self::tab(
                                        ui,
                                        TabView {
                                            title,
                                            name,
                                            folder,
                                            selected,
                                            is_document,
                                            reveal: selected && reveal_selected,
                                        },
                                        |action| match action {
                                            TabAction::Select => self.selected_tab = index,
                                            TabAction::Close => close = Some(index),
                                        },
                                    );
                                }
                            });
                        });
                });
            },
        );
        ui.separator();
        if let Some(index) = close {
            self.close_tab(index);
        } else {
            self.sync_tab_activity();
        }
    }

    fn tab(ui: &mut egui::Ui, view: TabView, mut act: impl FnMut(TabAction)) {
        let TabView {
            title,
            name,
            folder,
            selected,
            is_document,
            reveal,
        } = view;
        let fill = if selected {
            ui.visuals().widgets.inactive.weak_bg_fill
        } else {
            egui::Color32::TRANSPARENT
        };
        let frame = egui::Frame::new()
            .fill(fill)
            .corner_radius(egui::CornerRadius {
                nw: 4,
                ne: 4,
                sw: 0,
                se: 0,
            })
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                // Allocate the full label height so the icon centers on the name. CLICK without
                // FOCUSABLE: the name button is the tab's keyboard stop, not the icon.
                let (slot, icon_response) =
                    ui.allocate_exact_size(egui::vec2(TAB_ICON_SIZE, 24.0), egui::Sense::CLICK);
                let icon_hovered = icon_response.hovered();
                if icon_response.clicked() {
                    act(TabAction::Select);
                }
                let icon =
                    egui::Rect::from_center_size(slot.center(), egui::Vec2::splat(TAB_ICON_SIZE));
                let icon_color = if selected {
                    crate::theme::active(ui.ctx()).app_theme().accent()
                } else {
                    ui.visuals().weak_text_color()
                };
                if is_document {
                    crate::sidebar::document_icon(ui, icon, icon_color);
                } else {
                    crate::sidebar::folder_icon(ui, icon, false, icon_color);
                }
                ui.add_space(4.0);
                let font = egui::TextStyle::Button.resolve(ui.style());
                let width = ui
                    .painter()
                    .layout_no_wrap(name.to_owned(), font, ui.visuals().text_color())
                    .size()
                    .x
                    .clamp(48.0, 180.0);
                let label = egui::RichText::new(name).color(if selected {
                    ui.visuals().strong_text_color()
                } else {
                    ui.visuals().text_color()
                });
                let response = ui.add_sized(
                    [width, 24.0],
                    egui::Button::new(label).frame(false).truncate(),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::SelectableLabel,
                        ui.is_enabled(),
                        selected,
                        name,
                    )
                });
                ui.ctx().accesskit_node_builder(response.id, |node| {
                    node.set_role(egui::accesskit::Role::Tab);
                    node.clear_toggled();
                    node.set_selected(selected);
                });
                if response.has_focus() {
                    ui.painter().rect_stroke(
                        response.rect,
                        2.0,
                        ui.visuals().selection.stroke,
                        egui::StrokeKind::Inside,
                    );
                }
                let mut tab_hovered = icon_hovered || response.hovered();
                if response.on_hover_text(title).clicked() {
                    act(TabAction::Select);
                }
                if let Some(folder) = folder {
                    ui.add_space(2.0);
                    let font = egui::TextStyle::Small.resolve(ui.style());
                    let width = ui
                        .painter()
                        .layout_no_wrap(folder.to_owned(), font, ui.visuals().weak_text_color())
                        .size()
                        .x
                        .min(TAB_SUFFIX_MAX_WIDTH);
                    let suffix = ui.add_sized(
                        [width, 24.0],
                        egui::Label::new(egui::RichText::new(folder).small().weak())
                            .selectable(false)
                            .truncate()
                            .sense(egui::Sense::CLICK),
                    );
                    tab_hovered |= suffix.hovered();
                    if suffix.clicked() {
                        act(TabAction::Select);
                    }
                }
                ui.add_space(TAB_CLOSE_GAP);
                let response = ui.add_sized([24.0, 24.0], egui::Button::new("").frame(false));
                let close_label = format!("Close {name} tab");
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        ui.is_enabled(),
                        &close_label,
                    )
                });
                if response.has_focus() {
                    ui.painter().rect_stroke(
                        response.rect,
                        2.0,
                        ui.visuals().selection.stroke,
                        egui::StrokeKind::Inside,
                    );
                }
                let center = response.rect.center();
                let color = if response.hovered() {
                    ui.visuals().strong_text_color()
                } else if selected || tab_hovered {
                    ui.visuals().text_color()
                } else {
                    ui.visuals().weak_text_color()
                };
                let stroke = egui::Stroke::new(1.2, color);
                for (a, b) in [((-3.0, -3.0), (3.0, 3.0)), ((-3.0, 3.0), (3.0, -3.0))] {
                    ui.painter().line_segment(
                        [center + egui::vec2(a.0, a.1), center + egui::vec2(b.0, b.1)],
                        stroke,
                    );
                }
                if response.on_hover_text(close_label).clicked() {
                    act(TabAction::Close);
                }
            });
        if selected {
            if reveal {
                frame.response.scroll_to_me(Some(egui::Align::Center));
            }
            ui.painter().line_segment(
                [
                    frame.response.rect.left_bottom(),
                    frame.response.rect.right_bottom(),
                ],
                egui::Stroke::new(2.0, crate::theme::active(ui.ctx()).app_theme().accent()),
            );
        }
    }

    /// Window-wide controls: they apply to every tab, so they sit beside the tabs, not in a tab's toolbar.
    /// Called inside a right-to-left layout, so the widgets are added in reverse visual order.
    fn global_controls(ui: &mut egui::Ui, help_open: &mut bool) {
        if ui.small_button("Help").clicked() {
            *help_open = true;
        }
        let zoom = ui.ctx().zoom_factor();
        if ui
            .small_button("UI +")
            .on_hover_text("Scale the whole interface")
            .clicked()
        {
            ui.ctx().set_zoom_factor((zoom + 0.1).clamp(0.75, 2.0));
        }
        ui.small(format!("{:.0}%", zoom * 100.0));
        if ui
            .small_button("UI −")
            .on_hover_text("Scale the whole interface")
            .clicked()
        {
            ui.ctx().set_zoom_factor((zoom - 0.1).clamp(0.75, 2.0));
        }
    }

    fn toolbar(tab: &mut ReviewTab, ui: &mut egui::Ui) {
        ui.add_space(TOOLBAR_TOP_GAP);
        if !toolbar_needs_two_rows(ui.available_width(), !tab.search.is_empty()) {
            ui.horizontal(|ui| {
                Self::search_controls(tab, ui);
                Self::mode_controls(tab, ui);
                Self::view_controls(tab, ui, true);
            });
        } else {
            // Last-resort per-widget wrapping for very high UI zoom on the narrowest window.
            ui.horizontal_wrapped(|ui| {
                Self::search_controls(tab, ui);
                Self::mode_controls(tab, ui);
            });
            if tab.has_repository {
                ui.horizontal_wrapped(|ui| Self::view_controls(tab, ui, false));
            }
        }
    }

    fn search_controls(tab: &mut ReviewTab, ui: &mut egui::Ui) {
        // The field and its arrows sit 1px apart; the scope keeps the normal gap after them.
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            Self::search_field(tab, ui);
        });
        if !tab.search.is_empty() {
            ui.small(format!("{} matches", tab.search_hits.len()));
        }
    }

    fn search_field(tab: &mut ReviewTab, ui: &mut egui::Ui) {
        let search_label = if tab.document_mode {
            "Search document"
        } else {
            "Search diff"
        };
        let response = ui.add(
            egui::TextEdit::singleline(&mut tab.search)
                .id(egui::Id::new("diff-search"))
                .hint_text(search_label)
                .return_key(None)
                .desired_width(150.0_f32.min(ui.available_width().max(40.0))),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, ui.is_enabled(), search_label)
        });
        if response.changed() {
            tab.clear_search_results();
            tab.request_search();
        }
        if response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            tab.navigate_search(!ui.input(|input| input.modifiers.shift));
        }
        if search_arrow(ui, true, "Previous match", "Previous match (Shift+F3)").clicked() {
            tab.navigate_search(false);
        }
        if search_arrow(ui, false, "Next match", "Next match (F3)").clicked() {
            tab.navigate_search(true);
        }
    }

    fn mode_controls(tab: &mut ReviewTab, ui: &mut egui::Ui) {
        if !(tab.has_repository
            || tab.selected.is_some()
            || tab.request.target == OpenTarget::Document)
        {
            return;
        }
        ui.separator();
        if ui
            .add_enabled(
                tab.full_available
                    && (tab.selected.is_some() || tab.request.target == OpenTarget::Document),
                egui::Button::selectable(tab.document_mode, "Full file"),
            )
            .clicked()
        {
            tab.toggle_document(true);
        }
        if ui
            .add_enabled(
                tab.has_repository,
                egui::Button::selectable(!tab.document_mode, "Git Diff"),
            )
            .clicked()
        {
            tab.toggle_document(false);
        }
    }

    fn view_controls(tab: &mut ReviewTab, ui: &mut egui::Ui, leading_separator: bool) {
        if !tab.has_repository {
            return;
        }
        if leading_separator {
            ui.separator();
        }
        for (label, view) in [
            ("All", View::All),
            ("Unstaged", View::Unstaged),
            ("Staged", View::Staged),
            ("Unpushed", View::Unpushed),
        ] {
            let response = ui.add_enabled(
                !tab.document_mode,
                egui::Button::selectable(tab.view == view, label),
            );
            let response = if view == View::All {
                response.on_hover_text(
                    "Unpushed commits, staged edits, unstaged edits, and untracked files",
                )
            } else {
                response
            };
            if response.clicked() {
                tab.set_view(view);
            }
        }
    }

    /// Selected file or document path with the actions that apply to it. Always rendered so Copy
    /// and the layout choice stay in one place.
    fn path_row(tab: &mut ReviewTab, ui: &mut egui::Ui, unified: &mut bool) {
        let path = if tab.request.target == OpenTarget::Document {
            Some(tab.request.repository.clone())
        } else {
            tab.selected.as_ref().map(GitPath::display)
        };
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), ui.spacing().interact_size.y),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                Self::copy_menu(tab, ui);
                ui.add_enabled_ui(!tab.document_mode, |ui| {
                    for (label, value) in [("Unified", true), ("Side by side", false)] {
                        if ui
                            .add(egui::Button::selectable(*unified == value, label))
                            .on_hover_text(shortcuts::binding_label(
                                "Diff layout for all tabs (Ctrl+Shift+U)",
                            ))
                            .clicked()
                        {
                            *unified = value;
                        }
                    }
                });
                let path_layout = egui::Layout::left_to_right(egui::Align::Center)
                    .with_main_align(egui::Align::Min)
                    .with_main_justify(true);
                ui.with_layout(path_layout, |ui| {
                    if let Some(path) = &path {
                        // A truncated label already shows the full path on hover.
                        ui.add(
                            egui::Label::new(egui::RichText::new(path).monospace())
                                .truncate()
                                .halign(egui::Align::Min),
                        );
                    }
                });
            },
        );
        ui.separator();
    }

    fn copy_menu(tab: &mut ReviewTab, ui: &mut egui::Ui) {
        ui.menu_button("Copy", |ui| {
            let path = if tab.request.target == OpenTarget::Document {
                Some(tab.request.repository.clone())
            } else {
                tab.selected.as_ref().map(GitPath::display)
            };
            if ui
                .add_enabled(path.is_some(), egui::Button::new("Copy path"))
                .clicked()
            {
                if let Some(path) = path {
                    ui.ctx().copy_text(path);
                }
                ui.close();
            }
            ui.separator();
            let available = !tab.document_mode
                && tab.presentation.as_ref().is_some_and(|p| {
                    p.document
                        .document
                        .comparisons
                        .get(tab.comparison)
                        .is_some()
                });
            ui.add_enabled_ui(available, |ui| {
                if let Some(presentation) = &mut tab.presentation {
                    let comparisons = &presentation.document.document.comparisons;
                    if let Some(comparison) = comparisons.get(tab.comparison) {
                        ui.label(format!("Copy comparison: {}", comparison.label));
                    }
                    if comparisons.len() > 1 {
                        let previous = tab.comparison;
                        for (index, comparison) in comparisons.iter().enumerate() {
                            ui.selectable_value(
                                &mut tab.comparison,
                                index,
                                format!("{} comparison", comparison.label),
                            );
                        }
                        if previous != tab.comparison {
                            presentation.clear_selection();
                        }
                        ui.separator();
                    }
                }
                for (label, source) in [("Copy patch", false), ("Copy source", true)] {
                    if ui.button(label).clicked() {
                        tab.copy_comparison(ui.ctx(), source);
                        ui.close();
                    }
                }
            });
        });
    }

    fn sidebar(tab: &mut ReviewTab, ui: &mut egui::Ui, tab_index: usize, theme: bool) {
        let height = ui.available_height();
        let header_height = ui.spacing().interact_size.y + 8.0;
        let panel_height = if tab.request.target == OpenTarget::Document {
            height
        } else if tab.reflog_open {
            (height * 0.4).max(header_height)
        } else {
            header_height
        };
        let panel = egui::TopBottomPanel::bottom(egui::Id::new((
            "reflog-panel",
            tab_index,
            tab.reflog_open,
        )));
        let panel = if tab.reflog_open && tab.request.target != OpenTarget::Document {
            panel
                .resizable(true)
                .default_height(panel_height)
                .height_range(header_height..=(height - header_height * 3.0).max(header_height))
        } else {
            panel.exact_height(panel_height.min(height))
        };
        panel.show_inside(ui, |ui| {
            let response = egui::CollapsingHeader::new("Reflog")
                .id_salt(("reflog-disclosure", tab_index))
                .open(Some(tab.reflog_open))
                .show(ui, |ui| tab.show_reflog(ui, tab_index, theme));
            if response.header_response.clicked() {
                tab.toggle_reflog();
            }
        });
        if tab.request.target == OpenTarget::Document {
            return;
        }
        ui.shrink_clip_rect(ui.available_rect_before_wrap());
        ui.add_space(SIDEBAR_TOP_GAP);
        let previous_filter = tab.filter.clone();
        let selected = tab.tree.show(
            ui,
            &tab.root,
            &tab.files,
            &mut tab.filter,
            tab.selected.as_ref(),
            theme,
        );
        if previous_filter != tab.filter {
            tab.visible_files = tab.tree.filtered_files(&tab.filter);
        }
        if let Some(index) = selected {
            tab.select_file(index, false);
        }
    }
}

/// Chevron button drawn with the painter: egui's default fonts have no arrow glyphs.
fn search_arrow(ui: &mut egui::Ui, up: bool, name: &str, hint: &str) -> egui::Response {
    let response = ui
        .add_sized([20.0, 20.0], egui::Button::new(""))
        .on_hover_text(hint);
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), name));
    let color = if response.hovered() {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().text_color()
    };
    let tip = if up { -2.5 } else { 2.5 };
    let center = response.rect.center();
    ui.painter().add(egui::Shape::line(
        vec![
            center + egui::vec2(-4.0, -tip),
            center + egui::vec2(0.0, tip),
            center + egui::vec2(4.0, -tip),
        ],
        egui::Stroke::new(1.4, color),
    ));
    response
}

fn join_worker(worker: JoinHandle<()>) {
    if worker.join().is_err() {
        eprintln!("fastdiff worker: thread panicked during cleanup");
    }
}

fn same_wsl(left: &OpenRequest, right: &OpenRequest) -> bool {
    match (&left.wsl, &right.wsl) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.distribution == right.distribution
                && left.user == right.user
                && left.helper == right.helper
        }
        _ => false,
    }
}

impl eframe::App for Fastdiff {
    fn update(&mut self, context: &egui::Context, frame: &mut eframe::Frame) {
        self.benchmark.record_frame(frame.info().cpu_usage);
        self.poll(context);
        self.ensure_fallback_fonts(context);
        self.show(context);
        self.collect_finished_workers(context);
        if !context.will_discard() {
            self.benchmark.frame_ready();
        }
        self.benchmark.tick(context);
    }
}

impl Drop for Fastdiff {
    fn drop(&mut self) {
        for tab in &mut self.tabs {
            if let Some(worker) = tab.worker.take()
                && let Some(thread) = worker.retire()
            {
                self.retired_workers.push(thread);
            }
        }
        self.tabs.clear();
        for worker in self.retired_workers.drain(..) {
            join_worker(worker);
        }
        self.appearance.store.flush();
        self.server.take();
        self.benchmark.finish();
    }
}

/// Verbatim Windows roots (`\\?\C:\...`) bypass `/` normalization, so Git's `/`-separated
/// relative paths must use the native separator.
fn join_document_path(root: &str, relative: &str, separator: char) -> String {
    let relative = if separator == '\\' {
        relative.replace('/', "\\")
    } else {
        relative.to_owned()
    };
    format!(
        "{}{}{}",
        root.trim_end_matches(separator),
        separator,
        relative
    )
}

#[cfg(test)]
mod tests;
