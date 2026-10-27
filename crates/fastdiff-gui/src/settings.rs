//! Persistent settings: theme choices and UI zoom, stored as `fastdiff/settings.json` in the
//! OS config directory. Reads tolerate a missing, corrupt or partly invalid file; writes go
//! through a background thread so the UI never waits on the disk.
use crate::theme::ThemeId;
use serde_json::{Map, Value, json};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::mpsc::{self, Sender},
    thread::JoinHandle,
};

pub const MIN_ZOOM: f32 = 0.75;
pub const MAX_ZOOM: f32 = 2.0;
const VERSION: u64 = 1;
const FILE_NAME: &str = "settings.json";

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub theme: ThemeId,
    pub follow_system: bool,
    pub dark_theme: ThemeId,
    pub light_theme: ThemeId,
    pub code_follows_app: bool,
    pub code_theme: ThemeId,
    pub zoom: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeId::Fastdiff,
            follow_system: false,
            dark_theme: ThemeId::Fastdiff,
            light_theme: ThemeId::GithubLight,
            code_follows_app: true,
            code_theme: ThemeId::Fastdiff,
            zoom: 1.0,
        }
    }
}

impl Settings {
    /// Reads each field on its own: an invalid value falls back to that field's default and
    /// unknown keys are ignored.
    pub fn parse(text: &str) -> Self {
        let defaults = Self::default();
        let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(text) else {
            return defaults;
        };
        let theme = |key: &str, fallback: ThemeId| {
            fields
                .get(key)
                .and_then(Value::as_str)
                .and_then(ThemeId::from_key)
                .unwrap_or(fallback)
        };
        let flag = |key: &str, fallback: bool| {
            fields.get(key).and_then(Value::as_bool).unwrap_or(fallback)
        };
        Self {
            theme: theme("theme", defaults.theme),
            follow_system: flag("follow_system", defaults.follow_system),
            dark_theme: theme("dark_theme", defaults.dark_theme),
            light_theme: theme("light_theme", defaults.light_theme),
            code_follows_app: flag("code_follows_app", defaults.code_follows_app),
            code_theme: theme("code_theme", defaults.code_theme),
            zoom: fields
                .get("zoom")
                .and_then(Value::as_f64)
                .filter(|zoom| zoom.is_finite())
                .map_or(defaults.zoom, |zoom| clamp_zoom(zoom as f32)),
        }
    }

    pub fn to_json(&self) -> String {
        let mut fields = Map::new();
        fields.insert("version".into(), json!(VERSION));
        fields.insert("theme".into(), json!(self.theme.key()));
        fields.insert("follow_system".into(), json!(self.follow_system));
        fields.insert("dark_theme".into(), json!(self.dark_theme.key()));
        fields.insert("light_theme".into(), json!(self.light_theme.key()));
        fields.insert("code_follows_app".into(), json!(self.code_follows_app));
        fields.insert("code_theme".into(), json!(self.code_theme.key()));
        fields.insert("zoom".into(), json!(f64::from(clamp_zoom(self.zoom))));
        let mut text = serde_json::to_string_pretty(&Value::Object(fields))
            .expect("settings are plain JSON values");
        text.push('\n');
        text
    }
}

pub fn clamp_zoom(zoom: f32) -> f32 {
    if zoom.is_finite() {
        zoom.clamp(MIN_ZOOM, MAX_ZOOM)
    } else {
        1.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
}

impl Os {
    pub fn current() -> Self {
        match std::env::consts::OS {
            "macos" => Os::MacOs,
            "windows" => Os::Windows,
            _ => Os::Linux,
        }
    }
}

/// Where `settings.json` lives, or `None` when the base directory cannot be determined (which
/// disables persistence). `env` reads one environment variable.
pub fn config_path(os: Os, env: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let set = |name: &str| env(name).filter(|value| !value.is_empty());
    let base = match os {
        Os::Linux => match set("XDG_CONFIG_HOME").map(PathBuf::from) {
            Some(path) if path.is_absolute() => path,
            _ => PathBuf::from(set("HOME")?).join(".config"),
        },
        Os::MacOs => PathBuf::from(set("HOME")?).join("Library/Application Support"),
        Os::Windows => PathBuf::from(set("APPDATA")?),
    };
    Some(base.join("fastdiff").join(FILE_NAME))
}

struct Writer {
    sender: Sender<Vec<u8>>,
    thread: JoinHandle<()>,
}

/// Holds the current settings and saves them when they change.
pub struct SettingsStore {
    path: Option<PathBuf>,
    settings: Settings,
    /// Serialization of the settings as last loaded or queued, so an unchanged value is never
    /// written again.
    saved: String,
    saves: usize,
    writer: Option<Writer>,
}

impl Default for SettingsStore {
    fn default() -> Self {
        Self::memory()
    }
}

impl SettingsStore {
    /// Reads the user's settings file once; used by the application.
    pub fn open_default() -> Self {
        Self::open(config_path(Os::current(), |name| std::env::var(name).ok()))
    }

    /// Never touches the disk; for tests, smoke tests and benchmarks.
    pub fn memory() -> Self {
        Self::with(None, Settings::default())
    }

    /// Uses an explicit file, for tests with a temporary directory.
    pub fn at(path: PathBuf) -> Self {
        Self::open(Some(path))
    }

    fn open(path: Option<PathBuf>) -> Self {
        let settings = path
            .as_deref()
            .and_then(|path| match fs::read_to_string(path) {
                Ok(text) => Some(Settings::parse(&text)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => {
                    eprintln!("fastdiff: could not read {}: {error}", path.display());
                    None
                }
            })
            .unwrap_or_default();
        Self::with(path, settings)
    }

    fn with(path: Option<PathBuf>, settings: Settings) -> Self {
        Self {
            path,
            saved: settings.to_json(),
            settings,
            saves: 0,
            writer: None,
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Number of changed settings handed to the writer so far.
    pub fn saves(&self) -> usize {
        self.saves
    }

    /// Stores `settings` and queues a write when they differ from the last saved value.
    pub fn save(&mut self, settings: Settings) {
        let text = settings.to_json();
        self.settings = settings;
        if text == self.saved {
            return;
        }
        self.saved = text.clone();
        self.saves += 1;
        let Some(path) = &self.path else {
            return;
        };
        let writer = self
            .writer
            .get_or_insert_with(|| spawn_writer(path.clone()));
        // A closed channel means the writer thread died: drop it so the next save starts a new one.
        if writer.sender.send(text.into_bytes()).is_err() {
            self.writer = None;
        }
    }

    /// Waits for queued writes; the next save starts a new writer.
    pub fn flush(&mut self) {
        if let Some(Writer { sender, thread }) = self.writer.take() {
            drop(sender);
            let _ = thread.join();
        }
    }
}

impl Drop for SettingsStore {
    fn drop(&mut self) {
        self.flush();
    }
}

fn spawn_writer(path: PathBuf) -> Writer {
    let (sender, receiver) = mpsc::channel::<Vec<u8>>();
    let thread = std::thread::spawn(move || {
        while let Ok(mut bytes) = receiver.recv() {
            // The newest queued write wins.
            while let Ok(newer) = receiver.try_recv() {
                bytes = newer;
            }
            if let Err(error) = write_atomic(&path, &bytes) {
                eprintln!("fastdiff: could not save {}: {error}", path.display());
            }
        }
    });
    Writer { sender, thread }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    let temporary = path.with_file_name(format!("{FILE_NAME}.{}.tmp", std::process::id()));
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    fn changed() -> Settings {
        Settings {
            theme: ThemeId::Dracula,
            follow_system: true,
            dark_theme: ThemeId::OneDarkPro,
            light_theme: ThemeId::GithubLight,
            code_follows_app: false,
            code_theme: ThemeId::CatppuccinMocha,
            zoom: 1.3,
        }
    }

    #[test]
    fn parsing_is_tolerant_and_config_paths_follow_each_platform() {
        assert_eq!(Settings::parse(&changed().to_json()), changed());
        for text in ["", "not json", "[]", "{\"theme\":"] {
            assert_eq!(Settings::parse(text), Settings::default(), "{text:?}");
        }
        let parsed = Settings::parse(
            r#"{"theme":"dracula","dark_theme":"nope","follow_system":"yes","zoom":"big"}"#,
        );
        assert_eq!(parsed.theme, ThemeId::Dracula);
        assert_eq!(parsed.dark_theme, Settings::default().dark_theme);
        assert_eq!(parsed.follow_system, Settings::default().follow_system);
        assert_eq!(parsed.zoom, 1.0);
        assert_eq!(Settings::parse(r#"{"zoom":0.1}"#).zoom, MIN_ZOOM);
        assert_eq!(Settings::parse(r#"{"zoom":9}"#).zoom, MAX_ZOOM);
        assert_eq!(Settings::parse(r#"{"zoom":1e999}"#).zoom, 1.0);
        // Absolute on every platform: "/xdg" has no drive letter on Windows.
        let base = std::env::temp_dir();
        let dir = |name: &str| base.join(name).to_string_lossy().into_owned();
        let (xdg, home, appdata) = (dir("xdg"), dir("home"), dir("appdata"));
        let path = |os, pairs: &[(&str, &str)]| config_path(os, env(pairs));
        let file = |root: &Path| Some(root.join("fastdiff").join("settings.json"));
        assert_eq!(
            path(Os::Linux, &[("XDG_CONFIG_HOME", &xdg), ("HOME", &home)]),
            file(Path::new(&xdg))
        );
        let home_config = file(&Path::new(&home).join(".config"));
        for relative_or_empty in ["relative", ""] {
            assert_eq!(
                path(
                    Os::Linux,
                    &[("XDG_CONFIG_HOME", relative_or_empty), ("HOME", &home)]
                ),
                home_config
            );
        }
        assert_eq!(
            path(Os::MacOs, &[("HOME", &home)]),
            file(&Path::new(&home).join("Library/Application Support"))
        );
        assert_eq!(
            path(Os::Windows, &[("APPDATA", &appdata)]),
            file(Path::new(&appdata))
        );
        assert_eq!(path(Os::Linux, &[]), None);
        assert_eq!(path(Os::MacOs, &[("XDG_CONFIG_HOME", &xdg)]), None);
        assert_eq!(path(Os::Windows, &[("HOME", &home)]), None);
    }

    #[test]
    fn store_writes_only_changes_and_leaves_no_temp_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/settings.json");
        let mut store = SettingsStore::at(path.clone());
        store.save(Settings::default());
        store.flush();
        assert!(!path.exists(), "an unchanged value is not written");
        store.save(changed());
        store.save(changed());
        store.flush();
        assert_eq!(store.saves(), 1);
        assert_eq!(SettingsStore::at(path.clone()).settings(), &changed());
        let files: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(files, [std::ffi::OsString::from(FILE_NAME)]);
        fs::write(&path, "{ broken").unwrap();
        assert_eq!(SettingsStore::at(path).settings(), &Settings::default());
    }
}
