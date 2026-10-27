//! Color themes: egui chrome, diff and syntax colors, and the theme painted for one frame.
//!
//! Each theme is `const` data in `palettes`. The active selection lives in egui context data
//! (`install`/`active`), so render code reads it the way it reads `ui.visuals()`.
use egui::{Color32, Context, Id, Stroke, Visuals};

mod palettes;
#[cfg(test)]
mod tests;

/// Selection stroke of `Visuals::dark()`; the fastdiff theme keeps it for tab accents.
const FASTDIFF_ACCENT: Color32 = Color32::from_rgb(192, 222, 255);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThemeId {
    Fastdiff,
    DarkModern,
    OneDarkPro,
    Dracula,
    CatppuccinMocha,
    GithubLight,
}

impl ThemeId {
    pub const ALL: [ThemeId; 6] = [
        ThemeId::Fastdiff,
        ThemeId::DarkModern,
        ThemeId::OneDarkPro,
        ThemeId::Dracula,
        ThemeId::CatppuccinMocha,
        ThemeId::GithubLight,
    ];

    /// Stable id used by `--color-theme` and the settings file.
    pub const fn key(self) -> &'static str {
        match self {
            ThemeId::Fastdiff => "fastdiff",
            ThemeId::DarkModern => "dark-modern",
            ThemeId::OneDarkPro => "one-dark-pro",
            ThemeId::Dracula => "dracula",
            ThemeId::CatppuccinMocha => "catppuccin-mocha",
            ThemeId::GithubLight => "github-light",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|id| id.key() == key)
    }

    pub fn theme(self) -> &'static Theme {
        match self {
            ThemeId::Fastdiff => &palettes::FASTDIFF,
            ThemeId::DarkModern => &palettes::DARK_MODERN,
            ThemeId::OneDarkPro => &palettes::ONE_DARK_PRO,
            ThemeId::Dracula => &palettes::DRACULA,
            ThemeId::CatppuccinMocha => &palettes::CATPPUCCIN_MOCHA,
            ThemeId::GithubLight => &palettes::GITHUB_LIGHT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

#[derive(Debug)]
pub struct Theme {
    pub id: ThemeId,
    pub name: &'static str,
    pub appearance: Appearance,
    /// `None` keeps egui's stock visuals, which is exactly the original fastdiff look.
    pub chrome: Option<Chrome>,
    /// Comparison labels and header rows.
    pub header: Color32,
    pub code: CodePalette,
    pub status: StatusColors,
}

/// Window chrome colors written onto `egui::Visuals`.
#[derive(Debug)]
pub struct Chrome {
    pub panel: Color32,
    pub window: Color32,
    pub border: Color32,
    pub text: Color32,
    pub strong: Color32,
    pub input: Color32,
    pub control: Color32,
    pub control_hover: Color32,
    pub selection_bg: Color32,
    pub selection_fg: Color32,
    pub accent: Color32,
}

/// Everything the code panel paints, so a code-viewer override swaps the whole panel.
#[derive(Debug)]
pub struct CodePalette {
    pub surface: Color32,
    pub text: Color32,
    pub gutter: Color32,
    pub addition: Color32,
    pub deletion: Color32,
    /// Search match background.
    pub highlight: Color32,
    /// Text selection background.
    pub selection: Color32,
    pub syntax: SyntaxColors,
}

#[derive(Debug)]
pub struct SyntaxColors {
    pub keyword: Color32,
    pub string: Color32,
    pub comment: Color32,
    pub number: Color32,
    // Read once the lexer emits function and type tokens.
    #[allow(dead_code)]
    pub function: Color32,
    #[allow(dead_code)]
    pub ty: Color32,
}

#[derive(Debug)]
pub struct StatusColors {
    pub modified: Color32,
    pub untracked: Color32,
    pub conflict: Color32,
    pub error: Color32,
    pub warning: Color32,
    pub reflog_id: Color32,
    pub reflog_branch: Color32,
}

impl Theme {
    pub fn visuals(&self) -> Visuals {
        let mut visuals = match self.appearance {
            Appearance::Dark => Visuals::dark(),
            Appearance::Light => Visuals::light(),
        };
        visuals.error_fg_color = self.status.error;
        visuals.warn_fg_color = self.status.warning;
        let Some(chrome) = &self.chrome else {
            return visuals;
        };
        visuals.panel_fill = chrome.panel;
        visuals.window_fill = chrome.window;
        visuals.window_stroke = Stroke::new(1.0, chrome.border);
        visuals.extreme_bg_color = chrome.input;
        visuals.hyperlink_color = chrome.accent;
        visuals.selection.bg_fill = chrome.selection_bg;
        visuals.selection.stroke = Stroke::new(1.0, chrome.selection_fg);
        let widgets = &mut visuals.widgets;
        widgets.noninteractive.bg_fill = chrome.panel;
        widgets.noninteractive.weak_bg_fill = chrome.panel;
        widgets.noninteractive.bg_stroke.color = chrome.border;
        widgets.noninteractive.fg_stroke.color = chrome.text;
        for (state, fill, text) in [
            (&mut widgets.inactive, chrome.control, chrome.text),
            (&mut widgets.hovered, chrome.control_hover, chrome.strong),
            (&mut widgets.active, chrome.control_hover, chrome.strong),
            (&mut widgets.open, chrome.control, chrome.text),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.fg_stroke.color = text;
        }
        widgets.hovered.bg_stroke.color = chrome.accent;
        widgets.active.bg_stroke.color = chrome.accent;
        visuals
    }

    /// Tab underline and icon color.
    pub fn accent(&self) -> Color32 {
        self.chrome
            .as_ref()
            .map_or(FASTDIFF_ACCENT, |chrome| chrome.accent)
    }
}

/// The themes painted this frame: the app chrome and the code panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Active {
    pub app: ThemeId,
    pub code: ThemeId,
}

impl Default for Active {
    fn default() -> Self {
        Self {
            app: ThemeId::Fastdiff,
            code: ThemeId::Fastdiff,
        }
    }
}

impl Active {
    pub fn app_theme(self) -> &'static Theme {
        self.app.theme()
    }

    pub fn code_palette(self) -> &'static CodePalette {
        &self.code.theme().code
    }
}

fn active_id() -> Id {
    Id::new("fastdiff-active-theme")
}

pub fn install(ctx: &Context, active: Active) {
    ctx.data_mut(|data| data.insert_temp(active_id(), active));
}

pub fn active(ctx: &Context) -> Active {
    ctx.data(|data| data.get_temp(active_id()))
        .unwrap_or_default()
}
