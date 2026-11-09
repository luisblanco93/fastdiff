//! The repository tab strip: identity tabs, their close controls and the window-wide controls beside them.
use super::{Fastdiff, theme_menu};
use fastdiff_cli::OpenTarget;

const TAB_ROW_HEIGHT: f32 = 32.0;
const TAB_ICON_SIZE: f32 = 11.0;
/// Space between the tab name and its close control; with the control's own padding the ✕ sits
/// as far from the name as from the tab's right edge.
const TAB_CLOSE_GAP: f32 = 8.0;
/// A folder suffix wider than this is truncated so it cannot push the close control away.
const TAB_SUFFIX_MAX_WIDTH: f32 = 140.0;

struct TabView<'a> {
    title: &'a str,
    name: &'a str,
    folder: Option<&'a str>,
    selected: bool,
    is_document: bool,
    reveal: bool,
}

enum TabAction {
    Select,
    Close,
}

pub(super) fn tab_name(title: &str) -> &str {
    title
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(title)
}

fn parent_segments(title: &str) -> Vec<&str> {
    let mut segments: Vec<&str> = title
        .trim_end_matches(['/', '\\'])
        .split(['/', '\\'])
        .filter(|segment| !segment.is_empty())
        .collect();
    segments.pop();
    segments
}

fn parent_tail(parents: &[&str], depth: usize) -> Option<String> {
    let tail = &parents[parents.len().saturating_sub(depth)..];
    (!tail.is_empty()).then(|| tail.join("/"))
}

/// For tabs that share a name, the shortest run of parent folders that tells them apart.
pub(super) fn tab_suffixes(titles: &[&str]) -> Vec<Option<String>> {
    titles
        .iter()
        .enumerate()
        .map(|(index, title)| {
            let name = tab_name(title);
            let rivals: Vec<Vec<&str>> = titles
                .iter()
                .enumerate()
                .filter(|&(other, other_title)| other != index && tab_name(other_title) == name)
                .map(|(_, other_title)| parent_segments(other_title))
                .collect();
            if rivals.is_empty() {
                return None;
            }
            let own = parent_segments(title);
            let longest = rivals.iter().map(Vec::len).fold(own.len(), usize::max);
            let depth = (1..=longest.max(1))
                .find(|&depth| {
                    rivals
                        .iter()
                        .all(|rival| parent_tail(rival, depth) != parent_tail(&own, depth))
                })
                .unwrap_or(longest);
            parent_tail(&own, depth)
        })
        .collect()
}

impl Fastdiff {
    pub(super) fn tab_bar(&mut self, ui: &mut egui::Ui) {
        let selection_id = ui.id().with("selected-repository-tab");
        let selection = (self.selected_tab, self.tabs.len());
        let reveal_selected = ui.data_mut(|data| {
            let previous = data.get_temp::<(usize, usize)>(selection_id);
            data.insert_temp(selection_id, selection);
            previous != Some(selection)
        });
        let titles: Vec<&str> = self
            .tabs
            .iter()
            .map(|tab| {
                if tab.request.target == OpenTarget::Document {
                    tab.request.repository.as_str()
                } else {
                    tab.root.as_str()
                }
            })
            .collect();
        let suffixes = tab_suffixes(&titles);
        let mut close = None;
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), TAB_ROW_HEIGHT),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                Self::global_controls(ui, &mut self.help_open, &mut self.appearance);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    egui::ScrollArea::horizontal()
                        .id_salt("repository-tabs")
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                for (index, tab) in self.tabs.iter().enumerate() {
                                    let title = titles[index];
                                    let name = tab_name(title);
                                    let folder = suffixes[index].as_deref();
                                    let selected = index == self.selected_tab;
                                    let is_document = tab.request.target == OpenTarget::Document;
                                    Self::tab(
                                        ui,
                                        TabView {
                                            title,
                                            name,
                                            folder,
                                            selected,
                                            is_document,
                                            reveal: selected && reveal_selected,
                                        },
                                        |action| match action {
                                            TabAction::Select => self.selected_tab = index,
                                            TabAction::Close => close = Some(index),
                                        },
                                    );
                                }
                            });
                        });
                });
            },
        );
        ui.separator();
        if let Some(index) = close {
            self.close_tab(index);
        } else {
            self.sync_tab_activity();
        }
    }

    fn tab(ui: &mut egui::Ui, view: TabView, mut act: impl FnMut(TabAction)) {
        let TabView {
            title,
            name,
            folder,
            selected,
            is_document,
            reveal,
        } = view;
        let fill = if selected {
            ui.visuals().widgets.inactive.weak_bg_fill
        } else {
            egui::Color32::TRANSPARENT
        };
        let frame = egui::Frame::new()
            .fill(fill)
            .corner_radius(egui::CornerRadius {
                nw: 4,
                ne: 4,
                sw: 0,
                se: 0,
            })
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                // Allocate the full label height so the icon centers on the name. CLICK without
                // FOCUSABLE: the name button is the tab's keyboard stop, not the icon.
                let (slot, icon_response) =
                    ui.allocate_exact_size(egui::vec2(TAB_ICON_SIZE, 24.0), egui::Sense::CLICK);
                let icon_hovered = icon_response.hovered();
                if icon_response.clicked() {
                    act(TabAction::Select);
                }
                let icon =
                    egui::Rect::from_center_size(slot.center(), egui::Vec2::splat(TAB_ICON_SIZE));
                let icon_color = if selected {
                    crate::theme::active(ui.ctx()).app_theme().accent()
                } else {
                    ui.visuals().weak_text_color()
                };
                if is_document {
                    crate::sidebar::document_icon(ui, icon, icon_color);
                } else {
                    crate::sidebar::folder_icon(ui, icon, false, icon_color);
                }
                ui.add_space(4.0);
                let font = egui::TextStyle::Button.resolve(ui.style());
                let width = ui
                    .painter()
                    .layout_no_wrap(name.to_owned(), font, ui.visuals().text_color())
                    .size()
                    .x
                    .clamp(48.0, 180.0);
                let label = egui::RichText::new(name).color(if selected {
                    ui.visuals().strong_text_color()
                } else {
                    ui.visuals().text_color()
                });
                let response = ui.add_sized(
                    [width, 24.0],
                    egui::Button::new(label).frame(false).truncate(),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::SelectableLabel,
                        ui.is_enabled(),
                        selected,
                        name,
                    )
                });
                ui.ctx().accesskit_node_builder(response.id, |node| {
                    node.set_role(egui::accesskit::Role::Tab);
                    node.clear_toggled();
                    node.set_selected(selected);
                });
                if response.has_focus() {
                    ui.painter().rect_stroke(
                        response.rect,
                        2.0,
                        ui.visuals().selection.stroke,
                        egui::StrokeKind::Inside,
                    );
                }
                let mut tab_hovered = icon_hovered || response.hovered();
                if response.on_hover_text(title).clicked() {
                    act(TabAction::Select);
                }
                if let Some(folder) = folder {
                    ui.add_space(2.0);
                    let font = egui::TextStyle::Small.resolve(ui.style());
                    let width = ui
                        .painter()
                        .layout_no_wrap(folder.to_owned(), font, ui.visuals().weak_text_color())
                        .size()
                        .x
                        .min(TAB_SUFFIX_MAX_WIDTH);
                    let suffix = ui.add_sized(
                        [width, 24.0],
                        egui::Label::new(egui::RichText::new(folder).small().weak())
                            .selectable(false)
                            .truncate()
                            .sense(egui::Sense::CLICK),
                    );
                    tab_hovered |= suffix.hovered();
                    if suffix.clicked() {
                        act(TabAction::Select);
                    }
                }
                ui.add_space(TAB_CLOSE_GAP);
                let response = ui.add_sized([24.0, 24.0], egui::Button::new("").frame(false));
                let close_label = format!("Close {name} tab");
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        ui.is_enabled(),
                        &close_label,
                    )
                });
                if response.has_focus() {
                    ui.painter().rect_stroke(
                        response.rect,
                        2.0,
                        ui.visuals().selection.stroke,
                        egui::StrokeKind::Inside,
                    );
                }
                let center = response.rect.center();
                let color = if response.hovered() {
                    ui.visuals().strong_text_color()
                } else if selected || tab_hovered {
                    ui.visuals().text_color()
                } else {
                    ui.visuals().weak_text_color()
                };
                let stroke = egui::Stroke::new(1.2, color);
                for (a, b) in [((-3.0, -3.0), (3.0, 3.0)), ((-3.0, 3.0), (3.0, -3.0))] {
                    ui.painter().line_segment(
                        [center + egui::vec2(a.0, a.1), center + egui::vec2(b.0, b.1)],
                        stroke,
                    );
                }
                if response.on_hover_text(close_label).clicked() {
                    act(TabAction::Close);
                }
            });
        if selected {
            if reveal {
                frame.response.scroll_to_me(Some(egui::Align::Center));
            }
            ui.painter().line_segment(
                [
                    frame.response.rect.left_bottom(),
                    frame.response.rect.right_bottom(),
                ],
                egui::Stroke::new(2.0, crate::theme::active(ui.ctx()).app_theme().accent()),
            );
        }
    }

    /// Window-wide controls: they apply to every tab, so they sit beside the tabs, not in a tab's toolbar.
    /// Called inside a right-to-left layout, so the widgets are added in reverse visual order.
    fn global_controls(
        ui: &mut egui::Ui,
        help_open: &mut bool,
        appearance: &mut theme_menu::Appearance,
    ) {
        if ui.small_button("Help").clicked() {
            *help_open = true;
        }
        appearance.show_menu(ui);
        let zoom = ui.ctx().zoom_factor();
        if ui
            .small_button("UI +")
            .on_hover_text("Scale the whole interface")
            .clicked()
        {
            ui.ctx().set_zoom_factor((zoom + 0.1).clamp(0.75, 2.0));
        }
        ui.small(format!("{:.0}%", zoom * 100.0));
        if ui
            .small_button("UI −")
            .on_hover_text("Scale the whole interface")
            .clicked()
        {
            ui.ctx().set_zoom_factor((zoom - 0.1).clamp(0.75, 2.0));
        }
    }
}
