use super::{PreparedDocument, PreparedRow};
use std::{collections::VecDeque, mem::size_of, sync::Arc};

use fastdiff_core::{CancelToken, Comparison, DiffDocument, DiffRow, FileChange, View};

use crate::{highlight::Span, render::DisplayLine};

const ENTRY_LIMIT: usize = 4;
const BYTE_LIMIT: usize = 16 * 1024 * 1024;

pub(super) struct Entry {
    view: View,
    file: FileChange,
    document: Arc<PreparedDocument>,
    document_bytes: usize,
}

pub(super) struct PreparedCache {
    entries: VecDeque<Entry>,
    entry_limit: usize,
    byte_limit: usize,
}

impl Default for PreparedCache {
    fn default() -> Self {
        Self::with_limits(ENTRY_LIMIT, BYTE_LIMIT)
    }
}

impl PreparedCache {
    pub(super) fn with_limits(entry_limit: usize, byte_limit: usize) -> Self {
        let entry_limit = entry_limit.min(byte_limit / size_of::<Entry>());
        Self {
            entries: VecDeque::with_capacity(entry_limit),
            entry_limit,
            byte_limit,
        }
    }

    pub(super) fn get(
        &mut self,
        view: View,
        file: &FileChange,
        fresh: &DiffDocument,
        token: &CancelToken,
    ) -> Option<Arc<PreparedDocument>> {
        if token.is_cancelled() {
            return None;
        }
        let index = self
            .entries
            .iter()
            .position(|entry| entry.view == view && entry.file == *file)?;
        let equal = self.entries[index].document.document == *fresh;
        if token.is_cancelled() {
            return None;
        }
        let entry = self.entries.remove(index)?;
        if !equal {
            return None;
        }
        let document = Arc::clone(&entry.document);
        self.entries.push_back(entry);
        Some(document)
    }

    /// Last prepared document for this exact file, without verifying it or changing recency.
    pub(super) fn peek(
        &self,
        view: View,
        file: &FileChange,
        token: &CancelToken,
    ) -> Option<Arc<PreparedDocument>> {
        if token.is_cancelled() {
            return None;
        }
        self.entries
            .iter()
            .find(|entry| entry.view == view && entry.file == *file)
            .map(|entry| Arc::clone(&entry.document))
    }

    pub(super) fn candidate(
        &self,
        view: View,
        file: &FileChange,
        document: Arc<PreparedDocument>,
        token: &CancelToken,
    ) -> Option<Entry> {
        if token.is_cancelled() || self.entry_limit == 0 || document.is_document {
            return None;
        }
        let file = file.clone();
        let document_bytes = self
            .entries
            .iter()
            .find(|entry| Arc::ptr_eq(&entry.document, &document))
            .map_or_else(|| document_bytes(&document), |entry| entry.document_bytes);
        let minimum = self
            .collection_bytes()
            .saturating_add(key_bytes(&file))
            .saturating_add(document_bytes);
        (minimum <= self.byte_limit && !token.is_cancelled()).then_some(Entry {
            view,
            file,
            document,
            document_bytes,
        })
    }

    // Return evictions so large document destruction stays outside the cancellation lock.
    pub(super) fn admit(&mut self, candidate: Option<Entry>, token: &CancelToken) -> Vec<Entry> {
        let mut retired = Vec::new();
        let Some(entry) = candidate else {
            return retired;
        };
        if token.is_cancelled() {
            retired.push(entry);
            return retired;
        }
        if let Some(index) = self
            .entries
            .iter()
            .position(|previous| previous.view == entry.view && previous.file == entry.file)
            && let Some(previous) = self.entries.remove(index)
        {
            retired.push(previous);
        }
        // Evict before pushing so VecDeque never grows beyond its reserved slots.
        if self.entries.len() == self.entry_limit
            && let Some(previous) = self.entries.pop_front()
        {
            retired.push(previous);
        }
        self.entries.push_back(entry);
        while self.retained_bytes() > self.byte_limit {
            if let Some(previous) = self.entries.pop_front() {
                retired.push(previous);
            } else {
                break;
            }
        }
        retired
    }

    pub(super) fn obsolete(
        &self,
        view: View,
        files: &[FileChange],
        token: &CancelToken,
    ) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                (!token.is_cancelled() && entry.view == view && !files.contains(&entry.file))
                    .then_some(index)
            })
            .collect()
    }

    pub(super) fn remove_obsolete(&mut self, indices: &[usize]) -> Vec<Entry> {
        indices
            .iter()
            .rev()
            .filter_map(|&index| self.entries.remove(index))
            .collect()
    }

    #[cfg(test)]
    pub(super) fn insert(
        &mut self,
        view: View,
        file: &FileChange,
        document: Arc<PreparedDocument>,
        token: &CancelToken,
    ) {
        let candidate = self.candidate(view, file, document, token);
        self.admit(candidate, token);
    }

    #[cfg(test)]
    pub(super) fn prune(&mut self, view: View, files: &[FileChange], token: &CancelToken) {
        let indices = self.obsolete(view, files, token);
        if !token.is_cancelled() {
            self.remove_obsolete(&indices);
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    fn collection_bytes(&self) -> usize {
        self.entries.capacity().saturating_mul(size_of::<Entry>())
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.entries
            .iter()
            .enumerate()
            .fold(self.collection_bytes(), |bytes, (index, entry)| {
                let shared = self
                    .entries
                    .iter()
                    .take(index)
                    .any(|previous| Arc::ptr_eq(&previous.document, &entry.document));
                bytes
                    .saturating_add(key_bytes(&entry.file))
                    .saturating_add(if shared { 0 } else { entry.document_bytes })
            })
    }
}

fn key_bytes(file: &FileChange) -> usize {
    let bytes = file
        .path
        .0
        .capacity()
        .saturating_add(file.old_path.as_ref().map_or(0, |path| path.0.capacity()))
        .saturating_add(file.status.capacity());
    file.committed.as_ref().map_or(bytes, |committed| {
        bytes
            .saturating_add(committed.base.capacity())
            .saturating_add(committed.head.capacity())
            .saturating_add(committed.path.0.capacity())
            .saturating_add(
                committed
                    .old_path
                    .as_ref()
                    .map_or(0, |path| path.0.capacity()),
            )
            .saturating_add(committed.status.capacity())
    })
}

fn vector_bytes<T>(items: &Vec<T>) -> usize {
    items.capacity().saturating_mul(size_of::<T>())
}

fn arc_slice_bytes(lines: &Arc<[DisplayLine]>) -> usize {
    lines
        .len()
        .saturating_mul(size_of::<DisplayLine>())
        .saturating_add(2 * size_of::<usize>())
}

fn document_bytes(prepared: &PreparedDocument) -> usize {
    let base = size_of::<PreparedDocument>()
        .saturating_add(2 * size_of::<usize>())
        .saturating_add(vector_bytes::<Comparison>(&prepared.document.comparisons))
        .saturating_add(vector_bytes::<PreparedRow>(&prepared.rows))
        .saturating_add(vector_bytes(&prepared.context))
        .saturating_add(vector_bytes(&prepared.long_line_checkpoints))
        .saturating_add(arc_slice_bytes(&prepared.side_lines))
        .saturating_add(
            if Arc::ptr_eq(&prepared.side_lines, &prepared.unified_lines) {
                0
            } else {
                arc_slice_bytes(&prepared.unified_lines)
            },
        );
    let source = prepared
        .document
        .comparisons
        .iter()
        .fold(base, |bytes, comparison| {
            let bytes = bytes
                .saturating_add(comparison.label.capacity())
                .saturating_add(comparison.patch.capacity())
                .saturating_add(comparison.summary.as_ref().map_or(0, String::capacity))
                .saturating_add(vector_bytes::<DiffRow>(&comparison.rows));
            comparison.rows.iter().fold(bytes, |bytes, row| {
                bytes
                    .saturating_add(row.left.capacity())
                    .saturating_add(row.right.capacity())
            })
        });
    prepared.rows.iter().fold(source, |bytes, row| {
        bytes
            .saturating_add(vector_bytes::<Span>(&row.left))
            .saturating_add(vector_bytes::<Span>(&row.right))
    })
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
