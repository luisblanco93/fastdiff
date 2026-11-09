//! Windows system font (Segoe UI) for the Markdown preview body and headings.
//!
//! egui loads no system fonts, so the font is read lazily the first time a preview is drawn and
//! registered as a named family that only the preview uses. A named family that is not defined
//! panics in egui, and fonts take effect at the start of the next pass, so the family is handed
//! out only once the active font definitions contain it. Anything that goes wrong keeps the
//! bundled fonts.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) const PREVIEW_FONT_NAME: &str = "fastdiff-preview-segoe-ui";
pub(crate) const PREVIEW_FAMILY_NAME: &str = "fastdiff-preview";
const SEGOE_UI_FILE: &str = "segoeui.ttf";
const DEFAULT_FONTS_DIR: &str = "C:\\Windows\\Fonts";
/// Segoe UI Regular is about 1 MB; the cap only guards against unexpected files.
const MAX_FONT_BYTES: u64 = 32 * 1024 * 1024;
/// Leading bytes of TrueType, OpenType, and TrueType Collection files.
const FONT_MAGIC: [&[u8; 4]; 4] = [b"\0\x01\0\0", b"true", b"OTTO", b"ttcf"];

/// What the preview knows about the font, kept in egui's temporary data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// The font file is missing or unusable; the bundled fonts stay in use.
    Unavailable,
    /// Registered during this pass number; active from the next pass on.
    Installed { pass: u64 },
}

fn state_id() -> egui::Id {
    egui::Id::new("fastdiff-preview-font-state")
}

pub(crate) fn preview_family() -> egui::FontFamily {
    egui::FontFamily::Name(PREVIEW_FAMILY_NAME.into())
}

/// The system fonts directory: `%WINDIR%\Fonts`, or `C:\Windows\Fonts` when `WINDIR` is unset.
pub(crate) fn fonts_dir(windir: Option<OsString>) -> PathBuf {
    windir.map_or_else(
        || PathBuf::from(DEFAULT_FONTS_DIR),
        |dir| PathBuf::from(dir).join("Fonts"),
    )
}

fn segoe_ui_path() -> PathBuf {
    fonts_dir(std::env::var_os("WINDIR")).join(SEGOE_UI_FILE)
}

/// The file's bytes, or `None` when it is missing, not a regular file, too large, or unreadable.
pub(crate) fn read_font(path: &Path) -> Option<Vec<u8>> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FONT_BYTES {
        return None;
    }
    std::fs::read(path).ok()
}

fn looks_like_font(bytes: &[u8]) -> bool {
    bytes
        .get(..4)
        .is_some_and(|head| FONT_MAGIC.iter().any(|magic| head == magic.as_slice()))
}

/// egui parses every registered font eagerly at the start of the next pass and panics on bad
/// data, so a damaged file with a valid signature must be rejected here, before `set_fonts`.
fn parses_as_font(bytes: &[u8]) -> bool {
    ab_glyph::FontRef::try_from_slice_and_index(bytes, 0).is_ok()
}

/// Segoe UI first, then every Proportional font, so symbols and emoji still resolve.
fn definitions_with_preview_font(
    current: &egui::FontDefinitions,
    bytes: Vec<u8>,
) -> egui::FontDefinitions {
    let mut definitions = current.clone();
    let mut chain = vec![PREVIEW_FONT_NAME.to_owned()];
    chain.extend(
        current
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    definitions.font_data.insert(
        PREVIEW_FONT_NAME.to_owned(),
        Arc::new(egui::FontData::from_owned(bytes)),
    );
    definitions.families.insert(preview_family(), chain);
    definitions
}

/// Whether the active definitions have the preview family led by the preview font.
fn is_active(context: &egui::Context) -> bool {
    context.fonts(|fonts| {
        let definitions = fonts.definitions();
        definitions.font_data.contains_key(PREVIEW_FONT_NAME)
            && definitions
                .families
                .get(&preview_family())
                .and_then(|chain| chain.first())
                .is_some_and(|first| first == PREVIEW_FONT_NAME)
    })
}

fn install(context: &egui::Context, path: &Path) -> State {
    let Some(bytes) =
        read_font(path).filter(|bytes| looks_like_font(bytes) && parses_as_font(bytes))
    else {
        return State::Unavailable;
    };
    let current = context.fonts(|fonts| fonts.definitions().clone());
    context.set_fonts(definitions_with_preview_font(&current, bytes));
    context.request_repaint();
    State::Installed {
        pass: context.cumulative_pass_nr(),
    }
}

/// The preview family once the font is really active; `None` until then or when unavailable.
/// Installs the font on the first call. Must be called from inside a pass.
fn ensure_family(
    context: &egui::Context,
    path: impl FnOnce() -> PathBuf,
) -> Option<egui::FontFamily> {
    let known = context.data(|data| data.get_temp::<State>(state_id()));
    let state = known.unwrap_or_else(|| {
        let state = install(context, &path());
        context.data_mut(|data| data.insert_temp(state_id(), state));
        state
    });
    match state {
        State::Unavailable => None,
        State::Installed { pass } => {
            (context.cumulative_pass_nr() > pass && is_active(context)).then(preview_family)
        }
    }
}

/// The family for preview Body and Heading text: Segoe UI on Windows once it is active, otherwise
/// `None` so the bundled fonts are used.
pub(crate) fn preview_font_family(context: &egui::Context) -> Option<egui::FontFamily> {
    if !cfg!(windows) {
        return None;
    }
    ensure_family(context, segoe_ui_path)
}

/// Test hook: records an install that finishes in a pass that never comes.
#[cfg(test)]
pub(crate) fn mark_install_pending(context: &egui::Context) {
    context.data_mut(|data| data.insert_temp(state_id(), State::Installed { pass: u64::MAX }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fallback_fonts::add_fallback;

    fn bundled_font_bytes() -> Vec<u8> {
        let definitions = egui::FontDefinitions::default();
        definitions.font_data["Ubuntu-Light"].font.to_vec()
    }

    fn run_pass(context: &egui::Context, ui: impl FnMut(&egui::Context)) {
        let _ = context.run(egui::RawInput::default(), ui);
    }

    fn temp_font_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fastdiff-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn chain(context: &egui::Context, family: &egui::FontFamily) -> Vec<String> {
        context.fonts(|fonts| fonts.definitions().families[family].clone())
    }

    #[test]
    fn fonts_dir_uses_windir_or_the_default() {
        assert_eq!(fonts_dir(None), PathBuf::from("C:\\Windows\\Fonts"));
        assert_eq!(
            fonts_dir(Some("D:\\Win".into())),
            PathBuf::from("D:\\Win").join("Fonts")
        );
    }

    #[test]
    fn read_font_returns_bytes_for_a_file_and_none_otherwise() {
        let dir = temp_font_dir("read-font");
        let file = dir.join("tiny.ttf");
        std::fs::write(&file, b"abc").expect("write");
        assert_eq!(read_font(&file), Some(b"abc".to_vec()));
        assert_eq!(read_font(&dir.join("missing.ttf")), None);
        assert_eq!(read_font(&dir), None, "a directory is not a font");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn only_font_signatures_are_accepted() {
        assert!(looks_like_font(&bundled_font_bytes()));
        for bytes in [&b""[..], b"abc", b"<html>", b"\0\x01\0"] {
            assert!(!looks_like_font(bytes));
        }
    }

    #[test]
    fn missing_or_corrupt_font_keeps_bundled_fonts_without_panicking() {
        let dir = temp_font_dir("corrupt-font");
        let corrupt = dir.join("corrupt.ttf");
        std::fs::write(&corrupt, b"not a font at all").expect("write");
        let truncated = dir.join("truncated.ttf");
        std::fs::write(&truncated, b"\0\x01\0\0garbage").expect("write");
        for path in [dir.join("missing.ttf"), corrupt, truncated] {
            let context = egui::Context::default();
            run_pass(&context, |context| {
                assert_eq!(ensure_family(context, || path.clone()), None);
            });
            run_pass(&context, |context| {
                assert_eq!(ensure_family(context, || path.clone()), None);
                assert!(!is_active(context));
            });
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn family_is_withheld_until_the_next_pass_then_led_by_the_font() {
        let dir = temp_font_dir("install-font");
        let file = dir.join("font.ttf");
        std::fs::write(&file, bundled_font_bytes()).expect("write");
        let context = egui::Context::default();
        run_pass(&context, |context| {
            assert_eq!(
                ensure_family(context, || file.clone()),
                None,
                "pass of install"
            );
        });
        run_pass(&context, |context| {
            assert_eq!(
                ensure_family(context, || file.clone()),
                Some(preview_family())
            );
            let proportional = chain(context, &egui::FontFamily::Proportional);
            let preview = chain(context, &preview_family());
            assert_eq!(preview[0], PREVIEW_FONT_NAME);
            assert_eq!(&preview[1..], proportional.as_slice());
            let id = egui::FontId::new(14.0, preview_family());
            assert!(context.fonts_mut(|fonts| fonts.has_glyph(&id, 'A')));
        });
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn a_pending_install_never_hands_out_the_family() {
        let context = egui::Context::default();
        run_pass(&context, |context| {
            mark_install_pending(context);
            assert_eq!(ensure_family(context, || unreachable!()), None);
        });
    }

    fn assert_fallback_reaches_all_families(context: &egui::Context) {
        for family in [
            egui::FontFamily::Proportional,
            egui::FontFamily::Monospace,
            preview_family(),
        ] {
            let list = chain(context, &family);
            assert_eq!(
                list.last().map(String::as_str),
                Some("test-fallback"),
                "{family:?}"
            );
        }
        assert_eq!(chain(context, &preview_family())[0], PREVIEW_FONT_NAME);
    }

    #[test]
    fn the_fallback_font_and_the_preview_font_coexist_in_either_order() {
        let dir = temp_font_dir("coexist-font");
        let file = dir.join("font.ttf");
        std::fs::write(&file, bundled_font_bytes()).expect("write");
        let fallback = || egui::FontData::from_owned(bundled_font_bytes());
        for fallback_first in [true, false] {
            let context = egui::Context::default();
            run_pass(&context, |_| {});
            if fallback_first {
                add_fallback(&context, "test-fallback", fallback());
            }
            run_pass(&context, |context| {
                ensure_family(context, || file.clone());
            });
            if !fallback_first {
                add_fallback(&context, "test-fallback", fallback());
            }
            run_pass(&context, |_| {});
            run_pass(&context, |context| {
                assert_eq!(
                    ensure_family(context, || file.clone()),
                    Some(preview_family())
                );
                assert_fallback_reaches_all_families(context);
            });
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
