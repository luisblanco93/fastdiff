use crate::worker::PreparedDocument;
use fastdiff_cli::{OpenRequest, OpenTarget};
use fastdiff_core::View;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

mod control;
mod identity;
#[cfg(test)]
pub(crate) mod tests;

use control::{ControlPoll, FailureLog};
use identity::Identity;
pub(crate) use identity::{TabLocus, TabState};

/// Report format. Schema 2 only adds fields to schema 1; every schema-1 field keeps its meaning.
const REPORT_SCHEMA: u64 = 2;
/// Control files are versioned independently of reports and stay at schema 1.
const CONTROL_SCHEMA: u64 = 1;
/// Repaint cadence while a sample still waits for its content, its marker render, the frame
/// that settles its statistics, or the write that publishes them. It is also the control poll
/// interval while an edit-trigger control waits for the selected tab to match.
const ACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(25);
/// Control poll and repaint cadence with nothing in flight. A warm window sees the next
/// control (sample, edit trigger, `close`) within one interval. This is the instrument's own
/// steady-state wake rate, so reports state it as `control_poll_ms`.
const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Opt-in measurement hooks used by the release benchmark runner.
///
/// Readiness is bound to a requested identity (repository, file, view): a frame only counts
/// when the selected tab settled on diff content for that identity. The identity is the
/// current control-file sample when one was accepted, otherwise the first open request.
pub struct Benchmark {
    started: Instant,
    started_unix_ms: u128,
    report: Option<PathBuf>,
    exit_after: Option<Duration>,
    first_frame_ms: Option<f64>,
    requested_content_ms: Option<f64>,
    pending_content: bool,
    smoke: bool,
    frames: FrameStats,
    first: Option<Identity>,
    request: Option<RequestMeasurement>,
    accepted: HashSet<String>,
    /// Throttled reads of the control file shared by every per-frame consumer.
    control: ControlPoll,
    /// A control asking for an edit sample is waiting for the selected tab to match it.
    edit_pending: bool,
    /// The last report write failed (on Windows the harness may hold the file open while we
    /// rename over it); retried on later frames until one succeeds.
    report_dirty: bool,
    write_failures: FailureLog,
}

#[derive(Clone, Copy, Default)]
struct FrameStats {
    frames: u64,
    total_ms: f64,
    max_ms: f64,
}

impl FrameStats {
    fn with(self, milliseconds: f64) -> Self {
        Self {
            frames: self.frames + 1,
            total_ms: self.total_ms + milliseconds,
            max_ms: self.max_ms.max(milliseconds),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Trigger {
    Open,
    Edit,
}

impl Trigger {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "open" => Some(Self::Open),
            "edit" => Some(Self::Edit),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Edit => "edit",
        }
    }
}

struct RequestMeasurement {
    sample_id: String,
    identity: Identity,
    marker: String,
    trigger: Trigger,
    started: Instant,
    received_unix_ms: u128,
    /// Content shown when an edit sample was armed; only a different document is new content.
    baseline: Option<Arc<PreparedDocument>>,
    content_ready_ms: Option<f64>,
    rendered_unix_ms: Option<u128>,
    render_ms: Option<f64>,
    pending_render: bool,
    frames: FrameStats,
    /// Set once the frame that rendered the marker had its CPU time reported; the sample's
    /// completion and frame statistics are published together from then on.
    settled: bool,
}

impl RequestMeasurement {
    fn from_control(value: &Value) -> Option<Self> {
        if value["schema"].as_u64() != Some(CONTROL_SCHEMA) {
            return None;
        }
        let text = |name: &str| value[name].as_str().filter(|text| !text.is_empty());
        let trigger = match value.get("trigger") {
            None | Some(Value::Null) => Trigger::Open,
            Some(name) => Trigger::parse(name.as_str()?)?,
        };
        let view = match value.get("view") {
            None | Some(Value::Null) => None,
            Some(name) => Some(serde_json::from_value::<View>(name.clone()).ok()?),
        };
        Some(Self {
            sample_id: text("sample_id")?.into(),
            identity: Identity::new(
                OpenTarget::Repository,
                text("repository")?,
                Some(text("file")?),
                view,
            ),
            marker: text("marker")?.into(),
            trigger,
            started: Instant::now(),
            received_unix_ms: unix_ms(),
            baseline: None,
            content_ready_ms: None,
            rendered_unix_ms: None,
            render_ms: None,
            pending_render: false,
            frames: FrameStats::default(),
            settled: false,
        })
    }

    /// Accept an open-trigger sample for `request`, binding an unset view to the request's.
    fn for_open(self, request: &OpenRequest) -> Option<Self> {
        (self.trigger == Trigger::Open
            && request.target == OpenTarget::Repository
            && self.identity.is_requested_by(request))
        .then(|| Self {
            identity: self.identity.with_view(request.view),
            ..self
        })
    }

    /// Whether `tab` presents the requested identity with content newer than the sample's.
    ///
    /// For an edit sample, "newer" is a different `Arc` than the baseline captured at arming.
    /// An automatic refresh that lands between arming and the edit also replaces the document,
    /// so it passes this check: `content_ready_ms` and `render_ms` of such a sample are
    /// unreliable. The marker gate still prevents a false completion, because a sample only
    /// completes in a frame that shows its marker, and the marker is written by the edit.
    fn shows_new_content(&self, tab: &TabState<'_>) -> bool {
        self.identity.is_presented(tab)
            && self.baseline.as_ref().is_none_or(|baseline| {
                tab.document()
                    .is_some_and(|document| !Arc::ptr_eq(baseline, document))
            })
    }

    fn awaiting_publication(&self) -> bool {
        self.pending_render && self.rendered_unix_ms.is_some() && self.settled
    }

    /// Whether the sample still needs frames: its content, its marker render, the frame that
    /// settles its statistics, or the write that publishes them.
    fn in_flight(&self) -> bool {
        self.rendered_unix_ms.is_none() || !self.settled || self.pending_render
    }

    fn record_frame(&mut self, milliseconds: Option<f64>) {
        if self.settled {
            return;
        }
        if let Some(milliseconds) = milliseconds {
            self.frames = self.frames.with(milliseconds);
        }
        self.settled = self.rendered_unix_ms.is_some();
    }

    fn report(&self) -> Value {
        json!({
            "sample_id": self.sample_id,
            "repository": self.identity.repository(),
            "file": self.identity.file(),
            "view": self.identity.view().and_then(view_name),
            "trigger": self.trigger.name(),
            "marker": self.marker,
            "received_unix_ms": self.received_unix_ms,
            // Completion is only visible once the sample's frame statistics are final.
            "rendered_unix_ms": self.rendered_unix_ms.filter(|_| self.settled),
            "content_ready_ms": self.content_ready_ms,
            "render_ms": self.render_ms.filter(|_| self.settled),
            "frames": self.frames.frames,
            "frame_cpu_total_ms": self.frames.total_ms,
            "max_frame_cpu_ms": self.frames.max_ms,
        })
    }
}

fn view_name(view: View) -> Option<String> {
    serde_json::to_value(view).ok()?.as_str().map(str::to_owned)
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn build_info() -> Value {
    let revision = option_env!("FASTDIFF_BUILD_REVISION").filter(|revision| !revision.is_empty());
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "revision": revision,
        "profile": profile,
        "protocol": fastdiff_cli::PROTOCOL_VERSION,
    })
}

fn platform_info() -> Value {
    json!({ "os": std::env::consts::OS, "arch": std::env::consts::ARCH })
}

impl Benchmark {
    pub fn new(smoke: bool) -> Self {
        let exit_after = std::env::var("FASTDIFF_BENCH_EXIT_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_millis)
            .or_else(|| smoke.then_some(Duration::from_millis(1500)));
        let mut benchmark = Self::blank(
            smoke,
            std::env::var_os("FASTDIFF_BENCH_REPORT").map(PathBuf::from),
            exit_after,
        );
        benchmark.write();
        benchmark
    }

    fn blank(smoke: bool, report: Option<PathBuf>, exit_after: Option<Duration>) -> Self {
        Self {
            started: Instant::now(),
            started_unix_ms: unix_ms(),
            report,
            exit_after,
            first_frame_ms: None,
            requested_content_ms: None,
            pending_content: false,
            smoke,
            frames: FrameStats::default(),
            first: None,
            request: None,
            accepted: HashSet::new(),
            control: ControlPoll::default(),
            edit_pending: false,
            report_dirty: false,
            write_failures: FailureLog::default(),
        }
    }

    /// Whether any measurement output is configured; everything else is a no-op without it.
    pub fn is_measuring(&self) -> bool {
        self.report.is_some() || self.smoke
    }

    /// The interval at which the control file is polled right now.
    fn poll_interval(&self) -> Duration {
        if self.edit_pending {
            ACTIVE_POLL_INTERVAL
        } else {
            CONTROL_POLL_INTERVAL
        }
    }

    fn arm(&mut self, sample: RequestMeasurement) {
        self.accepted.insert(sample.sample_id.clone());
        self.request = Some(sample);
        self.frames = FrameStats::default();
        self.write();
    }

    /// Record an incoming open request: it arms the control-file sample it matches, and the
    /// first request becomes the process's requested identity when no sample claims it.
    pub(crate) fn accept_request(&mut self, request: &OpenRequest) {
        if !self.is_measuring() {
            return;
        }
        let control = self
            .report
            .as_deref()
            .and_then(|report| self.control.read(report));
        let sample = control
            .as_ref()
            .and_then(RequestMeasurement::from_control)
            .and_then(|sample| sample.for_open(request))
            .filter(|sample| !self.accepted.contains(&sample.sample_id));
        match sample {
            Some(sample) => self.arm(sample),
            None => {
                if self.first.is_none() {
                    self.first = Some(Identity::from_request(request));
                }
            }
        }
    }

    /// Observe the selected tab once per frame: arm edit-trigger samples against it and
    /// record when it settles on the requested content.
    pub(crate) fn observe(&mut self, tab: &TabState<'_>) {
        if !self.is_measuring() {
            return;
        }
        self.arm_edit(tab, Instant::now());
        self.evaluate(tab);
    }

    /// Arm a new edit-trigger control against the selected tab. The control file is read at
    /// most once per poll interval, shared with `tick`, and never without a configured report.
    fn arm_edit(&mut self, tab: &TabState<'_>, now: Instant) {
        let interval = self.poll_interval();
        let Some(report) = self.report.as_deref() else {
            return;
        };
        self.control.poll(report, now, interval);
        if !self.control.take_unseen() {
            return;
        }
        let outcome = match &self.control.latest {
            Some(control) => EditControl::new(control, &self.accepted, tab),
            None => EditControl::Absent,
        };
        self.edit_pending = matches!(outcome, EditControl::Waiting);
        if let EditControl::Ready(sample) = outcome {
            self.arm(*sample);
        }
    }

    fn evaluate(&mut self, tab: &TabState<'_>) {
        if self.requested_content_ms.is_none() {
            let identity = self
                .request
                .as_ref()
                .map(|sample| &sample.identity)
                .or(self.first.as_ref());
            self.pending_content |= identity.is_some_and(|identity| identity.is_presented(tab));
        }
        if let Some(sample) = &mut self.request
            && sample.content_ready_ms.is_none()
            && sample.shows_new_content(tab)
        {
            sample.content_ready_ms = Some(sample.started.elapsed().as_secs_f64() * 1000.0);
        }
    }

    /// The marker to look for in `tab`, once its requested content is ready and until a frame
    /// showed it.
    pub(crate) fn marker(&self, tab: &TabLocus<'_>) -> Option<&str> {
        self.request
            .as_ref()
            .filter(|sample| {
                sample.content_ready_ms.is_some()
                    && sample.rendered_unix_ms.is_none()
                    && sample.identity.selects(tab)
            })
            .map(|sample| sample.marker.as_str())
    }

    pub(crate) fn rendered(&mut self, marker_visible: bool) {
        if let Some(sample) = &mut self.request
            && sample.content_ready_ms.is_some()
            && sample.rendered_unix_ms.is_none()
        {
            sample.pending_render = marker_visible;
        }
    }

    /// Collect opt-in UI/render work after loading; excludes vsync waiting.
    ///
    /// Report-wide totals start once the requested content was shown; the active sample
    /// counts every frame from acceptance until the frame that rendered its marker.
    pub fn record_frame(&mut self, seconds: Option<f32>) {
        if self.report.is_none() {
            return;
        }
        let milliseconds = seconds
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map(|value| f64::from(value) * 1000.0);
        if self.requested_content_ms.is_some()
            && let Some(milliseconds) = milliseconds
        {
            self.frames = self.frames.with(milliseconds);
        }
        if let Some(sample) = &mut self.request {
            sample.record_frame(milliseconds);
        }
    }

    pub fn frame_ready(&mut self) {
        let elapsed_ms = self.started.elapsed().as_secs_f64() * 1000.0;
        if self.first_frame_ms.is_none() {
            self.first_frame_ms = Some(elapsed_ms);
        }
        let first_content = self.pending_content && self.requested_content_ms.is_none();
        self.pending_content = false;
        if first_content {
            self.requested_content_ms = Some(elapsed_ms);
        }
        if let Some(sample) = &mut self.request
            && sample.pending_render
            && sample.rendered_unix_ms.is_none()
            && let Some(ready) = sample.content_ready_ms
        {
            sample.rendered_unix_ms = Some(unix_ms());
            sample.render_ms = Some(sample.started.elapsed().as_secs_f64() * 1000.0 - ready);
        }
        // A completed sample is published after its render frame's CPU time was reported, so
        // the report holds the sample's final frame statistics.
        let completed_request = self
            .request
            .as_ref()
            .is_some_and(RequestMeasurement::awaiting_publication);
        if !(first_content || completed_request || self.report_dirty) {
            return;
        }
        let written = self.write();
        if written
            && completed_request
            && let Some(sample) = &mut self.request
        {
            sample.pending_render = false;
        }
    }

    pub fn tick(&mut self, context: &egui::Context) {
        self.tick_at(context, Instant::now());
    }

    fn tick_at(&mut self, context: &egui::Context, now: Instant) {
        let interval = self.poll_interval();
        if let Some(report) = self.report.as_deref() {
            self.control.poll(report, now, interval);
        }
        let close = self.close_requested();
        if close {
            context.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if let Some(delay) = self.repaint_delay(now) {
            context.request_repaint_after(delay);
        }
        if let Some(deadline) = self.exit_after {
            let elapsed = now.duration_since(self.started);
            if elapsed >= deadline {
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                context.request_repaint_after(deadline - elapsed);
            }
        }
    }

    /// Whether the latest control read asked the viewer to close.
    fn close_requested(&self) -> bool {
        self.report.is_some()
            && self
                .control
                .latest
                .as_ref()
                .is_some_and(|value| value["schema"] == CONTROL_SCHEMA && value["close"] == true)
    }

    /// When the UI must wake for the instrument itself, if at all.
    ///
    /// Frames come at the active cadence only while something is in flight: a sample that is
    /// not published yet, or an edit control waiting for its tab. Otherwise a window that
    /// holds a sample or a control file wakes once per poll interval, for the time remaining
    /// until the next poll, so it can still see a new sample, an edit trigger, or `close`.
    fn repaint_delay(&self, now: Instant) -> Option<Duration> {
        self.report.as_ref()?;
        let in_flight = self
            .request
            .as_ref()
            .is_some_and(RequestMeasurement::in_flight);
        if in_flight || self.edit_pending {
            return Some(ACTIVE_POLL_INTERVAL);
        }
        let watching = self.request.is_some() || self.control.latest.is_some() || self.report_dirty;
        watching.then(|| self.control.due_in(now, CONTROL_POLL_INTERVAL))
    }

    /// Final flush when the viewer closes: no further frame will report its CPU time, so a
    /// rendered sample's statistics are as complete as they will get.
    pub fn finish(&mut self) -> bool {
        if let Some(sample) = &mut self.request {
            sample.settled |= sample.rendered_unix_ms.is_some();
        }
        self.write()
    }

    pub fn write(&mut self) -> bool {
        let value = json!({
            "schema": REPORT_SCHEMA,
            "pid": std::process::id(),
            "started_unix_ms": self.started_unix_ms,
            "first_frame_ms": self.first_frame_ms,
            "ui_ready_ms": self.first_frame_ms,
            // Deprecated alias of `requested_content_ms`, kept for one release.
            "first_diff_ms": self.requested_content_ms,
            "requested_content_ms": self.requested_content_ms,
            "measured_frames": self.frames.frames,
            "max_frame_cpu_ms": self.frames.max_ms,
            "frame_cpu_total_ms": self.frames.total_ms,
            // The instrument's own steady-state wake rate while it waits for the next control.
            "control_poll_ms": self.report.as_ref().map(|_| CONTROL_POLL_INTERVAL.as_millis()),
            "build": build_info(),
            "platform": platform_info(),
            "request": self.request.as_ref().map(RequestMeasurement::report),
        });
        let failure = self.report.as_deref().and_then(|path| {
            write_report(path, &value.to_string()).err().map(|error| {
                format!(
                    "Could not write benchmark report {}: {error}",
                    path.display()
                )
            })
        });
        let written = failure.is_none();
        match failure {
            Some(failure) => {
                self.write_failures.record(failure);
            }
            None => self.write_failures.clear(),
        }
        self.report_dirty = !written;
        if self.smoke && self.requested_content_ms.is_some() {
            eprintln!("FASTDIFF_SMOKE {value}");
        }
        written
    }
}

/// What a control file asks of the selected tab, as an edit-trigger sample.
enum EditControl {
    /// Nothing to arm: no control, an old sample id, another trigger, or an invalid control.
    Absent,
    /// A new edit control that does not select the tab yet.
    Waiting,
    Ready(Box<RequestMeasurement>),
}

impl EditControl {
    fn new(control: &Value, accepted: &HashSet<String>, tab: &TabState<'_>) -> Self {
        if control["sample_id"]
            .as_str()
            .is_some_and(|id| accepted.contains(id))
        {
            return Self::Absent;
        }
        let Some(sample) = RequestMeasurement::from_control(control) else {
            return Self::Absent;
        };
        if sample.trigger != Trigger::Edit {
            return Self::Absent;
        }
        if !sample.identity.selects(tab.locus()) {
            return Self::Waiting;
        }
        let view = sample.identity.view().unwrap_or_else(|| tab.locus().view());
        Self::Ready(Box::new(RequestMeasurement {
            baseline: tab.document().cloned(),
            identity: sample.identity.with_view(view),
            ..sample
        }))
    }
}

fn write_report(path: &Path, value: &str) -> std::io::Result<()> {
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, value)?;
    std::fs::rename(temporary, path)
}
