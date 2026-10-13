//! The instrument's own overhead: when a measuring window repaints, reads its control file,
//! and retries a report write.
use super::*;

const MILLISECOND: Duration = Duration::from_millis(1);

/// A benchmark with an open sample armed on `/repo` and a tab that settled on it.
fn armed_open_sample(directory: &tempfile::TempDir) -> (Benchmark, Tab) {
    let mut benchmark = in_directory(directory);
    control(&benchmark, sample("poll", "/repo", "marker"));
    benchmark.accept_request(&request("/repo"));
    (benchmark, Tab::showing("/repo"))
}

/// Render the armed sample's marker and publish its completion.
fn publish(benchmark: &mut Benchmark, tab: &Tab) {
    benchmark.observe(&tab.state());
    benchmark.rendered(true);
    benchmark.frame_ready();
    settle(benchmark, tab);
    assert!(report_of(benchmark)["request"]["rendered_unix_ms"].is_number());
}

fn control_file(benchmark: &Benchmark) -> PathBuf {
    let mut path = benchmark
        .report
        .as_ref()
        .unwrap()
        .as_os_str()
        .to_os_string();
    path.push(".request.json");
    PathBuf::from(path)
}

fn with_repository(mut value: Value, repository: &str) -> Value {
    value["repository"] = json!(repository);
    value
}

#[test]
fn repaints_every_active_interval_until_published_then_backs_off() {
    let directory = tempfile::tempdir().unwrap();
    let (mut benchmark, tab) = armed_open_sample(&directory);
    let start = benchmark.started;
    let delay = |benchmark: &mut Benchmark, now: Instant| tick_output(benchmark, now).repaint_delay;

    assert_eq!(
        delay(&mut benchmark, start),
        ACTIVE_POLL_INTERVAL,
        "waiting for content"
    );
    benchmark.observe(&tab.state());
    assert_eq!(
        delay(&mut benchmark, start + MILLISECOND),
        ACTIVE_POLL_INTERVAL,
        "waiting for the marker render"
    );
    benchmark.rendered(true);
    benchmark.frame_ready();
    assert_eq!(
        delay(&mut benchmark, start + 2 * MILLISECOND),
        ACTIVE_POLL_INTERVAL,
        "waiting for the settling frame"
    );
    settle(&mut benchmark, &tab);
    assert!(report_of(&benchmark)["request"]["rendered_unix_ms"].is_number());
    assert!(!sample_of(&benchmark).in_flight());

    let polled = benchmark.control.last_poll.expect("the control was polled");
    assert_eq!(
        delay(&mut benchmark, polled + CONTROL_POLL_INTERVAL),
        CONTROL_POLL_INTERVAL,
        "published: a warm window only polls for the next control"
    );
    let polled = benchmark.control.last_poll.unwrap();
    assert_eq!(
        delay(&mut benchmark, polled + 100 * MILLISECOND),
        CONTROL_POLL_INTERVAL - 100 * MILLISECOND,
        "the wake-up is scheduled for the remaining time, not a fresh interval"
    );
}

#[test]
fn a_new_sample_returns_the_window_to_the_active_interval() {
    let directory = tempfile::tempdir().unwrap();
    let (mut benchmark, tab) = armed_open_sample(&directory);
    publish(&mut benchmark, &tab);
    let polled = benchmark.control.last_poll.unwrap();
    assert_eq!(
        tick_output(&mut benchmark, polled + CONTROL_POLL_INTERVAL).repaint_delay,
        CONTROL_POLL_INTERVAL
    );
    control(&benchmark, sample("again", "/repo", "second marker"));
    benchmark.accept_request(&request("/repo"));
    assert_eq!(sample_of(&benchmark).sample_id, "again");
    let polled = benchmark.control.last_poll.unwrap();
    assert_eq!(
        tick_output(&mut benchmark, polled + MILLISECOND).repaint_delay,
        ACTIVE_POLL_INTERVAL
    );
}

#[test]
fn every_consumer_shares_one_read_per_interval_and_close_is_honored() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    let tab = Tab::showing("/repo");
    let start = Instant::now();
    control(&benchmark, sample("open", "/repo", "marker"));
    benchmark.arm_edit(&tab.state(), start);
    control(&benchmark, json!({"schema": 1, "close": true}));

    for elapsed in [MILLISECOND, CONTROL_POLL_INTERVAL - MILLISECOND] {
        let output = tick_output(&mut benchmark, start + elapsed);
        assert!(
            !output.commands.contains(&egui::ViewportCommand::Close),
            "read again {elapsed:?} after the last read"
        );
    }
    benchmark.arm_edit(&tab.state(), start + CONTROL_POLL_INTERVAL - MILLISECOND);
    let output = tick_output(&mut benchmark, start + CONTROL_POLL_INTERVAL);
    assert!(output.commands.contains(&egui::ViewportCommand::Close));
}

#[test]
fn a_tick_without_observation_still_lets_the_next_observation_arm_the_edit() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    let tab = Tab::showing("/repo");
    let start = Instant::now();
    control(&benchmark, edit_sample("edit", "edited"));
    let _ = tick_output(&mut benchmark, start);
    assert!(benchmark.request.is_none());
    benchmark.arm_edit(&tab.state(), start + MILLISECOND);
    assert_eq!(
        sample_of(&benchmark).sample_id,
        "edit",
        "the read the tick made is evaluated, not postponed by another interval"
    );
}

#[test]
fn nothing_is_read_or_polled_without_a_report() {
    for smoke in [false, true] {
        let mut benchmark = Benchmark::blank(smoke, None, None);
        let start = benchmark.started;
        let output = tick_output(&mut benchmark, start);
        assert_eq!(output.repaint_delay, Duration::MAX, "smoke: {smoke}");
        assert!(benchmark.control.last_poll.is_none(), "smoke: {smoke}");
    }
}

#[test]
fn pending_edit_arming_polls_at_the_active_interval_and_arms_within_it() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    let tab = Tab::showing("/repo");
    let start = Instant::now();
    control(
        &benchmark,
        with_repository(edit_sample("late", "edited"), "/other"),
    );
    benchmark.arm_edit(&tab.state(), start);
    assert!(benchmark.request.is_none());
    assert!(benchmark.edit_pending);
    let output = tick_output(&mut benchmark, start);
    assert_eq!(output.repaint_delay, ACTIVE_POLL_INTERVAL);

    control(&benchmark, edit_sample("late", "edited"));
    benchmark.arm_edit(&tab.state(), start + ACTIVE_POLL_INTERVAL - MILLISECOND);
    assert!(benchmark.request.is_none());
    benchmark.arm_edit(&tab.state(), start + ACTIVE_POLL_INTERVAL);
    assert_eq!(sample_of(&benchmark).sample_id, "late");
    assert!(!benchmark.edit_pending);
}

#[test]
fn a_warm_idle_window_arms_the_next_edit_within_one_idle_interval() {
    let directory = tempfile::tempdir().unwrap();
    let (mut benchmark, tab) = armed_open_sample(&directory);
    publish(&mut benchmark, &tab);
    let polled = benchmark.control.last_poll.expect("observation polled");
    control(&benchmark, edit_sample("next", "edited"));
    benchmark.arm_edit(&tab.state(), polled + CONTROL_POLL_INTERVAL - MILLISECOND);
    assert_eq!(sample_of(&benchmark).sample_id, "poll");
    benchmark.arm_edit(&tab.state(), polled + CONTROL_POLL_INTERVAL);
    assert_eq!(sample_of(&benchmark).sample_id, "next");
    assert!(sample_of(&benchmark).in_flight());
}

#[test]
fn a_persistently_invalid_control_is_reported_once_per_distinct_failure() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = in_directory(&directory);
    let report = benchmark.report.clone().unwrap();
    let path = control_file(&benchmark);
    let mut now = Instant::now();
    let mut poll = |benchmark: &mut Benchmark| {
        now += CONTROL_POLL_INTERVAL;
        benchmark.control.poll(&report, now, CONTROL_POLL_INTERVAL);
        benchmark.control.failures.printed
    };

    std::fs::write(&path, "{ not json").unwrap();
    assert_eq!(poll(&mut benchmark), 1);
    assert_eq!(poll(&mut benchmark), 1, "the same failure is not repeated");
    assert_eq!(poll(&mut benchmark), 1);
    std::fs::write(&path, "[1, 2").unwrap();
    assert_eq!(poll(&mut benchmark), 2, "a different failure is new");
    std::fs::write(&path, sample("ok", "/repo", "marker").to_string()).unwrap();
    assert_eq!(poll(&mut benchmark), 2);
    assert!(benchmark.control.latest.is_some());
    std::fs::write(&path, "[1, 2").unwrap();
    assert_eq!(
        poll(&mut benchmark),
        3,
        "a recovered file re-arms the report"
    );
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        poll(&mut benchmark),
        3,
        "a missing control is not a failure"
    );
}

#[test]
fn failed_first_content_write_is_retried_without_changing_any_timestamp() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = measured();
    let report = directory.path().join("report.json");
    std::fs::create_dir(&report).unwrap();
    benchmark.report = Some(report.clone());
    benchmark.accept_request(&request("/repo"));
    let tab = Tab::showing("/repo");

    show(&mut benchmark, &tab);
    let content_ms = benchmark.requested_content_ms;
    let first_frame_ms = benchmark.first_frame_ms;
    assert!(content_ms.is_some());
    assert!(benchmark.report_dirty, "the failed write is remembered");
    let polled = benchmark.started;
    assert_eq!(
        tick_output(&mut benchmark, polled).repaint_delay,
        CONTROL_POLL_INTERVAL,
        "retried on the poll cadence, never in a tight loop"
    );
    show(&mut benchmark, &tab);
    assert!(benchmark.report_dirty);
    assert_eq!(benchmark.requested_content_ms, content_ms);

    std::fs::remove_dir(&report).unwrap();
    benchmark.frame_ready();
    assert!(!benchmark.report_dirty);
    let value = report_of(&benchmark);
    // The report holds the timestamp recorded at the first frame, not a later one. Parsing
    // is not guaranteed to round-trip the last digit of a float.
    let reported = value["requested_content_ms"].as_f64().unwrap();
    assert!((reported - content_ms.unwrap()).abs() < 1e-6);
    assert_eq!(value["first_frame_ms"].as_f64(), first_frame_ms);
    assert_eq!(benchmark.requested_content_ms, content_ms);
    assert_eq!(
        tick_output(&mut benchmark, polled + 3 * CONTROL_POLL_INTERVAL).repaint_delay,
        Duration::MAX,
        "a published legacy report needs no further wake-ups"
    );
}

#[test]
fn failed_arming_write_is_retried_on_a_later_frame() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = measured();
    let report = directory.path().join("report.json");
    std::fs::create_dir(&report).unwrap();
    benchmark.report = Some(report.clone());
    control(&benchmark, sample("armed", "/repo", "marker"));
    benchmark.accept_request(&request("/repo"));
    assert_eq!(sample_of(&benchmark).sample_id, "armed");
    assert!(benchmark.report_dirty);

    std::fs::remove_dir(&report).unwrap();
    benchmark.frame_ready();
    assert!(!benchmark.report_dirty);
    assert_eq!(report_of(&benchmark)["request"]["sample_id"], "armed");
}

#[test]
fn repeated_write_failures_are_reported_once() {
    let directory = tempfile::tempdir().unwrap();
    let mut benchmark = measured();
    let report = directory.path().join("report.json");
    std::fs::create_dir(&report).unwrap();
    benchmark.report = Some(report);
    for _ in 0..3 {
        assert!(!benchmark.write());
    }
    assert_eq!(benchmark.write_failures.printed, 1);
}

#[test]
fn canonical_path_comparison_asks_the_filesystem_once_per_candidate() {
    let directory = tempfile::tempdir().unwrap();
    let repository = directory.path().join("repo");
    std::fs::create_dir(&repository).unwrap();
    let identity = Identity::new(
        OpenTarget::Repository,
        repository.to_str().unwrap(),
        Some("src/main.rs"),
        None,
    );
    let alias = repository.join("..").join("repo");
    let open = request(alias.to_str().unwrap());
    assert!(identity.is_requested_by(&open));

    std::fs::remove_dir(&repository).unwrap();
    assert!(
        identity.is_requested_by(&open),
        "the answer is remembered; the frame loop does not canonicalize again"
    );
    let other = request(directory.path().join("elsewhere").to_str().unwrap());
    assert!(!identity.is_requested_by(&other));
}
