use crate::{Error, OpenTarget, PROTOCOL_VERSION, Result, read_frame, write_frame};
use fastdiff_core::{
    CancelToken, DiffDocument, FileChange, GitPath, ReflogList, ReflogRequest, Repository,
    TextDocument, View,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestEnvelope {
    pub version: u32,
    pub id: u64,
    pub request: HelperRequest,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HelperRequest {
    Hello,
    Status { view: View },
    CurrentBranch,
    Reflog { request: ReflogRequest },
    ReflogSnapshot,
    Document { path: String },
    DocumentExists { path: GitPath },
    Diff { change: FileChange, view: View },
    Cancel { request_id: u64 },
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseEnvelope {
    pub version: u32,
    pub id: u64,
    pub response: HelperResponse,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HelperResponse {
    Hello {
        root: String,
        has_repository: bool,
    },
    CurrentBranch {
        branch: Option<String>,
    },
    Reflog {
        list: ReflogList,
    },
    ReflogSnapshot {
        snapshot: Option<u64>,
    },
    Status {
        files: Vec<FileChange>,
        notice: Option<String>,
    },
    Document {
        document: TextDocument,
    },
    DocumentExists {
        available: bool,
    },
    Diff {
        document: DiffDocument,
    },
    Cancelled,
    Shutdown,
    Error {
        code: String,
        message: String,
    },
}

/// stdin EOF, shutdown, or malformed framing cancels all jobs and joins owned workers.
pub fn run_helper<R: Read, W: Write + Send + 'static>(
    repository: &str,
    mut input: R,
    output: W,
) -> Result<()> {
    run_helper_with_stop(
        repository,
        OpenTarget::Repository,
        &mut input,
        output,
        Arc::new(AtomicBool::new(false)),
    )
}

pub fn run_stdio_helper(repository: &str) -> Result<()> {
    run_stdio_helper_target(repository, OpenTarget::Repository)
}

pub fn run_stdio_helper_target(repository: &str, target: OpenTarget) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let output = std::io::stdout();
    crate::pipe::nonblocking(&output)?;
    let output = crate::pipe::CancellablePipe::writer(output, stop.clone());
    run_helper_with_stop(repository, target, std::io::stdin().lock(), output, stop)
}

fn run_helper_with_stop<R: Read, W: Write + Send + 'static>(
    repository: &str,
    target: OpenTarget,
    mut input: R,
    output: W,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let repository = Arc::new(CachedRepository {
        path: repository.into(),
        target,
        repository: Mutex::new(None),
    });
    let writer = Arc::new(Mutex::new(output));
    let active = Arc::new(Mutex::new(HashMap::<u64, CancelToken>::new()));
    let mut jobs = HelperJobs {
        active: active.clone(),
        workers: Vec::new(),
        output_stop: stop,
    };
    let mut result = Ok(());
    loop {
        let envelope: RequestEnvelope = match read_frame(&mut input) {
            Ok(Some(envelope)) => envelope,
            Ok(None) => break,
            Err(error) => {
                result = Err(error);
                break;
            }
        };
        if envelope.version != PROTOCOL_VERSION {
            send_response(
                &writer,
                envelope.id,
                HelperResponse::Error {
                    code: "incompatible_version".into(),
                    message: format!(
                        "Helper protocol {} does not match {}; install matching Windows GUI and WSL helper",
                        envelope.version, PROTOCOL_VERSION
                    ),
                },
            )?;
            break;
        }
        match envelope.request {
            HelperRequest::Shutdown => {
                send_response(&writer, envelope.id, HelperResponse::Shutdown)?;
                break;
            }
            HelperRequest::Cancel { request_id } => {
                if let Ok(jobs) = active.lock()
                    && let Some(token) = jobs.get(&request_id)
                {
                    token.cancel();
                }
                send_response(&writer, envelope.id, HelperResponse::Cancelled)?;
            }
            request => {
                reap_finished(&mut jobs.workers);
                let token = CancelToken::default();
                // Bound work even for a malicious peer. Normal clients serialize selected-file requests.
                if jobs.workers.len() >= 4 {
                    send_response(
                        &writer,
                        envelope.id,
                        HelperResponse::Error {
                            code: "busy".into(),
                            message: "Helper has too many active requests".into(),
                        },
                    )?;
                    continue;
                }
                let duplicate = {
                    let mut jobs = active
                        .lock()
                        .map_err(|_| Error::Protocol("Helper job lock poisoned".into()))?;
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        jobs.entry(envelope.id)
                    {
                        entry.insert(token.clone());
                        false
                    } else {
                        true
                    }
                };
                if duplicate {
                    send_response(
                        &writer,
                        envelope.id,
                        failure("duplicate_request_id", "Request ID is already active"),
                    )?;
                    continue;
                }
                let repository = repository.clone();
                let writer = writer.clone();
                let active = active.clone();
                jobs.workers.push(std::thread::spawn(move || {
                    let response = execute(repository.as_ref(), request, &token);
                    if let Err(error) = send_response(&writer, envelope.id, response) {
                        eprintln!("fastdiff helper: {error}");
                    }
                    if let Ok(mut jobs) = active.lock() {
                        jobs.remove(&envelope.id);
                    }
                }));
            }
        }
    }
    result
}

struct HelperJobs {
    active: Arc<Mutex<HashMap<u64, CancelToken>>>,
    workers: Vec<JoinHandle<()>>,
    output_stop: Arc<AtomicBool>,
}
impl Drop for HelperJobs {
    fn drop(&mut self) {
        self.output_stop.store(true, Ordering::Release);
        if let Ok(jobs) = self.active.lock() {
            for token in jobs.values() {
                token.cancel();
            }
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

struct CachedRepository {
    target: OpenTarget,
    path: String,
    repository: Mutex<Option<Repository>>,
}
impl CachedRepository {
    fn discover(&self, token: &CancelToken) -> Result<Repository> {
        if token.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let mut cached = self
            .repository
            .lock()
            .map_err(|_| Error::Protocol("Helper repository lock poisoned".into()))?;
        if token.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if let Some(repository) = &*cached {
            return Ok(repository.clone());
        }
        let path = std::path::Path::new(&self.path);
        let path = if self.target == OpenTarget::Document {
            path.parent().unwrap_or(std::path::Path::new("."))
        } else {
            path
        };
        let repository = Repository::discover_with_cancel(path, token)
            .map_err(|error| Error::Backend(error.to_string()))?;
        *cached = Some(repository.clone());
        Ok(repository)
    }
}

fn execute(
    repository: &CachedRepository,
    request: HelperRequest,
    token: &CancelToken,
) -> HelperResponse {
    if let HelperRequest::DocumentExists { path } = &request {
        if token.is_cancelled() {
            return HelperResponse::Cancelled;
        }
        let path = match path.to_path_buf() {
            Ok(path) if !path.as_os_str().is_empty() => path,
            _ => return failure("document", "Invalid document path"),
        };
        let path = if path.is_absolute() {
            path
        } else {
            match repository.discover(token) {
                Ok(repository) => repository.root().join(path),
                Err(error) => return failure("repository", error),
            }
        };
        return match std::fs::metadata(path) {
            Ok(metadata) => HelperResponse::DocumentExists {
                available: metadata.is_file(),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                HelperResponse::DocumentExists { available: false }
            }
            Err(error) => failure("document", error),
        };
    }
    if let HelperRequest::Document { path } = &request {
        if path.is_empty() || path.contains('\0') {
            return failure(
                "document",
                "Document path must be nonempty and contain no NUL",
            );
        }
        let path = if std::path::Path::new(path).is_absolute() {
            std::path::PathBuf::from(path)
        } else {
            let base = std::path::Path::new(&repository.path);
            let base = if repository.target == OpenTarget::Document {
                base.parent().unwrap_or(std::path::Path::new("."))
            } else {
                base
            };
            base.join(path)
        };
        return match fastdiff_core::read_document(path, token) {
            Ok(document) => HelperResponse::Document { document },
            Err(fastdiff_core::Error::Cancelled) => HelperResponse::Cancelled,
            Err(error) => failure("document", error),
        };
    }
    if matches!(request, HelperRequest::Hello) && repository.target == OpenTarget::Document {
        let path = match std::path::absolute(&repository.path) {
            Ok(path) => path,
            Err(error) => return failure("document", error),
        };
        let parent = path.parent().unwrap_or(std::path::Path::new("."));
        let found = repository.discover(token).ok();
        if token.is_cancelled() {
            return HelperResponse::Cancelled;
        }
        return HelperResponse::Hello {
            root: found
                .as_ref()
                .map(|repo| repo.root())
                .unwrap_or(parent)
                .to_string_lossy()
                .into_owned(),
            has_repository: found.is_some(),
        };
    }
    let repository = match repository.discover(token) {
        Ok(repository) => repository,
        Err(error) => return failure("repository", error),
    };
    let result = match request {
        HelperRequest::Hello => {
            return HelperResponse::Hello {
                root: repository.root().to_string_lossy().into_owned(),
                has_repository: true,
            };
        }
        HelperRequest::CurrentBranch => repository
            .current_branch(token)
            .map(|branch| HelperResponse::CurrentBranch { branch }),
        HelperRequest::Reflog { request } => repository
            .reflog_page(&request, token)
            .map(|list| HelperResponse::Reflog { list }),
        HelperRequest::ReflogSnapshot => repository
            .reflog_snapshot(token)
            .map(|snapshot| HelperResponse::ReflogSnapshot { snapshot }),
        HelperRequest::Status { view } => repository
            .status_with_notice(view, token)
            .map(|(files, notice)| HelperResponse::Status { files, notice }),
        HelperRequest::Diff { change, view } => repository
            .diff(&change, view, token)
            .map(|document| HelperResponse::Diff { document }),
        _ => return failure("unexpected_request", "Expected status, document or diff"),
    };
    if token.is_cancelled() {
        return HelperResponse::Cancelled;
    }
    result.unwrap_or_else(|error| failure("git", error))
}

fn failure(code: &str, error: impl std::fmt::Display) -> HelperResponse {
    HelperResponse::Error {
        code: code.into(),
        message: error.to_string(),
    }
}

fn send_response<W: Write>(writer: &Mutex<W>, id: u64, response: HelperResponse) -> Result<()> {
    let mut writer = writer
        .lock()
        .map_err(|_| Error::Protocol("Helper output lock poisoned".into()))?;
    let envelope = ResponseEnvelope {
        version: PROTOCOL_VERSION,
        id,
        response,
    };
    match write_frame(&mut *writer, &envelope) {
        Err(Error::Protocol(message)) => write_frame(
            &mut *writer,
            &ResponseEnvelope {
                version: PROTOCOL_VERSION,
                id,
                response: HelperResponse::Error {
                    code: "message_too_large".into(),
                    message: format!("{message}. Try a smaller document or the native GUI"),
                },
            },
        ),
        result => result,
    }
}

fn reap_finished(workers: &mut Vec<JoinHandle<()>>) {
    let mut pending = Vec::new();
    for worker in workers.drain(..) {
        if worker.is_finished() {
            let _ = worker.join();
        } else {
            pending.push(worker);
        }
    }
    *workers = pending;
}
