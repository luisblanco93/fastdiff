use super::*;
use fastdiff_core::{REFLOG_PAGE_SIZE, ReflogEntry};

#[derive(Default)]
pub(super) struct ReflogState {
    pub generation: Option<u64>,
    pub request: Option<ReflogRequest>,
    snapshot: Option<u64>,
    loaded: usize,
    boundary: Option<ReflogEntry>,
    has_more: bool,
}

impl ReflogState {
    pub fn open(&mut self, generation: u64, enabled: bool) {
        *self = Self {
            generation: enabled.then_some(generation),
            request: enabled.then(ReflogRequest::default),
            ..Self::default()
        };
    }

    pub fn more(&mut self, generation: u64, request: ReflogRequest) -> bool {
        if self.generation != Some(generation)
            || self.request.is_some()
            || !self.has_more
            || request.start != self.loaded
            || request.snapshot != self.snapshot
            || request.boundary != self.boundary
        {
            return false;
        }
        self.request = Some(request);
        true
    }

    pub fn periodic_request(&self, snapshot: Option<u64>) -> Option<ReflogRequest> {
        if snapshot.is_some() && snapshot == self.snapshot {
            return None;
        }
        if snapshot.is_some() {
            return Some(ReflogRequest::default());
        }
        // Unreliable metadata requires replacing the currently loaded prefix.
        Some(ReflogRequest {
            start: self.loaded.saturating_sub(REFLOG_PAGE_SIZE),
            snapshot: None,
            boundary: self.boundary.clone(),
        })
    }

    pub fn accept(&mut self, list: &ReflogList) {
        self.loaded = list.start + list.entries.len();
        self.snapshot = list.snapshot;
        self.boundary = list.entries.last().cloned();
        self.has_more = list.has_more;
        self.request = None;
    }

    pub fn poll(
        &mut self,
        backend: &Backend,
        shared: &Shared,
        context: &egui::Context,
        token: &CancelToken,
    ) -> bool {
        let result = if let Some(request) = &self.request {
            backend.reflog_page(request, token).map(Some)
        } else {
            backend.reflog_snapshot(token).and_then(|snapshot| {
                self.periodic_request(snapshot)
                    .map(|request| backend.reflog_page(&request, token))
                    .transpose()
            })
        };
        self.publish(shared, context, token, result)
    }

    pub(super) fn publish(
        &mut self,
        shared: &Shared,
        context: &egui::Context,
        token: &CancelToken,
        result: fastdiff_cli::Result<Option<ReflogList>>,
    ) -> bool {
        // A queued reset or close wins over a result from the previous prefix.
        let retired = if let Ok(pending) = shared.pending.lock()
            && !matches!(pending.reflog, Some(Command::Reflog { .. }))
            && let Ok(_active) = shared.active.lock()
            && !token.is_cancelled()
            && shared.foreground.load(Ordering::Acquire)
            && !shared.stopped.load(Ordering::Acquire)
            && let Some(generation) = self.generation
        {
            match result {
                Ok(Some(list)) => {
                    self.accept(&list);
                    queue_event(shared, context, Event::Reflog { generation, list })
                }
                Ok(None) => None,
                Err(error) => {
                    self.request = None;
                    queue_event(
                        shared,
                        context,
                        Event::ReflogError {
                            generation,
                            message: error.to_string(),
                        },
                    )
                }
            }
        } else {
            return false;
        };
        drop(retired);
        true
    }
}
