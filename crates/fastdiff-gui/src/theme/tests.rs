use super::*;
use egui::Color32;

fn luminance(color: Color32) -> f64 {
    let linear = |channel: u8| {
        let value = f64::from(channel) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
}

fn contrast(a: Color32, b: Color32) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn syntax(theme: &Theme) -> [(&'static str, Color32); 6] {
    let syntax = &theme.code.syntax;
    [
        ("keyword", syntax.keyword),
        ("string", syntax.string),
        ("comment", syntax.comment),
        ("number", syntax.number),
        ("function", syntax.function),
        ("type", syntax.ty),
    ]
}

#[test]
fn every_theme_meets_the_contrast_thresholds() {
    let mut failures = Vec::new();
    let mut check = |theme: &Theme, name: &str, fg: Color32, bg: Color32, minimum: f64| {
        let ratio = contrast(fg, bg);
        if ratio < minimum {
            failures.push(format!("{} {name}: {ratio:.2}:1 < {minimum}:1", theme.name));
        }
    };
    for id in ThemeId::ALL {
        let theme = id.theme();
        let code = &theme.code;
        let visuals = theme.visuals();
        let panel = visuals.panel_fill;
        for (surface, background) in [
            ("context", code.surface),
            ("addition", code.addition),
            ("deletion", code.deletion),
        ] {
            check(
                theme,
                &format!("text on {surface}"),
                code.text,
                background,
                4.5,
            );
            check(
                theme,
                &format!("gutter on {surface}"),
                code.gutter,
                background,
                3.0,
            );
            for (kind, color) in syntax(theme) {
                check(
                    theme,
                    &format!("{kind} on {surface}"),
                    color,
                    background,
                    4.5,
                );
            }
        }
        for (surface, background) in [("match", code.highlight), ("selection", code.selection)] {
            check(
                theme,
                &format!("text on {surface}"),
                code.text,
                background,
                4.5,
            );
            for (kind, color) in syntax(theme) {
                check(
                    theme,
                    &format!("{kind} on {surface}"),
                    color,
                    background,
                    3.0,
                );
            }
        }
        let status = &theme.status;
        for (name, color) in [
            ("header", theme.header),
            ("error", status.error),
            ("warning", status.warning),
            ("modified badge", status.modified),
            ("untracked badge", status.untracked),
            ("conflict badge", status.conflict),
            ("reflog id", status.reflog_id),
            ("reflog branch", status.reflog_branch),
        ] {
            check(theme, &format!("{name} on panel"), color, panel, 4.5);
        }
        check(theme, "accent on panel", theme.accent(), panel, 3.0);
        check(
            theme,
            "selection text on selection fill",
            visuals.selection.stroke.color,
            visuals.selection.bg_fill,
            4.5,
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn fastdiff_keeps_its_original_look_and_theme_ids_stay_in_sync() {
    let theme = ThemeId::Fastdiff.theme();
    let rgb = |color: Color32| (color.r(), color.g(), color.b());
    let code = &theme.code;
    assert_eq!(rgb(code.surface), (24, 27, 33));
    assert_eq!(rgb(code.text), (213, 220, 230));
    assert_eq!(rgb(code.gutter), (117, 127, 143));
    assert_eq!(rgb(code.addition), (27, 55, 42));
    assert_eq!(rgb(code.deletion), (66, 35, 40));
    assert_eq!(rgb(code.highlight), (55, 78, 115));
    assert_eq!(code.highlight, code.selection);
    assert_eq!(rgb(code.syntax.keyword), (193, 151, 230));
    assert_eq!(rgb(code.syntax.string), (169, 204, 148));
    assert_eq!(rgb(code.syntax.comment), (145, 170, 155));
    assert_eq!(rgb(code.syntax.number), (229, 177, 113));
    assert_eq!(rgb(theme.header), (147, 180, 239));
    assert_eq!(rgb(theme.status.error), (235, 146, 138));
    assert_eq!(rgb(theme.status.warning), (229, 177, 113));
    assert_eq!(rgb(theme.status.conflict), (245, 120, 120));
    assert_eq!(rgb(theme.status.untracked), (110, 205, 145));
    assert_eq!(rgb(theme.status.modified), (145, 180, 215));
    assert_eq!(rgb(theme.status.reflog_id), (190, 154, 74));
    assert_eq!(rgb(theme.status.reflog_branch), (88, 234, 81));
    let mut expected = Visuals::dark();
    expected.error_fg_color = theme.status.error;
    expected.warn_fg_color = theme.status.warning;
    assert_eq!(theme.visuals(), expected);
    assert_eq!(theme.accent(), expected.selection.stroke.color);
    for id in ThemeId::ALL {
        assert_eq!(ThemeId::from_key(id.key()), Some(id));
        assert_eq!(id.theme().id, id);
    }
    assert_eq!(ThemeId::from_key("unknown"), None);
    let keys = ThemeId::ALL.map(ThemeId::key);
    assert_eq!(
        keys,
        fastdiff_cli::COLOR_THEMES,
        "update COLOR_THEMES with the themes"
    );
    let mut keys: Vec<_> = ThemeId::ALL.iter().map(|id| id.key()).collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), ThemeId::ALL.len());
}
