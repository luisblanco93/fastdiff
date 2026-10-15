//! Lazy system-font fallback for scripts the bundled fonts cannot draw (CJK).
//!
//! egui loads no system fonts, and the system CJK fonts are 15-80 MB, so reading one on every
//! launch would cost startup time and memory for users who never see such text. The fallback is
//! installed once, the first time a file name or diff row contains a glyph the defaults lack.

use std::{path::PathBuf, sync::Arc};

/// Largest font file read; guards against pathological or unexpected files.
const MAX_FONT_BYTES: u64 = 128 * 1024 * 1024;
const FONT_NAME: &str = "fastdiff-system-fallback";

/// A system font file and the face to use from it (`.ttc` collections hold several).
#[derive(Debug, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub face: u32,
}

/// Candidate files for this platform, most broadly covering first.
pub fn candidates() -> Vec<Candidate> {
    let files: Vec<PathBuf> = if cfg!(target_os = "macos") {
        [
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/System/Library/Fonts/STHeiti Light.ttc",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    } else if cfg!(windows) {
        let fonts =
            PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()))
                .join("Fonts");
        [
            "msyh.ttc",
            "YuGothM.ttc",
            "meiryo.ttc",
            "malgun.ttf",
            "simsun.ttc",
        ]
        .iter()
        .map(|name| fonts.join(name))
        .collect()
    } else {
        [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    };
    files
        .into_iter()
        .map(|path| Candidate { path, face: 0 })
        .collect()
}

/// The first candidate for which `usable` holds, in order.
pub fn first_usable(
    candidates: Vec<Candidate>,
    usable: impl Fn(&Candidate) -> bool,
) -> Option<Candidate> {
    candidates.into_iter().find(|candidate| usable(candidate))
}

fn is_readable_font(candidate: &Candidate) -> bool {
    std::fs::metadata(&candidate.path)
        .map(|metadata| metadata.is_file() && metadata.len() <= MAX_FONT_BYTES)
        .unwrap_or(false)
}

/// Whether any text contains a character the current fonts cannot draw. ASCII is skipped, so
/// ordinary diffs cost one `is_ascii` pass per string.
pub fn missing_glyphs<'a>(
    context: &egui::Context,
    texts: impl IntoIterator<Item = &'a str>,
) -> bool {
    let mut seen = std::collections::HashSet::new();
    let ids = [
        egui::FontId::monospace(13.0),
        egui::FontId::proportional(13.0),
    ];
    for text in texts {
        if text.is_ascii() {
            continue;
        }
        for character in text.chars().filter(|c| !c.is_ascii() && !c.is_control()) {
            if seen.insert(character)
                && context.fonts_mut(|fonts| ids.iter().any(|id| !fonts.has_glyph(id, character)))
            {
                return true;
            }
        }
    }
    false
}

/// Appends the first usable system font to both families. Returns whether one was installed; a
/// missing or unreadable font is a no-op, never an error.
pub fn install(context: &egui::Context) -> bool {
    let Some(candidate) = first_usable(candidates(), is_readable_font) else {
        return false;
    };
    let Ok(bytes) = std::fs::read(&candidate.path) else {
        return false;
    };
    let mut data = egui::FontData::from_owned(bytes);
    data.index = candidate.face;
    let mut definitions = egui::FontDefinitions::default();
    definitions
        .font_data
        .insert(FONT_NAME.to_owned(), Arc::new(data));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        definitions
            .families
            .entry(family)
            .or_default()
            .push(FONT_NAME.to_owned());
    }
    context.set_fonts(definitions);
    true
}

/// Remembers which (files, document) pair each tab was last scanned for, and whether the
/// fallback is already installed or unavailable, so scans run once per change.
#[derive(Default)]
pub struct Watch {
    done: bool,
    seen: Vec<(usize, usize)>,
}

impl Watch {
    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn finish(&mut self) {
        self.done = true;
        self.seen.clear();
    }

    /// True when `tab` shows different files or a different document since the last call.
    pub fn changed(&mut self, tab: usize, files: usize, document: usize) -> bool {
        if self.seen.len() <= tab {
            self.seen.resize(tab + 1, (0, 0));
        }
        let current = (files, document);
        let changed = self.seen[tab] != current;
        self.seen[tab] = current;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(path: &str) -> Candidate {
        Candidate {
            path: PathBuf::from(path),
            face: 0,
        }
    }

    #[test]
    fn first_usable_keeps_candidate_order_and_skips_missing_files() {
        let list = vec![candidate("/a"), candidate("/b"), candidate("/c")];
        let picked = first_usable(list, |c| c.path != PathBuf::from("/a"));
        assert_eq!(picked, Some(candidate("/b")));
        assert_eq!(first_usable(vec![candidate("/a")], |_| false), None);
    }

    #[test]
    fn every_platform_lists_candidates() {
        assert!(!candidates().is_empty());
    }

    #[test]
    fn watch_reports_a_tab_only_when_its_inputs_change() {
        let mut watch = Watch::default();
        assert!(watch.changed(0, 1, 2));
        assert!(!watch.changed(0, 1, 2));
        assert!(watch.changed(0, 1, 3));
        assert!(watch.changed(1, 1, 3));
        assert!(!watch.is_done());
        watch.finish();
        assert!(watch.is_done());
    }

    #[test]
    fn ascii_never_needs_a_fallback_and_cjk_does_with_the_default_fonts() {
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |_| {});
        assert!(!missing_glyphs(
            &context,
            ["plain ascii", "tabs\tand\nnewlines"]
        ));
        assert!(!missing_glyphs(&context, ["résumé"]));
        assert!(missing_glyphs(&context, ["日本語"]));
    }
}
