use crate::{Error, OpenRequest, OpenTarget, READY_MESSAGE, Result, WslTarget, forward};
use std::{
    ffi::OsStr,
    io::{BufRead, BufReader, Read, Seek, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const ACCEPT_TIMEOUT: Duration = Duration::from_secs(10);
const STARTUP_OUTPUT_LIMIT: u64 = 4096;

pub fn launch(mut request: OpenRequest, native: bool) -> Result<()> {
    let repository = measure_launch_phase("repository_validation", || {
        let repository = std::fs::canonicalize(&request.repository)?;
        if request.target == OpenTarget::Repository {
            fastdiff_core::Repository::discover(&repository).map_err(|error| {
                Error::Backend(format!(
                    "Cannot open Git repository at '{}': {error}",
                    repository.display()
                ))
            })?;
        }
        Ok::<_, Error>(repository)
    })?;
    request.repository = repository
        .into_os_string()
        .into_string()
        .map_err(|_| Error::InvalidArgument("Repository paths must be valid Unicode".into()))?;
    let windows_from_wsl =
        cfg!(target_os = "linux") && !native && std::env::var_os("WSL_DISTRO_NAME").is_some();
    let executable = if windows_from_wsl {
        request.wsl = Some(wsl_target()?);
        windows_executable()?
    } else {
        let phase = LaunchPhase::start();
        let forwarded = forward(&request);
        if let Some(phase) = phase {
            phase.record(if matches!(&forwarded, Ok(true)) {
                "forwarding_acceptance"
            } else {
                "forwarding_probe"
            });
        }
        if forwarded? {
            return Ok(());
        }
        native_executable()?
    };
    request.validate()?;
    let mut command = Command::new(executable);
    command
        .arg("--request")
        .arg(serde_json::to_string(&request)?)
        .arg("--ready-stdio");
    if windows_from_wsl {
        propagate_benchmark_environment(&mut command);
    }
    start_and_wait_for_acceptance(command)
}

fn start_and_wait_for_acceptance(mut command: Command) -> Result<()> {
    // ponytail: stderr uses a temporary file until GUI exit; rotate if runtime logging grows.
    let stderr = tempfile::tempfile()?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(stderr.try_clone()?);
    let mut child = measure_launch_phase("process_start", || command.spawn())?;
    let output = child
        .stdout
        .take()
        .ok_or_else(|| Error::Protocol("GUI startup acknowledgement pipe unavailable".into()))?;
    let (sender, received) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut reader = BufReader::new(output.take(STARTUP_OUTPUT_LIMIT));
        let mut line = String::new();
        let result = reader
            .read_line(&mut line)
            .map(|_| line.trim_end().to_owned());
        let _ = sender.send(result);
    });
    let acknowledgement = measure_launch_phase("forwarding_acceptance", || {
        received.recv_timeout(ACCEPT_TIMEOUT)
    });
    if matches!(&acknowledgement, Ok(Ok(message)) if message == READY_MESSAGE) {
        let _ = reader.join();
        return Ok(());
    }
    let reason = match acknowledgement {
        Ok(Ok(message)) if message.is_empty() => "GUI closed stdout without acknowledging".into(),
        Ok(Ok(message)) => format!("Unexpected GUI acknowledgement: {message}"),
        Ok(Err(error)) => format!("Cannot read GUI acknowledgement: {error}"),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            format!("Timed out after {} seconds", ACCEPT_TIMEOUT.as_secs())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => "GUI acknowledgement reader stopped".into(),
    };
    let status = match child.try_wait()? {
        Some(status) => status,
        None => {
            let _ = child.kill();
            child.wait()?
        }
    };
    let details = match read_startup_errors(stderr) {
        Ok(details) if !details.trim().is_empty() => format!("\nGUI stderr: {}", details.trim()),
        Err(error) => format!("\nCannot read GUI stderr: {error}"),
        _ => String::new(),
    };
    Err(Error::Protocol(format!(
        "Desktop app did not accept the request. {reason}. GUI exit status: {status}.{details}\nCheck desktop access (including agent sandbox permissions) and that launcher and GUI versions match"
    )))
}

fn read_startup_errors(mut stderr: std::fs::File) -> std::io::Result<String> {
    stderr.rewind()?;
    let mut bytes = Vec::new();
    stderr.take(STARTUP_OUTPUT_LIMIT).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn native_executable() -> Result<PathBuf> {
    if let Some(executable) = std::env::var_os("FASTDIFF_GUI_EXE") {
        return existing_executable(PathBuf::from(executable));
    }
    let filename = if cfg!(windows) {
        "fastdiff-gui.exe"
    } else {
        "fastdiff-gui"
    };
    if let Some(parent) = std::env::current_exe()?.parent() {
        let sibling = parent.join(filename);
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    find_on_path(filename).ok_or_else(|| {
        Error::InvalidArgument(format!(
            "Cannot find {filename}; install it alongside fastdiff or set FASTDIFF_GUI_EXE"
        ))
    })
}

fn windows_executable() -> Result<PathBuf> {
    if let Some(executable) = std::env::var_os("FASTDIFF_WINDOWS_EXE") {
        let string = executable.to_string_lossy();
        if string.contains('\\') || string.as_bytes().get(1) == Some(&b':') {
            let output = Command::new("wslpath")
                .arg("-u")
                .arg(&executable)
                .output()?;
            if !output.status.success() {
                return Err(Error::InvalidArgument(
                    "Cannot convert the configured Windows GUI path with wslpath -u".into(),
                ));
            }
            let path = String::from_utf8(output.stdout).map_err(|_| {
                Error::InvalidArgument("wslpath returned a non-Unicode executable path".into())
            })?;
            return existing_executable(PathBuf::from(path.trim_end_matches(['\r', '\n'])));
        }
        return existing_executable(PathBuf::from(executable));
    }
    find_on_path("fastdiff-gui.exe").ok_or_else(|| Error::InvalidArgument("Native Windows app not found. Set FASTDIFF_WINDOWS_EXE to the installed fastdiff-gui.exe or add its Windows directory to PATH. Use --native for a Linux window".into()))
}

fn wsl_target() -> Result<WslTarget> {
    let distribution = std::env::var("WSL_DISTRO_NAME")
        .map_err(|_| Error::InvalidArgument("WSL_DISTRO_NAME is unavailable".into()))?;
    let output = Command::new("id").arg("-un").output()?;
    if !output.status.success() {
        return Err(Error::InvalidArgument(
            "Cannot determine current Linux user with id -un".into(),
        ));
    }
    let user = String::from_utf8(output.stdout)
        .map_err(|_| Error::InvalidArgument("Linux username is not Unicode".into()))?
        .trim_end_matches(['\r', '\n'])
        .to_owned();
    let helper = std::fs::canonicalize(std::env::current_exe()?)?
        .into_os_string()
        .into_string()
        .map_err(|_| {
            Error::InvalidArgument("Linux helper executable path is not Unicode".into())
        })?;
    Ok(WslTarget {
        distribution,
        user,
        helper,
    })
}

fn existing_executable(path: PathBuf) -> Result<PathBuf> {
    if path.is_file() {
        Ok(path)
    } else {
        Err(Error::InvalidArgument(format!(
            "Executable does not exist: {}",
            path.display()
        )))
    }
}
fn find_on_path(filename: impl AsRef<OsStr>) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|entry| entry.join(Path::new(filename.as_ref())))
            .find(|candidate| candidate.is_file())
    })
}
fn propagate_benchmark_environment(command: &mut Command) {
    let mut values = std::env::var("WSLENV")
        .unwrap_or_default()
        .split(':')
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for value in [
        "FASTDIFF_BENCH_REPORT/p",
        "FASTDIFF_BENCH_EXIT_MS",
        "FASTDIFF_RUNTIME_DIR/p",
        "FASTDIFF_LAUNCH_TRACE/p",
        "FASTDIFF_LAUNCH_SAMPLE_ID",
    ] {
        let variable = value.split('/').next().unwrap_or(value);
        if std::env::var_os(variable).is_some()
            && !values
                .iter()
                .any(|entry| entry.split('/').next() == Some(variable))
        {
            values.push(value.into());
        }
    }
    command.env("WSLENV", values.join(":"));
}

struct LaunchPhase {
    path: PathBuf,
    sample_id: String,
    started: Instant,
}

impl LaunchPhase {
    fn start() -> Option<Self> {
        let path = std::env::var_os("FASTDIFF_LAUNCH_TRACE").filter(|path| !path.is_empty())?;
        Some(Self {
            path: path.into(),
            sample_id: std::env::var("FASTDIFF_LAUNCH_SAMPLE_ID").unwrap_or_default(),
            started: Instant::now(),
        })
    }

    fn record(self, phase: &str) {
        let row = serde_json::json!({
            "schema": 1,
            "sample_id": self.sample_id,
            "phase": phase,
            "pid": std::process::id(),
            "duration_ms": self.started.elapsed().as_secs_f64() * 1000.0,
        });
        if let Ok(mut bytes) = serde_json::to_vec(&row) {
            bytes.push(b'\n');
            if let Ok(mut output) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.path)
            {
                // Optional profiling must not break launch; the harness rejects missing phases.
                let _ = output.write_all(&bytes);
            }
        }
    }
}

fn measure_launch_phase<T>(phase: &str, operation: impl FnOnce() -> T) -> T {
    let started = LaunchPhase::start();
    let result = operation();
    if let Some(started) = started {
        started.record(phase);
    }
    result
}
