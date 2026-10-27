use super::ReviewTab;
use fastdiff_core::{ReflogList, ReflogRequest};

impl ReviewTab {
    pub(super) fn toggle_reflog(&mut self) {
        if !self.has_repository {
            return;
        }
        self.reflog_open = !self.reflog_open;
        self.refresh_reflog();
    }

    pub(super) fn refresh_reflog(&mut self) {
        self.reflog_generation += 1;
        self.reflog_loading = self.reflog_open;
        self.reflog_error = None;
        self.reflog_pending = None;
        self.reflog_scroll_reset = self.reflog_open;
        if let Some(worker) = &self.worker {
            worker.reflog(self.reflog_generation, self.reflog_open);
        }
    }

    pub(super) fn load_more_reflog(&mut self) {
        if !self.reflog_open || !self.foreground || self.reflog_loading {
            return;
        }
        let Some(list) = &self.reflog else {
            return;
        };
        if !list.has_more || list.entries.is_empty() {
            return;
        }
        let request = ReflogRequest {
            start: list.entries.len(),
            snapshot: list.snapshot,
            boundary: list.entries.last().cloned(),
        };
        self.reflog_loading = true;
        self.reflog_error = None;
        self.reflog_pending = Some(request.clone());
        if let Some(worker) = &self.worker {
            worker.reflog_more(self.reflog_generation, request);
        }
    }

    pub(super) fn accept_reflog(&mut self, list: ReflogList) -> bool {
        if list.start == 0 {
            // Fallback layouts replace their loaded prefix; ordinary resets return to row zero.
            self.reflog_scroll_reset |= list.snapshot.is_some()
                || self
                    .reflog
                    .as_ref()
                    .is_none_or(|current| current.snapshot.is_some());
            self.reflog = Some(list);
        } else {
            let Some(current) = &mut self.reflog else {
                return false;
            };
            let Some(request) = &self.reflog_pending else {
                return false;
            };
            if request.start != list.start
                || current.entries.len() != list.start
                || request.snapshot != list.snapshot
                || current.snapshot != list.snapshot
                || request.boundary.as_ref() != current.entries.last()
            {
                return false;
            }
            current.entries.extend(list.entries);
            current.branch = list.branch;
            current.notice = list.notice;
            current.has_more = list.has_more;
        }
        self.branch = self.reflog.as_ref().map(|list| {
            list.branch
                .clone()
                .unwrap_or_else(|| "Detached HEAD".into())
        });
        self.reflog_loading = false;
        self.reflog_error = None;
        self.reflog_pending = None;
        true
    }

    pub(super) fn show_reflog(&mut self, ui: &mut egui::Ui, tab_index: usize, theme: bool) {
        if let Some(error) = &self.reflog_error {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("{error}\nRefresh with F5 to retry."))
                        .color(ui.visuals().error_fg_color),
                )
                .truncate(),
            );
            if ui.button("Retry reflog").clicked() {
                if self.reflog_pending.is_some() {
                    self.load_more_reflog();
                } else {
                    self.refresh_reflog();
                }
            }
        }
        let Some(list) = &self.reflog else {
            if self.reflog_loading {
                ui.centered_and_justified(|ui| ui.label("Loading reflog…"));
            }
            return;
        };
        if let Some(notice) = &list.notice {
            ui.add(egui::Label::new(notice).truncate());
        }
        let status = &crate::theme::active(ui.ctx()).app_theme().status;
        let id_color = if theme {
            status.reflog_id
        } else {
            ui.visuals().text_color()
        };
        let branch_color = if theme {
            status.reflog_branch
        } else {
            ui.visuals().text_color()
        };
        let width = ui.available_width();
        let id_width = 68.0_f32.min(width * 0.25);
        let branch_width = 160.0_f32.min(width * 0.3);
        let mut near_end = false;
        let scroll = egui::ScrollArea::vertical()
            .id_salt(("reflog", tab_index))
            .auto_shrink([false, false]);
        let scroll = if self.reflog_scroll_reset {
            self.reflog_scroll_reset = false;
            scroll.vertical_scroll_offset(0.0)
        } else {
            scroll
        };
        scroll.show_rows(ui, 22.0, list.entries.len(), |ui, rows| {
            near_end = rows.end > list.entries.len().saturating_sub(3);
            for index in rows {
                let entry = &list.entries[index];
                ui.horizontal(|ui| {
                    reflog_cell(
                        ui,
                        [id_width, 22.0],
                        egui::Label::new(
                            egui::RichText::new(&entry.short_id)
                                .monospace()
                                .color(id_color),
                        )
                        .truncate(),
                    );
                    let decorations = if entry.decorations.is_empty() {
                        String::new()
                    } else {
                        format!("({})", entry.decorations)
                    };
                    reflog_cell(
                        ui,
                        [branch_width, 22.0],
                        egui::Label::new(egui::RichText::new(decorations).color(branch_color))
                            .truncate(),
                    );
                    reflog_cell(
                        ui,
                        [ui.available_width().max(1.0), 22.0],
                        egui::Label::new(format!("{}: {}", entry.selector, entry.action))
                            .truncate(),
                    );
                });
            }
        });
        if near_end && self.reflog_error.is_none() {
            self.load_more_reflog();
        }
    }
}

fn reflog_cell(ui: &mut egui::Ui, size: [f32; 2], label: egui::Label) {
    ui.allocate_ui_with_layout(
        size.into(),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_size(size.into());
            ui.add(label);
        },
    );
}
