//! Changed-file hierarchy. Git path bytes are identities; display strings never select files.
use egui::{Id, Key, Modifiers, Ui};
use fastdiff_core::{FileChange, GitPath};
use std::collections::HashSet;
use std::sync::Arc;

mod icons;
mod material_icons;
mod model;
mod paint;
pub(crate) use model::TreeModel;
use model::{NodeIdentity, TreeRow};
#[cfg(test)]
use paint::status_badge;
pub(crate) use paint::{document_icon, folder_icon};
use paint::{row_button, search_field};

const ROW_HEIGHT: f32 = 24.0;

#[derive(Default)]
pub struct FileTree {
    model: Arc<TreeModel>,
    collapsed: HashSet<Vec<u8>>,
    cache: Vec<TreeRow>,
    cached_filter: String,
    dirty: bool,
    reveal_target: Option<NodeIdentity>,
    scroll_offset: f32,
    viewport_height: f32,
    focused: Option<(NodeIdentity, Id)>,
    focus_target: Option<NodeIdentity>,
    #[cfg(test)]
    rendered_rows: usize,
}

impl FileTree {
    /// Call on a status refresh, rather than every frame.
    pub(crate) fn update(&mut self, model: Arc<TreeModel>) -> bool {
        if Arc::ptr_eq(&self.model, &model) {
            return false;
        }
        self.collapsed
            .retain(|path| path.is_empty() || model.has_folder(path));
        self.focused = self
            .focused
            .take()
            .filter(|(identity, _)| model.has_node(identity));
        self.focus_target = self
            .focus_target
            .take()
            .filter(|identity| model.has_node(identity));
        self.reveal_target = self
            .reveal_target
            .take()
            .filter(|identity| model.has_node(identity));
        self.model = model;
        self.dirty = true;
        true
    }

    pub(crate) fn file_index(&self, path: &[u8]) -> Option<usize> {
        self.model.file_index(path)
    }

    #[cfg(test)]
    pub(crate) fn model_snapshot(&self) -> Arc<TreeModel> {
        self.model.clone()
    }

    /// Navigation visits matching files even when their parent folder is collapsed.
    pub fn filtered_files(&self, filter: &str) -> Vec<usize> {
        self.model.filtered_files(filter)
    }

    pub fn reveal(&mut self, index: usize) {
        if let Some(path) = self.model.file_paths.get(index) {
            if self.cached_filter.is_empty() {
                self.dirty |= self.collapsed.remove(&Vec::new());
                for ancestor in model::ancestors(path) {
                    self.dirty |= self.collapsed.remove(ancestor);
                }
            }
            self.reveal_target = Some(NodeIdentity {
                path: path.clone(),
                file: true,
            });
        }
    }

    fn toggle(&mut self, key: &[u8]) {
        if !self.collapsed.remove(key) {
            self.collapsed.insert(key.to_vec());
        }
        self.dirty = true;
    }

    fn rows(&mut self, filter: &str) -> &[TreeRow] {
        if self.dirty || self.cached_filter != filter {
            self.cache = self.model.rows(filter, &self.collapsed);
            self.cached_filter = filter.to_owned();
            self.dirty = false;
        }
        &self.cache
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        root: &str,
        files: &[FileChange],
        filter: &mut String,
        selected: Option<&GitPath>,
        theme: bool,
    ) -> Option<usize> {
        search_field(ui, filter);
        self.rows(filter);
        let mut choice = self.keyboard(ui, filter);
        self.rows(filter);
        let target = self.scroll_target();
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("changed-file-tree")
            .auto_shrink([false, false]);
        if let Some(index) = target {
            let row_top = index as f32 * ROW_HEIGHT;
            let height = self.viewport_height.max(ROW_HEIGHT);
            let offset = self
                .scroll_offset
                .clamp(row_top + ROW_HEIGHT - height, row_top);
            scroll = scroll.vertical_scroll_offset(offset.max(0.0));
        }
        #[cfg(test)]
        {
            self.rendered_rows = 0;
        }
        ui.spacing_mut().item_spacing.y = 0.0;
        let output = scroll.show_rows(ui, ROW_HEIGHT, self.cache.len() + 1, |ui, visible| {
            for index in visible {
                let row = if index == 0 {
                    TreeRow::root(root)
                } else {
                    self.cache[index - 1].clone()
                };
                let expanded = !filter.is_empty() || !self.collapsed.contains(&row.key);
                let file = row.file.and_then(|index| files.get(index));
                let selected = file.is_some_and(|file| Some(&file.path) == selected);
                let response = row_button(ui, &row, file, selected, expanded, theme);
                #[cfg(test)]
                {
                    self.rendered_rows += 1;
                }
                if self
                    .focus_target
                    .as_ref()
                    .is_some_and(|target| target.matches(&row))
                {
                    response.request_focus();
                    self.focus_target = None;
                }
                if response.has_focus() {
                    ui.memory_mut(|memory| {
                        memory.set_focus_lock_filter(
                            response.id,
                            egui::EventFilter {
                                horizontal_arrows: true,
                                vertical_arrows: true,
                                ..egui::EventFilter::default()
                            },
                        )
                    });
                    self.focused = Some((row.identity(), response.id));
                }
                if response.clicked() {
                    response.request_focus();
                    self.focused = Some((row.identity(), response.id));
                    if let Some(index) = row.file {
                        choice = Some(index);
                    } else if filter.is_empty() {
                        self.toggle(&row.key);
                    }
                }
            }
        });
        self.scroll_offset = output.state.offset.y;
        self.viewport_height = output.inner_rect.height();
        choice
    }

    fn scroll_target(&mut self) -> Option<usize> {
        let reveal = self.reveal_target.take();
        if reveal.is_none() && self.focus_target.is_none() {
            return None;
        }
        self.cache
            .iter()
            .position(|row| {
                reveal.as_ref().is_some_and(|target| target.matches(row))
                    || self
                        .focus_target
                        .as_ref()
                        .is_some_and(|target| target.matches(row))
            })
            .map(|index| index + 1)
            .or_else(|| {
                self.focus_target
                    .as_ref()
                    .is_some_and(NodeIdentity::is_root)
                    .then_some(0)
            })
    }

    fn keyboard(&mut self, ui: &mut Ui, filter: &str) -> Option<usize> {
        let (_, focus_id) = self.focused.as_ref()?;
        if !ui.memory(|memory| memory.has_focus(*focus_id)) {
            return None;
        }
        let command = ui.input_mut(|input| {
            if input.modifiers != Modifiers::NONE {
                return None;
            }
            [
                Key::ArrowUp,
                Key::ArrowDown,
                Key::ArrowLeft,
                Key::ArrowRight,
                Key::Enter,
                Key::Space,
            ]
            .into_iter()
            .find(|key| input.consume_key(Modifiers::NONE, *key))
        })?;
        let (identity, _) = self.focused.as_ref()?;
        let index = if identity.is_root() {
            0
        } else {
            self.cache.iter().position(|row| identity.matches(row))? + 1
        };
        let row = if index == 0 {
            TreeRow::root("")
        } else {
            self.cache[index - 1].clone()
        };
        let key = &row.key;
        match command {
            Key::ArrowUp => self.focus_index(index.saturating_sub(1)),
            Key::ArrowDown => self.focus_index((index + 1).min(self.cache.len())),
            Key::ArrowLeft if row.file.is_none() && !self.collapsed.contains(key) => {
                if filter.is_empty() {
                    self.toggle(key);
                }
            }
            Key::ArrowLeft => self.focus_parent(&row),
            Key::ArrowRight if row.file.is_none() => {
                if self.collapsed.contains(key) && filter.is_empty() {
                    self.toggle(key);
                } else {
                    self.focus_index((index + 1).min(self.cache.len()));
                }
            }
            Key::Enter | Key::Space => {
                if row.file.is_none() {
                    if filter.is_empty() {
                        self.toggle(key);
                    }
                } else if command == Key::Enter {
                    return row.file;
                }
            }
            _ => {}
        }
        None
    }

    fn focus_index(&mut self, index: usize) {
        self.focus_target = Some(if index == 0 {
            TreeRow::root("").identity()
        } else {
            self.cache[index - 1].identity()
        });
    }

    fn focus_parent(&mut self, row: &TreeRow) {
        let parent = self.cache.iter().rev().find(|candidate| {
            candidate.file.is_none()
                && row.key.starts_with(&candidate.key)
                && row.key.get(candidate.key.len()) == Some(&b'/')
        });
        self.focus_target = Some(
            parent
                .map(TreeRow::identity)
                .unwrap_or_else(|| TreeRow::root("").identity()),
        );
    }
}

#[cfg(test)]
#[path = "sidebar/tests.rs"]
mod tests;
