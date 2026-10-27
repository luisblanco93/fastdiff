//! Built-in palettes. Diff backgrounds are the official VS Code colors alpha-blended over each
//! theme's editor background. A value marked "official" was adjusted for readability by the
//! contrast tests; the comment names the upstream value.
use super::{Appearance, Chrome, CodePalette, StatusColors, SyntaxColors, Theme, ThemeId};
use egui::Color32;

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub(super) static FASTDIFF: Theme = Theme {
    id: ThemeId::Fastdiff,
    name: "Default",
    appearance: Appearance::Dark,
    chrome: None,
    header: hex(0x93B4EF),
    code: CodePalette {
        surface: hex(0x181B21),
        text: hex(0xD5DCE6),
        gutter: hex(0x757F8F),
        addition: hex(0x1B372A),
        deletion: hex(0x422328),
        highlight: hex(0x374E73),
        selection: hex(0x374E73),
        syntax: SyntaxColors {
            keyword: hex(0xC197E6),
            string: hex(0xA9CC94),
            comment: hex(0x91AA9B),
            number: hex(0xE5B171),
            function: hex(0x93B4EF),
            ty: hex(0x91B4D7),
        },
    },
    status: StatusColors {
        modified: hex(0x91B4D7),
        untracked: hex(0x6ECD91),
        conflict: hex(0xF57878),
        error: hex(0xEB928A),
        warning: hex(0xE5B171),
        reflog_id: hex(0xBE9A4A),
        reflog_branch: hex(0x58EA51),
    },
};

pub(super) static DARK_MODERN: Theme = Theme {
    id: ThemeId::DarkModern,
    name: "Dark Modern",
    appearance: Appearance::Dark,
    chrome: Some(Chrome {
        panel: hex(0x181818),
        window: hex(0x1F1F1F),
        border: hex(0x2B2B2B),
        text: hex(0xCCCCCC),
        strong: hex(0xFFFFFF),
        input: hex(0x313131),
        control: hex(0x2B2B2B),
        control_hover: hex(0x3C3C3C),
        selection_bg: hex(0x04395E),
        selection_fg: hex(0xFFFFFF),
        accent: hex(0x0078D4),
    }),
    header: hex(0x4DAAFC),
    code: CodePalette {
        surface: hex(0x1F1F1F),
        text: hex(0xCCCCCC),
        // Official line number color #6E7681.
        gutter: hex(0x7F8690),
        addition: hex(0x383E2A),
        deletion: hex(0x4C1919),
        highlight: hex(0x633315),
        selection: hex(0x264F78),
        syntax: SyntaxColors {
            // Official #569CD6.
            keyword: hex(0x6FABDC),
            // Official #CE9178.
            string: hex(0xD19880),
            comment: hex(0x8BAF7A),
            number: hex(0xB5CEA8),
            function: hex(0xDCDCAA),
            ty: hex(0x4EC9B0),
        },
    },
    status: StatusColors {
        modified: hex(0x4DAAFC),
        untracked: hex(0x73C991),
        conflict: hex(0xF85149),
        error: hex(0xF85149),
        // Official editorWarning.foreground.
        warning: hex(0xCCA700),
        reflog_id: hex(0xCCA700),
        reflog_branch: hex(0x73C991),
    },
};

pub(super) static ONE_DARK_PRO: Theme = Theme {
    id: ThemeId::OneDarkPro,
    name: "One Dark Pro",
    appearance: Appearance::Dark,
    chrome: Some(Chrome {
        panel: hex(0x21252B),
        window: hex(0x282C34),
        border: hex(0x181A1F),
        text: hex(0xABB2BF),
        strong: hex(0xD7DAE0),
        input: hex(0x1D1F23),
        control: hex(0x2C313A),
        control_hover: hex(0x3E4452),
        // Official list selection #2C313A is nearly invisible on the panel; this is the
        // official editor selection (#67769660) blended over the panel.
        selection_bg: hex(0x3B4353),
        selection_fg: hex(0xD7DAE0),
        accent: hex(0x61AFEF),
    }),
    header: hex(0x61AFEF),
    code: CodePalette {
        surface: hex(0x282C34),
        text: hex(0xABB2BF),
        // Official line number color #495162.
        gutter: hex(0x8C919C),
        addition: hex(0x3F473B),
        deletion: hex(0x53232A),
        highlight: hex(0x42454C),
        // Official editor selection #67769660 blended, darkened to keep text at 4.5:1.
        selection: hex(0x3D4556),
        syntax: SyntaxColors {
            // Official #C678DD.
            keyword: hex(0xD79FE7),
            string: hex(0x98C379),
            // Official #D19A66.
            number: hex(0xD8AA7E),
            // Official #7F848E.
            comment: hex(0xB0B3B9),
            // Official #61AFEF.
            function: hex(0x74B9F1),
            ty: hex(0xE5C07B),
        },
    },
    status: StatusColors {
        modified: hex(0x61AFEF),
        untracked: hex(0x98C379),
        conflict: hex(0xE06C75),
        // Official editorError.foreground #C24038.
        error: hex(0xDC766E),
        // Official editorWarning.foreground.
        warning: hex(0xD19A66),
        reflog_id: hex(0xD19A66),
        reflog_branch: hex(0x98C379),
    },
};

pub(super) static DRACULA: Theme = Theme {
    id: ThemeId::Dracula,
    name: "Dracula",
    appearance: Appearance::Dark,
    chrome: Some(Chrome {
        panel: hex(0x21222C),
        window: hex(0x282A36),
        border: hex(0x191A21),
        text: hex(0xF8F8F2),
        strong: hex(0xFFFFFF),
        input: hex(0x282A36),
        control: hex(0x343746),
        control_hover: hex(0x424450),
        selection_bg: hex(0x44475A),
        selection_fg: hex(0xF8F8F2),
        // Official tab.activeBorderTop is pink at 80% opacity.
        accent: hex(0xFF79C6),
    }),
    header: hex(0xBD93F9),
    code: CodePalette {
        surface: hex(0x282A36),
        text: hex(0xF8F8F2),
        // Official line number color #6272A4.
        gutter: hex(0x8390B7),
        addition: hex(0x3F473C),
        deletion: hex(0x53222B),
        highlight: hex(0x5D5E67),
        selection: hex(0x44475A),
        syntax: SyntaxColors {
            // Official #FF79C6.
            keyword: hex(0xFF8CCE),
            string: hex(0xF1FA8C),
            // Official #BD93F9.
            number: hex(0xC6A1FA),
            // Official #6272A4.
            comment: hex(0xA9B1CD),
            function: hex(0x50FA7B),
            ty: hex(0x8BE9FD),
        },
    },
    status: StatusColors {
        modified: hex(0x8BE9FD),
        untracked: hex(0x50FA7B),
        conflict: hex(0xFF5555),
        error: hex(0xFF5555),
        // The official editorWarning.foreground is cyan; orange keeps the warning meaning.
        warning: hex(0xFFB86C),
        reflog_id: hex(0xFFB86C),
        reflog_branch: hex(0x50FA7B),
    },
};

pub(super) static CATPPUCCIN_MOCHA: Theme = Theme {
    id: ThemeId::CatppuccinMocha,
    name: "Catppuccin Mocha",
    appearance: Appearance::Dark,
    chrome: Some(Chrome {
        panel: hex(0x181825),
        window: hex(0x1E1E2E),
        border: hex(0x11111B),
        text: hex(0xCDD6F4),
        strong: hex(0xFFFFFF),
        input: hex(0x11111B),
        control: hex(0x313244),
        control_hover: hex(0x45475A),
        selection_bg: hex(0x313244),
        selection_fg: hex(0xCDD6F4),
        // Mauve, the default Catppuccin accent.
        accent: hex(0xCBA6F7),
    }),
    header: hex(0x89B4FA),
    code: CodePalette {
        surface: hex(0x1E1E2E),
        text: hex(0xCDD6F4),
        gutter: hex(0x7F849C),
        addition: hex(0x323C3F),
        deletion: hex(0x3E2E40),
        highlight: hex(0x3E5767),
        // Official overlay2 at 25% over the base.
        selection: hex(0x3B3D4F),
        syntax: SyntaxColors {
            keyword: hex(0xCBA6F7),
            string: hex(0xA6E3A1),
            number: hex(0xFAB387),
            // Official #9399B2.
            comment: hex(0x9EA3BA),
            function: hex(0x89B4FA),
            ty: hex(0xF9E2AF),
        },
    },
    status: StatusColors {
        modified: hex(0x89B4FA),
        untracked: hex(0xA6E3A1),
        conflict: hex(0xF38BA8),
        error: hex(0xF38BA8),
        warning: hex(0xFAB387),
        reflog_id: hex(0xFAB387),
        reflog_branch: hex(0xA6E3A1),
    },
};

pub(super) static GITHUB_LIGHT: Theme = Theme {
    id: ThemeId::GithubLight,
    name: "GitHub Light",
    appearance: Appearance::Light,
    chrome: Some(Chrome {
        panel: hex(0xF6F8FA),
        window: hex(0xFFFFFF),
        border: hex(0xD0D7DE),
        text: hex(0x1F2328),
        strong: hex(0x000000),
        input: hex(0xFFFFFF),
        control: hex(0xEAEEF2),
        control_hover: hex(0xD0D7DE),
        // Official list selection (neutral.muted) is invisible on the panel; this is the
        // Primer gray-2 fill.
        selection_bg: hex(0xD0D7DE),
        selection_fg: hex(0x1F2328),
        // Official tab.activeBorderTop #FD8C73 is salmon; Primer's accent blue is used instead.
        accent: hex(0x0969DA),
    }),
    header: hex(0x0969DA),
    code: CodePalette {
        surface: hex(0xFFFFFF),
        text: hex(0x1F2328),
        // Official line number color #8C959F.
        gutter: hex(0x858E97),
        addition: hex(0xE6FAEB),
        deletion: hex(0xFFF0EF),
        highlight: hex(0xFCF0BE),
        // Official accent.fg #0969DA at 20% over white.
        selection: hex(0xCEE1F8),
        syntax: SyntaxColors {
            keyword: hex(0xCF222E),
            string: hex(0x0A3069),
            number: hex(0x0550AE),
            // Official #6E7781.
            comment: hex(0x677079),
            function: hex(0x8250DF),
            ty: hex(0x953800),
        },
    },
    status: StatusColors {
        modified: hex(0x0969DA),
        untracked: hex(0x1A7F37),
        conflict: hex(0xCF222E),
        error: hex(0xCF222E),
        warning: hex(0x9A6700),
        reflog_id: hex(0x9A6700),
        reflog_branch: hex(0x1A7F37),
    },
};
