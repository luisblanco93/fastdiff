//! Repository IO, filesystem notifications, and source tokenization stay off the UI thread.
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use fastdiff_cli::{Backend, OpenRequest, OpenTarget};
use fastdiff_core::{CancelToken, DiffDocument, FileChange, ReflogList, ReflogRequest, View};
use unicode_width::UnicodeWidthChar;

use crate::{
    highlight::{PREVIEW_SUFFIX, Span, highlight, preview_len},
    render::{DisplayLine, make_lines},
    sidebar::TreeModel,
};

mod cache;
mod watch;
use cache::PreparedCache;
mod reflog;
use reflog::ReflogState;

#[cfg(test)]
mod cache_worker_tests;

#[cfg(test)]
mod reflog_tests;

#[cfg(test)]
mod lifecycle_tests;

#[cfg(all(test, unix))]
mod recovery_tests;

pub struct PreparedRow {
    pub comparison: usize,
    pub index: usize,
    pub left: Vec<Span>,
    pub right: Vec<Span>,
}

const LONG_LINE_BYTES: usize = 4096;
const CHECKPOINT_CHARACTERS: usize = 256;

#[derive(Clone, Copy, Default)]
pub(crate) struct LineCheckpoint {
    pub row: usize,
    pub byte: usize,
    pub character: usize,
    pub columns: usize,
}

pub struct PreparedDocument {
    pub is_document: bool,
    pub document: DiffDocument,
    pub rows: Vec<PreparedRow>,
    pub context: Vec<bool>,
    pub max_columns: usize,
    pub left_columns: usize,
    pub right_columns: usize,
    pub side_lines: Arc<[DisplayLine]>,
    pub unified_lines: Arc<[DisplayLine]>,
    pub(crate) long_line_checkpoints: Vec<LineCheckpoint>,
}

impl PreparedDocument {
    pub fn new(document: DiffDocument, cancel: &CancelToken) -> Self {
        let rows: Vec<PreparedRow> = document
            .comparisons
            .iter()
            .enumerate()
            .flat_map(|(comparison, item)| {
                item.rows
                    .iter()
                    .enumerate()
                    .map(move |(index, row)| PreparedRow {
                        comparison,
                        index,
                        left: if cancel.is_cancelled() {
                            Vec::new()
                        } else {
                            highlight(&row.left)
                        },
                        right: if cancel.is_cancelled() {
                            Vec::new()
                        } else {
                            highlight(&row.right)
                        },
                    })
            })
            .collect();
        let (left_columns, right_columns) = rows.iter().fold((0, 0), |(left, right), prepared| {
            let row = &document.comparisons[prepared.comparison].rows[prepared.index];
            (
                left.max(display_columns(&row.left)),
                right.max(display_columns(&row.right)),
            )
        });
        let max_columns = left_columns.max(right_columns);
        let context = rows
            .iter()
            .map(|prepared| {
                document.comparisons[prepared.comparison].rows[prepared.index].kind
                    == fastdiff_core::RowKind::Context
            })
            .collect();
        let mut prepared = Self {
            is_document: false,
            document,
            rows,
            context,
            max_columns,
            left_columns,
            right_columns,
            side_lines: Arc::from([]),
            unified_lines: Arc::from([]),
            long_line_checkpoints: Vec::new(),
        };
        prepared.side_lines = make_lines(&prepared, false, &Default::default()).into();
        prepared.unified_lines = make_lines(&prepared, true, &Default::default()).into();
        prepared
    }
    pub fn from_text(_path: String, text: String, cancel: &CancelToken) -> Self {
        let rows = text
            .lines()
            .enumerate()
            .map(|(index, line)| fastdiff_core::DiffRow {
                kind: fastdiff_core::RowKind::Context,
                old_number: None,
                new_number: Some(index + 1),
                left: String::new(),
                right: line.to_owned(),
            })
            .collect();
        let mut prepared = Self::new(
            DiffDocument {
                comparisons: vec![fastdiff_core::Comparison {
                    label: "Full file".into(),
                    patch: String::new(),
                    summary: None,
                    rows,
                }],
            },
            cancel,
        );
        prepared.is_document = true;
        let mut checkpoints = Vec::new();
        prepared.right_columns = prepared.document.comparisons[0]
            .rows
            .iter()
            .enumerate()
            .map(|(row, source)| {
                let indexed = source.right.len() > LONG_LINE_BYTES;
                let mut columns = 0;
                let mut characters = 0;
                for (byte, character) in source.right.char_indices() {
                    if characters % CHECKPOINT_CHARACTERS == 0 {
                        if cancel.is_cancelled() {
                            return 0;
                        }
                        if indexed {
                            checkpoints.push(LineCheckpoint {
                                row,
                                byte,
                                character: characters,
                                columns,
                            });
                        }
                    }
                    columns += character_columns(character);
                    characters += 1;
                }
                if indexed {
                    checkpoints.push(LineCheckpoint {
                        row,
                        byte: source.right.len(),
                        character: characters,
                        columns,
                    });
                }
                columns
            })
            .max()
            .unwrap_or(0);
        if !cancel.is_cancelled() {
            prepared.long_line_checkpoints = checkpoints;
        }
        prepared.max_columns = prepared.right_columns;
        prepared.context.fill(false);
        prepared.side_lines = make_lines(&prepared, true, &Default::default()).into();
        prepared.unified_lines = Arc::clone(&prepared.side_lines);
        prepared
    }

    pub(crate) fn line_checkpoints(&self, row: usize) -> &[LineCheckpoint] {
        let start = self
            .long_line_checkpoints
            .partition_point(|checkpoint| checkpoint.row < row);
        let count =
            self.long_line_checkpoints[start..].partition_point(|checkpoint| checkpoint.row == row);
        &self.long_line_checkpoints[start..start + count]
    }

    pub(crate) fn line_character_count(&self, row: usize) -> Option<usize> {
        self.line_checkpoints(row)
            .last()
            .map(|checkpoint| checkpoint.character)
    }
}

pub(crate) fn character_columns(character: char) -> usize {
    if character == '\t' {
        4
    } else {
        character.width().unwrap_or(1).max(1)
    }
}

fn display_columns(text: &str) -> usize {
    let end = preview_len(text);
    let columns: usize = text[..end]
        .chars()
        .map(|character| {
            // egui advances a tab by four monospace spaces (epaint::text::TAB_SIZE).
            if character == '\t' {
                4
            } else {
                character.width().unwrap_or(1)
            }
        })
        .sum();
    columns
        + if end < text.len() {
            PREVIEW_SUFFIX.chars().count()
        } else {
            0
        }
}

#[derive(Default)]
struct PreparedStatus {
    files: Arc<Vec<FileChange>>,
    model: Arc<TreeModel>,
}

impl PreparedStatus {
    fn prepare(&self, files: Vec<FileChange>, token: &CancelToken) -> Option<Self> {
        if token.is_cancelled() {
            return None;
        }
        let equal = files == *self.files;
        let same_paths = equal
            || files
                .iter()
                .map(|file| &file.path)
                .eq(self.files.iter().map(|file| &file.path));
        let model = if same_paths {
            Arc::clone(&self.model)
        } else {
            Arc::new(TreeModel::new(&files))
        };
        let files = if equal {
            Arc::clone(&self.files)
        } else {
            Arc::new(files)
        };
        (!token.is_cancelled()).then_some(Self { files, model })
    }
}

pub(crate) enum Event {
    Ready(String, bool),
    Branch {
        generation: u64,
        branch: Option<String>,
    },
    Reflog {
        generation: u64,
        list: ReflogList,
    },
    ReflogError {
        generation: u64,
        message: String,
    },
    Notice {
        generation: u64,
        message: Option<String>,
    },
    DocumentPath {
        generation: u64,
        path: String,
    },
    Availability {
        generation: u64,
        available: bool,
    },
    Status {
        generation: u64,
        automatic: bool,
        files: Arc<Vec<FileChange>>,
        model: Arc<TreeModel>,
    },
    Diff {
        generation: u64,
        document: Arc<PreparedDocument>,
    },
    Search {
        generation: u64,
        request_id: u64,
        query: String,
        hits: Vec<usize>,
    },
    Error {
        generation: u64,
        message: String,
    },
}

enum Command {
    Reflog {
        generation: u64,
        enabled: bool,
    },
    ReflogMore {
        generation: u64,
        request: ReflogRequest,
    },
    Document {
        generation: u64,
        path: String,
        view: View,
    },
    Refresh {
        generation: u64,
        view: View,
    },
    Diff {
        generation: u64,
        view: View,
        file: FileChange,
    },
    Search {
        generation: u64,
        request_id: u64,
        query: String,
    },
}

#[derive(Default)]
struct Pending {
    reflog: Option<Command>,
    work: Option<Command>,
    search: Option<Command>,
}

struct Shared {
    pending: Mutex<Pending>,
    events: Mutex<VecDeque<Event>>,
    active: Mutex<CancelToken>,
    latest_search_id: AtomicU64,
    foreground: AtomicBool,
    stopped: AtomicBool,
    dirty: AtomicBool,
    initialized: AtomicBool,
}

pub struct Worker {
    wake: SyncSender<()>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    #[cfg(test)]
    pub(crate) fn test_with_thread(thread: JoinHandle<()>) -> (Self, CancelToken) {
        let (wake, _) = mpsc::sync_channel(1);
        let token = CancelToken::default();
        let shared = Arc::new(Shared {
            pending: Mutex::new(Pending::default()),
            events: Mutex::new(VecDeque::new()),
            active: Mutex::new(token.clone()),
            latest_search_id: AtomicU64::new(0),
            foreground: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            initialized: AtomicBool::new(false),
        });
        (
            Self {
                wake,
                shared,
                thread: Some(thread),
            },
            token,
        )
    }

    pub fn spawn(request: OpenRequest, context: egui::Context) -> Self {
        let (wake, receiver) = mpsc::sync_channel(1);
        let _ = wake.try_send(());
        let shared = Arc::new(Shared {
            pending: Mutex::new(Pending::default()),
            events: Mutex::new(VecDeque::new()),
            active: Mutex::new(CancelToken::default()),
            latest_search_id: AtomicU64::new(0),
            foreground: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            initialized: AtomicBool::new(false),
        });
        let thread_shared = Arc::clone(&shared);
        let thread_wake = wake.clone();
        let worker_thread = thread::spawn(move || {
            run(request, receiver, thread_wake, context, thread_shared);
        });
        Self {
            wake,
            shared,
            thread: Some(worker_thread),
        }
    }

    fn cancel(&self) {
        if let Ok(active) = self.shared.active.lock() {
            active.cancel();
        }
    }

    fn stop(&self) {
        self.shared.stopped.store(true, Ordering::Release);
        self.cancel();
        let _ = self.wake.try_send(());
    }

    pub(crate) fn retire(mut self) -> Option<JoinHandle<()>> {
        self.stop();
        self.thread.take()
    }

    pub fn set_foreground(&self, foreground: bool) {
        if self.shared.foreground.swap(foreground, Ordering::AcqRel) == foreground {
            return;
        }
        if !foreground && self.shared.initialized.load(Ordering::Acquire) {
            self.cancel();
        }
        let _ = self.wake.try_send(());
    }

    pub fn refresh(&self, generation: u64, view: View) {
        self.submit(Command::Refresh { generation, view });
    }

    pub fn reflog(&self, generation: u64, enabled: bool) {
        self.submit(Command::Reflog {
            generation,
            enabled,
        });
    }

    pub fn reflog_more(&self, generation: u64, request: ReflogRequest) {
        self.submit(Command::ReflogMore {
            generation,
            request,
        });
    }

    pub fn diff(&self, generation: u64, view: View, file: FileChange) {
        self.submit(Command::Diff {
            generation,
            view,
            file,
        });
    }

    pub fn document(&self, generation: u64, path: String, view: View) {
        self.submit(Command::Document {
            generation,
            path,
            view,
        });
    }

    pub fn search(&self, generation: u64, request_id: u64, query: String) {
        self.submit(Command::Search {
            generation,
            request_id,
            query,
        });
    }

    fn submit(&self, command: Command) {
        if let Ok(mut pending) = self.shared.pending.lock() {
            if matches!(command, Command::Reflog { .. }) {
                pending.reflog = Some(command);
            } else if matches!(command, Command::ReflogMore { .. }) {
                if pending.reflog.is_none() {
                    pending.reflog = Some(command);
                }
            } else if let Command::Search { request_id, .. } = &command {
                self.shared
                    .latest_search_id
                    .store(*request_id, Ordering::Release);
                pending.search = Some(command);
            } else {
                if self.shared.initialized.load(Ordering::Acquire) {
                    self.cancel();
                }
                pending.work = Some(command);
                pending.search = None;
            }
        }
        let _ = self.wake.try_send(());
    }

    pub(crate) fn drain_events(&self) -> Vec<Event> {
        self.shared
            .events
            .lock()
            .map(|mut events| events.drain(..).collect())
            .unwrap_or_default()
    }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
        if let Some(worker) = self.thread.take()
            && worker.join().is_err()
        {
            eprintln!("fastdiff worker: thread panicked during shutdown");
        }
    }
}

/// Latest point at which a deferred watcher is built when no diff was requested.
const WATCHER_DEFER_LIMIT: Duration = Duration::from_millis(1500);

/// A deferred watcher is built once the first diff is shown, or at the deadline (counted from
/// the first published status) when none is.
fn watcher_due(
    deferred: bool,
    first_diff_shown: bool,
    now: Instant,
    deadline: Option<Instant>,
) -> bool {
    deferred && (first_diff_shown || deadline.is_some_and(|at| now >= at))
}

fn run(
    request: OpenRequest,
    receiver: Receiver<()>,
    wake: SyncSender<()>,
    context: egui::Context,
    shared: Arc<Shared>,
) {
    run_with_discovery(
        request,
        receiver,
        wake,
        context,
        shared,
        Backend::discover_with_cancel,
    );
}

fn run_with_discovery(
    request: OpenRequest,
    receiver: Receiver<()>,
    wake: SyncSender<()>,
    context: egui::Context,
    shared: Arc<Shared>,
    mut discover: impl FnMut(&OpenRequest, &CancelToken) -> fastdiff_cli::Result<Backend>,
) {
    let Some(startup_cancel) = activate(&shared) else {
        return;
    };
    let mut backend = match discover(&request, &startup_cancel) {
        Ok(backend) => backend,
        Err(error) => {
            emit(
                &shared,
                &context,
                Event::Error {
                    generation: 0,
                    message: error.to_string(),
                },
            );
            return;
        }
    };
    shared.initialized.store(true, Ordering::Release);
    emit(
        &shared,
        &context,
        Event::Ready(backend.root().to_owned(), backend.has_repository()),
    );
    let mut document_path =
        (request.target == OpenTarget::Document).then(|| request.repository.clone());
    // Building the watcher costs hundreds of milliseconds on Windows and nothing needs it
    // before the first diff is shown, so a repository review creates it afterwards. The 2 s
    // poll covers the gap. Document targets still watch from the start.
    let mut watcher_deferred = request.wsl.is_none() && document_path.is_none();
    let mut watcher_deadline: Option<Instant> = None;
    let mut first_diff_shown = false;
    let mut watcher = (request.wsl.is_none() && !watcher_deferred).then(|| {
        watch::WatchCoverage::new(
            &backend,
            document_path.as_deref(),
            wake.clone(),
            &startup_cancel,
        )
    });
    if let Some(watcher) = &mut watcher {
        report_watch_diagnostics(watcher.take_diagnostics());
    }
    let mut generation = 0;
    let mut view = request.view;
    let mut next_poll = Instant::now();
    let mut explicit_refresh = true;
    let mut dirty: Option<Instant> = None;
    let mut watch_dirty: Option<Instant> = None;
    let mut was_foreground = true;
    let mut current_document: Option<Arc<PreparedDocument>> = None;
    let mut current_status = PreparedStatus::default();
    let mut prepared_cache = PreparedCache::default();
    let mut reflog = ReflogState::default();
    let mut next_reflog_poll = Instant::now();
    while !shared.stopped.load(Ordering::Acquire) {
        if shared.foreground.load(Ordering::Acquire) {
            let _ = receiver.recv_timeout(Duration::from_millis(50));
        } else {
            let _ = receiver.recv();
        }
        if shared.stopped.load(Ordering::Acquire) {
            break;
        }
        if !shared.foreground.load(Ordering::Acquire) {
            was_foreground = false;
            continue;
        }
        if !was_foreground {
            was_foreground = true;
            if let Some(watcher) = &mut watcher
                && let Some(token) = activate(&shared)
            {
                report_watch_diagnostics(watcher.rebuild(&token));
            }
        }
        if watcher
            .as_ref()
            .is_some_and(watch::WatchCoverage::take_received)
        {
            watch_dirty = Some(Instant::now() + Duration::from_millis(250));
        }
        if shared.dirty.swap(false, Ordering::AcqRel) {
            dirty = Some(Instant::now() + Duration::from_millis(250));
        }
        let job = shared
            .pending
            .lock()
            .ok()
            .and_then(|mut pending| take_pending_job(&shared, &mut pending));
        if backend.needs_reconnect()
            && let Some((
                Command::Refresh {
                    generation: requested,
                    ..
                }
                | Command::Document {
                    generation: requested,
                    ..
                },
                token,
            )) = &job
        {
            if token.is_cancelled() {
                continue;
            }
            match discover(&request, token) {
                Ok(reconnected) if !token.is_cancelled() => {
                    drop(std::mem::replace(&mut backend, reconnected));
                    current_document = None;
                    current_status = PreparedStatus::default();
                    prepared_cache = PreparedCache::default();
                    if let Some(reflog_generation) = reflog.generation {
                        reflog.open(reflog_generation, true);
                    }
                    next_reflog_poll = Instant::now();
                    if token.is_cancelled() {
                        continue;
                    }
                }
                Err(error) if !token.is_cancelled() => {
                    emit(
                        &shared,
                        &context,
                        Event::Error {
                            generation: *requested,
                            message: error.to_string(),
                        },
                    );
                    continue;
                }
                _ => continue,
            }
        }
        match job {
            Some((
                Command::Reflog {
                    generation: requested,
                    enabled,
                },
                _,
            )) => {
                reflog.open(requested, enabled);
                next_reflog_poll = Instant::now();
            }
            Some((
                Command::ReflogMore {
                    generation,
                    request,
                },
                _,
            )) => {
                reflog.more(generation, request);
            }
            Some((
                Command::Document {
                    generation: requested,
                    path,
                    view: requested_view,
                },
                token,
            )) => {
                generation = requested;
                document_path = Some(path);
                if let Some(watcher) = &mut watcher {
                    report_watch_diagnostics(
                        watcher.update_document(document_path.as_deref(), &token),
                    );
                }
                view = requested_view;
                explicit_refresh = true;
                next_poll = Instant::now();
            }

            Some((
                Command::Refresh {
                    generation: requested,
                    view: requested_view,
                },
                token,
            )) => {
                generation = requested;
                view = requested_view;
                explicit_refresh = true;
                document_path = None;
                if let Some(watcher) = &mut watcher {
                    report_watch_diagnostics(watcher.update_document(None, &token));
                    report_watch_diagnostics(watcher.rebuild(&token));
                }
                if current_document
                    .as_ref()
                    .is_some_and(|document| document.is_document)
                {
                    current_document = None;
                }
                if let Some(reflog_generation) = reflog.generation {
                    reflog.open(reflog_generation, true);
                }
                next_poll = Instant::now();
            }
            Some((
                Command::Diff {
                    generation: requested,
                    view: requested_view,
                    file,
                },
                token,
            )) => {
                generation = requested;
                view = requested_view;
                document_path = None;
                if let Some(watcher) = &mut watcher {
                    report_watch_diagnostics(watcher.update_document(None, &token));
                }
                let availability_error = match backend.document_exists(&file.path, &token) {
                    Ok(available) => {
                        emit(
                            &shared,
                            &context,
                            Event::Availability {
                                generation,
                                available,
                            },
                        );
                        None
                    }
                    Err(error) if !token.is_cancelled() => {
                        emit(
                            &shared,
                            &context,
                            Event::Availability {
                                generation,
                                available: false,
                            },
                        );
                        Some(error.to_string())
                    }
                    _ => continue,
                };
                if token.is_cancelled() || backend.needs_reconnect() {
                    if let Some(message) = availability_error
                        && !token.is_cancelled()
                    {
                        emit(
                            &shared,
                            &context,
                            Event::Error {
                                generation,
                                message,
                            },
                        );
                    }
                    continue;
                }
                match backend.diff(&file, view, &token) {
                    Ok(document) if !token.is_cancelled() => {
                        let document = if let Some(current) = &current_document
                            && !current.is_document
                            && current.document == document
                            && !token.is_cancelled()
                        {
                            Arc::clone(current)
                        } else if let Some(cached) =
                            prepared_cache.get(view, &file, &document, &token)
                        {
                            cached
                        } else {
                            if token.is_cancelled() {
                                continue;
                            }
                            Arc::new(PreparedDocument::new(document, &token))
                        };
                        let candidate =
                            prepared_cache.candidate(view, &file, Arc::clone(&document), &token);
                        let mut retired = Vec::new();
                        let mut previous = None;
                        let mut retired_event = None;
                        // Serialize only admission and publication with cancellation.
                        if let Ok(_active) = shared.active.lock()
                            && !token.is_cancelled()
                        {
                            retired = prepared_cache.admit(candidate, &token);
                            previous = current_document.replace(Arc::clone(&document));
                            retired_event = queue_event(
                                &shared,
                                &context,
                                Event::Diff {
                                    generation,
                                    document,
                                },
                            );
                        }
                        drop((retired, previous, retired_event));
                    }
                    Err(error) if !token.is_cancelled() => emit(
                        &shared,
                        &context,
                        Event::Error {
                            generation,
                            message: error.to_string(),
                        },
                    ),
                    _ => {}
                }
                if let Some(message) = availability_error {
                    emit(
                        &shared,
                        &context,
                        Event::Error {
                            generation,
                            message,
                        },
                    );
                }
                first_diff_shown |= !token.is_cancelled();
            }
            Some((
                Command::Search {
                    generation: requested,
                    request_id,
                    query,
                },
                token,
            )) => {
                if let Some(hits) = search_document(current_document.as_deref(), &query, || {
                    search_current(&shared, &token, request_id)
                }) {
                    publish_search(
                        &shared, &context, &token, requested, request_id, query, hits,
                    );
                }
            }
            None => {}
        }
        if shared.stopped.load(Ordering::Acquire) {
            break;
        }
        if backend.needs_reconnect() {
            continue;
        }
        if watcher_due(
            watcher_deferred,
            first_diff_shown,
            Instant::now(),
            watcher_deadline,
        ) && shared.pending.lock().is_ok_and(|pending| {
            pending.work.is_none() && pending.reflog.is_none() && pending.search.is_none()
        }) && let Some(token) = activate(&shared)
        {
            // A request that arrives mid-build cancels the token; keep deferring and retry.
            if let Some(mut created) = watch::WatchCoverage::new_unless_cancelled(
                &backend,
                document_path.as_deref(),
                wake.clone(),
                &token,
            ) {
                report_watch_diagnostics(created.take_diagnostics());
                watcher = Some(created);
                watcher_deferred = false;
            }
        }
        if watch_dirty.is_some_and(|at| Instant::now() >= at)
            && let Some(token) = activate(&shared)
            && let Some(watcher) = &mut watcher
        {
            watch_dirty = None;
            let change = watcher.take_pending(&token);
            report_watch_diagnostics(change.diagnostics);
            if change.rebuild {
                report_watch_diagnostics(watcher.rebuild(&token));
            }
            if change.relevant || token.is_cancelled() {
                dirty = Some(Instant::now());
            }
        }
        if reflog.generation.is_some()
            && (reflog.request.is_some()
                || Instant::now() >= next_reflog_poll
                || dirty.is_some_and(|at| Instant::now() >= at))
        {
            let token = shared.pending.lock().ok().and_then(|pending| {
                if pending.work.is_some() || pending.reflog.is_some() {
                    None
                } else {
                    activate(&shared)
                }
            });
            if let Some(token) = token {
                if reflog.poll(&backend, &shared, &context, &token) {
                    next_reflog_poll = Instant::now() + Duration::from_secs(2);
                }
                if token.is_cancelled() || backend.needs_reconnect() {
                    continue;
                }
            }
        }
        if backend.needs_reconnect() {
            continue;
        }
        if Instant::now() >= next_poll || dirty.is_some_and(|at| Instant::now() >= at) {
            dirty = None;
            next_poll = Instant::now() + Duration::from_secs(2);
            let token = shared.pending.lock().ok().and_then(|pending| {
                if pending.work.is_some() {
                    None
                } else {
                    activate(&shared)
                }
            });
            let Some(token) = token else {
                continue;
            };
            if backend.has_repository() {
                match backend.current_branch(&token) {
                    Ok(branch) if !token.is_cancelled() => {
                        emit(&shared, &context, Event::Branch { generation, branch })
                    }
                    Err(error) if !token.is_cancelled() => emit(
                        &shared,
                        &context,
                        Event::Error {
                            generation,
                            message: error.to_string(),
                        },
                    ),
                    _ => continue,
                }
                if token.is_cancelled() || backend.needs_reconnect() {
                    continue;
                }
            }
            if let Some(path) = &document_path {
                match backend.document(path, &token) {
                    Ok(text) if !token.is_cancelled() => {
                        emit(
                            &shared,
                            &context,
                            Event::Availability {
                                generation,
                                available: true,
                            },
                        );
                        emit(
                            &shared,
                            &context,
                            Event::DocumentPath {
                                generation,
                                path: text.path.clone(),
                            },
                        );
                        let document = if let Some(current) = &current_document
                            && current.is_document
                            && current.document.comparisons[0]
                                .rows
                                .iter()
                                .map(|row| row.right.as_str())
                                .eq(text.text.lines())
                        {
                            Arc::clone(current)
                        } else {
                            Arc::new(PreparedDocument::from_text(text.path, text.text, &token))
                        };
                        if token.is_cancelled() {
                            continue;
                        }
                        current_document = Some(Arc::clone(&document));
                        emit(
                            &shared,
                            &context,
                            Event::Diff {
                                generation,
                                document,
                            },
                        );
                        first_diff_shown = true;
                    }
                    Err(error) if !token.is_cancelled() => emit(
                        &shared,
                        &context,
                        Event::Error {
                            generation,
                            message: error.to_string(),
                        },
                    ),
                    _ => {}
                }
                if token.is_cancelled() || backend.needs_reconnect() {
                    continue;
                }
            }
            if !backend.has_repository() {
                continue;
            }
            match backend.status_with_notice(view, &token) {
                Ok((files, message)) if !token.is_cancelled() => {
                    let Some(status) = current_status.prepare(files, &token) else {
                        continue;
                    };
                    let obsolete = prepared_cache.obsolete(view, &status.files, &token);
                    let mut retired = Vec::new();
                    let mut previous = None;
                    let mut retired_events = [None, None];
                    if let Ok(_active) = shared.active.lock()
                        && !token.is_cancelled()
                    {
                        retired = prepared_cache.remove_obsolete(&obsolete);
                        retired_events[0] = queue_event(
                            &shared,
                            &context,
                            Event::Notice {
                                generation,
                                message,
                            },
                        );
                        previous = Some(std::mem::replace(&mut current_status, status));
                        retired_events[1] = queue_event(
                            &shared,
                            &context,
                            Event::Status {
                                generation,
                                automatic: !explicit_refresh,
                                files: Arc::clone(&current_status.files),
                                model: Arc::clone(&current_status.model),
                            },
                        );
                        explicit_refresh = false;
                        watcher_deadline
                            .get_or_insert_with(|| Instant::now() + WATCHER_DEFER_LIMIT);
                    }
                    drop((retired, previous, retired_events));
                }
                Err(error) if !token.is_cancelled() => emit(
                    &shared,
                    &context,
                    Event::Error {
                        generation,
                        message: error.to_string(),
                    },
                ),
                _ => {}
            }
        }
    }
}

fn search_document(
    document: Option<&PreparedDocument>,
    query: &str,
    mut current: impl FnMut() -> bool,
) -> Option<Vec<usize>> {
    let needle = query.to_lowercase();
    let mut hits = Vec::new();
    if !needle.is_empty()
        && let Some(document) = document
    {
        for (index, prepared) in document.rows.iter().enumerate() {
            if !current() {
                return None;
            }
            let row = &document.document.comparisons[prepared.comparison].rows[prepared.index];
            let matches = if row.left.to_lowercase().contains(&needle) {
                true
            } else {
                if !current() {
                    return None;
                }
                row.right.to_lowercase().contains(&needle)
            };
            if matches {
                hits.push(index);
            }
        }
    }
    current().then_some(hits)
}

fn search_current(shared: &Shared, token: &CancelToken, request_id: u64) -> bool {
    !token.is_cancelled()
        && shared.latest_search_id.load(Ordering::Acquire) == request_id
        && !shared.stopped.load(Ordering::Acquire)
        && shared.foreground.load(Ordering::Acquire)
}

fn publish_search(
    shared: &Shared,
    context: &egui::Context,
    token: &CancelToken,
    generation: u64,
    request_id: u64,
    query: String,
    hits: Vec<usize>,
) {
    // A newer submission cannot race the final check and event enqueue.
    let retired = if let Ok(_pending) = shared.pending.lock()
        && search_current(shared, token, request_id)
    {
        queue_event(
            shared,
            context,
            Event::Search {
                generation,
                request_id,
                query,
                hits,
            },
        )
    } else {
        None
    };
    drop(retired);
}

/// Selects the next queued command under the `pending` lock (lock order: pending, then active).
/// Activation precedes dequeue, so a deactivation observed there leaves the command queued.
fn take_pending_job(shared: &Shared, pending: &mut Pending) -> Option<(Command, CancelToken)> {
    if pending.reflog.is_none() && pending.work.is_none() && pending.search.is_none() {
        return None;
    }
    let token = activate(shared)?;
    let command = pending
        .reflog
        .take()
        .or_else(|| pending.work.take())
        .or_else(|| pending.search.take())?;
    Some((command, token))
}

fn activate(shared: &Shared) -> Option<CancelToken> {
    let token = CancelToken::default();
    let mut current = shared.active.lock().ok()?;
    if shared.stopped.load(Ordering::Acquire)
        || (shared.initialized.load(Ordering::Acquire)
            && !shared.foreground.load(Ordering::Acquire))
    {
        return None;
    }
    *current = token.clone();
    Some(token)
}

fn emit(shared: &Shared, context: &egui::Context, event: Event) {
    drop(queue_event(shared, context, event));
}

fn queue_event(shared: &Shared, context: &egui::Context, event: Event) -> Option<Event> {
    let retired = if let Ok(mut events) = shared.events.lock() {
        let duplicate_error = if let Event::ReflogError { generation, .. } = &event {
            events.iter().position(|queued| {
                matches!(queued, Event::ReflogError { generation: queued_generation, .. }
                    if generation == queued_generation)
            })
        } else {
            None
        };
        let retired = if let Some(index) = duplicate_error {
            events.remove(index)
        } else if events.len() >= 16 {
            // Cached reflog pages must survive ordinary polling until the UI drains them.
            let index = events
                .iter()
                .position(|queued| {
                    !matches!(queued, Event::Reflog { .. } | Event::ReflogError { .. })
                })
                .unwrap_or(0);
            events.remove(index)
        } else {
            None
        };
        events.push_back(event);
        retired
    } else {
        Some(event)
    };
    context.request_repaint();
    retired
}

fn report_watch_diagnostics(messages: Vec<String>) {
    for message in messages {
        eprintln!("fastdiff watcher: {message}");
    }
}

#[cfg(test)]
mod search_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn status_file(path: &[u8]) -> FileChange {
        FileChange {
            path: fastdiff_core::GitPath(path.to_vec()),
            old_path: None,
            staged: false,
            unstaged: true,
            untracked: false,
            conflict: false,
            status: "M".into(),
            committed: None,
        }
    }

    #[test]
    fn prepared_status_reuses_equal_files_and_metadata_only_models() {
        let token = CancelToken::default();
        let first = PreparedStatus::default()
            .prepare(
                vec![status_file(b"a/item.rs"), status_file(b"b/item.rs")],
                &token,
            )
            .unwrap();
        let equal = first.prepare(first.files.as_ref().clone(), &token).unwrap();
        assert!(Arc::ptr_eq(&first.files, &equal.files));
        assert!(Arc::ptr_eq(&first.model, &equal.model));
        let mut metadata = first.files.as_ref().clone();
        metadata[0].staged = true;
        metadata[0].old_path = Some(fastdiff_core::GitPath(b"old.rs".to_vec()));
        let changed = equal.prepare(metadata, &token).unwrap();
        assert!(!Arc::ptr_eq(&equal.files, &changed.files));
        assert!(Arc::ptr_eq(&equal.model, &changed.model));
        assert!(changed.files[0].staged);
    }

    #[test]
    fn prepared_status_rebuilds_for_order_additions_removals_and_raw_paths() {
        let token = CancelToken::default();
        let mut previous = PreparedStatus::default();
        for paths in [
            vec![b"a/item.rs".as_slice(), b"b/\xff.rs".as_slice()],
            vec![b"b/\xff.rs".as_slice(), b"a/item.rs".as_slice()],
            vec![
                b"b/\xff.rs".as_slice(),
                b"a/item.rs".as_slice(),
                b"new.rs".as_slice(),
            ],
            vec![b"b/\xfe.rs".as_slice()],
            vec![],
        ] {
            let next = previous
                .prepare(paths.iter().map(|path| status_file(path)).collect(), &token)
                .unwrap();
            assert!(!Arc::ptr_eq(&previous.model, &next.model));
            assert_eq!(next.model.file_paths, paths);
            previous = next;
        }
    }

    #[test]
    fn cancelled_status_does_not_prepare_a_snapshot() {
        let previous = PreparedStatus::default();
        let token = CancelToken::default();
        token.cancel();
        assert!(
            previous
                .prepare(vec![status_file(b"item.rs")], &token)
                .is_none()
        );
        assert!(previous.files.is_empty());
        assert!(previous.model.file_paths.is_empty());
    }
    use std::process::Command as ProcessCommand;

    #[test]
    fn tabbed_source_lines_have_their_full_visual_scroll_width() {
        let source = format!("{}{}", "\t".repeat(20), "x".repeat(20));
        let document = DiffDocument {
            comparisons: vec![fastdiff_core::Comparison {
                label: "Unstaged".into(),
                patch: String::new(),
                summary: None,
                rows: vec![fastdiff_core::DiffRow {
                    kind: fastdiff_core::RowKind::Addition,
                    old_number: None,
                    new_number: Some(1),
                    left: String::new(),
                    right: source.clone(),
                }],
            }],
        };
        let prepared = PreparedDocument::new(document, &CancelToken::default());
        assert_eq!(prepared.max_columns, 100);
        assert_eq!(prepared.left_columns, 0);
        assert_eq!(prepared.right_columns, 100);
        assert_eq!(prepared.document.comparisons[0].rows[0].right, source);
    }

    #[test]
    fn visual_columns_include_wide_unicode_and_long_line_notice() {
        assert_eq!(display_columns("漢字\tx"), 9);
        let line = "x".repeat(crate::highlight::MAX_PREVIEW_BYTES + 1);
        assert_eq!(
            display_columns(&line),
            crate::highlight::MAX_PREVIEW_BYTES + PREVIEW_SUFFIX.chars().count()
        );
    }

    pub(super) fn wait_event(worker: &Worker, predicate: impl Fn(&Event) -> bool) -> Event {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            for event in worker.drain_events() {
                if predicate(&event) {
                    return event;
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("Worker did not deliver expected event");
    }

    fn wait_reflog(worker: &Worker, generation: u64) -> fastdiff_core::ReflogList {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            for event in worker.drain_events() {
                match event {
                    Event::Reflog {
                        generation: actual,
                        list,
                    } if actual == generation => {
                        return list;
                    }
                    Event::ReflogError {
                        generation: actual,
                        message,
                    } if actual == generation => {
                        panic!("Commit metadata failed: {message}");
                    }
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("Worker did not deliver reflog");
    }

    #[test]
    fn reflog_refresh_branch_changes_alongside_file_loading() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            ProcessCommand::new("git")
                .args(["init", "-q", "-b", "first"])
                .current_dir(directory.path())
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(directory.path().join("notes.txt"), "needle\n").unwrap();
        let worker = Worker::spawn(
            OpenRequest {
                target: OpenTarget::Repository,
                repository: directory.path().to_string_lossy().into_owned(),
                file: None,
                view: View::All,
                wsl: None,
                theme: false,
            },
            egui::Context::default(),
        );
        worker.reflog(1, true);
        let list = wait_reflog(&worker, 1);
        assert_eq!(list.branch.as_deref(), Some("first"));
        assert!(list.entries.is_empty());
        assert!(
            ProcessCommand::new("git")
                .args(["symbolic-ref", "HEAD", "refs/heads/second"])
                .current_dir(directory.path())
                .status()
                .unwrap()
                .success()
        );
        worker.reflog(2, true);
        assert_eq!(wait_reflog(&worker, 2).branch.as_deref(), Some("second"));

        assert!(
            ProcessCommand::new("git")
                .args(["symbolic-ref", "HEAD", "refs/heads/third"])
                .current_dir(directory.path())
                .status()
                .unwrap()
                .success()
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let list = wait_reflog(&worker, 2);
            if list.branch.as_deref() == Some("third") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Reflog polling kept an old branch"
            );
        }

        worker.refresh(3, View::All);
        let Event::Branch { branch, .. } = wait_event(&worker, |event| {
            matches!(event, Event::Branch { generation: 3, .. })
        }) else {
            panic!("Expected branch metadata with reflog open");
        };
        assert_eq!(branch.as_deref(), Some("third"));
        // A second refresh avoids relying on events already drained with Branch.
        worker.refresh(4, View::All);
        wait_event(&worker, |event| {
            matches!(event, Event::Status { generation: 4, .. })
        });
        // The unchanged poll no longer republishes metadata drained with Status.
        worker.reflog(2, true);
        assert_eq!(wait_reflog(&worker, 2).branch.as_deref(), Some("third"));
        worker.reflog(5, false);
        worker.refresh(6, View::All);
        wait_event(&worker, |event| {
            matches!(event, Event::Status { generation: 6, .. })
        });
        thread::sleep(Duration::from_millis(2200));
        assert!(
            !worker
                .drain_events()
                .iter()
                .any(|event| { matches!(event, Event::Reflog { .. } | Event::ReflogError { .. }) }),
            "Collapsed panel kept polling reflog"
        );
    }

    #[test]
    fn reflog_commands_preserve_active_work_and_pending_searches() {
        let (wake, _) = mpsc::sync_channel(1);
        let shared = Arc::new(Shared {
            pending: Mutex::new(Pending::default()),
            events: Mutex::new(VecDeque::new()),
            active: Mutex::new(CancelToken::default()),
            latest_search_id: AtomicU64::new(0),
            foreground: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            initialized: AtomicBool::new(true),
        });
        let worker = Worker {
            wake,
            shared,
            thread: None,
        };
        let active = activate(&worker.shared).unwrap();
        worker.search(1, 1, "needle".into());
        worker.reflog(2, true);
        assert!(!active.is_cancelled());
        let pending = worker.shared.pending.lock().unwrap();
        assert!(matches!(
            pending.reflog,
            Some(Command::Reflog {
                generation: 2,
                enabled: true
            })
        ));
        assert!(matches!(
            pending.search,
            Some(Command::Search { generation: 1, .. })
        ));
        drop(pending);
        worker.refresh(3, View::All);
        assert!(matches!(
            worker.shared.pending.lock().unwrap().reflog,
            Some(Command::Reflog {
                generation: 2,
                enabled: true
            })
        ));
        worker.reflog(4, false);
        assert!(matches!(
            worker.shared.pending.lock().unwrap().work,
            Some(Command::Refresh { generation: 3, .. })
        ));
    }

    #[test]
    fn real_repository_loads_searches_refreshes_and_stops_owned_worker() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            ProcessCommand::new("git")
                .args(["init", "-q"])
                .current_dir(directory.path())
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(
            directory.path().join("hello.rs"),
            "fn main() { println!(\"first\"); }\n",
        )
        .unwrap();
        let request = OpenRequest {
            target: OpenTarget::Repository,
            repository: directory.path().to_string_lossy().into_owned(),
            file: None,
            view: View::All,
            wsl: None,
            theme: false,
        };
        let worker = Worker::spawn(request, egui::Context::default());
        // A request made before repository discovery completes must survive startup.
        worker.refresh(17, View::All);
        let Event::Status { files, .. } = wait_event(&worker, |event| {
            matches!(event, Event::Status { generation: 17, .. })
        }) else {
            panic!()
        };
        worker.diff(1, View::All, files[0].clone());
        let Event::Diff { document, .. } = wait_event(&worker, |event| {
            matches!(event, Event::Diff { generation: 1, .. })
        }) else {
            panic!()
        };
        assert!(document.document.comparisons[0].patch.contains("first"));
        worker.search(1, 1, "FIRST".into());
        let Event::Search { hits, .. } =
            wait_event(&worker, |event| matches!(event, Event::Search { .. }))
        else {
            panic!()
        };
        assert!(!hits.is_empty());
        std::fs::write(
            directory.path().join("hello.rs"),
            "fn main() { println!(\"second\"); }\n",
        )
        .unwrap();
        worker.refresh(2, View::All);
        let Event::Status { files, .. } = wait_event(&worker, |event| {
            matches!(event, Event::Status { generation: 2, .. })
        }) else {
            panic!()
        };
        worker.diff(3, View::All, files[0].clone());
        let Event::Diff { document, .. } = wait_event(&worker, |event| {
            matches!(event, Event::Diff { generation: 3, .. })
        }) else {
            panic!()
        };
        assert!(document.document.comparisons[0].patch.contains("second"));
        let stop = Instant::now();
        drop(worker);
        assert!(stop.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn git_refresh_stops_document_polling_and_clears_document_search() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            ProcessCommand::new("git")
                .args(["init", "-q"])
                .current_dir(directory.path())
                .status()
                .unwrap()
                .success()
        );
        let path = directory.path().join("notes.txt");
        std::fs::write(&path, "needle\n").unwrap();
        let worker = Worker::spawn(
            OpenRequest {
                target: OpenTarget::Repository,
                repository: directory.path().to_string_lossy().into_owned(),
                file: None,
                view: View::Staged,
                wsl: None,
                theme: false,
            },
            egui::Context::default(),
        );
        worker.document(1, "notes.txt".into(), View::Staged);
        wait_event(
            &worker,
            |event| matches!(event, Event::Diff { generation: 1, document } if document.is_document),
        );

        worker.reflog(90, true);
        wait_reflog(&worker, 90);
        worker.search(1, 1, "needle".into());
        let Event::Search { hits, .. } = wait_event(&worker, |event| {
            matches!(event, Event::Search { generation: 1, .. })
        }) else {
            panic!("Expected document search while reflog is open");
        };
        assert_eq!(hits, vec![0]);
        worker.reflog(91, false);

        worker.refresh(2, View::Staged);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut status_received = false;
        while Instant::now() < deadline && !status_received {
            for event in worker.drain_events() {
                match event {
                    Event::Diff { generation: 2, .. }
                    | Event::DocumentPath { generation: 2, .. } => {
                        panic!("Git refresh restored full-file content");
                    }
                    Event::Status {
                        generation: 2,
                        files,
                        ..
                    } => {
                        assert!(files.is_empty());
                        status_received = true;
                    }
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(status_received);
        worker.search(2, 2, "needle".into());
        let Event::Search { hits, .. } = wait_event(&worker, |event| {
            matches!(event, Event::Search { generation: 2, .. })
        }) else {
            panic!("Expected search result");
        };
        assert!(hits.is_empty());

        worker.document(3, "notes.txt".into(), View::Staged);
        wait_event(
            &worker,
            |event| matches!(event, Event::Diff { generation: 3, document } if document.is_document),
        );
        std::fs::write(&path, "updated contents\n").unwrap();
        let Event::Diff { document, .. } = wait_event(&worker, |event| {
            matches!(event, Event::Diff { generation: 3, document }
                if document.document.comparisons[0].rows[0].right == "updated contents")
        }) else {
            panic!("Expected refreshed document");
        };
        assert!(document.is_document);
    }

    #[test]
    fn standalone_document_searches_full_lines_and_refreshes_after_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("notes.txt");
        let source = format!("{}Needle\nsecond nEeDle line\n", "x".repeat(20_000));
        std::fs::write(&path, &source).unwrap();
        let worker = Worker::spawn(
            OpenRequest {
                target: OpenTarget::Document,
                repository: path.to_string_lossy().into_owned(),
                file: None,
                view: View::All,
                wsl: None,
                theme: false,
            },
            egui::Context::default(),
        );
        let Event::Diff { document, .. } =
            wait_event(&worker, |event| matches!(event, Event::Diff { .. }))
        else {
            panic!("Expected document");
        };
        assert!(document.is_document);
        assert_eq!(document.rows.len(), 2);
        assert_eq!(document.right_columns, 20_006);
        worker.search(0, 1, "NEEDLE".into());
        let Event::Search { hits, .. } =
            wait_event(&worker, |event| matches!(event, Event::Search { .. }))
        else {
            panic!("Expected search result");
        };
        assert_eq!(hits, vec![0, 1]);
        let replacement = directory.path().join("replacement.txt");
        std::fs::write(&replacement, "replacement contents\n").unwrap();
        #[cfg(windows)]
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        let Event::Diff { document, .. } = wait_event(&worker, |event| {
            matches!(event, Event::Diff { document, .. }
                if document.document.comparisons[0].rows[0].right == "replacement contents")
        }) else {
            panic!("Expected replaced document");
        };
        assert_eq!(document.rows.len(), 1);
        std::fs::remove_file(&path).unwrap();
        let Event::Error { generation, .. } =
            wait_event(&worker, |event| matches!(event, Event::Error { .. }))
        else {
            panic!("Expected missing-file error");
        };
        assert_eq!(generation, 0);
    }

    #[test]
    fn rapid_requests_and_filesystem_events_are_bounded_and_keep_latest_work() {
        let (wake, _) = mpsc::sync_channel(1);
        let shared = Arc::new(Shared {
            pending: Mutex::new(Pending::default()),
            events: Mutex::new(VecDeque::new()),
            active: Mutex::new(CancelToken::default()),
            latest_search_id: AtomicU64::new(0),
            foreground: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            initialized: AtomicBool::new(true),
        });
        let worker = Worker {
            wake,
            shared,
            thread: None,
        };
        for generation in 0..1000 {
            worker.refresh(generation, View::All);
        }
        assert!(matches!(
            worker.shared.pending.lock().unwrap().work,
            Some(Command::Refresh {
                generation: 999,
                ..
            })
        ));
        let context = egui::Context::default();
        let mut snapshot = PreparedStatus::default();
        for generation in 0..1000 {
            snapshot = snapshot
                .prepare(
                    vec![status_file(format!("dir/{generation}.rs").as_bytes())],
                    &CancelToken::default(),
                )
                .unwrap();
            emit(
                &worker.shared,
                &context,
                Event::Status {
                    generation,
                    automatic: true,
                    files: Arc::clone(&snapshot.files),
                    model: Arc::clone(&snapshot.model),
                },
            );
        }
        let events = worker.drain_events();
        assert_eq!(events.len(), 16);
        for (offset, event) in events.into_iter().enumerate() {
            let Event::Status {
                generation,
                files,
                model,
                ..
            } = event
            else {
                panic!("Expected complete status snapshot");
            };
            assert_eq!(generation, 984 + offset as u64);
            assert_eq!(files[0].path.0, format!("dir/{generation}.rs").as_bytes());
            assert_eq!(model.file_paths, vec![files[0].path.0.clone()]);
        }
        let active = activate(&worker.shared).unwrap();
        worker.set_foreground(false);
        assert!(active.is_cancelled());
        assert!(activate(&worker.shared).is_none());
        worker.set_foreground(true);
        assert!(!activate(&worker.shared).unwrap().is_cancelled());
        worker.shared.stopped.store(true, Ordering::Release);
        assert!(activate(&worker.shared).is_none());
    }

    #[test]
    fn startup_requests_do_not_cancel_discovery_but_shutdown_does() {
        let (wake, _) = mpsc::sync_channel(1);
        let shared = Arc::new(Shared {
            pending: Mutex::new(Pending::default()),
            events: Mutex::new(VecDeque::new()),
            active: Mutex::new(CancelToken::default()),
            latest_search_id: AtomicU64::new(0),
            foreground: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            initialized: AtomicBool::new(false),
        });
        let worker = Worker {
            wake,
            shared,
            thread: None,
        };
        let startup = activate(&worker.shared).unwrap();
        worker.set_foreground(false);
        worker.refresh(1, View::Staged);
        assert!(!startup.is_cancelled());
        drop(worker);
        assert!(startup.is_cancelled());
    }

    pub(super) fn git(root: &std::path::Path, args: &[&str]) {
        let output = ProcessCommand::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    pub(super) fn repository_worker() -> (tempfile::TempDir, Worker) {
        let directory = tempfile::tempdir().unwrap();
        git(directory.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(directory.path().join("notes.txt"), "base\n").unwrap();
        git(directory.path(), &["add", "notes.txt"]);
        git(
            directory.path(),
            &[
                "-c",
                "user.name=Worker test",
                "-c",
                "user.email=worker@example.invalid",
                "commit",
                "-qm",
                "base",
            ],
        );
        git(directory.path(), &["branch", "upstream"]);
        git(directory.path(), &["branch", "--set-upstream-to=upstream"]);
        let worker = Worker::spawn(
            OpenRequest {
                target: OpenTarget::Repository,
                repository: directory.path().to_string_lossy().into_owned(),
                file: None,
                view: View::All,
                wsl: None,
                theme: false,
            },
            egui::Context::default(),
        );
        (directory, worker)
    }

    pub(super) fn wait_status(
        worker: &Worker,
        generation: u64,
        automatic: bool,
    ) -> Arc<Vec<FileChange>> {
        let Event::Status { files, .. } = wait_event(worker, |event| {
            matches!(event, Event::Status {
                generation: actual, automatic: actual_automatic, ..
            } if *actual == generation && *actual_automatic == automatic)
        }) else {
            panic!("Expected repository status");
        };
        files
    }

    #[test]
    fn explicit_status_refreshes_reuse_complete_snapshots() {
        let (directory, worker) = repository_worker();
        wait_status(&worker, 0, false);
        std::fs::write(directory.path().join("notes.txt"), "changed\n").unwrap();
        worker.refresh(1, View::All);
        let Event::Status {
            files: first,
            model: first_model,
            ..
        } = wait_event(&worker, |event| {
            matches!(
                event,
                Event::Status {
                    generation: 1,
                    automatic: false,
                    ..
                }
            )
        })
        else {
            panic!("Expected initial modified status");
        };
        worker.refresh(2, View::All);
        let Event::Status {
            files: equal,
            model: equal_model,
            ..
        } = wait_event(&worker, |event| {
            matches!(
                event,
                Event::Status {
                    generation: 2,
                    automatic: false,
                    ..
                }
            )
        })
        else {
            panic!("Expected explicit unchanged status");
        };
        assert!(Arc::ptr_eq(&first, &equal));
        assert!(Arc::ptr_eq(&first_model, &equal_model));
        git(directory.path(), &["add", "notes.txt"]);
        worker.refresh(3, View::All);
        let Event::Status {
            files: staged,
            model: staged_model,
            ..
        } = wait_event(&worker, |event| {
            matches!(
                event,
                Event::Status {
                    generation: 3,
                    automatic: false,
                    ..
                }
            )
        })
        else {
            panic!("Expected explicit staged status");
        };
        assert!(staged[0].staged && !staged[0].unstaged);
        assert!(!Arc::ptr_eq(&equal, &staged));
        assert!(Arc::ptr_eq(&equal_model, &staged_model));
    }

    #[test]
    fn inactive_repository_keeps_requests_bounded_without_starting_work() {
        let (directory, worker) = repository_worker();
        wait_status(&worker, 0, false);
        worker.reflog(1, true);
        wait_reflog(&worker, 1);
        worker.set_foreground(false);
        assert!(worker.shared.active.lock().unwrap().is_cancelled());
        worker.drain_events();
        for generation in 1..1000 {
            worker.refresh(generation, View::All);
            worker.search(generation, generation, "queued".into());
            worker.reflog(generation, true);
        }
        std::fs::write(directory.path().join("notes.txt"), "aaaa\n").unwrap();
        worker.shared.dirty.store(true, Ordering::Release);
        let _ = worker.wake.try_send(());
        thread::sleep(Duration::from_millis(2250));
        assert!(
            worker.drain_events().is_empty(),
            "Inactive worker emitted repository work"
        );
        assert!(
            worker.shared.active.lock().unwrap().is_cancelled(),
            "Inactive work replaced the cancelled token"
        );
        assert!(matches!(
            worker.shared.pending.lock().unwrap().work,
            Some(Command::Refresh {
                generation: 999,
                ..
            })
        ));
        worker.refresh(1000, View::All);
        worker.set_foreground(true);
        assert_eq!(wait_status(&worker, 1000, false).len(), 1);
        let stop = Instant::now();
        worker.set_foreground(false);
        drop(worker);
        assert!(stop.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn deactivation_during_job_activation_preserves_queued_command() {
        const WAIT: Duration = Duration::from_secs(10);
        let shared = Shared {
            pending: Mutex::new(Pending {
                reflog: Some(Command::Reflog {
                    generation: 7,
                    enabled: true,
                }),
                ..Pending::default()
            }),
            events: Mutex::new(VecDeque::new()),
            active: Mutex::new(CancelToken::default()),
            latest_search_id: AtomicU64::new(0),
            foreground: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            initialized: AtomicBool::new(true),
        };
        let is_queued_reflog = |shared: &Shared| {
            matches!(
                shared.pending.lock().unwrap().reflog,
                Some(Command::Reflog {
                    generation: 7,
                    enabled: true
                })
            )
        };
        let (holding_pending, holding_pending_receiver) = mpsc::channel();
        let active = shared.active.lock().unwrap();
        let (confirmed, blocked_job) = thread::scope(|scope| {
            let blocked = scope.spawn(|| {
                let mut pending = shared.pending.lock().unwrap();
                holding_pending.send(()).unwrap();
                take_pending_job(&shared, &mut pending).map(|(command, _)| command)
            });
            let confirmed = holding_pending_receiver.recv_timeout(WAIT).is_ok();
            shared.foreground.store(false, Ordering::Release);
            drop(active);
            (confirmed, blocked.join())
        });
        assert!(confirmed, "Selection never took the pending lock");
        assert!(
            matches!(blocked_job, Ok(None)),
            "Deactivation during activation delivered or lost the command"
        );
        assert!(
            is_queued_reflog(&shared),
            "Deactivation dropped the command"
        );
        shared.foreground.store(true, Ordering::Release);
        let job = take_pending_job(&shared, &mut shared.pending.lock().unwrap());
        assert!(matches!(
            job,
            Some((
                Command::Reflog {
                    generation: 7,
                    enabled: true
                },
                _
            ))
        ));
        assert!(take_pending_job(&shared, &mut shared.pending.lock().unwrap()).is_none());
    }

    #[test]
    fn reflog_queued_while_inactive_is_delivered_after_foreground_resume() {
        let (_directory, worker) = repository_worker();
        wait_status(&worker, 0, false);
        worker.set_foreground(false);
        worker.reflog(7, true);
        worker.set_foreground(true);
        wait_reflog(&worker, 7);
    }

    #[test]
    fn activation_reads_same_size_content_with_restored_timestamp_and_reuses_equal_documents() {
        let (directory, worker) = repository_worker();
        wait_status(&worker, 0, false);
        let path = directory.path().join("notes.txt");
        std::fs::write(&path, "aaaa\n").unwrap();
        worker.refresh(1, View::All);
        let files = wait_status(&worker, 1, false);
        worker.diff(2, View::All, files[0].clone());
        let Event::Diff {
            document: first, ..
        } = wait_event(&worker, |event| {
            matches!(event, Event::Diff { generation: 2, .. })
        })
        else {
            panic!("Expected initial diff")
        };
        worker.diff(3, View::All, files[0].clone());
        let Event::Diff {
            document: unchanged,
            ..
        } = wait_event(&worker, |event| {
            matches!(event, Event::Diff { generation: 3, .. })
        })
        else {
            panic!("Expected unchanged diff")
        };
        assert!(Arc::ptr_eq(&first, &unchanged));
        worker.set_foreground(false);
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, "bbbb\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        worker.refresh(4, View::All);
        worker.set_foreground(true);
        let refreshed = wait_status(&worker, 4, false);
        assert_eq!(refreshed, files);
        worker.diff(5, View::All, refreshed[0].clone());
        let Event::Diff {
            document: changed, ..
        } = wait_event(&worker, |event| {
            matches!(event, Event::Diff { generation: 5, .. })
        })
        else {
            panic!("Expected refreshed diff")
        };
        assert!(changed.document.comparisons[0].patch.contains("bbbb"));
        assert!(!Arc::ptr_eq(&first, &changed));
        assert_eq!(wait_status(&worker, 5, true), refreshed);
    }

    #[test]
    fn deferred_watcher_waits_for_the_first_diff_or_the_deadline() {
        let start = Instant::now();
        let deadline = Some(start + WATCHER_DEFER_LIMIT);
        assert!(!watcher_due(true, false, start, deadline));
        assert!(!watcher_due(
            true,
            false,
            start + WATCHER_DEFER_LIMIT * 2,
            None
        ));
        assert!(watcher_due(true, true, start, None));
        assert!(watcher_due(
            true,
            false,
            start + WATCHER_DEFER_LIMIT,
            deadline
        ));
        assert!(!watcher_due(false, true, start, deadline));
    }

    #[test]
    fn automatic_repository_checks_observe_stage_unstage_commit_and_ref_movement() {
        let (directory, worker) = repository_worker();
        wait_status(&worker, 0, false);
        std::fs::write(directory.path().join("notes.txt"), "aaaa\n").unwrap();
        worker.refresh(1, View::All);
        let files = wait_status(&worker, 1, false);
        assert!(files[0].unstaged && !files[0].staged);
        git(directory.path(), &["add", "notes.txt"]);
        let Event::Status { files, .. } = wait_event(&worker, |event| {
            matches!(event, Event::Status { generation: 1, automatic: true, files, .. }
                if files.len() == 1 && files[0].staged && !files[0].unstaged)
        }) else {
            panic!("Expected staged status")
        };
        assert!(files[0].staged);
        git(
            directory.path(),
            &["reset", "-q", "HEAD", "--", "notes.txt"],
        );
        wait_event(&worker, |event| {
            matches!(event, Event::Status { generation: 1, automatic: true, files, .. }
                if files.len() == 1 && files[0].unstaged && !files[0].staged)
        });
        git(directory.path(), &["add", "notes.txt"]);
        git(
            directory.path(),
            &[
                "-c",
                "user.name=Worker test",
                "-c",
                "user.email=worker@example.invalid",
                "commit",
                "-qm",
                "changed",
            ],
        );
        wait_event(&worker, |event| {
            matches!(event, Event::Status { generation: 1, automatic: true, files, .. }
                if files.len() == 1 && files[0].committed.is_some()
                    && !files[0].staged && !files[0].unstaged)
        });
        git(directory.path(), &["reset", "--hard", "-q", "HEAD~1"]);
        std::fs::write(directory.path().join("notes.txt"), "aaaa\n").unwrap();
        wait_event(&worker, |event| {
            matches!(event, Event::Status { generation: 1, automatic: true, files, .. }
                if files.len() == 1 && files[0].unstaged)
        });
        git(directory.path(), &["checkout", "-qb", "moved"]);
        wait_event(
            &worker,
            |event| matches!(event, Event::Branch { generation: 1, branch } if branch.as_deref() == Some("moved")),
        );
    }

    #[test]
    fn failed_explicit_refresh_remains_explicit_until_status_succeeds() {
        let (directory, worker) = repository_worker();
        wait_status(&worker, 0, false);
        worker.set_foreground(false);
        let index = directory.path().join(".git/index");
        let contents = std::fs::read(&index).unwrap();
        std::fs::write(&index, "invalid index").unwrap();
        worker.refresh(1, View::Staged);
        worker.set_foreground(true);
        wait_event(&worker, |event| {
            matches!(event, Event::Error { generation: 1, .. })
        });
        std::fs::write(&index, contents).unwrap();
        assert!(wait_status(&worker, 1, false).is_empty());
        assert!(wait_status(&worker, 1, true).is_empty());
    }
}
