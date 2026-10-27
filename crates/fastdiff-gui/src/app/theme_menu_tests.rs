use super::tests::viewer;
use super::theme_menu::{Appearance, resolve};
use super::*;
use crate::settings::{Settings, SettingsStore};
use crate::theme::ThemeId;
use egui_kittest::{Harness, kittest::Queryable};

fn following() -> Settings {
    Settings {
        follow_system: true,
        dark_theme: ThemeId::OneDarkPro,
        light_theme: ThemeId::GithubLight,
        theme: ThemeId::Dracula,
        ..Settings::default()
    }
}

fn title_bar_commands(
    appearance: &mut Appearance,
    system: Option<egui::Theme>,
) -> Vec<egui::SystemTheme> {
    let context = egui::Context::default();
    context.set_theme(egui::ThemePreference::Light);
    let input = egui::RawInput {
        system_theme: system,
        ..Default::default()
    };
    let output = context.run(input, |context| appearance.apply(context));
    output.viewport_output[&egui::ViewportId::ROOT]
        .commands
        .iter()
        .filter_map(|command| match command {
            egui::ViewportCommand::SetTheme(theme) => Some(*theme),
            _ => None,
        })
        .collect()
}

#[test]
fn resolve_and_title_bar_follow_preview_session_system_and_saved_theme() {
    let saved = Settings {
        theme: ThemeId::Dracula,
        code_theme: ThemeId::GithubLight,
        ..Settings::default()
    };
    let dark = Some(egui::Theme::Dark);
    let light = Some(egui::Theme::Light);
    let session = Some(ThemeId::DarkModern);
    assert_eq!(resolve(&saved, dark, None, None).app, ThemeId::Dracula);
    assert_eq!(
        resolve(&saved, dark, session, None).app,
        ThemeId::DarkModern
    );
    let preview = Some(ThemeId::GithubLight);
    assert_eq!(
        resolve(&saved, dark, session, preview).app,
        ThemeId::GithubLight
    );
    let follow = following();
    assert_eq!(resolve(&follow, dark, None, None).app, ThemeId::OneDarkPro);
    assert_eq!(
        resolve(&follow, light, None, None).app,
        ThemeId::GithubLight
    );
    assert_eq!(resolve(&follow, None, None, None).app, ThemeId::OneDarkPro);
    assert_eq!(
        resolve(&follow, light, session, None).app,
        ThemeId::DarkModern
    );
    // The code panel follows the app unless overridden.
    assert_eq!(
        resolve(&saved, dark, session, None).code,
        ThemeId::DarkModern
    );
    let overridden = Settings {
        code_follows_app: false,
        ..saved
    };
    assert_eq!(
        resolve(&overridden, dark, session, None).code,
        ThemeId::GithubLight
    );

    // An explicit theme ignores the system; the title bar tracks the resolved theme.
    let mut pinned = Appearance::default();
    assert_eq!(
        title_bar_commands(&mut pinned, light),
        [egui::SystemTheme::Dark]
    );
    let mut github = Appearance::default();
    github.store.save(Settings {
        theme: ThemeId::GithubLight,
        ..Settings::default()
    });
    assert_eq!(
        title_bar_commands(&mut github, dark),
        [egui::SystemTheme::Light]
    );
    let mut system = Appearance::default();
    system.store.save(following());
    assert_eq!(
        title_bar_commands(&mut system, light),
        [egui::SystemTheme::SystemDefault]
    );
}

const RADIO: egui::accesskit::Role = egui::accesskit::Role::RadioButton;

fn harness_with_store(store: SettingsStore) -> Harness<'static, Fastdiff> {
    let mut app = viewer();
    app.appearance = Appearance::new(store, None);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1000.0, 600.0))
        .build_state(|context, app| app.show(context), app);
    harness.run();
    harness
}

fn painted(harness: &Harness<'static, Fastdiff>) -> ThemeId {
    crate::theme::active(&harness.ctx).app
}

fn menu_is_open(harness: &Harness<'static, Fastdiff>) -> bool {
    egui::Popup::is_id_open(&harness.ctx, egui::Id::new("theme-menu"))
}

#[test]
fn menu_previews_on_hover_reverts_on_escape_and_saves_a_click_once() {
    let mut harness = harness_with_store(SettingsStore::memory());
    harness.get_by_label("Theme ⏷").click();
    harness.run();
    harness.get_by_role_and_label(RADIO, "GitHub Light").hover();
    harness.run_steps(2);
    assert_eq!(painted(&harness), ThemeId::GithubLight);
    assert!(!harness.ctx.style().visuals.dark_mode);
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    assert!(!menu_is_open(&harness));
    assert_eq!(painted(&harness), ThemeId::Fastdiff);
    assert_eq!(harness.state().appearance.store.saves(), 0);

    harness.state_mut().appearance.session = Some(ThemeId::DarkModern);
    harness.get_by_label("Theme ⏷").click();
    harness.run();
    harness.get_by_role_and_label(RADIO, "Dracula").click();
    harness.run_steps(2);
    assert!(!menu_is_open(&harness));
    let appearance = &harness.state().appearance;
    assert_eq!(appearance.store.saves(), 1);
    assert_eq!(appearance.store.settings().theme, ThemeId::Dracula);
    assert_eq!(
        appearance.session, None,
        "a click replaces the session override"
    );
    assert_eq!(painted(&harness), ThemeId::Dracula);
}

#[test]
fn chord_opens_the_menu_outside_text_fields_and_a_stray_key_cancels_it() {
    let mut harness = harness_with_store(SettingsStore::memory());
    let ctrl = egui::Modifiers::COMMAND;
    harness.key_press_modifiers(ctrl, egui::Key::K);
    harness.run();
    harness.key_press_modifiers(ctrl, egui::Key::T);
    harness.run_steps(2);
    assert!(menu_is_open(&harness));
    harness.key_press(egui::Key::Escape);
    harness.run();

    harness.key_press_modifiers(ctrl, egui::Key::K);
    harness.run();
    harness.key_press(egui::Key::A);
    harness.run();
    harness.key_press_modifiers(ctrl, egui::Key::T);
    harness.run();
    assert!(!menu_is_open(&harness));

    // In the search field Ctrl+K stays a text-editing key.
    harness.key_press_modifiers(ctrl, egui::Key::F);
    harness.run();
    harness.key_press_modifiers(ctrl, egui::Key::K);
    harness.run();
    assert!(harness.state().appearance.chord_started.is_none());
}
