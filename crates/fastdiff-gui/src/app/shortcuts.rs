use super::{Fastdiff, ReviewTab, RowKind, View};
use egui::{Event, Key, Modifiers};

const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+F", "Search document / diff"),
    ("Ctrl+Alt+F", "Filter filenames"),
    ("F3 / Shift+F3", "Next / previous match"),
    ("Enter / Shift+Enter in search", "Next / previous match"),
    ("Ctrl+C", "Copy selection in active pane"),
    ("Ctrl+A", "Select all in active pane"),
    (
        "Up / Down / Left / Right",
        "Scroll active document / diff pane",
    ),
    ("F1 / Escape", "Open / close Help"),
    ("F5", "Refresh"),
    ("Ctrl+5", "Expand / collapse Reflog panel"),
    ("Alt+Up / Alt+Down", "Previous / next file"),
    ("Ctrl+Up / Ctrl+Down", "Previous / next hunk"),
    ("Ctrl+- / Ctrl++", "Smaller / larger text"),
    (
        "Ctrl+1 / Ctrl+2 / Ctrl+3 / Ctrl+4",
        "All / Unstaged / Staged / Unpushed",
    ),
    ("Ctrl+Shift+U", "Toggle unified layout"),
    ("Ctrl+Shift+K", "Collapse context"),
    ("Ctrl+Shift+C", "Copy path"),
    ("Ctrl+Alt+C", "Copy patch"),
    ("Ctrl+Alt+Shift+C", "Copy comparison source"),
    (
        "Ctrl+Alt+Left / Ctrl+Alt+Right",
        "Previous / next copy comparison",
    ),
];

// consume_key uses logical matching, which permits extra Alt/Shift modifiers.
// Exact matching keeps copy/search variants and text editing independent.
fn consume(context: &egui::Context, modifiers: Modifiers, key: Key) -> bool {
    context.input_mut(|input| {
        let before = input.events.len();
        let actual_modifiers = input
            .events
            .iter()
            .find_map(|event| match event {
                Event::Key {
                    key: Key::C,
                    pressed: false,
                    modifiers,
                    ..
                } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or(input.modifiers);
        input.events.retain(|event| {
            // Native egui-winit translates every Ctrl+C variant to Copy.
            !((key == Key::C
                && matches!(event, Event::Copy)
                && actual_modifiers.matches_exact(modifiers))
                || matches!(event,
                    Event::Key { key: pressed, pressed: true, modifiers: actual, .. }
                        if *pressed == key && actual.matches_exact(modifiers)
                ))
        });
        input.events.len() != before
    })
}

impl Fastdiff {
    pub(super) fn shortcuts(&mut self, context: &egui::Context) {
        let ctrl = Modifiers::CTRL;
        let ctrl_alt = ctrl | Modifiers::ALT;
        let ctrl_shift = ctrl | Modifiers::SHIFT;
        if consume(context, Modifiers::NONE, Key::F1) {
            self.help_open = true;
        }
        if self.help_open {
            if consume(context, Modifiers::NONE, Key::Escape) {
                self.help_open = false;
            }
            return;
        }
        let Some(tab) = self.tabs.get_mut(self.selected_tab) else {
            return;
        };
        if consume(context, ctrl, Key::Num5) {
            tab.toggle_reflog();
        }
        if consume(context, Modifiers::NONE, Key::F5) {
            tab.refresh();
        }
        let text_focus = context.memory(|memory| {
            memory.focused().is_some_and(|id| {
                id == egui::Id::new("filename-filter") || id == egui::Id::new("diff-search")
            })
        });
        if !text_focus {
            for (key, view) in [
                (Key::Num1, View::All),
                (Key::Num2, View::Unstaged),
                (Key::Num3, View::Staged),
                (Key::Num4, View::Unpushed),
            ] {
                if consume(context, ctrl, key) {
                    tab.set_view(view);
                }
            }
        }
        if consume(context, ctrl_alt, Key::F) {
            context.memory_mut(|memory| memory.request_focus(egui::Id::new("filename-filter")));
        } else if consume(context, ctrl, Key::F) {
            context.memory_mut(|memory| memory.request_focus(egui::Id::new("diff-search")));
        }
        if consume(context, Modifiers::SHIFT, Key::F3) {
            tab.navigate_search(false);
        } else if consume(context, Modifiers::NONE, Key::F3) {
            tab.navigate_search(true);
        }
        if text_focus {
            return;
        }
        for (key, next) in [(Key::ArrowUp, false), (Key::ArrowDown, true)] {
            if consume(context, Modifiers::ALT, key) {
                tab.navigate_file(next);
            }
            if consume(context, ctrl, key)
                && let Some(presentation) = &mut tab.presentation
            {
                presentation.navigate_hunk(next, self.preferences.font_size + 5.0);
            }
        }
        if consume(context, ctrl, Key::Minus) {
            self.preferences.font_size = (self.preferences.font_size - 1.0).max(9.0);
        }
        if consume(context, ctrl, Key::Plus)
            || consume(context, ctrl_shift, Key::Plus)
            || consume(context, ctrl, Key::Equals)
            || consume(context, ctrl_shift, Key::Equals)
        {
            self.preferences.font_size = (self.preferences.font_size + 1.0).min(28.0);
        }
        if consume(context, ctrl_shift, Key::U) {
            self.preferences.unified = !self.preferences.unified;
        }
        if consume(context, ctrl_shift, Key::K)
            && let Some(presentation) = &mut tab.presentation
        {
            presentation.expanded.clear();
            presentation.rebuild();
        }
        if consume(context, ctrl_shift, Key::C)
            && let Some(path) = &tab.selected
        {
            context.copy_text(path.display());
        }
        let Some(presentation) = &mut tab.presentation else {
            return;
        };
        let count = presentation.document.document.comparisons.len();
        for (key, next) in [(Key::ArrowLeft, false), (Key::ArrowRight, true)] {
            if consume(context, ctrl_alt, key) && count > 0 {
                tab.comparison = if next {
                    (tab.comparison + 1) % count
                } else {
                    (tab.comparison + count - 1) % count
                };
                presentation.clear_selection();
            }
        }
        if presentation
            .document
            .document
            .comparisons
            .get(tab.comparison)
            .is_some()
        {
            if consume(context, ctrl_alt, Key::C) {
                tab.copy_comparison(context, false);
            }
            if consume(context, ctrl_alt | Modifiers::SHIFT, Key::C) {
                tab.copy_comparison(context, true);
            }
        }
    }

    pub(super) fn help(&mut self, context: &egui::Context) {
        if !self.help_open {
            return;
        }
        let comparison = self
            .tabs
            .get(self.selected_tab)
            .and_then(|tab| {
                tab.presentation.as_ref().and_then(|presentation| {
                    presentation
                        .document
                        .document
                        .comparisons
                        .get(tab.comparison)
                })
            })
            .map(|comparison| comparison.label.as_str())
            .unwrap_or("None");
        egui::Window::new("Keyboard shortcuts")
            .open(&mut self.help_open)
            .default_width(540.0)
            .max_width((context.content_rect().width() - 32.0).max(1.0))
            .show(context, |ui| {
                ui.label(format!("Copy comparison: {comparison}"));
                ui.label("Text fields retain their normal editing shortcuts.");
                egui::ScrollArea::both()
                    .max_height((context.content_rect().height() - 150.0).max(100.0))
                    .show(ui, |ui| {
                        egui::Grid::new("shortcut-reference")
                            .striped(true)
                            .show(ui, |ui| {
                                for (binding, action) in SHORTCUTS {
                                    ui.monospace(*binding);
                                    ui.label(*action);
                                    ui.end_row();
                                }
                            });
                    });
            });
    }
}

impl ReviewTab {
    pub(super) fn copy_comparison(&self, context: &egui::Context, source: bool) {
        let Some(comparison) = self.presentation.as_ref().and_then(|presentation| {
            presentation
                .document
                .document
                .comparisons
                .get(self.comparison)
        }) else {
            return;
        };
        if !source {
            context.copy_text(comparison.patch.clone());
            return;
        }
        let has_new_source = comparison.rows.iter().any(|row| row.new_number.is_some());
        let source = comparison
            .rows
            .iter()
            .filter(|row| {
                row.kind != RowKind::Header
                    && if has_new_source {
                        row.new_number.is_some()
                    } else {
                        row.old_number.is_some()
                    }
            })
            .map(|row| {
                if has_new_source {
                    row.right.as_str()
                } else {
                    row.left.as_str()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        context.copy_text(source);
    }
}
