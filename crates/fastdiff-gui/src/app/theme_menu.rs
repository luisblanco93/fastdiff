//! Appearance state of the window: the saved settings and the values derived from them.
use super::Fastdiff;
use crate::settings::{SettingsStore, clamp_zoom};

#[derive(Default)]
pub(super) struct Appearance {
    pub(super) store: SettingsStore,
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
