use crate::{
    Error, HelperRequest, HelperResponse, OpenRequest, OpenTarget, PROTOCOL_VERSION,
    RequestEnvelope, ResponseEnvelope, Result, WslTarget, read_frame, write_frame,
};
use fastdiff_core::{
    CancelToken, DiffDocument, FileChange, GitPath, ReflogList, ReflogRequest, Repository,
    TextDocument, View,
};
use std::{
    io::Write,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// Git and filesystem operations always execute on the repository's operating system.
pub enum Backend {
    Local {
        repository: Repository,
        root: String,
    },
    Document {
        repository: Option<Repository>,
        root: String,
    },
    Wsl(HelperClient),
}
impl Backend {
    pub fn discover(request: &OpenRequest) -> Result<Self> {
        Self::discover_with_cancel(request, &CancelToken::default())
    }
    pub fn discover_with_cancel(request: &OpenRequest, cancel: &CancelToken) -> Result<Self> {
        request.validate()?;
        if let Some(wsl) = &request.wsl {
            return Ok(Self::Wsl(HelperClient::spawn_wsl_target(
                wsl,
                &request.repository,
                request.target,
                cancel,
            )?));
        }
        if request.target == OpenTarget::Document {
            let path = std::path::absolute(&request.repository)?;
            let parent = path
                .parent()
                .ok_or_else(|| Error::InvalidArgument("Document has no parent directory".into()))?;
            let repository = Repository::discover_with_cancel(parent, cancel).ok();
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let root = repository
                .as_ref()
                .map(|repo| repo.root())
                .unwrap_or(parent)
                .to_string_lossy()
                .into_owned();
            return Ok(Self::Document { repository, root });
        }
        let repository = Repository::discover_with_cancel(&request.repository, cancel)
            .map_err(|error| Error::Backend(error.to_string()))?;
        let root = repository.root().to_string_lossy().into_owned();
        Ok(Self::Local { repository, root })
    }
    pub fn root(&self) -> &str {
        match self {
            Self::Local { root, .. } | Self::Document { root, .. } => root,
            Self::Wsl(client) => client.root(),
        }
    }
    pub fn has_repository(&self) -> bool {
        match self {
            Self::Local { .. } => true,
            Self::Document { repository, .. } => repository.is_some(),
            Self::Wsl(client) => client.has_repository,
        }
    }
    /// Reaps failed WSL helpers; call from the worker thread.
    pub fn needs_reconnect(&self) -> bool {
        match self {
            Self::Wsl(client) => client.needs_reconnect(),
            Self::Local { .. } | Self::Document { .. } => false,
        }
    }
    pub fn current_branch(&self, cancel: &CancelToken) -> Result<Option<String>> {
        match self {
            Self::Local { repository, .. }
            | Self::Document {
                repository: Some(repository),
                ..
            } => repository
                .current_branch(cancel)
                .map_err(|error| Error::Backend(error.to_string())),
            Self::Wsl(client) => client.current_branch(cancel),
            Self::Document {
                repository: None, ..
            } => Ok(None),
        }
    }
    pub fn reflog(&self, cancel: &CancelToken) -> Result<ReflogList> {
        self.reflog_page(&ReflogRequest::default(), cancel)
    }
    pub fn reflog_page(&self, request: &ReflogRequest, cancel: &CancelToken) -> Result<ReflogList> {
        match self {
            Self::Local { repository, .. }
            | Self::Document {
                repository: Some(repository),
                ..
            } => repository
                .reflog_page(request, cancel)
                .map_err(|error| Error::Backend(error.to_string())),
            Self::Wsl(client) => client.reflog_page(request, cancel),
            Self::Document {
                repository: None, ..
            } => Err(Error::Backend(
                "Document is outside a Git repository".into(),
            )),
        }
    }
    pub fn reflog_snapshot(&self, cancel: &CancelToken) -> Result<Option<u64>> {
        match self {
            Self::Local { repository, .. }
            | Self::Document {
                repository: Some(repository),
                ..
            } => repository
                .reflog_snapshot(cancel)
                .map_err(|error| Error::Backend(error.to_string())),
            Self::Wsl(client) => client.reflog_snapshot(cancel),
            Self::Document {
                repository: None, ..
            } => Ok(None),
        }
    }
    pub fn document(&self, path: &str, cancel: &CancelToken) -> Result<TextDocument> {
        if path.is_empty() || path.contains('\0') {
            return Err(Error::InvalidArgument(
                "Document path must be nonempty and contain no NUL".into(),
            ));
        }
        match self {
            Self::Wsl(client) => client.document(path, cancel),
            _ => {
                let path = if Path::new(path).is_absolute() {
                    std::path::PathBuf::from(path)
                } else {
                    Path::new(self.root()).join(path)
                };
                fastdiff_core::read_document(path, cancel)
                    .map_err(|error| Error::Backend(error.to_string()))
            }
        }
    }
    pub fn document_exists(&self, path: &GitPath, cancel: &CancelToken) -> Result<bool> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if let Self::Wsl(client) = self {
            return client.document_exists(path, cancel);
        }
        let path = path
            .to_path_buf()
            .map_err(|error| Error::Backend(error.to_string()))?;
        if path.as_os_str().is_empty() {
            return Err(Error::InvalidArgument(
                "Document path must not be empty".into(),
            ));
        }
        let path = if path.is_absolute() {
            path
        } else {
            Path::new(self.root()).join(path)
        };
        match std::fs::metadata(path) {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
    pub fn status(&self, view: View, cancel: &CancelToken) -> Result<Vec<FileChange>> {
        self.status_with_notice(view, cancel)
            .map(|(files, _)| files)
    }
    pub fn status_with_notice(
        &self,
        view: View,
        cancel: &CancelToken,
    ) -> Result<(Vec<FileChange>, Option<String>)> {
        match self {
            Self::Local { repository, .. }
            | Self::Document {
                repository: Some(repository),
                ..
            } => repository
                .status_with_notice(view, cancel)
                .map_err(|error| Error::Backend(error.to_string())),
            Self::Wsl(client) => client.status_with_notice(view, cancel),
            Self::Document {
                repository: None, ..
            } => Ok((Vec::new(), None)),
        }
    }
    pub fn diff(
        &self,
        change: &FileChange,
        view: View,
        cancel: &CancelToken,
    ) -> Result<DiffDocument> {
        match self {
            Self::Local { repository, .. }
            | Self::Document {
                repository: Some(repository),
                ..
            } => repository
                .diff(change, view, cancel)
                .map_err(|error| Error::Backend(error.to_string())),
            Self::Wsl(client) => client.diff(change, view, cancel),
            Self::Document {
                repository: None, ..
            } => Err(Error::Backend(
                "Document is outside a Git repository".into(),
            )),
        }
    }
}

pub struct HelperClient {
    has_repository: bool,
    root: String,
    inner: Mutex<ClientProcess>,
}
struct ClientProcess {
    child: Child,
    input: Option<crate::pipe::CancellablePipe<ChildStdin>>,
    responses: mpsc::Receiver<Result<ResponseEnvelope>>,
    reader: Option<JoinHandle<()>>,
    next_id: u64,
    reader_stop: Arc<AtomicBool>,
}

impl HelperClient {
    pub fn spawn(program: &Path, repository: &str) -> Result<Self> {
        let mut command = Command::new(program);
        command.arg("--helper").arg(repository);
        Self::from_command(command, &CancelToken::default())
    }
    pub fn spawn_wsl(target: &WslTarget, repository: &str) -> Result<Self> {
        Self::spawn_wsl_with_cancel(target, repository, &CancelToken::default())
    }
    pub fn spawn_wsl_with_cancel(
        target: &WslTarget,
        repository: &str,
        cancel: &CancelToken,
    ) -> Result<Self> {
        Self::spawn_wsl_target(target, repository, OpenTarget::Repository, cancel)
    }
    pub fn spawn_wsl_target(
        target: &WslTarget,
        repository: &str,
        kind: OpenTarget,
        cancel: &CancelToken,
    ) -> Result<Self> {
        target.validate()?;
        let mut command = Command::new("wsl.exe");
        command.args([
            "--distribution",
            &target.distribution,
            "--user",
            &target.user,
            "--exec",
            &target.helper,
            "--helper",
            repository,
        ]);
        if kind == OpenTarget::Document {
            command.arg("--document");
        }
        Self::from_command(command, cancel)
    }
    fn from_command(mut command: Command, cancel: &CancelToken) -> Result<Self> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW for the owned WSL helper.
        }
        let mut child = command.spawn()?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| Error::Protocol("Helper stdin unavailable".into()))?;
        if let Err(error) = crate::pipe::nonblocking(&input) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
        let output = child
            .stdout
            .take()
            .ok_or_else(|| Error::Protocol("Helper stdout unavailable".into()))?;
        let reader_stop = Arc::new(AtomicBool::new(false));
        let input = crate::pipe::CancellablePipe::writer(input, reader_stop.clone());
        let mut output = match crate::pipe::reader(output, reader_stop.clone()) {
            Ok(output) => output,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        };
        let (sender, responses) = mpsc::sync_channel(8);
        let reader = thread::spawn(move || {
            loop {
                match read_frame(&mut output) {
                    Ok(Some(response)) => {
                        if sender.try_send(Ok(response)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {
                        let _ = sender.try_send(Err(Error::Protocol("Helper connection closed; check WSL distribution and matching helper installation".into())));
                        break;
                    }
                    Err(error) => {
                        let _ = sender.try_send(Err(error));
                        break;
                    }
                }
            }
        });
        let mut process = ClientProcess {
            child,
            input: Some(input),
            responses,
            reader: Some(reader),
            next_id: 1,
            reader_stop,
        };
        let response = process.request(HelperRequest::Hello, cancel)?;
        let (root, has_repository) = match response {
            HelperResponse::Hello {
                root,
                has_repository,
            } => (root, has_repository),
            HelperResponse::Error { message, .. } => return Err(Error::Backend(message)),
            _ => {
                return Err(Error::Protocol(
                    "Helper did not send a hello response".into(),
                ));
            }
        };
        Ok(Self {
            has_repository,
            root,
            inner: Mutex::new(process),
        })
    }
    pub fn root(&self) -> &str {
        &self.root
    }
    fn needs_reconnect(&self) -> bool {
        let (mut process, poisoned) = match self.inner.lock() {
            Ok(process) => (process, false),
            Err(poisoned) => (poisoned.into_inner(), true),
        };
        let needs_reconnect = poisoned
            || process.input.is_none()
            || process.reader.as_ref().is_some_and(JoinHandle::is_finished);
        if needs_reconnect && process.input.is_some() {
            process.terminate();
        }
        needs_reconnect
    }
    pub fn current_branch(&self, cancel: &CancelToken) -> Result<Option<String>> {
        if !self.has_repository {
            return Ok(None);
        }
        match self.request(HelperRequest::CurrentBranch, cancel)? {
            HelperResponse::CurrentBranch { branch } => Ok(branch),
            response => Err(self.unexpected(response)),
        }
    }
    pub fn reflog(&self, cancel: &CancelToken) -> Result<ReflogList> {
        self.reflog_page(&ReflogRequest::default(), cancel)
    }
    pub fn reflog_page(&self, request: &ReflogRequest, cancel: &CancelToken) -> Result<ReflogList> {
        match self.request(
            HelperRequest::Reflog {
                request: request.clone(),
            },
            cancel,
        )? {
            HelperResponse::Reflog { list } => Ok(list),
            response => Err(self.unexpected(response)),
        }
    }
    pub fn reflog_snapshot(&self, cancel: &CancelToken) -> Result<Option<u64>> {
        match self.request(HelperRequest::ReflogSnapshot, cancel)? {
            HelperResponse::ReflogSnapshot { snapshot } => Ok(snapshot),
            response => Err(self.unexpected(response)),
        }
    }
    pub fn status(&self, view: View, cancel: &CancelToken) -> Result<Vec<FileChange>> {
        self.status_with_notice(view, cancel)
            .map(|(files, _)| files)
    }
    pub fn status_with_notice(
        &self,
        view: View,
        cancel: &CancelToken,
    ) -> Result<(Vec<FileChange>, Option<String>)> {
        match self.request(HelperRequest::Status { view }, cancel)? {
            HelperResponse::Status { files, notice } => Ok((files, notice)),
            response => Err(self.unexpected(response)),
        }
    }
    pub fn diff(
        &self,
        change: &FileChange,
        view: View,
        cancel: &CancelToken,
    ) -> Result<DiffDocument> {
        match self.request(
            HelperRequest::Diff {
                change: change.clone(),
                view,
            },
            cancel,
        )? {
            HelperResponse::Diff { document } => Ok(document),
            response => Err(self.unexpected(response)),
        }
    }
    pub fn document(&self, path: &str, cancel: &CancelToken) -> Result<TextDocument> {
        match self.request(HelperRequest::Document { path: path.into() }, cancel)? {
            HelperResponse::Document { document } => Ok(document),
            response => Err(self.unexpected(response)),
        }
    }
    pub fn document_exists(&self, path: &GitPath, cancel: &CancelToken) -> Result<bool> {
        match self.request(HelperRequest::DocumentExists { path: path.clone() }, cancel)? {
            HelperResponse::DocumentExists { available } => Ok(available),
            response => Err(self.unexpected(response)),
        }
    }
    fn request(&self, request: HelperRequest, cancel: &CancelToken) -> Result<HelperResponse> {
        let mut process = self.inner.lock().map_err(|poisoned| {
            poisoned.into_inner().terminate();
            Error::Protocol("Helper client lock poisoned".into())
        })?;
        process.request(request, cancel)
    }
    fn unexpected(&self, response: HelperResponse) -> Error {
        match response {
            HelperResponse::Error { message, .. } => Error::Backend(message),
            HelperResponse::Cancelled => Error::Cancelled,
            _ => {
                self.inner
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .terminate();
                Error::Protocol("Unexpected helper response".into())
            }
        }
    }
}

impl ClientProcess {
    fn send(&mut self, request: HelperRequest) -> Result<u64> {
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .ok_or_else(|| Error::Protocol("Helper request ID overflow".into()))?;
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| Error::Protocol("Helper is closed".into()))?;
        let result = write_frame(
            input,
            &RequestEnvelope {
                version: PROTOCOL_VERSION,
                id,
                request,
            },
        );
        if let Err(error) = result {
            self.terminate();
            return Err(error);
        }
        Ok(id)
    }
    fn request(&mut self, request: HelperRequest, cancel: &CancelToken) -> Result<HelperResponse> {
        self.request_with_timeout(request, cancel, Duration::from_secs(30))
    }
    fn request_with_timeout(
        &mut self,
        request: HelperRequest,
        cancel: &CancelToken,
        timeout: Duration,
    ) -> Result<HelperResponse> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let id = self.send(request)?;
        let mut cancellation_sent = false;
        let mut deadline = Instant::now() + timeout;
        loop {
            if cancel.is_cancelled() && !cancellation_sent {
                self.send(HelperRequest::Cancel { request_id: id })?;
                cancellation_sent = true;
                deadline = Instant::now() + Duration::from_secs(2);
            }
            if Instant::now() >= deadline {
                self.terminate();
                return Err(Error::Protocol(
                    "Helper request timed out; press F5 to reconnect".into(),
                ));
            }
            match self.responses.recv_timeout(Duration::from_millis(10)) {
                Ok(Ok(response)) => {
                    if response.version != PROTOCOL_VERSION {
                        self.terminate();
                        return Err(Error::Protocol("Incompatible helper version; install matching Windows GUI and WSL helper".into()));
                    }
                    if response.id == id {
                        if cancel.is_cancelled() {
                            return Err(Error::Cancelled);
                        }
                        return Ok(response.response);
                    }
                }
                Ok(Err(error)) => {
                    self.terminate();
                    return Err(error);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => (),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.terminate();
                    return Err(Error::Protocol("Helper response reader stopped".into()));
                }
            }
        }
    }
    fn terminate(&mut self) {
        self.input.take();
        // Allow EOF to cancel Git inside the helper before forcibly stopping a stalled transport.
        let deadline = Instant::now() + Duration::from_millis(100);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        #[cfg(unix)]
        {
            if matches!(self.child.try_wait(), Ok(None)) {
                // SAFETY: this child was spawned as leader of its own process group; only its
                // owned descendants share the group. It is still alive, so its PID is not reused.
                unsafe {
                    libc::kill(-(self.child.id() as i32), libc::SIGKILL);
                }
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.reader_stop.store(true, Ordering::Release);
    }
}

#[cfg(all(test, unix))]
mod recovery_tests;

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    pub(super) fn process() -> (ClientProcess, mpsc::Sender<Result<ResponseEnvelope>>) {
        use std::os::unix::process::CommandExt;
        let mut child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let reader_stop = Arc::new(AtomicBool::new(false));
        let input = child.stdin.take().map(|input| {
            crate::pipe::nonblocking(&input).unwrap();
            crate::pipe::CancellablePipe::writer(input, reader_stop.clone())
        });
        let (sender, responses) = mpsc::channel();
        (
            ClientProcess {
                child,
                input,
                responses,
                reader: None,
                next_id: 1,
                reader_stop,
            },
            sender,
        )
    }
    fn response(id: u64, value: HelperResponse) -> Result<ResponseEnvelope> {
        Ok(ResponseEnvelope {
            version: PROTOCOL_VERSION,
            id,
            response: value,
        })
    }

    #[test]
    fn request_rejects_version_mismatch_reader_errors_and_disconnect() {
        let (mut client, sender) = process();
        sender
            .send(Ok(ResponseEnvelope {
                version: 99,
                id: 1,
                response: HelperResponse::Shutdown,
            }))
            .unwrap();
        assert!(
            client
                .request(HelperRequest::Hello, &CancelToken::default())
                .unwrap_err()
                .to_string()
                .contains("matching Windows GUI")
        );
        assert!(client.input.is_none());
        assert!(client.child.try_wait().unwrap().is_some());
        assert!(
            client
                .request(HelperRequest::Hello, &CancelToken::default())
                .unwrap_err()
                .to_string()
                .contains("Helper is closed")
        );
        let (mut client, sender) = process();
        sender
            .send(Err(Error::Protocol("broken frame".into())))
            .unwrap();
        assert!(
            client
                .request(HelperRequest::Hello, &CancelToken::default())
                .is_err()
        );
        assert!(client.input.is_none());
        assert!(client.child.try_wait().unwrap().is_some());
        assert!(
            client
                .request(HelperRequest::Hello, &CancelToken::default())
                .unwrap_err()
                .to_string()
                .contains("Helper is closed")
        );
        let (mut client, sender) = process();
        drop(sender);
        assert!(
            client
                .request(HelperRequest::Hello, &CancelToken::default())
                .unwrap_err()
                .to_string()
                .contains("reader stopped")
        );
        assert!(client.input.is_none());
        assert!(client.child.try_wait().unwrap().is_some());
        assert!(
            client
                .request(HelperRequest::Hello, &CancelToken::default())
                .unwrap_err()
                .to_string()
                .contains("Helper is closed")
        );
    }

    #[test]
    fn request_ignores_other_ids_and_maps_unexpected_results() {
        let (client, sender) = process();
        sender
            .send(response(99, HelperResponse::Cancelled))
            .unwrap();
        sender
            .send(response(
                1,
                HelperResponse::Hello {
                    root: "/repo".into(),
                    has_repository: true,
                },
            ))
            .unwrap();
        let helper = HelperClient {
            has_repository: true,
            root: "/repo".into(),
            inner: Mutex::new(client),
        };
        assert!(
            helper
                .status(View::All, &CancelToken::default())
                .unwrap_err()
                .to_string()
                .contains("Unexpected")
        );
        {
            let mut process = helper.inner.lock().unwrap();
            assert!(process.input.is_none());
            assert!(process.child.try_wait().unwrap().is_some());
        }
        assert!(
            helper
                .status(View::All, &CancelToken::default())
                .unwrap_err()
                .to_string()
                .contains("Helper is closed")
        );
        let (client, sender) = process();
        let helper = HelperClient {
            has_repository: true,
            root: "/repo".into(),
            inner: Mutex::new(client),
        };
        sender
            .send(response(
                1,
                HelperResponse::Error {
                    code: "git".into(),
                    message: "missing Git".into(),
                },
            ))
            .unwrap();
        assert!(
            helper
                .status(View::All, &CancelToken::default())
                .unwrap_err()
                .to_string()
                .contains("missing Git")
        );
        sender
            .send(response(
                2,
                HelperResponse::Status {
                    files: Vec::new(),
                    notice: None,
                },
            ))
            .unwrap();
        assert!(
            helper
                .status(View::All, &CancelToken::default())
                .unwrap()
                .is_empty()
        );
        sender.send(response(3, HelperResponse::Cancelled)).unwrap();
        assert!(matches!(
            helper.status(View::All, &CancelToken::default()),
            Err(Error::Cancelled)
        ));
        sender
            .send(response(
                4,
                HelperResponse::Status {
                    files: Vec::new(),
                    notice: None,
                },
            ))
            .unwrap();
        assert!(
            helper
                .status(View::All, &CancelToken::default())
                .unwrap()
                .is_empty()
        );
        assert!(helper.inner.lock().unwrap().input.is_some());
        assert!(!helper.needs_reconnect());
    }

    #[test]
    fn cancelled_request_sends_cancel_and_drains_final_response() {
        let (mut client, sender) = process();
        let token = CancelToken::default();
        let cancellation = token.clone();
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            cancellation.cancel();
            thread::sleep(Duration::from_millis(30));
            sender.send(response(2, HelperResponse::Cancelled)).unwrap();
            sender.send(response(1, HelperResponse::Cancelled)).unwrap();
            sender
                .send(response(
                    3,
                    HelperResponse::CurrentBranch {
                        branch: Some("main".into()),
                    },
                ))
                .unwrap();
        });
        assert!(matches!(
            client.request(HelperRequest::Hello, &token),
            Err(Error::Cancelled)
        ));
        assert_eq!(client.next_id, 3);
        worker.join().unwrap();
        assert!(matches!(
            client.request(HelperRequest::CurrentBranch, &CancelToken::default()),
            Ok(HelperResponse::CurrentBranch { branch: Some(_) })
        ));
        assert!(client.input.is_some());
    }

    #[test]
    fn timed_out_request_closes_and_reaps_owned_process() {
        let (mut client, _sender) = process();
        assert!(
            client
                .request_with_timeout(
                    HelperRequest::Hello,
                    &CancelToken::default(),
                    Duration::from_millis(20)
                )
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        assert!(client.input.is_none());
        assert!(client.child.try_wait().unwrap().is_some());
        assert!(client.reader_stop.load(Ordering::Acquire));
        assert!(client.send(HelperRequest::Hello).is_err());
    }

    #[test]
    fn request_id_overflow_is_reported() {
        let (mut client, _sender) = process();
        client.next_id = u64::MAX;
        assert!(
            client
                .send(HelperRequest::Hello)
                .unwrap_err()
                .to_string()
                .contains("overflow")
        );
    }

    #[test]
    fn helper_that_stops_reading_cannot_block_request_write_indefinitely() {
        use fastdiff_core::GitPath;
        use std::os::unix::process::CommandExt;
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        crate::pipe::nonblocking(&input).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let (_sender, responses) = mpsc::channel();
        let mut process = ClientProcess {
            child,
            input: Some(crate::pipe::CancellablePipe::writer(input, stop.clone())),
            responses,
            reader: None,
            next_id: 1,
            reader_stop: stop,
        };
        let change = FileChange {
            committed: None,
            path: GitPath(vec![b'x'; 1_000_000]),
            old_path: None,
            staged: false,
            unstaged: true,
            untracked: true,
            conflict: false,
            status: "?".into(),
        };
        let started = Instant::now();
        assert!(
            process
                .send(HelperRequest::Diff {
                    change,
                    view: View::All
                })
                .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(process.child.try_wait().unwrap().is_some());
    }
}
impl Drop for ClientProcess {
    fn drop(&mut self) {
        if let Some(input) = &mut self.input {
            let _ = input.flush();
        }
        self.input.take(); // EOF is the helper's shutdown and cancellation signal.
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => {
                    self.terminate();
                    break;
                }
            }
        }
        self.reader_stop.store(true, Ordering::Release);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
