use super::*;
use std::{collections::HashSet, panic::Location};

const WAIT: Duration = Duration::from_secs(5);

pub(crate) struct ProgressGuard {
    expected: String,
    budget: Duration,
    last_progress: Instant,
    milestones: HashSet<String>,
    last_milestone: Option<String>,
    observed: VecDeque<String>,
}

impl ProgressGuard {
    pub(crate) fn new(expected: impl Into<String>, budget: Duration) -> Self {
        Self::new_at(expected, budget, Instant::now())
    }

    fn new_at(expected: impl Into<String>, budget: Duration, now: Instant) -> Self {
        Self {
            expected: expected.into(),
            budget,
            last_progress: now,
            milestones: HashSet::new(),
            last_milestone: None,
            observed: VecDeque::new(),
        }
    }

    pub(crate) fn advance(&mut self, milestone: impl Into<String>) -> bool {
        self.advance_at(milestone.into(), Instant::now())
    }

    fn advance_at(&mut self, milestone: String, now: Instant) -> bool {
        if !self.milestones.insert(milestone.clone()) {
            return false;
        }
        self.last_milestone = Some(milestone);
        self.last_progress = now;
        true
    }

    pub(crate) fn check(&self, worker_finished: bool) {
        if let Some(message) = self.timeout_at(Instant::now(), worker_finished) {
            panic!("{message}");
        }
    }

    fn timeout_at(&self, now: Instant, worker_finished: bool) -> Option<String> {
        (now.duration_since(self.last_progress) >= self.budget)
            .then(|| self.failure("no new progress within the wait budget", worker_finished))
    }

    fn failure(&self, reason: &str, worker_finished: bool) -> String {
        format!(
            "Waiting for {}: {reason}; last milestone: {}; observed events: {:?}; worker finished: {worker_finished}",
            self.expected,
            self.last_milestone.as_deref().unwrap_or("none"),
            self.observed,
        )
    }

    fn observe(&mut self, event: &Event) {
        if self.observed.len() == 12 {
            self.observed.pop_front();
        }
        self.observed.push_back(event_summary(event));
    }
}

pub(super) enum EventProgress {
    Ignore,
    Advance(String),
    Complete,
    Fail(String),
}

fn event_summary(event: &Event) -> String {
    match event {
        Event::Ready(root, has_repository) => format!("Ready({root}, repository={has_repository})"),
        Event::Branch { generation, branch } => {
            format!("Branch(generation={generation}, {branch:?})")
        }
        Event::Reflog { generation, list } => format!(
            "Reflog(generation={generation}, snapshot={:?}, start={}, branch={:?})",
            list.snapshot, list.start, list.branch
        ),
        Event::Search {
            generation,
            request_id,
            ..
        } => {
            format!("Search(generation={generation}, request={request_id})")
        }
        Event::Error {
            generation,
            message,
        } => format!("Error(generation={generation}, {message})"),
        Event::ReflogError {
            generation,
            message,
        } => {
            format!("ReflogError(generation={generation}, {message})")
        }
        Event::Notice { generation, .. } => format!("Notice(generation={generation})"),
        Event::DocumentPath { generation, .. } => format!("DocumentPath(generation={generation})"),
        Event::Availability {
            generation,
            available,
        } => {
            format!("Availability(generation={generation}, {available})")
        }
        Event::Status {
            generation,
            automatic,
            ..
        } => {
            format!("Status(generation={generation}, automatic={automatic})")
        }
        Event::Diff { generation, .. } => format!("Diff(generation={generation})"),
        Event::Provisional { generation, .. } => format!("Provisional(generation={generation})"),
    }
}

pub(super) fn event_progress(event: &Event, generation: u64) -> EventProgress {
    match event {
        Event::Branch {
            generation: actual, ..
        }
        | Event::Notice {
            generation: actual, ..
        }
        | Event::DocumentPath {
            generation: actual, ..
        }
        | Event::Availability {
            generation: actual, ..
        }
        | Event::Status {
            generation: actual, ..
        }
        | Event::Diff {
            generation: actual, ..
        }
        | Event::Provisional {
            generation: actual, ..
        } if *actual == generation => EventProgress::Advance(event_summary(event)),
        _ => EventProgress::Ignore,
    }
}

pub(super) fn reflog_progress(event: &Event, generation: u64) -> EventProgress {
    match event {
        Event::ReflogError {
            generation: actual,
            message,
        } if *actual == generation => EventProgress::Fail(message.clone()),
        Event::Reflog {
            generation: actual, ..
        } if *actual == generation => EventProgress::Advance(event_summary(event)),
        _ => EventProgress::Ignore,
    }
}

fn inspect_batch(
    guard: &mut ProgressGuard,
    batch: &[Event],
    classify: &mut impl FnMut(&Event) -> EventProgress,
) -> Result<bool, String> {
    let mut complete = false;
    let mut failure = None;
    for event in batch {
        guard.observe(event);
        match classify(event) {
            EventProgress::Ignore => {}
            EventProgress::Advance(milestone) => {
                guard.advance(milestone);
            }
            EventProgress::Complete => complete = true,
            EventProgress::Fail(message) => {
                failure.get_or_insert(message);
            }
        }
    }
    failure.map_or(Ok(complete), Err)
}

pub(super) fn wait_events_until(
    worker: &Worker,
    expected: impl Into<String>,
    mut classify: impl FnMut(&Event) -> EventProgress,
) -> Vec<Event> {
    let mut guard = ProgressGuard::new(expected, WAIT);
    let mut events = Vec::new();
    loop {
        let batch = worker.drain_events();
        let complete = inspect_batch(&mut guard, &batch, &mut classify)
            .unwrap_or_else(|reason| panic!("{}", guard.failure(&reason, worker.is_finished())));
        events.extend(batch);
        if complete {
            return events;
        }
        if worker.is_finished() {
            // A worker may enqueue its final event between the first drain and its exit.
            let batch = worker.drain_events();
            let complete = inspect_batch(&mut guard, &batch, &mut classify)
                .unwrap_or_else(|reason| panic!("{}", guard.failure(&reason, true)));
            events.extend(batch);
            assert!(
                complete,
                "{}",
                guard.failure("worker terminated before completion", true)
            );
            return events;
        }
        guard.check(false);
        thread::sleep(Duration::from_millis(10));
    }
}

#[track_caller]
pub(super) fn wait_events(
    worker: &Worker,
    generation: u64,
    accepts: impl Fn(&Event) -> bool,
) -> Vec<Event> {
    wait_events_until(
        worker,
        format!(
            "worker event generation {generation} at {}",
            Location::caller()
        ),
        |event| {
            if accepts(event) {
                EventProgress::Complete
            } else {
                event_progress(event, generation)
            }
        },
    )
}

#[track_caller]
pub(super) fn wait_event(
    worker: &Worker,
    generation: u64,
    accepts: impl Fn(&Event) -> bool,
) -> Event {
    wait_events(worker, generation, &accepts)
        .into_iter()
        .find(accepts)
        .unwrap()
}

pub(super) fn wait_status(
    worker: &Worker,
    generation: u64,
    automatic: bool,
) -> Arc<Vec<FileChange>> {
    let Event::Status { files, .. } = wait_event(worker, generation, |event| {
        matches!(event, Event::Status { generation: actual, automatic: actual_automatic, .. }
            if *actual == generation && *actual_automatic == automatic)
    }) else {
        unreachable!()
    };
    files
}

pub(super) fn wait_reflog(worker: &Worker, generation: u64) -> ReflogList {
    let accepts = |event: &Event| matches!(event, Event::Reflog { generation: actual, .. } if *actual == generation);
    let events = wait_events_until(worker, format!("reflog generation {generation}"), |event| {
        if accepts(event) {
            EventProgress::Complete
        } else {
            reflog_progress(event, generation)
        }
    });
    let Event::Reflog { list, .. } = events.into_iter().find(accepts).unwrap() else {
        unreachable!()
    };
    list
}

#[test]
fn progress_can_exceed_the_total_wait_budget() {
    let start = Instant::now();
    let mut guard = ProgressGuard::new_at("diff generation 7", WAIT, start);
    for second in [4, 8, 12] {
        let now = start + Duration::from_secs(second);
        assert!(guard.timeout_at(now, false).is_none());
        assert!(guard.advance_at(format!("generation 7 milestone {second}"), now));
    }
    assert!(
        guard
            .timeout_at(start + Duration::from_secs(16), false)
            .is_none()
    );
}

#[test]
fn five_seconds_without_new_progress_times_out_with_context() {
    let start = Instant::now();
    let mut guard = ProgressGuard::new_at("search generation 7 request 3", WAIT, start);
    guard.advance_at("document loaded".into(), start);
    let message = guard.timeout_at(start + WAIT, true).unwrap();
    assert!(message.contains("search generation 7 request 3"));
    assert!(message.contains("document loaded"));
    assert!(message.contains("worker finished: true"));
}

#[test]
fn duplicate_and_unrelated_events_do_not_extend_progress() {
    let start = Instant::now();
    let mut guard = ProgressGuard::new_at("diff generation 7", WAIT, start);
    guard.advance_at("generation 7 provisional".into(), start);
    assert!(!guard.advance_at("generation 7 provisional".into(), start + WAIT));
    assert!(matches!(
        event_progress(
            &Event::Branch {
                generation: 8,
                branch: Some("unrelated".into())
            },
            7
        ),
        EventProgress::Ignore
    ));
    assert!(matches!(
        event_progress(
            &Event::Search {
                generation: 7,
                request_id: 99,
                query: String::new(),
                hits: vec![]
            },
            7
        ),
        EventProgress::Ignore
    ));
    assert!(guard.timeout_at(start + WAIT, false).is_some());
}

#[test]
fn equal_generation_numbers_do_not_mix_foreground_and_reflog_progress() {
    let reflog = Event::Reflog {
        generation: 7,
        list: ReflogList {
            branch: None,
            entries: vec![],
            notice: None,
            snapshot: Some(3),
            start: 0,
            has_more: false,
        },
    };
    let reflog_error = Event::ReflogError {
        generation: 7,
        message: "independent reflog failure".into(),
    };
    assert!(matches!(event_progress(&reflog, 7), EventProgress::Ignore));
    assert!(matches!(
        event_progress(&reflog_error, 7),
        EventProgress::Ignore
    ));
    assert!(matches!(
        reflog_progress(&reflog, 7),
        EventProgress::Advance(_)
    ));
    assert!(matches!(
        reflog_progress(&reflog_error, 7),
        EventProgress::Fail(_)
    ));
    let foreground = Event::Branch {
        generation: 7,
        branch: Some("main".into()),
    };
    let foreground_error = Event::Error {
        generation: 7,
        message: "independent foreground failure".into(),
    };
    assert!(matches!(
        reflog_progress(&foreground, 7),
        EventProgress::Ignore
    ));
    assert!(matches!(
        reflog_progress(&foreground_error, 7),
        EventProgress::Ignore
    ));
    assert!(matches!(
        event_progress(&foreground, 7),
        EventProgress::Advance(_)
    ));
    assert!(matches!(
        event_progress(&foreground_error, 7),
        EventProgress::Ignore
    ));
}

#[test]
fn a_batch_is_fully_classified_even_after_completion_and_errors() {
    let mut guard = ProgressGuard::new("status generation 7", WAIT);
    let batch = [
        Event::Branch {
            generation: 7,
            branch: None,
        },
        Event::Error {
            generation: 7,
            message: "forbidden after completion".into(),
        },
        Event::DocumentPath {
            generation: 7,
            path: "last".into(),
        },
    ];
    let mut seen = 0;
    let result = inspect_batch(&mut guard, &batch, &mut |event| {
        seen += 1;
        if matches!(event, Event::Branch { .. }) {
            EventProgress::Complete
        } else if let Event::Error { message, .. } = event {
            EventProgress::Fail(message.clone())
        } else {
            event_progress(event, 7)
        }
    });
    assert_eq!(seen, batch.len());
    assert_eq!(result.unwrap_err(), "forbidden after completion");
    assert!(
        guard
            .failure("failed", false)
            .contains("DocumentPath(generation=7)")
    );
}

#[test]
fn a_worker_error_reports_the_operation_and_generation() {
    let (mut worker, _) = Worker::test_with_thread(thread::spawn(|| {}));
    worker.thread.take().unwrap().join().unwrap();
    worker
        .shared
        .events
        .lock()
        .unwrap()
        .push_back(Event::Error {
            generation: 7,
            message: "backend failed".into(),
        });
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_events_until(&worker, "verified diff generation 7", |event| match event {
            Event::Error {
                generation: 7,
                message,
            } => EventProgress::Fail(message.clone()),
            _ => event_progress(event, 7),
        });
    }))
    .unwrap_err();
    let message = failure.downcast_ref::<String>().unwrap();
    assert!(message.contains("verified diff generation 7"));
    assert!(message.contains("backend failed"));
    assert!(message.contains("Error(generation=7"));
    assert!(message.contains("worker finished: true"));
}

#[test]
fn transient_errors_do_not_prevent_completion() {
    let error = Event::Error {
        generation: 7,
        message: "transient index.lock".into(),
    };
    let accepted = Event::Branch {
        generation: 7,
        branch: None,
    };
    let mut guard = ProgressGuard::new("branch generation 7", WAIT);
    let mut classify = |event: &Event| {
        if matches!(event, Event::Branch { generation: 7, .. }) {
            EventProgress::Complete
        } else {
            event_progress(event, 7)
        }
    };
    assert_eq!(
        inspect_batch(&mut guard, &[error], &mut classify),
        Ok(false)
    );
    assert_eq!(
        inspect_batch(&mut guard, &[accepted], &mut classify),
        Ok(true)
    );
    for batch in [
        vec![
            Event::Error {
                generation: 7,
                message: "transient".into(),
            },
            Event::Branch {
                generation: 7,
                branch: None,
            },
        ],
        vec![
            Event::Branch {
                generation: 7,
                branch: None,
            },
            Event::Error {
                generation: 7,
                message: "transient".into(),
            },
        ],
    ] {
        assert_eq!(inspect_batch(&mut guard, &batch, &mut classify), Ok(true));
    }
}

#[test]
fn final_event_from_a_finished_thread_is_retained_with_its_whole_batch() {
    let (mut worker, _) = Worker::test_with_thread(thread::spawn(|| {}));
    worker.thread.take().unwrap().join().unwrap();
    worker
        .shared
        .events
        .lock()
        .unwrap()
        .push_back(Event::Notice {
            generation: 7,
            message: Some("first drain".into()),
        });
    let events = wait_events(&worker, 7, |event| {
        if matches!(event, Event::Notice { generation: 7, .. }) {
            assert!(worker.is_finished());
            // Publish after the first drain so only the post-exit drain can see this batch.
            worker.shared.events.lock().unwrap().extend([
                Event::Branch {
                    generation: 7,
                    branch: None,
                },
                Event::DocumentPath {
                    generation: 7,
                    path: "after completion".into(),
                },
            ]);
        }
        matches!(event, Event::Branch { generation: 7, .. })
    });
    assert_eq!(events.len(), 3);
    assert!(matches!(&events[0], Event::Notice { .. }));
    assert!(matches!(&events[1], Event::Branch { generation: 7, .. }));
    assert!(matches!(&events[2], Event::DocumentPath { path, .. } if path == "after completion"));
}
