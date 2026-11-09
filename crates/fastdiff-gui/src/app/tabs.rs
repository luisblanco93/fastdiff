//! The repository tab strip: identity tabs, their close controls and the window-wide controls beside them.
use super::{Fastdiff, ReviewTab, theme_menu};
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
        let reveal_selected = reveal_selected(ui, self.selected_tab, self.tabs.len());
        let titles = tab_titles(&self.tabs);
        let suffixes = tab_suffixes(&titles);
        let mut close = None;
        // The tabs are created before the window-wide controls so keyboard and screen-reader
        // order reach them first. The controls sit at the right edge, so the tabs reserve the
        // width the controls had on the previous pass.
        let (_, row) = ui.allocate_space(egui::vec2(ui.available_width(), TAB_ROW_HEIGHT));
        let controls_width = ui
            .data(|data| data.get_temp::<f32>(controls_width_id(ui)))
            .unwrap_or(0.0);
        let gap = ui.spacing().item_spacing.x;
        let split = (row.max.x - controls_width).max(row.min.x);
        let tabs_rect = row.with_max_x((split - gap).max(row.min.x));
        let mut tabs_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(tabs_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        egui::ScrollArea::horizontal()
            .id_salt("repository-tabs")
            .auto_shrink([false, true])
            .show(&mut tabs_ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (index, tab) in self.tabs.iter().enumerate() {
                        let title = titles[index];
                        let selected = index == self.selected_tab;
                        let view = TabView {
                            title,
                            name: tab_name(title),
                            folder: suffixes[index].as_deref(),
                            selected,
                            is_document: tab.request.target == OpenTarget::Document,
                            reveal: selected && reveal_selected,
                        };
                        Self::tab(ui, view, |action| match action {
                            TabAction::Select => self.selected_tab = index,
                            TabAction::Close => close = Some(index),
                        });
                    }
                });
            });
        let mut controls_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(row.with_min_x(split))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        Self::global_controls(&mut controls_ui, &mut self.help_open, &mut self.appearance);
        let measured = controls_ui.min_rect().width();
        if (measured - controls_width).abs() > 0.5 {
            let id = controls_width_id(ui);
            ui.data_mut(|data| data.insert_temp(id, measured));
            ui.ctx().request_discard("global controls changed width");
        }
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
                let mut hovered = tab_icon(ui, selected, is_document, &mut act);
                ui.add_space(4.0);
                hovered |= tab_label(ui, title, name, selected, &mut act);
                if let Some(folder) = folder {
                    ui.add_space(2.0);
                    hovered |= tab_suffix(ui, folder, &mut act);
                }
                ui.add_space(TAB_CLOSE_GAP);
                close_control(ui, name, selected || hovered, &mut act);
            });
        if selected {
            selected_underline(ui, &frame.response, reveal);
        }
    }

    /// Window-wide controls: they apply to every tab, so they sit beside the tabs, not in a tab's toolbar.
    /// Added left to right in visual order, so keyboard and screen-reader order match what is seen.
    fn global_controls(
        ui: &mut egui::Ui,
        help_open: &mut bool,
        appearance: &mut theme_menu::Appearance,
    ) {
        let zoom = ui.ctx().zoom_factor();
        if ui
            .small_button("UI −")
            .on_hover_text("Scale the whole interface")
            .clicked()
        {
            ui.ctx().set_zoom_factor((zoom - 0.1).clamp(0.75, 2.0));
        }
        ui.small(format!("{:.0}%", zoom * 100.0));
        if ui
            .small_button("UI +")
            .on_hover_text("Scale the whole interface")
            .clicked()
        {
            ui.ctx().set_zoom_factor((zoom + 0.1).clamp(0.75, 2.0));
        }
        appearance.show_menu(ui);
        if ui.small_button("Help").clicked() {
            *help_open = true;
        }
    }
}

fn controls_width_id(ui: &egui::Ui) -> egui::Id {
    ui.id().with("global-controls-width")
}

/// Remembers the selection in egui's temp data and reports whether it changed since the last
/// frame, which is when the selected tab is scrolled into view.
fn reveal_selected(ui: &egui::Ui, selected_tab: usize, tab_count: usize) -> bool {
    let id = ui.id().with("selected-repository-tab");
    let selection = (selected_tab, tab_count);
    ui.data_mut(|data| {
        let previous = data.get_temp::<(usize, usize)>(id);
        data.insert_temp(id, selection);
        previous != Some(selection)
    })
}

/// The path each tab is named after: the file for a document tab, the repository root otherwise.
fn tab_titles(tabs: &[ReviewTab]) -> Vec<&str> {
    tabs.iter()
        .map(|tab| {
            if tab.request.target == OpenTarget::Document {
                tab.request.repository.as_str()
            } else {
                tab.root.as_str()
            }
        })
        .collect()
}

fn focus_ring(ui: &egui::Ui, response: &egui::Response) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            2.0,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Inside,
        );
    }
}

/// Width of `text` laid out on one line, for sizing a truncating widget to its content.
fn text_width(ui: &egui::Ui, text: &str, style: egui::TextStyle, color: egui::Color32) -> f32 {
    let font = style.resolve(ui.style());
    ui.painter()
        .layout_no_wrap(text.to_owned(), font, color)
        .size()
        .x
}

/// The tab's icon; returns whether the pointer is over it.
fn tab_icon(
    ui: &mut egui::Ui,
    selected: bool,
    is_document: bool,
    act: &mut impl FnMut(TabAction),
) -> bool {
    // Allocate the full label height so the icon centers on the name. CLICK without
    // FOCUSABLE: the name button is the tab's keyboard stop, not the icon.
    let (slot, response) =
        ui.allocate_exact_size(egui::vec2(TAB_ICON_SIZE, 24.0), egui::Sense::CLICK);
    if response.clicked() {
        act(TabAction::Select);
    }
    let icon = egui::Rect::from_center_size(slot.center(), egui::Vec2::splat(TAB_ICON_SIZE));
    let color = if selected {
        crate::theme::active(ui.ctx()).app_theme().accent()
    } else {
        ui.visuals().weak_text_color()
    };
    if is_document {
        crate::sidebar::document_icon(ui, icon, color);
    } else {
        crate::sidebar::folder_icon(ui, icon, false, color);
    }
    response.hovered()
}

/// The name button, the tab's keyboard stop; returns whether the pointer is over it.
fn tab_label(
    ui: &mut egui::Ui,
    title: &str,
    name: &str,
    selected: bool,
    act: &mut impl FnMut(TabAction),
) -> bool {
    let width =
        text_width(ui, name, egui::TextStyle::Button, ui.visuals().text_color()).clamp(48.0, 180.0);
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
    focus_ring(ui, &response);
    let hovered = response.hovered();
    if response.on_hover_text(title).clicked() {
        act(TabAction::Select);
    }
    hovered
}

/// The parent-folder suffix that tells same-named tabs apart; returns whether the pointer is over it.
fn tab_suffix(ui: &mut egui::Ui, folder: &str, act: &mut impl FnMut(TabAction)) -> bool {
    let width = text_width(
        ui,
        folder,
        egui::TextStyle::Small,
        ui.visuals().weak_text_color(),
    )
    .min(TAB_SUFFIX_MAX_WIDTH);
    let suffix = ui.add_sized(
        [width, 24.0],
        egui::Label::new(egui::RichText::new(folder).small().weak())
            .selectable(false)
            .truncate()
            .sense(egui::Sense::CLICK),
    );
    if suffix.clicked() {
        act(TabAction::Select);
    }
    suffix.hovered()
}

/// The ✕ button, painted by hand because egui's default fonts have no such glyph.
/// `emphasized` is true while the tab is selected or hovered.
fn close_control(ui: &mut egui::Ui, name: &str, emphasized: bool, act: &mut impl FnMut(TabAction)) {
    let response = ui.add_sized([24.0, 24.0], egui::Button::new("").frame(false));
    let close_label = format!("Close {name} tab");
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &close_label)
    });
    focus_ring(ui, &response);
    let center = response.rect.center();
    let color = if response.hovered() {
        ui.visuals().strong_text_color()
    } else if emphasized {
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
}

fn selected_underline(ui: &egui::Ui, frame: &egui::Response, reveal: bool) {
    if reveal {
        frame.scroll_to_me(Some(egui::Align::Center));
    }
    ui.painter().line_segment(
        [frame.rect.left_bottom(), frame.rect.right_bottom()],
        egui::Stroke::new(2.0, crate::theme::active(ui.ctx()).app_theme().accent()),
    );
}
