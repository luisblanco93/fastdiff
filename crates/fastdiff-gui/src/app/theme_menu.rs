//! Appearance state of the window: the saved settings, the session override, the hover preview
//! and the Theme menu that edits them.
use super::Fastdiff;
use crate::{
    settings::{Settings, SettingsStore, clamp_zoom},
    theme::{Active, Appearance as ThemeAppearance, Theme, ThemeId},
};
use egui::{
    Align2, Color32, FontId, Id, Popup, PopupCloseBehavior, Rect, Sense, SetOpenCommand, Stroke,
    Ui, WidgetInfo, WidgetType, pos2, vec2,
};

const MENU_ID: &str = "theme-menu";
const MENU_WIDTH: f32 = 300.0;
const ROW_HEIGHT: f32 = 26.0;
const SWATCH: f32 = 10.0;
const SWATCH_GAP: f32 = 3.0;

/// Window title bar override and the themes to paint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Resolved {
    pub(super) app: ThemeId,
    pub(super) code: ThemeId,
    pub(super) title_bar: egui::SystemTheme,
}

/// Picks the themes in force: a hover preview wins over the `--color-theme` override, which
/// wins over the system-following slot, which wins over the saved theme.
pub(super) fn resolve(
    settings: &Settings,
    system: Option<egui::Theme>,
    session: Option<ThemeId>,
    preview: Option<ThemeId>,
) -> Resolved {
    let following = settings.follow_system && session.is_none() && preview.is_none();
    let app = preview.or(session).unwrap_or(if settings.follow_system {
        match system {
            Some(egui::Theme::Light) => settings.light_theme,
            _ => settings.dark_theme,
        }
    } else {
        settings.theme
    });
    let code = if settings.code_follows_app {
        app
    } else {
        settings.code_theme
    };
    // The title bar sits outside egui's visuals: it follows the system only while the system
    // actually decides the theme, and is pinned to the theme's appearance otherwise.
    let title_bar = if following && system.is_some() {
        egui::SystemTheme::SystemDefault
    } else {
        match app.theme().appearance {
            ThemeAppearance::Dark => egui::SystemTheme::Dark,
            ThemeAppearance::Light => egui::SystemTheme::Light,
        }
    };
    Resolved {
        app,
        code,
        title_bar,
    }
}

#[derive(Default)]
pub(super) struct Appearance {
    pub(super) store: SettingsStore,
    /// `--color-theme`: lasts for the window and is never saved.
    pub(super) session: Option<ThemeId>,
    /// Theme under the pointer or keyboard focus in the open menu.
    pub(super) preview: Option<ThemeId>,
    /// Last painted themes and the system theme they were chosen under.
    applied: Option<(Resolved, Option<egui::Theme>)>,
    /// Set by the Ctrl+K Ctrl+T chord; opens the menu on the next frame.
    pub(super) menu_requested: bool,
    /// When Ctrl+K was pressed, while waiting for the second key of the chord.
    pub(super) chord_started: Option<f64>,
}

impl Appearance {
    pub(super) fn new(store: SettingsStore, session: Option<ThemeId>) -> Self {
        Self {
            store,
            session,
            ..Self::default()
        }
    }

    /// Paints the resolved themes. Does nothing while the themes and the system theme are
    /// unchanged; the system theme is part of the key so the Windows title bar is pinned again
    /// after winit resets it.
    pub(super) fn apply(&mut self, context: &egui::Context) {
        let system = context.system_theme();
        let resolved = resolve(self.store.settings(), system, self.session, self.preview);
        if self.applied == Some((resolved, system)) {
            return;
        }
        let theme = resolved.app.theme();
        let mode = match theme.appearance {
            ThemeAppearance::Dark => egui::Theme::Dark,
            ThemeAppearance::Light => egui::Theme::Light,
        };
        context.set_theme(mode);
        context.set_visuals_of(mode, theme.visuals());
        context.send_viewport_cmd(egui::ViewportCommand::SetTheme(resolved.title_bar));
        crate::theme::install(
            context,
            Active {
                app: resolved.app,
                code: resolved.code,
            },
        );
        self.applied = Some((resolved, system));
    }

    fn update(&mut self, change: impl FnOnce(&mut Settings)) {
        let mut settings = self.store.settings().clone();
        change(&mut settings);
        self.store.save(settings);
    }

    /// A click on a theme: save it, stop following the system and drop any override.
    fn choose(&mut self, id: ThemeId) {
        self.session = None;
        self.preview = None;
        self.update(|settings| {
            settings.theme = id;
            settings.follow_system = false;
        });
    }

    /// The Theme button and its menu. Called inside the right-to-left global controls.
    pub(super) fn show_menu(&mut self, ui: &mut Ui) {
        // The default fonts have the down-arrow glyph (but not the usual small triangle).
        let response = ui.add(egui::Button::new("Theme").right_text("⏷").small());
        let response = response.on_hover_text("Color theme (Ctrl+K Ctrl+T)");
        let menu_id = Id::new(MENU_ID);
        let mut popup = Popup::menu(&response)
            .id(menu_id)
            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
            .width(MENU_WIDTH);
        if std::mem::take(&mut self.menu_requested) {
            popup = popup.open_memory(SetOpenCommand::Bool(true));
        }
        let mut hovered = None;
        popup.show(|ui| self.menu_content(ui, &mut hovered));
        let context = ui.ctx();
        let open = Popup::is_id_open(context, menu_id);
        let next = if open {
            // The preview is sticky between rows: only another row replaces it.
            hovered.or(self.preview)
        } else {
            None
        };
        if next != self.preview {
            self.preview = next;
            context.request_repaint();
        }
    }

    fn menu_content(&mut self, ui: &mut Ui, hovered: &mut Option<ThemeId>) {
        ui.set_min_width(MENU_WIDTH - 16.0);
        let settings = self.store.settings().clone();
        let current = resolve(&settings, ui.ctx().system_theme(), self.session, None).app;
        let mut pointer_seen = false;
        for (heading, appearance) in [
            ("Dark", ThemeAppearance::Dark),
            ("Light", ThemeAppearance::Light),
        ] {
            ui.label(egui::RichText::new(heading).small().weak());
            for id in ThemeId::ALL {
                if id.theme().appearance != appearance {
                    continue;
                }
                let row = theme_row(ui, id.theme(), id == current);
                // The row under the pointer wins over a row that merely holds keyboard focus.
                if row.pointer_over || (row.hovered && !pointer_seen) {
                    *hovered = Some(id);
                    pointer_seen |= row.pointer_over;
                }
                if row.clicked {
                    self.choose(id);
                    ui.close();
                }
            }
        }
        ui.separator();
        let system_known = ui.ctx().system_theme().is_some();
        let mut follow = settings.follow_system;
        let response = ui
            .add_enabled(
                system_known,
                egui::Checkbox::new(&mut follow, "Follow system theme"),
            )
            .on_disabled_hover_text("This platform does not report the system theme");
        if response.changed() {
            self.update(|settings| settings.follow_system = follow);
        }
        if settings.follow_system {
            self.slot_menu(ui, "Dark", settings.dark_theme, ThemeAppearance::Dark);
            self.slot_menu(ui, "Light", settings.light_theme, ThemeAppearance::Light);
        }
        let mut same = settings.code_follows_app;
        if ui
            .checkbox(&mut same, "Code viewer same as app theme")
            .changed()
        {
            self.update(|settings| settings.code_follows_app = same);
        }
        if !settings.code_follows_app {
            self.slot_menu_code(ui, settings.code_theme);
        }
    }

    /// "Dark: <name>" / "Light: <name>": the theme used while the system is in that mode.
    fn slot_menu(
        &mut self,
        ui: &mut Ui,
        label: &str,
        chosen: ThemeId,
        appearance: ThemeAppearance,
    ) {
        let title = format!("{label}: {}", chosen.theme().name);
        ui.menu_button(title, |ui| {
            for id in ThemeId::ALL {
                if id.theme().appearance != appearance {
                    continue;
                }
                if theme_row(ui, id.theme(), id == chosen).clicked {
                    self.update(|settings| match appearance {
                        ThemeAppearance::Dark => settings.dark_theme = id,
                        ThemeAppearance::Light => settings.light_theme = id,
                    });
                    ui.close();
                }
            }
        });
    }

    fn slot_menu_code(&mut self, ui: &mut Ui, chosen: ThemeId) {
        let title = format!("Code viewer: {}", chosen.theme().name);
        ui.menu_button(title, |ui| {
            for id in ThemeId::ALL {
                if theme_row(ui, id.theme(), id == chosen).clicked {
                    self.update(|settings| settings.code_theme = id);
                    ui.close();
                }
            }
        });
    }
}

struct RowResult {
    /// Under the pointer or holding keyboard focus.
    hovered: bool,
    pointer_over: bool,
    clicked: bool,
}

/// One theme: a checkmark for the current one, its name and six color swatches. `✓` is missing
/// from egui's default fonts; the heavy `✔` is there.
fn theme_row(ui: &mut Ui, theme: &Theme, selected: bool) -> RowResult {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("theme-row", theme.id.key())),
        Sense::click(),
    );
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, true, selected, theme.name));
    let active = response.hovered() || response.has_focus();
    let pointer_over = response.hovered();
    let visuals = ui.style().interact_selectable(&response, selected);
    if active {
        ui.painter().rect_filled(
            rect,
            visuals.corner_radius,
            ui.visuals().widgets.hovered.weak_bg_fill,
        );
    }
    let text_color = ui.visuals().widgets.noninteractive.fg_stroke.color;
    let text_color = if active {
        visuals.text_color()
    } else {
        text_color
    };
    if selected {
        ui.painter().text(
            pos2(rect.left() + 10.0, rect.center().y),
            Align2::CENTER_CENTER,
            "✔",
            FontId::proportional(13.0),
            text_color,
        );
    }
    ui.painter().text(
        pos2(rect.left() + 26.0, rect.center().y),
        Align2::LEFT_CENTER,
        theme.name,
        FontId::proportional(14.0),
        text_color,
    );
    let swatches = [
        theme.code.surface,
        theme.code.addition,
        theme.code.deletion,
        theme.code.syntax.keyword,
        theme.code.syntax.string,
        theme.code.syntax.function,
    ];
    let outline = Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
    let swatches_width = swatches.len() as f32 * (SWATCH + SWATCH_GAP) - SWATCH_GAP;
    let mut x = rect.right() - 8.0 - swatches_width;
    for color in swatches {
        swatch(
            ui,
            Rect::from_min_size(
                pos2(x, rect.center().y - SWATCH / 2.0),
                vec2(SWATCH, SWATCH),
            ),
            color,
            outline,
        );
        x += SWATCH + SWATCH_GAP;
    }
    RowResult {
        hovered: active,
        pointer_over,
        clicked: response.clicked(),
    }
}

fn swatch(ui: &Ui, rect: Rect, color: Color32, outline: Stroke) {
    ui.painter().rect_filled(rect, 2.0, color);
    ui.painter()
        .rect_stroke(rect, 2.0, outline, egui::StrokeKind::Inside);
}

impl Fastdiff {
    /// Saves the UI zoom whenever it changes, including egui's own keyboard zoom.
    pub(super) fn sync_zoom(&mut self, context: &egui::Context) {
        let zoom = clamp_zoom(context.zoom_factor());
        let mut settings = self.appearance.store.settings().clone();
        if (settings.zoom - zoom).abs() > f32::EPSILON {
            settings.zoom = zoom;
            self.appearance.store.save(settings);
        }
    }
}
