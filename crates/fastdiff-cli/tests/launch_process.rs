#![cfg(unix)]
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn executable(path: &Path, script: &str) {
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(directory.path())
            .status()
            .unwrap()
            .success()
    );
    let gui = directory.path().join("fastdiff-gui.exe");
    let record = directory.path().join("request.json");
    executable(
        &gui,
        "#!/bin/sh\nprintf '%s' \"$2\" > \"$FASTDIFF_TEST_RECORD\"\nprintf '%s\\n' FASTDIFF_ACCEPTED\n",
    );
    (directory, gui, record)
}
fn command(repository: &Path, record: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_fastdiff"));
    command
        .arg(repository)
        .env("WSL_DISTRO_NAME", "Test Distribution")
        .env("FASTDIFF_TEST_RECORD", record)
        .env_remove("FASTDIFF_WINDOWS_EXE");
    command
}

#[test]
fn launcher_rejects_non_repository_current_directory_before_opening_gui() {
    let directory = tempfile::tempdir().unwrap();
    let gui = directory.path().join("fastdiff-gui.exe");
    let record = directory.path().join("request.json");
    let runtime = directory.path().join("runtime");
    let server = fastdiff_cli::IpcServer::bind_at(&runtime).unwrap().unwrap();
    executable(
        &gui,
        "#!/bin/sh\nprintf '%s' \"$2\" > \"$FASTDIFF_TEST_RECORD\"\nprintf '%s\\n' FASTDIFF_ACCEPTED\n",
    );
    for native in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fastdiff"));
        command
            .current_dir(directory.path())
            .env("WSL_DISTRO_NAME", "Test Distribution")
            .env("FASTDIFF_WINDOWS_EXE", &gui)
            .env("FASTDIFF_GUI_EXE", &gui)
            .env("FASTDIFF_TEST_RECORD", &record)
            .env("FASTDIFF_RUNTIME_DIR", &runtime);
        if native {
            command.arg("--native");
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Cannot open Git repository"));
        assert!(!record.exists());
        assert!(server.poll().unwrap().is_empty());
    }
}

#[test]
fn wsl_launcher_passes_absolute_linux_paths_and_selection_to_windows_gui() {
    let (directory, gui, record) = fixture();
    let output = command(directory.path(), &record)
        .args(["--theme", "--file", "odd $name.rs", "--view", "staged"])
        .env("FASTDIFF_WINDOWS_EXE", gui)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request: fastdiff_cli::OpenRequest =
        serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(&request).unwrap()["theme"], true);
    assert_eq!(request.repository, directory.path().to_str().unwrap());
    assert_eq!(request.file.unwrap().0, b"odd $name.rs");
    assert_eq!(request.view, fastdiff_core::View::Staged);
    let wsl = request.wsl.unwrap();
    assert_eq!(wsl.distribution, "Test Distribution");
    assert!(wsl.helper.starts_with('/'));
    assert!(!wsl.user.is_empty());
}

#[test]
fn wsl_launcher_finds_windows_gui_in_path_and_converts_configured_windows_path() {
    let (directory, gui, record) = fixture();
    let path = format!(
        "{}:{}",
        directory.path().display(),
        std::env::var("PATH").unwrap()
    );
    let output = command(directory.path(), &record)
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let converter = directory.path().join("wslpath");
    executable(
        &converter,
        "#!/bin/sh\nprintf '%s\\n' \"$FASTDIFF_TEST_GUI\"\n",
    );
    let output = command(directory.path(), &record)
        .env("PATH", path)
        .env("FASTDIFF_TEST_GUI", gui)
        .env("FASTDIFF_WINDOWS_EXE", r"C:\Program Files\fastdiff-gui.exe")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn launch_failure_reports_missing_program_or_unaccepted_gui() {
    let (directory, gui, record) = fixture();
    let output = command(directory.path(), &record)
        .env("FASTDIFF_WINDOWS_EXE", directory.path().join("missing.exe"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Executable does not exist"));
    executable(
        &gui,
        "#!/bin/sh\nprintf 'Display access denied\\n' >&2\nexit 1\n",
    );
    let output = command(directory.path(), &record)
        .env("FASTDIFF_WINDOWS_EXE", &gui)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("did not accept"));
    assert!(error.contains("without acknowledging"));
    assert!(error.contains("GUI exit status:"));
    assert!(error.contains("Display access denied"));
    executable(&gui, "#!/bin/sh\nprintf 'WRONG_ACK\\n'\n");
    let output = command(directory.path(), &record)
        .env("FASTDIFF_WINDOWS_EXE", &gui)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Unexpected GUI acknowledgement: WRONG_ACK")
    );
}

#[test]
fn launcher_help_and_version_do_not_open_a_window() {
    for argument in ["--help", "--version"] {
        let output = Command::new(env!("CARGO_BIN_EXE_fastdiff"))
            .arg(argument)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("fastdiff"));
        if argument == "--help" {
            assert!(String::from_utf8_lossy(&output.stdout).contains("--theme"));
            assert!(String::from_utf8_lossy(&output.stdout).contains("Material icons"));
        }
    }
}

#[test]
fn native_launch_returns_after_acceptance_without_waiting_for_gui_stderr_to_close() {
    let (directory, gui, record) = fixture();
    executable(
        &gui,
        "#!/bin/sh\nset -e\nprintf '%s' \"$2\" > \"$FASTDIFF_TEST_RECORD\"\nprintf '%s\\n' FASTDIFF_ACCEPTED\nsleep 0.5\nprintf 'Late GUI diagnostic\\n' >&2\ntouch \"$FASTDIFF_TEST_RECORD.stderr-ok\"\n",
    );
    let started = std::time::Instant::now();
    let output = command(directory.path(), &record)
        .args(["--native", "--theme"])
        .env("FASTDIFF_GUI_EXE", gui)
        .env("FASTDIFF_RUNTIME_DIR", directory.path().join("runtime"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(started.elapsed() < std::time::Duration::from_millis(400));
    let stderr_ok = record.with_file_name("request.json.stderr-ok");
    while !stderr_ok.exists() && started.elapsed() < std::time::Duration::from_secs(2) {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        stderr_ok.exists(),
        "GUI stderr must remain writable after launcher exit"
    );
    let request: fastdiff_cli::OpenRequest =
        serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert!(request.wsl.is_none());
    assert_eq!(serde_json::to_value(&request).unwrap()["theme"], true);
    let output = command(directory.path(), &record)
        .arg("--native")
        .env("FASTDIFF_GUI_EXE", directory.path().join("missing"))
        .env("FASTDIFF_RUNTIME_DIR", directory.path().join("runtime"))
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn native_launcher_forwards_to_existing_app_without_spawning_gui() {
    let (directory, _gui, record) = fixture();
    let runtime = directory.path().join("runtime");
    let server = fastdiff_cli::IpcServer::bind_at(&runtime).unwrap().unwrap();
    let output = command(directory.path(), &record)
        .args([
            "--native",
            "--theme",
            "--file",
            "src/main.rs",
            "--view",
            "unstaged",
        ])
        .env("FASTDIFF_GUI_EXE", directory.path().join("missing"))
        .env("FASTDIFF_RUNTIME_DIR", &runtime)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = server.poll().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(serde_json::to_value(&requests[0]).unwrap()["theme"], true);
    assert_eq!(requests[0].view, fastdiff_core::View::Unstaged);
    assert_eq!(requests[0].file.as_ref().unwrap().0, b"src/main.rs");
}

fn trace_rows(path: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn launch_trace_separates_validation_spawn_and_acceptance() {
    let (directory, gui, record) = fixture();
    let trace = directory.path().join("launch.jsonl");
    let output = command(directory.path(), &record)
        .arg("--native")
        .env("FASTDIFF_GUI_EXE", gui)
        .env("FASTDIFF_RUNTIME_DIR", directory.path().join("runtime"))
        .env("FASTDIFF_LAUNCH_TRACE", &trace)
        .env("FASTDIFF_LAUNCH_SAMPLE_ID", "sample with spaces")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows = trace_rows(&trace);
    for phase in [
        "repository_validation",
        "forwarding_probe",
        "process_start",
        "forwarding_acceptance",
    ] {
        assert!(
            rows.iter().any(|row| row["phase"] == phase),
            "{phase}: {rows:?}"
        );
    }
    for row in rows {
        assert_eq!(row["schema"], 1);
        assert_eq!(row["sample_id"], "sample with spaces");
        assert!(row["pid"].as_u64().unwrap() > 0);
        assert!(row["duration_ms"].as_f64().unwrap() >= 0.0);
    }
}

#[test]
fn launch_trace_forwarding_does_not_report_a_process_start() {
    let (directory, _gui, record) = fixture();
    let runtime = directory.path().join("runtime");
    let server = fastdiff_cli::IpcServer::bind_at(&runtime).unwrap().unwrap();
    let trace = directory.path().join("launch.jsonl");
    let output = command(directory.path(), &record)
        .arg("--native")
        .env("FASTDIFF_RUNTIME_DIR", &runtime)
        .env("FASTDIFF_GUI_EXE", directory.path().join("missing"))
        .env("FASTDIFF_LAUNCH_TRACE", &trace)
        .env("FASTDIFF_LAUNCH_SAMPLE_ID", "forwarded")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(server.poll().unwrap().len(), 1);
    let rows = trace_rows(&trace);
    assert!(
        rows.iter()
            .any(|row| row["phase"] == "forwarding_acceptance")
    );
    assert!(!rows.iter().any(|row| row["phase"] == "process_start"));
    assert!(!rows.iter().any(|row| row["phase"] == "forwarding_probe"));
}

#[test]
fn launch_trace_failures_and_disabled_tracing_preserve_launch() {
    let (directory, gui, record) = fixture();
    for trace in [directory.path().as_os_str(), std::ffi::OsStr::new("")] {
        let output = command(directory.path(), &record)
            .arg("--native")
            .env("FASTDIFF_GUI_EXE", &gui)
            .env("FASTDIFF_RUNTIME_DIR", directory.path().join("runtime"))
            .env("FASTDIFF_LAUNCH_TRACE", trace)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn wsl_launch_propagates_profiling_and_isolation_environment() {
    let (directory, gui, record) = fixture();
    executable(
        &gui,
        "#!/bin/sh\nprintf '%s' \"$WSLENV\" > \"$FASTDIFF_TEST_RECORD\"\nprintf '%s\\n' FASTDIFF_ACCEPTED\n",
    );
    let output = command(directory.path(), &record)
        .env("FASTDIFF_WINDOWS_EXE", gui)
        .env("WSLENV", "KEEP/u:FASTDIFF_BENCH_EXIT_MS/l")
        .env("FASTDIFF_BENCH_REPORT", directory.path().join("bench.json"))
        .env("FASTDIFF_BENCH_EXIT_MS", "500")
        .env("FASTDIFF_RUNTIME_DIR", directory.path().join("runtime"))
        .env(
            "FASTDIFF_LAUNCH_TRACE",
            directory.path().join("launch.jsonl"),
        )
        .env("FASTDIFF_LAUNCH_SAMPLE_ID", "wsl")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let values = fs::read_to_string(record).unwrap();
    for value in [
        "KEEP/u",
        "FASTDIFF_BENCH_REPORT/p",
        "FASTDIFF_BENCH_EXIT_MS/l",
        "FASTDIFF_RUNTIME_DIR/p",
        "FASTDIFF_LAUNCH_TRACE/p",
        "FASTDIFF_LAUNCH_SAMPLE_ID",
    ] {
        assert!(
            values.split(':').any(|entry| entry == value),
            "{value}: {values}"
        );
    }
    assert_eq!(values.matches("FASTDIFF_BENCH_EXIT_MS").count(), 1);
}
