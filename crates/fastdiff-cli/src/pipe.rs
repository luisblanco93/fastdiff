//! Cancellation-aware pipe I/O keeps EOF shutdown independent of a peer draining stdout.
use std::{
    io::{self, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

pub(crate) struct CancellablePipe<T> {
    inner: T,
    stop: Arc<AtomicBool>,
    stall_limit: Option<Duration>,
    #[cfg(windows)]
    read_handle: Option<usize>,
}
impl<T> CancellablePipe<T> {
    pub(crate) fn new(inner: T, stop: Arc<AtomicBool>) -> Self {
        Self {
            inner,
            stop,
            stall_limit: None,
            #[cfg(windows)]
            read_handle: None,
        }
    }
    pub(crate) fn writer(inner: T, stop: Arc<AtomicBool>) -> Self {
        Self {
            inner,
            stop,
            stall_limit: Some(Duration::from_secs(1)),
            #[cfg(windows)]
            read_handle: None,
        }
    }
    fn retry<U>(&mut self, mut operation: impl FnMut(&mut T) -> io::Result<U>) -> io::Result<U> {
        let deadline = self
            .stall_limit
            .map(|limit| std::time::Instant::now() + limit);
        loop {
            if self.stop.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "Helper pipe closed",
                ));
            }
            if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Helper peer did not drain its pipe",
                ));
            }
            match operation(&mut self.inner) {
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        || cfg!(windows) && error.raw_os_error() == Some(232) =>
                {
                    thread::sleep(Duration::from_millis(5))
                }
                result => return result,
            }
        }
    }
}
impl<T: Read> Read for CancellablePipe<T> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        #[cfg(windows)]
        let handle = self.read_handle;
        self.retry(|inner| {
            #[cfg(windows)]
            if let Some(handle) = handle {
                let mut available = 0;
                // SAFETY: the inner pipe owns this handle, the output DWORD is initialized,
                // and all optional read buffers are null. Peek never consumes pipe bytes.
                let success = unsafe {
                    windows_sys::Win32::System::Pipes::PeekNamedPipe(
                        handle as _,
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null_mut(),
                        &mut available,
                        std::ptr::null_mut(),
                    )
                };
                if success == 0 {
                    return Err(io::Error::last_os_error());
                }
                if available == 0 {
                    return Err(io::ErrorKind::WouldBlock.into());
                }
                let length = buffer.len().min(available as usize);
                return inner.read(&mut buffer[..length]);
            }
            inner.read(buffer)
        })
    }
}

#[cfg(unix)]
pub(crate) fn reader<T: Read + std::os::fd::AsRawFd>(
    inner: T,
    stop: Arc<AtomicBool>,
) -> io::Result<CancellablePipe<T>> {
    nonblocking(&inner)?;
    Ok(CancellablePipe::new(inner, stop))
}

#[cfg(windows)]
pub(crate) fn reader<T: Read + std::os::windows::io::AsRawHandle>(
    inner: T,
    stop: Arc<AtomicBool>,
) -> io::Result<CancellablePipe<T>> {
    let handle = inner.as_raw_handle() as usize;
    let pipe = CancellablePipe::new(inner, stop);
    Ok(CancellablePipe {
        read_handle: Some(handle),
        ..pipe
    })
}
impl<T: Write> Write for CancellablePipe<T> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.retry(|inner| match inner.write(buffer) {
            Ok(0) if !buffer.is_empty() => Err(io::ErrorKind::WouldBlock.into()),
            result => result,
        })
    }
    fn flush(&mut self) -> io::Result<()> {
        self.retry(Write::flush)
    }
}

#[cfg(unix)]
pub(crate) fn nonblocking(pipe: &impl std::os::fd::AsRawFd) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    // SAFETY: the borrowed pipe owns this valid descriptor throughout both fcntl calls.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: setting O_NONBLOCK on the same live descriptor does not change its ownership.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn nonblocking(pipe: &impl std::os::windows::io::AsRawHandle) -> io::Result<()> {
    use windows_sys::Win32::System::Pipes::{PIPE_NOWAIT, SetNamedPipeHandleState};
    let mode = PIPE_NOWAIT;
    // SAFETY: the borrowed anonymous pipe keeps its handle valid; optional output pointers are null.
    let result = unsafe {
        SetNamedPipeHandleState(
            pipe.as_raw_handle().cast(),
            &mode,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
