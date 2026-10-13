use fastdiff_cli::{Command, OpenRequest, PROTOCOL_VERSION, parse, read_frame, write_frame};
use fastdiff_core::View;
use std::{ffi::OsString, io::Cursor};

#[test]
fn parses_repository_file_and_view_without_shell_interpretation() {
    let command = parse(
        [
            "repo name",
            "--file",
            "src/weird $file.rs",
            "--view",
            "staged",
        ]
        .map(OsString::from),
    )
    .unwrap();
    let Command::Open { request, native } = command else {
        panic!("expected launch")
    };
    assert_eq!(request.repository, "repo name");
    assert_eq!(request.file.unwrap().0, b"src/weird $file.rs");
    assert_eq!(request.view, View::Staged);
    assert!(!native);
}

#[test]
fn rejects_unknown_view_and_missing_option_value() {
    assert!(parse(["--view", "checkpoint"].map(OsString::from)).is_err());
    assert!(parse(["--file"].map(OsString::from)).is_err());
}

#[test]
fn default_launch_is_all_in_current_directory() {
    let Command::Open { request, .. } = parse(Vec::<OsString>::new()).unwrap() else {
        panic!("expected launch")
    };
    assert_eq!(request.repository, ".");
    assert_eq!(request.view, View::All);
    assert!(!request.theme);
}

#[test]
fn bounded_frames_roundtrip_and_reject_truncation() {
    let request = OpenRequest {
        target: fastdiff_cli::OpenTarget::Repository,
        repository: "/repo".into(),
        file: None,
        view: View::All,
        wsl: None,
        theme: false,
    };
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &request).unwrap();
    let restored: OpenRequest = read_frame(&mut Cursor::new(bytes.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(restored.repository, "/repo");
    assert!(read_frame::<_, OpenRequest>(&mut Cursor::new(&bytes[..bytes.len() - 1])).is_err());
    assert!(read_frame::<_, OpenRequest>(&mut Cursor::new(u32::MAX.to_le_bytes())).is_err());
    assert_eq!(PROTOCOL_VERSION, 5);
}

#[test]
fn argument_boundaries_reject_invalid_paths_extra_repositories_and_unknown_options() {
    for args in [
        vec!["--unknown"],
        vec!["a", "b"],
        vec!["--file", ""],
        vec![""],
        vec!["--view"],
    ] {
        assert!(parse(args.into_iter().map(OsString::from)).is_err());
    }
    let Command::Open { request, native } =
        parse(["--native", "--", "-repo"].map(OsString::from)).unwrap()
    else {
        panic!("launch")
    };
    assert!(native);
    assert_eq!(request.repository, "-repo");
    let mut invalid = request;
    invalid.wsl = Some(fastdiff_cli::WslTarget {
        distribution: "Distro".into(),
        user: "user".into(),
        helper: "relative".into(),
    });
    assert!(invalid.validate().is_err());
    invalid.wsl.as_mut().unwrap().helper = "/helper".into();
    assert!(invalid.validate().is_err());
    invalid.repository = "/repo".into();
    assert!(invalid.validate().is_ok());
    invalid.wsl.as_mut().unwrap().user = String::new();
    assert!(invalid.validate().is_err());
}

#[test]
fn malformed_json_and_truncated_prefix_are_rejected_while_clean_eof_is_allowed() {
    assert!(
        read_frame::<_, OpenRequest>(&mut Cursor::new(Vec::new()))
            .unwrap()
            .is_none()
    );
    assert!(read_frame::<_, OpenRequest>(&mut Cursor::new([1u8, 0])).is_err());
    assert!(read_frame::<_, OpenRequest>(&mut Cursor::new([0u8; 4])).is_err());
    let mut bytes = 1u32.to_le_bytes().to_vec();
    bytes.push(b'x');
    assert!(read_frame::<_, OpenRequest>(&mut Cursor::new(bytes)).is_err());
    assert!(
        write_frame(
            &mut Vec::new(),
            &"x".repeat(fastdiff_cli::MAX_MESSAGE_BYTES + 1)
        )
        .is_err()
    );
}

#[test]
fn existing_file_selects_document_and_unpushed_is_accepted() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("read me.txt");
    std::fs::write(&path, "text").unwrap();
    let Command::Open { request, .. } =
        parse([path.into_os_string(), "--view".into(), "unpushed".into()]).unwrap()
    else {
        panic!("expected open")
    };
    assert_eq!(request.target, fastdiff_cli::OpenTarget::Document);
    assert_eq!(request.view, View::Unpushed);
    assert!(request.file.is_none());
}

#[test]
fn theme_flag_enables_icons_for_repository_and_document_launches() {
    for args in [
        vec!["--theme"],
        vec!["repo name", "--theme", "--native"],
        vec!["--theme", "--document", "notes.md"],
        vec!["--theme", "--theme", "--file", "src/main.rs"],
    ] {
        let Command::Open { request, .. } = parse(args.into_iter().map(OsString::from)).unwrap()
        else {
            panic!("expected launch")
        };
        assert_eq!(serde_json::to_value(request).unwrap()["theme"], true);
    }
    let Command::Open { request, .. } = parse(["--", "--theme"].map(OsString::from)).unwrap()
    else {
        panic!("expected launch")
    };
    assert_eq!(request.repository, "--theme");
    assert!(
        serde_json::to_value(request)
            .unwrap()
            .get("theme")
            .is_none()
    );
    assert!(parse(["--theme=true"].map(OsString::from)).is_err());
}

#[test]
fn theme_json_defaults_to_false_and_is_omitted_when_disabled() {
    let legacy = serde_json::json!({
        "target": "repository", "repository": "/repo", "file": null,
        "view": "all", "wsl": null
    });
    for payload in [legacy.clone(), {
        let mut payload = legacy.clone();
        payload["theme"] = false.into();
        payload
    }] {
        let request: OpenRequest = serde_json::from_value(payload).unwrap();
        assert!(!request.theme);
        assert_eq!(serde_json::to_value(request).unwrap(), legacy);
    }
    let mut enabled = legacy;
    enabled["theme"] = true.into();
    let request: OpenRequest = serde_json::from_value(enabled.clone()).unwrap();
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &request).unwrap();
    let restored: OpenRequest = read_frame(&mut Cursor::new(bytes)).unwrap().unwrap();
    assert_eq!(serde_json::to_value(restored).unwrap(), enabled);
}

#[test]
fn theme_json_rejects_non_boolean_values() {
    for theme in [
        serde_json::Value::Null,
        serde_json::json!("true"),
        serde_json::json!(1),
        serde_json::json!([]),
    ] {
        let payload = serde_json::json!({
            "target": "repository", "repository": "/repo", "file": null,
            "view": "all", "wsl": null, "theme": theme
        });
        assert!(serde_json::from_value::<OpenRequest>(payload).is_err());
    }
}
