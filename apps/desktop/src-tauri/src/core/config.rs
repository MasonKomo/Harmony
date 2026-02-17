use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const APP_CONFIG_DIR: &str = "Harmony";
const APP_CONFIG_FILE: &str = "config.json";
const DEV_CONFIG_FILE: &str = "dev-config.json";
const DEV_CONFIG_ENV: &str = "HARMONY_DEV_CONFIG";

pub const DEFAULT_SERVER_HOST: &str = "ec2-3-133-108-176.us-east-2.compute.amazonaws.com";
pub const DEFAULT_USER_PASSWORD: &str = "Hoez312!!!";
pub const SUPERUSER_TRIGGER_NICKNAME: &str = "spaceKomo";
pub const SUPERUSER_AUTH_USERNAME: &str = "SuperUser";
pub const SUPERUSER_AUTH_PASSWORD: &str = "Discourse312Gb!!!";
const LEGACY_LOCALHOST_IP: &str = "127.0.0.1";
const LEGACY_LOCALHOST_NAME: &str = "localhost";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub password: Option<String>,
    pub default_channel: String,
    #[serde(default)]
    pub allow_insecure_tls: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppConfig {
    pub nickname: String,
    #[serde(default = "default_remember_me")]
    pub remember_me: bool,
    #[serde(default)]
    pub ptt_enabled: bool,
    #[serde(default = "default_ptt_hotkey")]
    pub ptt_hotkey: String,
    #[serde(default)]
    pub input_device: Option<String>,
    #[serde(default)]
    pub output_device: Option<String>,
    #[serde(default = "default_output_volume")]
    pub output_volume: u8,
    #[serde(default = "default_auto_mute_on_deafen")]
    pub auto_mute_on_deafen: bool,
    #[serde(default)]
    pub server: ServerConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_SERVER_HOST.to_string(),
            port: 64738,
            password: Some(DEFAULT_USER_PASSWORD.to_string()),
            default_channel: "Game Night".to_string(),
            allow_insecure_tls: true,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            nickname: String::new(),
            remember_me: default_remember_me(),
            ptt_enabled: false,
            ptt_hotkey: default_ptt_hotkey(),
            input_device: None,
            output_device: None,
            output_volume: default_output_volume(),
            auto_mute_on_deafen: default_auto_mute_on_deafen(),
            server: ServerConfig::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: AppConfig,
    pub path: PathBuf,
    pub is_dev_override: bool,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to resolve config base directory")]
    NoConfigDirectory,
    #[error("failed to create config directory {path}: {source}")]
    CreateDir {
        path: String,
        source: std::io::Error,
    },
    #[error("failed to read config file {path}: {source}")]
    ReadFile {
        path: String,
        source: std::io::Error,
    },
    #[error("failed to parse config file {path}: {source}")]
    ParseFile {
        path: String,
        source: serde_json::Error,
    },
    #[error("failed to serialize config: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("failed to write config file {path}: {source}")]
    WriteFile {
        path: String,
        source: std::io::Error,
    },
}

pub fn load_config() -> Result<LoadedConfig, ConfigError> {
    if let Some(dev_path) = find_dev_config() {
        let config = read_config(&dev_path)?;
        return Ok(LoadedConfig {
            config,
            path: dev_path,
            is_dev_override: true,
        });
    }

    let path = persistent_config_path()?;
    if path.exists() {
        let mut config = read_config(&path)?;
        if apply_legacy_server_migration(&mut config) {
            save_config_to_path(&path, &config)?;
        }
        return Ok(LoadedConfig {
            config,
            path,
            is_dev_override: false,
        });
    }

    let config = AppConfig::default();
    save_config_to_path(&path, &config)?;

    Ok(LoadedConfig {
        config,
        path,
        is_dev_override: false,
    })
}

pub fn save_config_to_path(path: &Path, config: &AppConfig) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ConfigError::CreateDir {
            path: parent.display().to_string(),
            source,
        })?;
    }

    let content = serde_json::to_string_pretty(config)?;
    fs::write(path, content).map_err(|source| ConfigError::WriteFile {
        path: path.display().to_string(),
        source,
    })
}

pub fn persistent_config_path() -> Result<PathBuf, ConfigError> {
    let base_dir = dirs::config_dir().ok_or(ConfigError::NoConfigDirectory)?;
    Ok(base_dir.join(APP_CONFIG_DIR).join(APP_CONFIG_FILE))
}

fn read_config(path: &Path) -> Result<AppConfig, ConfigError> {
    let raw = fs::read_to_string(path).map_err(|source| ConfigError::ReadFile {
        path: path.display().to_string(),
        source,
    })?;
    serde_json::from_str(&raw).map_err(|source| ConfigError::ParseFile {
        path: path.display().to_string(),
        source,
    })
}

fn find_dev_config() -> Option<PathBuf> {
    if let Ok(path_from_env) = std::env::var(DEV_CONFIG_ENV) {
        let from_env = PathBuf::from(path_from_env);
        if from_env.exists() {
            return Some(from_env);
        }
    }

    let cwd = std::env::current_dir().ok()?;
    let direct = cwd.join(DEV_CONFIG_FILE);
    if direct.exists() {
        return Some(direct);
    }

    let parent = cwd.parent()?.join(DEV_CONFIG_FILE);
    if parent.exists() {
        return Some(parent);
    }

    None
}

fn apply_legacy_server_migration(config: &mut AppConfig) -> bool {
    let host = config.server.host.trim();
    let is_legacy_local = host.eq_ignore_ascii_case(LEGACY_LOCALHOST_IP)
        || host.eq_ignore_ascii_case(LEGACY_LOCALHOST_NAME);

    if is_legacy_local && config.server.password.is_none() {
        config.server.host = DEFAULT_SERVER_HOST.to_string();
        config.server.password = Some(DEFAULT_USER_PASSWORD.to_string());
        return true;
    }

    false
}

const fn default_remember_me() -> bool {
    true
}

fn default_ptt_hotkey() -> String {
    "AltLeft".to_string()
}

const fn default_output_volume() -> u8 {
    80
}

const fn default_auto_mute_on_deafen() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_round_trip_serializes() {
        let config = AppConfig::default();
        let serialized = serde_json::to_string(&config).expect("serializes config");
        let back: AppConfig = serde_json::from_str(&serialized).expect("deserializes config");
        assert_eq!(back, config);
    }

    #[test]
    fn migration_updates_legacy_localhost_config() {
        let mut config = AppConfig {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 64738,
                password: None,
                default_channel: "Game Night".to_string(),
                allow_insecure_tls: true,
            },
            ..AppConfig::default()
        };

        let migrated = apply_legacy_server_migration(&mut config);
        assert!(migrated);
        assert_eq!(config.server.host, DEFAULT_SERVER_HOST);
        assert_eq!(
            config.server.password.as_deref(),
            Some(DEFAULT_USER_PASSWORD)
        );
    }

    #[test]
    fn migration_keeps_non_legacy_server_config_untouched() {
        let mut config = AppConfig {
            server: ServerConfig {
                host: "voice.example.com".to_string(),
                port: 64738,
                password: None,
                default_channel: "Game Night".to_string(),
                allow_insecure_tls: true,
            },
            ..AppConfig::default()
        };

        let migrated = apply_legacy_server_migration(&mut config);
        assert!(!migrated);
        assert_eq!(config.server.host, "voice.example.com");
        assert_eq!(config.server.password, None);
    }
}
