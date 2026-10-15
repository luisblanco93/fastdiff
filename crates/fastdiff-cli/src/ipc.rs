use crate::{Error, OpenRequest, PROTOCOL_VERSION, Result, read_frame, write_frame};
use fs2::FileExt;
use interprocess::local_socket::{
    GenericFilePath, ListenerNonblockingMode, ListenerOptions, Stream, prelude::*,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const LOCK_FILE: &str = "instance.lock";
type WakeCallback = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Incoming {
    version: u32,
    request: OpenRequest,
}
#[derive(Serialize, Deserialize)]
struct Acknowledgement {
    version: u32,
    accepted: bool,
    error: Option<String>,
}

/// Owns the app's endpoint and queues accepted requests without blocking the UI thread.
pub struct IpcServer {
    _lock: File,
    stop: Arc<AtomicBool>,
    requests: mpsc::Receiver<OpenRequest>,
    errors: mpsc::Receiver<String>,
    wake_callback: WakeCallback,
    worker: Option<JoinHandle<()>>,
}

impl IpcServer {
    /// Returns `None` if another GUI owns the endpoint. Never replaces a live listener.
    pub fn bind() -> Result<Option<Self>> {
        Self::bind_at(&runtime_directory()?)
    }

    pub fn bind_at(directory: &Path) -> Result<Option<Self>> {
        let state = state_directory(directory);
        ensure_private_directory(&state)?;
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(state.join(LOCK_FILE))?;
        match lock.try_lock_exclusive() {
            Ok(()) => (),
            Err(error) if lock_is_contended(&error) => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        let name = endpoint(directory)?;
        let listener_options = ListenerOptions::new()
            .name(name.to_fs_name::<GenericFilePath>()?)
            .nonblocking(ListenerNonblockingMode::Both)
            .try_overwrite(true);
        #[cfg(windows)]
        let listener_options = {
            use interprocess::os::windows::{
                local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor,
            };
            // Grant the object's owner alone access to this per-user named pipe.
            listener_options.security_descriptor(SecurityDescriptor::deserialize(
                widestring::u16cstr!("D:P(A;;GA;;;OW)"),
            )?)
        };
        let listener = listener_options.create_sync()?;
        let (sender, requests) = mpsc::sync_channel(128);
        let (error_sender, errors) = mpsc::sync_channel(16);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let wake_callback: WakeCallback = Arc::new(Mutex::new(None));
        let worker_wake_callback = wake_callback.clone();
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok(stream) => {
                        if let Err(error) = accept_request(
                            stream,
                            &sender,
                            worker_stop.clone(),
                            &worker_wake_callback,
                        ) && error_sender.try_send(error.to_string()).is_ok()
                        {
                            notify(&worker_wake_callback);
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(POLL_INTERVAL)
                    }
                    Err(error) => {
                        if error_sender.try_send(error.to_string()).is_ok() {
                            notify(&worker_wake_callback);
                        }
                        break;
                    }
                }
            }
        });
        Ok(Some(Self {
            _lock: lock,
            stop,
            requests,
            errors,
            wake_callback,
            worker: Some(worker),
        }))
    }

    /// Installs or replaces the wake callback, then wakes once for any pending requests.
    /// The callback runs on the registering or listener thread and should return promptly.
    pub fn set_wake_callback(&self, callback: impl Fn() + Send + Sync + 'static) {
        let callback: Arc<dyn Fn() + Send + Sync> = Arc::new(callback);
        *self
            .wake_callback
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(callback.clone());
        callback();
    }

    pub fn poll(&self) -> Result<Vec<OpenRequest>> {
        // A malformed client is isolated. Report it once, then continue serving subsequent clients.
        if let Ok(error) = self.errors.try_recv() {
            return Err(Error::Protocol(error));
        }
        Ok(self.requests.try_iter().collect())
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        // The listener is gone before the ownership lock is released.
    }
}

pub fn forward(request: &OpenRequest) -> Result<bool> {
    forward_at(&runtime_directory()?, request)
}

pub fn forward_at(directory: &Path, request: &OpenRequest) -> Result<bool> {
    request.validate()?;
    let name = endpoint(directory)?;
    let deadline = Instant::now() + CONNECTION_TIMEOUT;
    let stream = loop {
        match Stream::connect(name.as_path().to_fs_name::<GenericFilePath>()?) {
            Ok(stream) => break stream,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                if !publisher_owns_lock(directory)? {
                    return Ok(false);
                }
                if Instant::now() >= deadline {
                    return Err(Error::Protocol(
                        "GUI owns the startup lock but did not publish its endpoint".into(),
                    ));
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(error) => return Err(error.into()),
        }
    };
    stream.set_nonblocking(true)?;
    let mut stream = DeadlineStream::new(stream, Arc::new(AtomicBool::new(false)));
    write_frame(
        &mut stream,
        &Incoming {
            version: PROTOCOL_VERSION,
            request: request.clone(),
        },
    )?;
    let ack: Acknowledgement = read_frame(&mut stream)?
        .ok_or_else(|| Error::Protocol("GUI closed before accepting request".into()))?;
    if ack.version != PROTOCOL_VERSION || !ack.accepted {
        return Err(Error::Protocol(ack.error.unwrap_or_else(|| {
            "Incompatible GUI protocol; install matching launcher and GUI versions".into()
        })));
    }
    Ok(true)
}

fn accept_request(
    stream: Stream,
    sender: &mpsc::SyncSender<OpenRequest>,
    stop: Arc<AtomicBool>,
    wake_callback: &WakeCallback,
) -> Result<()> {
    let mut stream = DeadlineStream::new(stream, stop);
    let incoming: Incoming =
        read_frame(&mut stream)?.ok_or_else(|| Error::Protocol("Empty launch request".into()))?;
    let accepted = if incoming.version != PROTOCOL_VERSION {
        Err(Error::Protocol(
            "Incompatible launcher protocol; install matching versions".into(),
        ))
    } else {
        incoming.request.validate().and_then(|()| {
            sender
                .try_send(incoming.request)
                .map_err(|_| Error::Protocol("GUI request queue is full or shutting down".into()))
        })
    };
    if accepted.is_ok() {
        notify(wake_callback);
    }
    // Acceptance acknowledges a request only after it has entered the GUI queue.
    write_frame(
        &mut stream,
        &Acknowledgement {
            version: PROTOCOL_VERSION,
            accepted: accepted.is_ok(),
            error: accepted.as_ref().err().map(ToString::to_string),
        },
    )?;
    accepted
}

fn notify(wake_callback: &WakeCallback) {
    let callback = wake_callback
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if let Some(callback) = callback {
        callback();
    }
}

fn publisher_owns_lock(directory: &Path) -> Result<bool> {
    let lock = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(state_directory(directory).join(LOCK_FILE))
    {
        Ok(lock) => lock,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    match lock.try_lock_exclusive() {
        Ok(()) => Ok(false),
        Err(error) if lock_is_contended(&error) => Ok(true),
        Err(error) => Err(error.into()),
    }
}

fn lock_is_contended(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
        || error.raw_os_error().is_some()
            && error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

fn runtime_directory() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("FASTDIFF_RUNTIME_DIR") {
        return Ok(PathBuf::from(path));
    }
    #[cfg(unix)]
    {
        let uid = current_uid();
        Ok(std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("fastdiff-{uid}")))
    }
    #[cfg(windows)]
    {
        let base = std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| Error::InvalidArgument("LOCALAPPDATA is unavailable".into()))?;
        Ok(PathBuf::from(base).join("fastdiff"))
    }
}

/// Effective user id of this process. Portable: `/proc` does not exist on macOS.
#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Directory that holds the instance lock for a runtime `directory`.
///
/// The default Windows runtime directory is also the installer's data root, so the lock
/// lives in a `run` subdirectory there and never keeps the root from being removed once it
/// is otherwise empty. Endpoint names still derive from `directory`, so pipe names are
/// unaffected.
fn state_directory(directory: &Path) -> PathBuf {
    if cfg!(windows) {
        directory.join("run")
    } else {
        directory.to_path_buf()
    }
}

fn endpoint(directory: &Path) -> Result<PathBuf> {
    #[cfg(unix)]
    {
        Ok(directory.join("gui.sock"))
    }
    #[cfg(windows)]
    {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        directory.hash(&mut hasher);
        Ok(PathBuf::from(format!(
            r"\\.\pipe\fastdiff-{:x}",
            hasher.finish()
        )))
    }
}

fn ensure_private_directory(directory: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        match builder.create(directory) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
        }
        let metadata = std::fs::symlink_metadata(directory)?;
        let uid = current_uid();
        if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            return Err(Error::InvalidArgument(
                "IPC directory must be owned by this user with mode 0700".into(),
            ));
        }
    }
    #[cfg(windows)]
    std::fs::create_dir_all(directory)?;
    Ok(())
}

struct DeadlineStream {
    stream: Stream,
    deadline: Instant,
    stop: Arc<AtomicBool>,
}
impl DeadlineStream {
    fn new(stream: Stream, stop: Arc<AtomicBool>) -> Self {
        Self {
            stream,
            deadline: Instant::now() + CONNECTION_TIMEOUT,
            stop,
        }
    }
    fn retry<T>(&self, mut operation: impl FnMut() -> io::Result<T>) -> io::Result<T> {
        loop {
            if self.stop.load(Ordering::Acquire) || Instant::now() >= self.deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "IPC request timed out",
                ));
            }
            match operation() {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(POLL_INTERVAL)
                }
                result => return result,
            }
        }
    }
}
impl Read for DeadlineStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        self.retry(|| {
            #[cfg(windows)]
            {
                use std::os::windows::io::{AsHandle, AsRawHandle};
                use windows_sys::Win32::{
                    Foundation::{ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED},
                    System::Pipes::PeekNamedPipe,
                };
                let Stream::NamedPipe(pipe) = &self.stream;
                let mut available = 0;
                // SAFETY: the stream owns this live pipe handle; the initialized DWORD is
                // writable and all optional peek buffers are null. No bytes are consumed.
                let success = unsafe {
                    PeekNamedPipe(
                        pipe.as_handle().as_raw_handle().cast(),
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null_mut(),
                        &mut available,
                        std::ptr::null_mut(),
                    )
                };
                if success == 0 {
                    let error = io::Error::last_os_error();
                    if matches!(error.raw_os_error(), Some(code) if code == ERROR_BROKEN_PIPE as i32 || code == ERROR_PIPE_NOT_CONNECTED as i32) {
                        return Ok(0);
                    }
                    return Err(error);
                }
                // PIPE_NOWAIT reads can report zero bytes for a connected, empty pipe.
                // Peek distinguishes that temporary state from an actual peer disconnect.
                if available == 0 {
                    return Err(io::ErrorKind::WouldBlock.into());
                }
                let length = buffer.len().min(available as usize);
                (&self.stream).read(&mut buffer[..length])
            }
            #[cfg(unix)]
            (&self.stream).read(buffer)
        })
    }
}
impl Write for DeadlineStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.retry(|| match (&self.stream).write(buffer) {
            Ok(0) if !buffer.is_empty() => Err(io::ErrorKind::WouldBlock.into()),
            result => result,
        })
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, atomic::AtomicUsize};

    #[cfg(unix)]
    #[test]
    fn current_uid_owns_files_created_by_this_process() {
        use std::os::unix::fs::MetadataExt;
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("owned");
        std::fs::write(&file, b"").unwrap();
        assert_eq!(current_uid(), std::fs::metadata(file).unwrap().uid());
    }

    fn request() -> OpenRequest {
        OpenRequest {
            target: crate::OpenTarget::Repository,
            repository: "/some/repository".into(),
            file: None,
            view: fastdiff_core::View::All,
            wsl: None,
            theme: false,
        }
    }

    fn callback_slot(callback: impl Fn() + Send + Sync + 'static) -> WakeCallback {
        Arc::new(Mutex::new(Some(Arc::new(callback))))
    }

    fn private_directory() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        directory
    }

    fn pair() -> (tempfile::TempDir, DeadlineStream, Stream) {
        let directory = private_directory();
        let name = endpoint(directory.path()).unwrap();
        let listener = ListenerOptions::new()
            .name(name.as_path().to_fs_name::<GenericFilePath>().unwrap())
            .create_sync()
            .unwrap();
        let peer =
            Stream::connect(name.as_path().to_fs_name::<GenericFilePath>().unwrap()).unwrap();
        let stream = listener.accept().unwrap();
        stream.set_nonblocking(true).unwrap();
        (
            directory,
            DeadlineStream::new(stream, Arc::new(AtomicBool::new(false))),
            peer,
        )
    }

    #[test]
    fn connected_empty_stream_waits_until_payload_arrives() {
        let (_directory, mut stream, mut peer) = pair();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            peer.write_all(b"open").unwrap();
        });
        let mut message = [0; 4];
        stream.read_exact(&mut message).unwrap();
        assert_eq!(&message, b"open");
        writer.join().unwrap();
    }

    #[test]
    fn connected_empty_stream_waits_until_peer_disconnects() {
        let (_directory, mut stream, peer) = pair();
        let closed = Arc::new(AtomicBool::new(false));
        let peer_closed = closed.clone();
        let closer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            peer_closed.store(true, Ordering::Release);
            drop(peer);
        });
        assert_eq!(stream.read(&mut [0; 4]).unwrap(), 0);
        assert!(closed.load(Ordering::Acquire));
        closer.join().unwrap();
    }

    #[test]
    fn shutdown_cancels_client_that_never_sends_a_frame() {
        let directory = private_directory();
        let server = IpcServer::bind_at(directory.path()).unwrap().unwrap();
        let name = endpoint(directory.path()).unwrap();
        let _peer = Stream::connect(name.to_fs_name::<GenericFilePath>().unwrap()).unwrap();
        thread::sleep(Duration::from_millis(30));
        let started = Instant::now();
        drop(server);
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn queued_request_wakes_before_acknowledgement_is_written() {
        let (_directory, stream, mut peer) = pair();
        write_frame(
            &mut peer,
            &Incoming {
                version: PROTOCOL_VERSION,
                request: request(),
            },
        )
        .unwrap();
        let (sender, requests) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let callback_stop = stop.clone();
        let wakes = Arc::new(AtomicUsize::new(0));
        let callback_wakes = wakes.clone();
        let callback = callback_slot(move || {
            callback_wakes.fetch_add(1, Ordering::SeqCst);
            callback_stop.store(true, Ordering::Release);
        });
        assert!(accept_request(stream.stream, &sender, stop, &callback).is_err());
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        assert_eq!(requests.try_recv().unwrap().repository, "/some/repository");
    }

    #[test]
    fn failed_acknowledgement_keeps_request_and_still_wakes() {
        let (_directory, stream, mut peer) = pair();
        write_frame(
            &mut peer,
            &Incoming {
                version: PROTOCOL_VERSION,
                request: request(),
            },
        )
        .unwrap();
        let peer = Mutex::new(Some(peer));
        let (sender, requests) = mpsc::sync_channel(1);
        let wakes = Arc::new(AtomicUsize::new(0));
        let callback_wakes = wakes.clone();
        let callback = callback_slot(move || {
            callback_wakes.fetch_add(1, Ordering::SeqCst);
            let peer = peer.lock().unwrap().take().unwrap();
            #[cfg(windows)]
            {
                let Stream::NamedPipe(pipe) = &peer;
                // The request was consumed; close now instead of deferring a flush to limbo.
                pipe.inner().assume_flushed();
            }
            drop(peer);
        });
        assert!(
            accept_request(
                stream.stream,
                &sender,
                Arc::new(AtomicBool::new(false)),
                &callback,
            )
            .is_err()
        );
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        assert_eq!(requests.try_recv().unwrap().repository, "/some/repository");
    }

    #[test]
    fn malformed_requests_wake_and_do_not_hide_valid_request_behind_errors() {
        let directory = private_directory();
        let server = IpcServer::bind_at(directory.path()).unwrap().unwrap();
        let (wake_sender, wakes) = mpsc::channel();
        server.set_wake_callback(move || {
            let _ = wake_sender.send(());
        });
        wakes.recv_timeout(CONNECTION_TIMEOUT).unwrap();
        for _ in 0..2 {
            let name = endpoint(directory.path()).unwrap();
            let mut peer = Stream::connect(name.to_fs_name::<GenericFilePath>().unwrap()).unwrap();
            write_frame(&mut peer, &serde_json::json!({"unexpected": true})).unwrap();
            drop(peer);
            wakes.recv_timeout(CONNECTION_TIMEOUT).unwrap();
        }
        assert!(forward_at(directory.path(), &request()).unwrap());
        wakes.recv_timeout(CONNECTION_TIMEOUT).unwrap();
        assert!(server.poll().is_err());
        assert!(server.poll().is_err());
        let queued = server.poll().unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].repository, "/some/repository");
        assert!(wakes.try_recv().is_err());
    }

    #[test]
    fn notification_releases_callback_lock_before_invoking() {
        let callback: WakeCallback = Arc::new(Mutex::new(None));
        let callback_lock = Arc::downgrade(&callback);
        let wakes = Arc::new(AtomicUsize::new(0));
        let callback_wakes = wakes.clone();
        *callback.lock().unwrap() = Some(Arc::new(move || {
            assert!(callback_lock.upgrade().unwrap().try_lock().is_ok());
            callback_wakes.fetch_add(1, Ordering::SeqCst);
        }));
        notify(&callback);
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn instance_lock_is_kept_out_of_the_runtime_directory_on_windows() {
        let directory = private_directory();
        let _server = IpcServer::bind_at(directory.path()).unwrap().unwrap();
        assert!(state_directory(directory.path()).join(LOCK_FILE).is_file());
        assert_eq!(directory.path().join(LOCK_FILE).exists(), !cfg!(windows));
    }

    #[test]
    fn registration_and_notification_recover_poisoned_callback_lock() {
        let directory = private_directory();
        let server = IpcServer::bind_at(directory.path()).unwrap().unwrap();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _guard = server.wake_callback.lock().unwrap();
                panic!("poison callback lock for recovery test");
            }))
            .is_err()
        );
        let wakes = Arc::new(AtomicUsize::new(0));
        let callback_wakes = wakes.clone();
        server.set_wake_callback(move || {
            callback_wakes.fetch_add(1, Ordering::SeqCst);
        });
        assert!(forward_at(directory.path(), &request()).unwrap());
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
        assert_eq!(server.poll().unwrap().len(), 1);
    }
}
