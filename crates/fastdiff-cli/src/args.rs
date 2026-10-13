use crate::{Error, Result};
use fastdiff_core::{GitPath, View};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;

pub const HELP: &str = "fastdiff [repository|file] [--file path] [--view all|unstaged|staged|unpushed] [--theme]\n\nOpens full text files or current Git changes in a desktop window and returns after acceptance.\nThe default repository is the current directory.\nUse --theme to enable Material icons for all tabs in the window until it closes.\nUse --native in WSL to open a Linux window.\nSet FASTDIFF_WINDOWS_EXE to the Windows fastdiff-gui.exe for WSL.\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenTarget {
    Repository,
    Document,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRequest {
    pub target: OpenTarget,
    pub repository: String,
    pub file: Option<GitPath>,
    pub view: View,
    pub wsl: Option<WslTarget>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub theme: bool,
}

impl OpenRequest {
    pub fn validate(&self) -> Result<()> {
        if self.repository.is_empty() || self.repository.contains('\0') {
            return Err(Error::InvalidArgument(
                "Repository path must not be empty or contain NUL".into(),
            ));
        }
        if self.target == OpenTarget::Document && self.file.is_some() {
            return Err(Error::InvalidArgument(
                "--file requires a repository target".into(),
            ));
        }
        if let Some(file) = &self.file
            && (file.0.contains(&0) || file.0.is_empty())
        {
            return Err(Error::InvalidArgument(
                "File path must not be empty or contain NUL".into(),
            ));
        }
        if let Some(wsl) = &self.wsl {
            wsl.validate()?;
            if !self.repository.starts_with('/') {
                return Err(Error::InvalidArgument(
                    "WSL repository must be an absolute Linux path".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WslTarget {
    pub distribution: String,
    pub user: String,
    pub helper: String,
}

impl WslTarget {
    pub fn validate(&self) -> Result<()> {
        for value in [&self.distribution, &self.user, &self.helper] {
            if value.is_empty() || value.contains('\0') {
                return Err(Error::InvalidArgument(
                    "WSL distribution, user, and helper must be nonempty and contain no NUL".into(),
                ));
            }
        }
        if !self.helper.starts_with('/') {
            return Err(Error::InvalidArgument(
                "WSL helper must be an absolute Linux executable path".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum Command {
    Open {
        request: OpenRequest,
        native: bool,
    },
    Helper {
        repository: String,
        target: OpenTarget,
    },
    Help,
    Version,
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command> {
    let mut args = args.into_iter();
    let mut repository = None;
    let mut file = None;
    let mut view = View::All;
    let mut native = false;
    let mut theme = false;
    let mut helper = false;
    let mut document = false;
    let mut positional_only = false;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--") if !positional_only => positional_only = true,
            Some("--help" | "-h") if !positional_only => return Ok(Command::Help),
            Some("--version" | "-V") if !positional_only => return Ok(Command::Version),
            Some("--native") if !positional_only => native = true,
            Some("--theme") if !positional_only => theme = true,
            Some("--helper") if !positional_only => helper = true,
            Some("--document") if !positional_only => document = true,
            Some("--file") if !positional_only => {
                let value = args.next().ok_or_else(|| missing("--file"))?;
                file = Some(GitPath::from_path(std::path::Path::new(&value)));
            }
            Some("--view") if !positional_only => {
                let value = args.next().ok_or_else(|| missing("--view"))?;
                view = match value.to_str() {
                    Some("all") => View::All,
                    Some("unstaged") => View::Unstaged,
                    Some("staged") => View::Staged,
                    Some("unpushed") => View::Unpushed,
                    _ => {
                        return Err(Error::InvalidArgument(
                            "--view must be all, unstaged, staged, or unpushed".into(),
                        ));
                    }
                };
            }
            _ if !positional_only && arg.to_string_lossy().starts_with('-') => {
                return Err(Error::InvalidArgument(format!(
                    "Unknown option: {}",
                    arg.to_string_lossy()
                )));
            }
            _ => {
                let value = arg.into_string().map_err(|_| {
                    Error::InvalidArgument("Repository paths must be valid Unicode".into())
                })?;
                if repository.replace(value).is_some() {
                    return Err(Error::InvalidArgument(
                        "Only one repository may be supplied".into(),
                    ));
                }
            }
        }
    }
    let repository = repository.unwrap_or_else(|| ".".into());
    let target = if document || std::path::Path::new(&repository).is_file() {
        OpenTarget::Document
    } else {
        OpenTarget::Repository
    };
    let request = OpenRequest {
        target,
        repository: repository.clone(),
        file,
        view,
        wsl: None,
        theme,
    };
    request.validate()?;
    if helper {
        Ok(Command::Helper { repository, target })
    } else {
        Ok(Command::Open { request, native })
    }
}

fn missing(option: &str) -> Error {
    Error::InvalidArgument(format!("{option} requires a value"))
}
