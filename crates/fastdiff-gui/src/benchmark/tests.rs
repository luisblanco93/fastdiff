use super::*;
use fastdiff_core::{CancelToken, GitPath};

fn measured() -> Benchmark {
    Benchmark {
        first_frame_ms: Some(1.0),
        ..Benchmark::blank(false, Some(PathBuf::from("unused")), None)
    }
}

pub(crate) fn reporting(path: PathBuf) -> Benchmark {
    Benchmark {
        report: Some(path),
        ..measured()
    }
}

fn in_directory(directory: &tempfile::TempDir) -> Benchmark {
    reporting(directory.path().join("report.json"))
}

fn document(text: &str) -> Arc<PreparedDocument> {
    Arc::new(PreparedDocument::from_text(
        "src/main.rs".into(),
        text.into(),
        &CancelToken::default(),
    ))
}

fn request(repository: &str) -> OpenRequest {
    OpenRequest {
        target: OpenTarget::Repository,
        repository: repository.into(),
        file: Some(GitPath(b"src/main.rs".to_vec())),
        view: View::All,
        wsl: None,
        theme: false,
        color_theme: None,
    }
}

/// An owned selected tab, shown to the benchmark as one frame's `TabState`.
struct Tab {
    request: OpenRequest,
    root: String,
    file: Option<GitPath>,
    view: View,
    document_mode: bool,
    loading: bool,
    failed: bool,
    document: Option<Arc<PreparedDocument>>,
}

impl Tab {
    /// A tab at `repository` that settled on the diff of `src/main.rs`.
    fn showing(repository: &str) -> Self {
        let request = request(repository);
        Self {
            file: request.file.clone(),
            view: request.view,
            root: repository.into(),
            request,
            document_mode: false,
            loading: false,
            failed: false,
            document: Some(document("fn main() {}")),
        }
    }

    fn state(&self) -> TabState<'_> {
        TabState::new(
            TabLocus::new(&self.request, &self.root, self.file.as_ref(), self.view),
            self.document_mode,
            self.loading,
            self.failed,
            self.document.as_ref(),
        )
    }

    fn locus(&self) -> TabLocus<'_> {
        TabLocus::new(&self.request, &self.root, self.file.as_ref(), self.view)
    }
}

/// One application frame: observe the tab, then finish the frame.
fn show(benchmark: &mut Benchmark, tab: &Tab) {
    benchmark.observe(&tab.state());
    benchmark.frame_ready();
}

/// The frame after a render: eframe reports the previous frame's CPU time at its start.
fn settle(benchmark: &mut Benchmark, tab: &Tab) {
    benchmark.record_frame(Some(0.001));
    show(benchmark, tab);
}

fn report_of(benchmark: &Benchmark) -> Value {
    serde_json::from_slice(&std::fs::read(benchmark.report.as_ref().unwrap()).unwrap()).unwrap()
}

fn control(benchmark: &Benchmark, value: Value) {
    let mut path = benchmark
        .report
        .as_ref()
        .unwrap()
        .as_os_str()
        .to_os_string();
    path.push(".request.json");
    std::fs::write(PathBuf::from(path), value.to_string()).unwrap();
}

fn sample(id: &str, repository: &str, marker: &str) -> Value {
    json!({"schema": 1, "sample_id": id, "repository": repository,
        "file": "src/main.rs", "marker": marker})
}

fn with(mut value: Value, key: &str, field: Value) -> Value {
    value[key] = field;
    value
}

fn sample_of(benchmark: &Benchmark) -> &RequestMeasurement {
    benchmark.request.as_ref().unwrap()
}

/// Make the next observation read the control file, however recently it last did.
pub(crate) fn make_poll_due(benchmark: &mut Benchmark) {
    benchmark.control.last_poll = None;
}

#[test]
fn frame_measurement_requires_opt_in_and_a_loaded_diff() {
    let mut benchmark = measured();
    benchmark.record_frame(Some(0.2));
    assert_eq!(benchmark.frames.frames, 0);
    benchmark.requested_content_ms = Some(2.0);
    benchmark.report = None;
    benchmark.record_frame(Some(0.2));
    assert_eq!(benchmark.frames.frames, 0);
}

#[test]
fn frame_measurement_tracks_only_finite_nonnegative_durations() {
    let mut benchmark = measured();
    benchmark.requested_content_ms = Some(2.0);
    for seconds in [None, Some(-1.0), Some(f32::NAN), Some(f32::INFINITY)] {
        benchmark.record_frame(seconds);
    }
    assert_eq!(benchmark.frames.frames, 0);
    benchmark.record_frame(Some(0.025));
    benchmark.record_frame(Some(0.003));
    assert_eq!(benchmark.frames.frames, 2);
    assert!((benchmark.frames.max_ms - 25.0).abs() < 0.001);
    assert!((benchmark.frames.total_ms - 28.0).abs() < 0.001);
}

fn tick_output(benchmark: &mut Benchmark, now: Instant) -> egui::ViewportOutput {
    let context = egui::Context::default();
    for _ in 0..3 {
        let _ = context.run(egui::RawInput::default(), |_| {});
    }
    let output = context.run(
        egui::RawInput {
            predicted_dt: 0.0,
            ..Default::default()
        },
        |context| benchmark.tick_at(context, now),
    );
    output.viewport_output[&egui::ViewportId::ROOT].clone()
}

#[test]
fn future_exit_deadline_schedules_remaining_time() {
    let mut benchmark = measured();
    benchmark.exit_after = Some(Duration::from_secs(2));
    let started = benchmark.started;
    let output = tick_output(&mut benchmark, started + Duration::from_millis(750));
    assert_eq!(output.repaint_delay, Duration::from_millis(1250));
    assert!(!output.commands.contains(&egui::ViewportCommand::Close));
}

#[test]
fn reached_exit_deadline_closes_viewport() {
    let mut benchmark = measured();
    benchmark.exit_after = Some(Duration::from_secs(2));
    let started = benchmark.started;
    for elapsed in [Duration::from_secs(2), Duration::from_secs(3)] {
        let output = tick_output(&mut benchmark, started + elapsed);
        assert!(output.commands.contains(&egui::ViewportCommand::Close));
    }
}

#[test]
fn no_exit_deadline_schedules_nothing() {
    let mut benchmark = measured();
    let started = benchmark.started;
    let output = tick_output(&mut benchmark, started + Duration::from_secs(3));
    assert_eq!(output.repaint_delay, Duration::MAX);
    assert!(!output.commands.contains(&egui::ViewportCommand::Close));
}

#[test]
fn legacy_request_completes_only_for_the_requested_identity() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    benchmark.accept_request(&request("/repo"));
    assert!(benchmark.request.is_none());
    let unrelated = Tab::showing("/other");
    let mut wrong_file = Tab::showing("/repo");
    wrong_file.file = Some(GitPath(b"other.rs".to_vec()));
    let mut wrong_view = Tab::showing("/repo");
    wrong_view.view = View::Staged;
    for tab in [&unrelated, &wrong_file, &wrong_view] {
        show(&mut benchmark, tab);
    }
    assert!(benchmark.requested_content_ms.is_none());
    benchmark.write();
    assert!(report_of(&benchmark)["first_diff_ms"].is_null());
    show(&mut benchmark, &Tab::showing("/repo"));
    assert!(benchmark.requested_content_ms.is_some());
    let value = report_of(&benchmark);
    assert!(value["first_diff_ms"].as_f64().unwrap() >= 0.0);
    assert_eq!(value["first_diff_ms"], value["requested_content_ms"]);
}

#[test]
fn only_the_first_request_defines_the_legacy_identity() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    benchmark.accept_request(&request("/repo"));
    benchmark.accept_request(&request("/later"));
    show(&mut benchmark, &Tab::showing("/later"));
    assert!(benchmark.requested_content_ms.is_none());
    show(&mut benchmark, &Tab::showing("/repo"));
    assert!(benchmark.requested_content_ms.is_some());
}

#[test]
fn unsettled_or_non_diff_states_never_complete_a_request() {
    type Change = fn(&mut Tab);
    let cases: [(&str, Change); 5] = [
        ("loading", |tab| tab.loading = true),
        ("error", |tab| tab.failed = true),
        ("no presentation", |tab| tab.document = None),
        ("empty state", |tab| {
            tab.document = None;
            tab.file = None;
        }),
        ("full document fallback", |tab| tab.document_mode = true),
    ];
    for (name, change) in cases {
        let directory = tempfile::tempdir().unwrap();
        let mut benchmark = in_directory(&directory);
        control(&benchmark, sample(name, "/repo", "marker"));
        benchmark.accept_request(&request("/repo"));
        let mut tab = Tab::showing("/repo");
        change(&mut tab);
        show(&mut benchmark, &tab);
        assert!(benchmark.requested_content_ms.is_none(), "{name}");
        assert!(sample_of(&benchmark).content_ready_ms.is_none(), "{name}");
        assert!(benchmark.marker(&tab.locus()).is_none(), "{name}");
        show(&mut benchmark, &Tab::showing("/repo"));
        assert!(benchmark.requested_content_ms.is_some(), "{name}");
    }
}

#[test]
fn request_without_a_file_accepts_the_auto_selected_file_but_not_an_empty_state() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    let mut open = request("/repo");
    open.file = None;
    benchmark.accept_request(&open);
    let mut empty = Tab::showing("/repo");
    empty.file = None;
    show(&mut benchmark, &empty);
    assert!(benchmark.requested_content_ms.is_none());
    show(&mut benchmark, &Tab::showing("/repo"));
    assert!(benchmark.requested_content_ms.is_some());
}

#[test]
fn document_requests_complete_only_in_document_mode() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    let mut open = request("/repo/notes.txt");
    open.target = OpenTarget::Document;
    open.file = None;
    benchmark.accept_request(&open);
    let mut tab = Tab::showing("/repo");
    tab.request = open;
    show(&mut benchmark, &tab);
    assert!(benchmark.requested_content_ms.is_none());
    tab.document_mode = true;
    show(&mut benchmark, &tab);
    assert!(benchmark.requested_content_ms.is_some());
}

#[test]
fn subdirectory_requests_match_the_tab_they_opened() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    benchmark.accept_request(&request("/repo/nested"));
    let mut tab = Tab::showing("/repo/nested");
    tab.root = "/repo".into();
    show(&mut benchmark, &tab);
    assert!(benchmark.requested_content_ms.is_some());
}

#[test]
fn request_measurement_rejects_wrong_identity_and_requires_rendered_content() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    control(&benchmark, sample("first", "/repo", "fresh marker"));
    benchmark.accept_request(&request("/wrong"));
    assert!(benchmark.request.is_none());
    let mut wrong_file = request("/repo");
    wrong_file.file = Some(GitPath(b"other.rs".to_vec()));
    benchmark.accept_request(&wrong_file);
    assert!(benchmark.request.is_none());
    benchmark.accept_request(&request("/repo"));
    let tab = Tab::showing("/repo");
    assert!(benchmark.marker(&tab.locus()).is_none());
    benchmark.observe(&Tab::showing("/wrong").state());
    assert!(sample_of(&benchmark).content_ready_ms.is_none());
    benchmark.observe(&tab.state());
    assert_eq!(benchmark.marker(&tab.locus()), Some("fresh marker"));
    assert!(benchmark.marker(&Tab::showing("/wrong").locus()).is_none());
    benchmark.rendered(false);
    assert!(sample_of(&benchmark).rendered_unix_ms.is_none());
    benchmark.rendered(true);
    assert!(sample_of(&benchmark).rendered_unix_ms.is_none());
    benchmark.frame_ready();
    assert!(sample_of(&benchmark).rendered_unix_ms.is_some());
    settle(&mut benchmark, &tab);
    let value = report_of(&benchmark);
    assert_eq!(value["pid"], std::process::id());
    assert_eq!(value["request"]["sample_id"], "first");
    assert_eq!(value["request"]["marker"], "fresh marker");
    assert!(
        value["request"]["rendered_unix_ms"].as_u64().unwrap()
            >= value["request"]["received_unix_ms"].as_u64().unwrap()
    );
    assert!(value["request"]["content_ready_ms"].as_f64().unwrap() >= 0.0);
    assert!(value["request"]["render_ms"].as_f64().unwrap() >= 0.0);
    assert!(value.get("first_frame_ms").is_some());
    assert!(value.get("first_diff_ms").is_some());
}

#[test]
fn control_view_and_trigger_are_validated_and_reported() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    let mut staged = request("/repo");
    staged.view = View::Staged;
    for invalid in [
        with(sample("view", "/repo", "m"), "view", json!("bogus")),
        with(sample("view", "/repo", "m"), "view", json!(3)),
        with(sample("trigger", "/repo", "m"), "trigger", json!("bogus")),
        with(sample("trigger", "/repo", "m"), "trigger", json!(true)),
    ] {
        control(&benchmark, invalid);
        benchmark.accept_request(&request("/repo"));
        assert!(benchmark.request.is_none());
    }
    control(
        &benchmark,
        with(sample("view", "/repo", "m"), "view", json!("staged")),
    );
    benchmark.accept_request(&request("/repo"));
    assert!(benchmark.request.is_none(), "request view differs");
    benchmark.accept_request(&staged);
    assert_eq!(report_of(&benchmark)["request"]["view"], "staged");
    assert_eq!(report_of(&benchmark)["request"]["trigger"], "open");
    let mut tab = Tab::showing("/repo");
    show(&mut benchmark, &tab);
    assert!(sample_of(&benchmark).content_ready_ms.is_none());
    tab.view = View::Staged;
    show(&mut benchmark, &tab);
    assert!(sample_of(&benchmark).content_ready_ms.is_some());
}

#[test]
fn control_without_a_view_binds_the_requests_view() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    control(&benchmark, sample("bound", "/repo", "marker"));
    benchmark.accept_request(&request("/repo"));
    assert_eq!(report_of(&benchmark)["request"]["view"], "all");
    let mut tab = Tab::showing("/repo");
    tab.view = View::Unstaged;
    show(&mut benchmark, &tab);
    assert!(sample_of(&benchmark).content_ready_ms.is_none());
}

#[test]
fn warm_request_resets_sample_measurements_and_duplicate_id_is_ignored() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    let request = request("/repo");
    let tab = Tab::showing("/repo");
    control(&benchmark, sample("first", "/repo", "first marker"));
    benchmark.accept_request(&request);
    benchmark.observe(&tab.state());
    benchmark.rendered(true);
    benchmark.frame_ready();
    benchmark.accept_request(&request);
    assert!(sample_of(&benchmark).rendered_unix_ms.is_some());
    benchmark.frames = FrameStats::default().with(2.0);
    control(&benchmark, sample("second", "/repo", "second marker"));
    benchmark.accept_request(&request);
    let measurement = sample_of(&benchmark);
    assert_eq!(measurement.sample_id, "second");
    assert!(measurement.content_ready_ms.is_none());
    assert!(measurement.rendered_unix_ms.is_none());
    assert_eq!(benchmark.frames.frames, 0);
    assert_eq!(benchmark.frames.max_ms, 0.0);
    assert_eq!(benchmark.frames.total_ms, 0.0);
}

#[test]
fn older_sample_ids_cannot_rearm_or_complete_a_newer_sample() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    let tab = Tab::showing("/repo");
    control(&benchmark, sample("first", "/repo", "first marker"));
    benchmark.accept_request(&request("/repo"));
    benchmark.observe(&tab.state());
    benchmark.rendered(true);
    control(&benchmark, sample("second", "/repo", "second marker"));
    benchmark.accept_request(&request("/repo"));
    let second = sample_of(&benchmark);
    assert_eq!(second.sample_id, "second");
    assert!(!second.pending_render, "first sample's render state leaked");
    benchmark.frame_ready();
    assert!(sample_of(&benchmark).rendered_unix_ms.is_none());
    assert!(sample_of(&benchmark).content_ready_ms.is_none());
    control(&benchmark, sample("first", "/repo", "first marker"));
    benchmark.accept_request(&request("/repo"));
    assert_eq!(sample_of(&benchmark).sample_id, "second");
    benchmark.observe(&tab.state());
    assert_eq!(benchmark.marker(&tab.locus()), Some("second marker"));
}

#[test]
fn schema_two_report_adds_fields_and_keeps_schema_one_fields() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    let tab = Tab::showing("/repo");
    control(&benchmark, sample("shape", "/repo", "marker"));
    benchmark.accept_request(&request("/repo"));
    benchmark.observe(&tab.state());
    benchmark.rendered(true);
    benchmark.frame_ready();
    settle(&mut benchmark, &tab);
    let value = report_of(&benchmark);
    assert_eq!(value["schema"], 2);
    for field in [
        "pid",
        "started_unix_ms",
        "first_frame_ms",
        "first_diff_ms",
        "measured_frames",
        "max_frame_cpu_ms",
        "request",
    ] {
        assert!(value.get(field).is_some(), "schema 1 field {field}");
    }
    assert_eq!(value["ui_ready_ms"], value["first_frame_ms"]);
    assert_eq!(
        value["control_poll_ms"], 250,
        "the idle control poll interval is part of the report"
    );
    assert_eq!(value["requested_content_ms"], value["first_diff_ms"]);
    assert!(value["requested_content_ms"].as_f64().is_some());
    assert!(value["frame_cpu_total_ms"].as_f64().is_some());
    assert_eq!(value["build"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(value["build"]["revision"].is_string() || value["build"]["revision"].is_null());
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    assert_eq!(value["build"]["profile"], profile);
    assert_eq!(
        value["build"]["protocol"],
        u64::from(fastdiff_cli::PROTOCOL_VERSION)
    );
    assert_eq!(value["platform"]["os"], std::env::consts::OS);
    assert_eq!(value["platform"]["arch"], std::env::consts::ARCH);
    let request = &value["request"];
    for field in [
        "sample_id",
        "repository",
        "file",
        "marker",
        "received_unix_ms",
        "rendered_unix_ms",
        "content_ready_ms",
        "render_ms",
        "view",
        "trigger",
        "frames",
        "frame_cpu_total_ms",
        "max_frame_cpu_ms",
    ] {
        assert!(request.get(field).is_some(), "request field {field}");
    }
    assert_eq!(request["trigger"], "open");
}

#[test]
fn sample_frame_statistics_are_final_when_the_completion_is_published() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    let tab = Tab::showing("/repo");
    control(&benchmark, sample("frames", "/repo", "marker"));
    benchmark.accept_request(&request("/repo"));
    benchmark.record_frame(Some(0.010));
    benchmark.observe(&tab.state());
    benchmark.rendered(true);
    benchmark.frame_ready();
    assert!(sample_of(&benchmark).rendered_unix_ms.is_some());
    assert!(
        report_of(&benchmark)["request"]["rendered_unix_ms"].is_null(),
        "published before the render frame's CPU time was known"
    );
    benchmark.record_frame(Some(0.030));
    benchmark.frame_ready();
    let completed = report_of(&benchmark);
    assert!(completed["request"]["rendered_unix_ms"].is_number());
    assert_eq!(completed["request"]["frames"], 2);
    assert!((completed["request"]["frame_cpu_total_ms"].as_f64().unwrap() - 40.0).abs() < 0.001);
    assert!((completed["request"]["max_frame_cpu_ms"].as_f64().unwrap() - 30.0).abs() < 0.001);
    benchmark.record_frame(Some(0.500));
    assert_eq!(sample_of(&benchmark).frames.frames, 2);
    assert_eq!(
        benchmark.frames.frames, 2,
        "report-wide totals start once the requested content was shown"
    );
}

#[test]
fn request_controls_are_opt_in_and_invalid_schema_or_marker_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    for value in [
        json!({"schema": 2, "sample_id": "x", "repository": "/repo",
            "file": "src/main.rs", "marker": "marker"}),
        sample("empty", "/repo", ""),
    ] {
        control(&benchmark, value);
        benchmark.accept_request(&request("/repo"));
        assert!(benchmark.request.is_none());
    }
    control(&benchmark, json!({"schema": 1, "close": true}));
    let started = benchmark.started;
    let output = tick_output(&mut benchmark, started);
    assert!(output.commands.contains(&egui::ViewportCommand::Close));
    benchmark.report = None;
    benchmark.accept_request(&request("/repo"));
    assert!(benchmark.request.is_none());
    let output = tick_output(&mut benchmark, started);
    assert!(!output.commands.contains(&egui::ViewportCommand::Close));
}

#[test]
fn failed_completed_request_write_is_retried_without_changing_render_time() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = measured();
    let report = directory.path().join("report.json");
    std::fs::create_dir(&report).unwrap();
    benchmark.report = Some(report.clone());
    let tab = Tab::showing("/repo");
    control(&benchmark, sample("retry", "/repo", "marker"));
    benchmark.accept_request(&request("/repo"));
    benchmark.observe(&tab.state());
    benchmark.rendered(true);
    benchmark.frame_ready();
    settle(&mut benchmark, &tab);
    let rendered = sample_of(&benchmark).rendered_unix_ms;
    assert!(rendered.is_some());
    assert!(sample_of(&benchmark).pending_render);
    assert!(
        sample_of(&benchmark).in_flight(),
        "unpublished stays in flight"
    );
    std::fs::remove_dir(&report).unwrap();
    benchmark.frame_ready();
    assert!(!sample_of(&benchmark).pending_render);
    assert!(!sample_of(&benchmark).in_flight());
    assert_eq!(sample_of(&benchmark).rendered_unix_ms, rendered);
    let value: Value = serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap();
    assert_eq!(value["request"]["sample_id"], "retry");
    assert!(value["request"]["rendered_unix_ms"].is_number());
}

#[test]
fn rejected_sample_keeps_polling_control_at_the_idle_interval_until_close() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    control(&benchmark, sample("rejected", "/repo", "marker"));
    benchmark.accept_request(&request("/wrong"));
    assert!(benchmark.request.is_none());
    let started = benchmark.started;
    let output = tick_output(&mut benchmark, started);
    assert_eq!(output.repaint_delay, CONTROL_POLL_INTERVAL);
    assert!(!output.commands.contains(&egui::ViewportCommand::Close));
    control(&benchmark, json!({"schema": 1, "close": true}));
    let output = tick_output(&mut benchmark, started + CONTROL_POLL_INTERVAL);
    assert!(output.commands.contains(&egui::ViewportCommand::Close));
}

fn edit_sample(id: &str, marker: &str) -> Value {
    with(sample(id, "/repo", marker), "trigger", json!("edit"))
}

#[test]
fn edit_trigger_arms_against_the_current_tab_and_ignores_a_mismatching_one() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    let start = Instant::now();
    control(
        &benchmark,
        with(edit_sample("edit", "edited"), "view", json!("all")),
    );
    let mut other_file = Tab::showing("/repo");
    other_file.file = Some(GitPath(b"other.rs".to_vec()));
    let mut other_view = Tab::showing("/repo");
    other_view.view = View::Staged;
    let mut now = start;
    for tab in [Tab::showing("/other"), other_file, other_view] {
        now += CONTROL_POLL_INTERVAL;
        benchmark.arm_edit(&tab.state(), now);
        assert!(benchmark.request.is_none());
        assert!(!benchmark.accepted.contains("edit"));
        assert!(
            benchmark.edit_pending,
            "an unmatched edit control keeps waiting"
        );
    }
    let tab = Tab::showing("/repo");
    benchmark.arm_edit(&tab.state(), now + CONTROL_POLL_INTERVAL);
    assert!(!benchmark.edit_pending);
    let armed = sample_of(&benchmark);
    assert_eq!(armed.sample_id, "edit");
    assert!(armed.trigger == Trigger::Edit);
    assert!(
        armed
            .baseline
            .as_ref()
            .zip(tab.document.as_ref())
            .is_some_and(|(baseline, shown)| Arc::ptr_eq(baseline, shown))
    );
    let value = report_of(&benchmark);
    assert_eq!(value["request"]["trigger"], "edit");
    assert_eq!(value["request"]["view"], "all");
    assert_eq!(value["request"]["file"], "src/main.rs");
}

#[test]
fn edit_control_without_a_view_binds_the_selected_tabs_view() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    let mut tab = Tab::showing("/repo");
    tab.view = View::Staged;
    control(&benchmark, edit_sample("edit", "edited"));
    benchmark.arm_edit(&tab.state(), Instant::now());
    assert_eq!(report_of(&benchmark)["request"]["view"], "staged");
    tab.view = View::All;
    benchmark.observe(&tab.state());
    assert!(sample_of(&benchmark).content_ready_ms.is_none());
}

#[test]
fn edit_control_is_not_armed_by_an_open_request_and_open_control_not_by_observation() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    control(&benchmark, edit_sample("edit", "edited"));
    benchmark.accept_request(&request("/repo"));
    assert!(benchmark.request.is_none());
    control(&benchmark, sample("open", "/repo", "marker"));
    benchmark.arm_edit(&Tab::showing("/repo").state(), Instant::now());
    assert!(benchmark.request.is_none());
}

#[test]
fn edit_sample_completes_only_on_new_content_that_shows_the_marker() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    let mut tab = Tab::showing("/repo");
    control(&benchmark, edit_sample("edit", "edited"));
    benchmark.arm_edit(&tab.state(), Instant::now());
    show(&mut benchmark, &tab);
    assert!(
        sample_of(&benchmark).content_ready_ms.is_none(),
        "the pre-edit presentation is not new content"
    );
    assert!(benchmark.marker(&tab.locus()).is_none());
    tab.loading = true;
    tab.document = Some(document("fn edited() {}"));
    show(&mut benchmark, &tab);
    assert!(sample_of(&benchmark).content_ready_ms.is_none());
    tab.loading = false;
    benchmark.observe(&tab.state());
    assert!(sample_of(&benchmark).content_ready_ms.is_some());
    assert_eq!(benchmark.marker(&tab.locus()), Some("edited"));
    benchmark.rendered(true);
    benchmark.frame_ready();
    settle(&mut benchmark, &tab);
    let value = report_of(&benchmark);
    assert!(value["request"]["rendered_unix_ms"].is_number());
    assert_eq!(value["request"]["trigger"], "edit");
}

#[test]
fn edit_control_is_polled_at_most_once_per_interval_and_never_without_a_report() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    let tab = Tab::showing("/repo");
    let start = Instant::now();
    benchmark.arm_edit(&tab.state(), start);
    assert!(benchmark.control.last_poll.is_some());
    control(&benchmark, edit_sample("edit", "edited"));
    benchmark.arm_edit(&tab.state(), start + Duration::from_millis(10));
    assert!(
        benchmark.request.is_none(),
        "read again inside the interval"
    );
    benchmark.arm_edit(
        &tab.state(),
        start + CONTROL_POLL_INTERVAL - Duration::from_millis(1),
    );
    assert!(
        benchmark.request.is_none(),
        "read again inside the interval"
    );
    benchmark.arm_edit(&tab.state(), start + CONTROL_POLL_INTERVAL);
    assert_eq!(sample_of(&benchmark).sample_id, "edit");
    let mut idle = Benchmark::blank(false, None, None);
    idle.arm_edit(&tab.state(), start);
    idle.observe(&tab.state());
    assert!(idle.control.last_poll.is_none());
    assert!(idle.request.is_none() && idle.first.is_none());
}

#[test]
fn duplicate_edit_sample_ids_are_ignored() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    let tab = Tab::showing("/repo");
    let start = Instant::now();
    control(&benchmark, edit_sample("edit", "edited"));
    benchmark.arm_edit(&tab.state(), start);
    benchmark.frames = FrameStats::default().with(5.0);
    benchmark.request.as_mut().unwrap().content_ready_ms = Some(1.0);
    benchmark.arm_edit(&tab.state(), start + CONTROL_POLL_INTERVAL);
    assert_eq!(sample_of(&benchmark).content_ready_ms, Some(1.0));
    assert_eq!(benchmark.frames.frames, 1);
    control(&benchmark, edit_sample("edit-2", "edited again"));
    benchmark.arm_edit(&tab.state(), start + 2 * CONTROL_POLL_INTERVAL);
    assert_eq!(sample_of(&benchmark).sample_id, "edit-2");
    assert_eq!(benchmark.frames.frames, 0);
}

#[test]
fn observation_is_inert_without_a_report_or_smoke_mode() {
    let tab = Tab::showing("/repo");
    let mut idle = Benchmark::blank(false, None, None);
    idle.accept_request(&request("/repo"));
    idle.observe(&tab.state());
    idle.frame_ready();
    assert!(idle.first.is_none());
    assert!(idle.requested_content_ms.is_none());
    let mut smoke = Benchmark::blank(true, None, None);
    smoke.accept_request(&request("/repo"));
    smoke.observe(&tab.state());
    assert!(smoke.pending_content);
}

#[path = "polling_tests.rs"]
mod polling;
