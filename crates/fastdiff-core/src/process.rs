use crate::{Error, Result};
#[cfg(windows)]
mod windows_job;
use std::{
    io::Read,
    path::Path,
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, ScopedJoinHandle},
    time::Duration,
};

#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub(crate) fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

pub(crate) struct GitOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Join a scoped thread, resuming its panic on the caller instead of swallowing it.
pub(crate) fn join_scoped<T>(handle: ScopedJoinHandle<'_, T>) -> T {
    handle
        .join()
        .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
}

pub(crate) fn git_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .args([
            "--no-pager",
            "--literal-pathspecs",
            "-c",
            "color.ui=false",
            "-c",
            "core.quotepath=false",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "diff.external=",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null());
    command
}

/// Drain both pipes concurrently so a noisy Git cannot deadlock the parent.
pub(crate) fn run(mut command: Command, limit: usize, cancel: &CancelToken) -> Result<GitOutput> {
    cancel.check()?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
        command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    }
    #[cfg(windows)]
    let job = windows_job::WindowsJob::new()?;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(Error::GitUnavailable)?;
    #[cfg(windows)]
    if let Err(error) = job.attach_and_resume(&child) {
        job.terminate();
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    let overflow = Arc::new(AtomicBool::new(false));
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::InvalidData("missing stdout".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::InvalidData("missing stderr".into()))?;
    let out_reader = read_pipe(stdout, limit, overflow.clone());
    let err_reader = read_pipe(stderr, 64 * 1024, overflow.clone());
    let status = loop {
        if cancel.is_cancelled() || overflow.load(Ordering::Acquire) {
            #[cfg(unix)]
            terminate_tree(child.id());
            #[cfg(windows)]
            job.terminate();
            let _ = child.kill();
            break child.wait();
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(error) => {
                #[cfg(unix)]
                terminate_tree(child.id());
                #[cfg(windows)]
                job.terminate();
                let _ = child.kill();
                let _ = child.wait();
                break Err(error);
            }
        }
    };
    // A child can exit before an owned descendant closes an inherited pipe.
    // Cleanup the isolated group before joining the output readers.
    #[cfg(unix)]
    terminate_tree(child.id());
    #[cfg(windows)]
    job.terminate();
    let stdout = join_reader(out_reader)?;
    let stderr = join_reader(err_reader)?;
    cancel.check()?;
    if overflow.load(Ordering::Acquire) {
        return Err(Error::OutputLimit);
    }
    Ok(GitOutput {
        status: status?,
        stdout,
        stderr,
    })
}

#[cfg(unix)]
fn terminate_tree(pid: u32) {
    // SAFETY: run() creates a new process group led by this owned child;
    // negative pid addresses only that group. kill does not dereference memory.
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
}

fn read_pipe(
    reader: impl Read + Send + 'static,
    limit: usize,
    overflow: Arc<AtomicBool>,
) -> thread::JoinHandle<std::io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut output = Vec::new();
        reader.take(limit as u64 + 1).read_to_end(&mut output)?;
        if output.len() > limit {
            overflow.store(true, Ordering::Release);
        }
        Ok(output)
    })
}

fn join_reader(reader: thread::JoinHandle<std::io::Result<Vec<u8>>>) -> Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| Error::InvalidData("Git output worker failed".into()))?
        .map_err(Error::Io)
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn windows_job_starts_with_no_owned_processes() {
        let job = windows_job::WindowsJob::new().unwrap();
        job.terminate();
    }

    #[test]
    fn background_descendant_cannot_keep_pipes_open_after_parent_exit() {
        let start = Instant::now();
        let mut command = Command::new("cmd");
        command.args(["/C", "start /B ping -n 5 127.0.0.1 > NUL"]);
        let _ = run(command, 4096, &CancelToken::default()).unwrap();
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}

pub(crate) fn checked(output: GitOutput) -> Result<Vec<u8>> {
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(Error::Git(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn unavailable_program_has_actionable_error() {
        let error = run(
            Command::new("/definitely/not/a/program"),
            100,
            &CancelToken::default(),
        )
        .err()
        .unwrap();
        assert!(matches!(error, Error::GitUnavailable(_)));
        assert!(error.to_string().contains("Install Git"));
    }

    #[test]
    fn large_output_is_bounded_without_deadlocking() {
        let mut command = Command::new("head");
        command.args(["-c", "10000000", "/dev/zero"]);
        assert!(matches!(
            run(command, 1024, &CancelToken::default()),
            Err(Error::OutputLimit)
        ));
    }

    #[test]
    fn cancelling_running_child_kills_and_reaps_it_promptly() {
        let cancel = CancelToken::default();
        let worker_cancel = cancel.clone();
        let start = Instant::now();
        let worker = thread::spawn(move || {
            let mut command = Command::new("sleep");
            command.arg("30");
            run(command, 100, &worker_cancel)
        });
        thread::sleep(Duration::from_millis(30));
        cancel.cancel();
        assert!(matches!(worker.join().unwrap(), Err(Error::Cancelled)));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn cancelling_child_also_terminates_descendants_holding_pipes() {
        let cancel = CancelToken::default();
        let worker_cancel = cancel.clone();
        let start = Instant::now();
        let worker = thread::spawn(move || {
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 2; printf done"]);
            run(command, 100, &worker_cancel)
        });
        thread::sleep(Duration::from_millis(30));
        cancel.cancel();
        assert!(matches!(worker.join().unwrap(), Err(Error::Cancelled)));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn unsuccessful_command_propagates_stderr() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf expected-error >&2; exit 2"]);
        let output = run(command, 100, &CancelToken::default()).unwrap();
        assert!(matches!(checked(output), Err(Error::Git(message)) if message == "expected-error"));
    }
}
