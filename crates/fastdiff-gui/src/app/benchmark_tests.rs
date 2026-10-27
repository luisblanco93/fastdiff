use super::*;
use crate::worker::PreparedDocument;
use std::path::{Path, PathBuf};

fn app(report: &Path) -> Fastdiff {
    Fastdiff {
        tabs: vec![tests::fixture()],
        selected_tab: 0,
        preferences: Preferences::default(),
        server: None,
        benchmark: crate::benchmark::tests::reporting(report.to_path_buf()),
        help_open: false,
        fallback_fonts: Default::default(),
        appearance: Default::default(),
        retired_workers: Vec::new(),
    }
}

fn render(app: &mut Fastdiff, context: &egui::Context) {
    let _ = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 600.0),
            )),
            ..Default::default()
        },
        |context| app.show(context),
    );
    app.benchmark.frame_ready();
}

/// One `update`: previous-frame CPU time, event polling and observation, then rendering.
fn frame(app: &mut Fastdiff, context: &egui::Context) {
    app.benchmark.record_frame(Some(0.001));
    app.poll(context);
    render(app, context);
}

fn document(text: &str) -> DiffPresentation {
    DiffPresentation::new(
        Arc::new(PreparedDocument::from_text(
            "src/main.rs".into(),
            text.into(),
            &fastdiff_core::CancelToken::default(),
        )),
        true,
    )
}

fn sample(id: &str, marker: &str) -> serde_json::Value {
    serde_json::json!({
        "schema": 1, "sample_id": id, "repository": "/repo",
        "file": "src/main.rs", "marker": marker,
    })
}

fn edit(id: &str, marker: &str) -> serde_json::Value {
    let mut value = sample(id, marker);
    value["trigger"] = "edit".into();
    value
}

fn read(report: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap()
}

fn control_path(report: &Path) -> PathBuf {
    let mut path = report.as_os_str().to_os_string();
    path.push(".request.json");
    PathBuf::from(path)
}

fn other_repository_tab() -> ReviewTab {
    let mut tab = tests::fixture();
    tab.root = "/other".into();
    tab.request.repository = "/other".into();
    tab
}

#[test]
fn warm_benchmark_request_rejects_old_content_and_other_selected_files() {
    let directory = tempfile::tempdir().unwrap();
    let report = directory.path().join("report.json");
    let control = control_path(&report);
    let mut app = app(&report);
    let context = egui::Context::default();
    let request = app.tabs[0].request.clone();
    std::fs::write(&control, sample("first", "fn main()").to_string()).unwrap();
    app.open(request.clone(), &context);
    assert!(app.tabs[0].loading);
    frame(&mut app, &context);
    assert!(read(&report)["request"]["rendered_unix_ms"].is_null());
    app.tabs[0].loading = false;
    frame(&mut app, &context);
    frame(&mut app, &context);
    assert!(read(&report)["request"]["rendered_unix_ms"].is_number());

    std::fs::write(&control, sample("second", "fresh marker").to_string()).unwrap();
    app.open(request.clone(), &context);
    app.tabs[0].loading = false;
    frame(&mut app, &context);
    frame(&mut app, &context);
    assert_eq!(read(&report)["request"]["sample_id"], "second");
    assert!(read(&report)["request"]["rendered_unix_ms"].is_null());
    app.tabs[0].presentation = Some(document("fresh marker"));
    app.tabs[0].selected = Some(GitPath(b"other.rs".to_vec()));
    frame(&mut app, &context);
    frame(&mut app, &context);
    assert!(read(&report)["request"]["rendered_unix_ms"].is_null());
    app.tabs[0].selected = request.file;
    frame(&mut app, &context);
    frame(&mut app, &context);
    let completed = read(&report);
    assert_eq!(completed["pid"], std::process::id());
    assert_eq!(completed["request"]["sample_id"], "second");
    assert!(completed["request"]["rendered_unix_ms"].is_number());
    assert!(completed["request"]["frames"].as_u64().unwrap() >= 2);
}

#[test]
fn unrelated_tabs_reflog_events_and_errors_do_not_complete_the_requested_identity() {
    let directory = tempfile::tempdir().unwrap();
    let report = directory.path().join("report.json");
    let mut app = app(&report);
    app.tabs.push(other_repository_tab());
    app.selected_tab = 1;
    let context = egui::Context::default();
    let request = app.tabs[0].request.clone();
    app.benchmark.accept_request(&request);
    let first_diff = |app: &mut Fastdiff| {
        app.benchmark.write();
        read(&report)["first_diff_ms"].clone()
    };

    frame(&mut app, &context);
    assert!(
        first_diff(&mut app).is_null(),
        "another tab's settled diff completed the request"
    );

    app.selected_tab = 0;
    frame(&mut app, &context);
    assert!(app.tabs[0].loading, "activation reloads the tab");
    app.tabs[0].reflog_open = true;
    let generation = app.tabs[0].reflog_generation;
    let list = ReflogList {
        branch: None,
        entries: Vec::new(),
        notice: None,
        snapshot: None,
        start: 0,
        has_more: false,
    };
    assert!(
        app.tabs[0].apply_events(vec![Event::Reflog { generation, list }], false),
        "a reflog event reports ready to the poll loop"
    );
    frame(&mut app, &context);
    assert!(
        first_diff(&mut app).is_null(),
        "a reflog event completed it"
    );

    let generation = app.tabs[0].generation;
    app.tabs[0].apply_events(
        vec![Event::Error {
            generation,
            message: "git failed".into(),
        }],
        false,
    );
    assert!(!app.tabs[0].loading);
    frame(&mut app, &context);
    assert!(
        first_diff(&mut app).is_null(),
        "an error state completed it"
    );

    app.tabs[0].error = None;
    frame(&mut app, &context);
    assert!(first_diff(&mut app).as_f64().is_some());
}

#[test]
fn empty_state_does_not_complete_the_requested_identity() {
    let directory = tempfile::tempdir().unwrap();
    let report = directory.path().join("report.json");
    let mut app = app(&report);
    let context = egui::Context::default();
    let request = app.tabs[0].request.clone();
    app.benchmark.accept_request(&request);
    app.tabs[0].presentation = None;
    app.tabs[0].selected = None;
    app.tabs[0].loading = false;
    frame(&mut app, &context);
    app.benchmark.write();
    assert!(read(&report)["first_diff_ms"].is_null());
}

#[test]
fn edit_trigger_arms_the_selected_tab_and_completes_on_the_edited_content() {
    let directory = tempfile::tempdir().unwrap();
    let report = directory.path().join("report.json");
    let mut app = app(&report);
    let context = egui::Context::default();
    frame(&mut app, &context);
    std::fs::write(
        control_path(&report),
        edit("edit", "fresh marker").to_string(),
    )
    .unwrap();
    // The control file is polled at most once per poll interval; the first frame polled it.
    crate::benchmark::tests::make_poll_due(&mut app.benchmark);
    frame(&mut app, &context);
    let armed = read(&report);
    assert_eq!(armed["request"]["sample_id"], "edit");
    assert_eq!(armed["request"]["trigger"], "edit");
    assert!(armed["request"]["rendered_unix_ms"].is_null());
    app.tabs[0].presentation = Some(document("fresh marker"));
    frame(&mut app, &context);
    frame(&mut app, &context);
    let completed = read(&report);
    assert!(completed["request"]["rendered_unix_ms"].is_number());
    assert_eq!(completed["request"]["sample_id"], "edit");
}

#[test]
fn edit_trigger_ignores_a_control_for_another_repository() {
    let directory = tempfile::tempdir().unwrap();
    let report = directory.path().join("report.json");
    let mut app = app(&report);
    let context = egui::Context::default();
    let mut value = edit("elsewhere", "marker");
    value["repository"] = "/other".into();
    std::fs::write(control_path(&report), value.to_string()).unwrap();
    frame(&mut app, &context);
    app.benchmark.write();
    assert!(read(&report)["request"].is_null());
}
