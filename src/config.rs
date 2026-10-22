use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_BASE_URL: &str = "https://apps.usos.pw.edu.pl";

/// Per-user settings. They hold the user's own API key, so they live in the user's
/// config directory and never in the project.
#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    pub base_url: String,
    pub consumer_key: String,
    pub consumer_secret: String,
    /// Preferred language of names in USOS, `pl` or `en`; the other is the fallback.
    #[serde(default = "default_language")]
    pub language: String,
}

fn default_language() -> String {
    "pl".into()
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Token {
    pub oauth_token: String,
    pub oauth_token_secret: String,
}

pub fn dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tuios")
}

pub fn settings_path() -> PathBuf {
    dir().join("config.toml")
}

/// None when the user has not set up the program yet.
pub fn load_settings() -> Result<Option<Settings>> {
    let path = settings_path();
    match fs::read_to_string(&path) {
        Ok(text) => Ok(Some(
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

pub fn save_settings(settings: &Settings) -> Result<()> {
    write_private("config.toml", &toml::to_string(settings)?)
}

pub fn load_token() -> Option<Token> {
    let text = fs::read_to_string(dir().join("token.json")).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_token(token: &Token) -> Result<()> {
    write_private("token.json", &serde_json::to_string(token)?)
}

/// Writes a file only its owner can read; both files grant access to USOS as the user.
fn write_private(name: &str, contents: &str) -> Result<()> {
    let dir = dir();
    fs::create_dir_all(&dir)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(dir.join(name))?;
    // `mode` applies only on creation; an older file may have looser permissions.
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(contents.as_bytes())?;
    Ok(())
}
