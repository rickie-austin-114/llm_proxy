use std::{fs, io::Write, path::PathBuf};

use proxy_core::{validate_endpoint, BrowserOrigins, Provider};
use serde::{Deserialize, Serialize};

use crate::error::AppError;

const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedProvider {
    pub id: Provider,
    pub upstream_base_url: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub schema_version: u32,
    pub local_port: u16,
    #[serde(default)]
    pub https_enabled: bool,
    #[serde(default = "default_https_port")]
    pub https_port: u16,
    pub start_at_login: bool,
    #[serde(default = "default_browser_origins")]
    pub browser_origins: Vec<String>,
    pub providers: Vec<SavedProvider>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            local_port: 8787,
            https_enabled: false,
            https_port: default_https_port(),
            start_at_login: true,
            browser_origins: default_browser_origins(),
            providers: Vec::new(),
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(AppError::new("unsupported_settings", "These settings were created by a different app version. Restore a compatible version or reset settings."));
        }
        validate_port(self.local_port)?;
        validate_port(self.https_port)?;
        if self.https_enabled {
            validate_ports(self.local_port, self.https_port)?;
        }
        BrowserOrigins::new(&self.browser_origins)?;
        let mut seen = Vec::new();
        for provider in &self.providers {
            validate_endpoint(&provider.upstream_base_url)?;
            if seen.contains(&provider.id) {
                return Err(AppError::new(
                    "invalid_settings",
                    "Duplicate provider settings were found. Reset settings to recover.",
                ));
            }
            seen.push(provider.id);
        }
        Ok(())
    }
}

fn default_browser_origins() -> Vec<String> {
    BrowserOrigins::default().to_vec()
}

fn default_https_port() -> u16 {
    8788
}

pub fn validate_ports(http: u16, https: u16) -> Result<(), AppError> {
    validate_port(http)?;
    validate_port(https)?;
    if http == https {
        return Err(AppError::new(
            "conflicting_ports",
            "Choose different ports for HTTP and HTTPS.",
        ));
    }
    Ok(())
}

pub fn validate_port(port: u16) -> Result<(), AppError> {
    if port < 1024 {
        return Err(AppError::new(
            "invalid_port",
            "Choose a local port between 1024 and 65535.",
        ));
    }
    Ok(())
}

#[derive(Clone)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> Result<Option<Settings>, AppError> {
        let data =
            match fs::read(&self.path) {
                Ok(data) => data,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(_) => return Err(AppError::new(
                    "settings_unreadable",
                    "Saved settings cannot be read. Check app-data permissions or reset settings.",
                )),
            };
        let settings: Settings = serde_json::from_slice(&data).map_err(|_| AppError::new("settings_invalid", "Saved settings are invalid. Reset app data to recover; existing data has not been overwritten."))?;
        settings.validate()?;
        Ok(Some(settings))
    }

    pub fn save(&self, settings: &Settings) -> Result<(), AppError> {
        settings.validate()?;
        let parent = self.path.parent().ok_or_else(settings_write_error)?;
        fs::create_dir_all(parent).map_err(|_| settings_write_error())?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|_| settings_write_error())?;
        serde_json::to_writer_pretty(&mut temporary, settings)
            .map_err(|_| settings_write_error())?;
        temporary
            .write_all(b"\n")
            .map_err(|_| settings_write_error())?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|_| settings_write_error())?;
        temporary
            .persist(&self.path)
            .map_err(|_| settings_write_error())?;
        Ok(())
    }
}

fn settings_write_error() -> AppError {
    AppError::new(
        "settings_write_failed",
        "Settings could not be saved. Check available disk space and app-data permissions.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_replaces_settings_without_credentials() {
        let directory = tempfile::tempdir().expect("temporary app data");
        let path = directory.path().join("settings.json");
        let store = SettingsStore::new(path.clone());
        assert!(store.load().expect("first launch").is_none());
        let mut settings = Settings {
            start_at_login: false,
            ..Settings::default()
        };
        store.save(&settings).expect("initial save");
        settings.local_port = 9000;
        settings
            .browser_origins
            .push("https://office.example.com".into());
        settings.providers.push(SavedProvider {
            id: Provider::Claude,
            upstream_base_url: "https://api.anthropic.com".into(),
        });
        store.save(&settings).expect("atomic replacement");
        let reloaded = store.load().expect("reload").expect("saved settings");
        assert_eq!(reloaded.local_port, 9000);
        assert!(!reloaded.start_at_login);
        assert_eq!(reloaded.browser_origins, settings.browser_origins);
        let saved = fs::read_to_string(path).expect("settings contents");
        assert!(!saved.contains("apiKey"));
        assert!(!saved.contains("localToken"));
    }

    #[test]
    fn upgrades_existing_settings_and_preserves_an_explicit_browser_opt_out() {
        let existing =
            r#"{"schemaVersion":1,"localPort":8787,"startAtLogin":false,"providers":[]}"#;
        let mut settings: Settings =
            serde_json::from_str(existing).expect("previous settings schema");
        assert_eq!(settings.browser_origins, default_browser_origins());
        assert!(!settings.start_at_login);
        settings.browser_origins.clear();
        let directory = tempfile::tempdir().expect("temporary settings");
        let store = SettingsStore::new(directory.path().join("settings.json"));
        store.save(&settings).expect("disable browser access");
        assert!(store
            .load()
            .expect("reload")
            .expect("saved settings")
            .browser_origins
            .is_empty());
        settings.browser_origins.push("https://*.claude.ai".into());
        assert!(store.save(&settings).is_err());
        assert!(store
            .load()
            .expect("previous valid settings")
            .expect("saved settings")
            .browser_origins
            .is_empty());
    }

    #[test]
    fn refuses_invalid_settings_without_overwriting_them() {
        let directory = tempfile::tempdir().expect("temporary app data");
        let path = directory.path().join("settings.json");
        fs::write(&path, "broken JSON").expect("invalid fixture");
        let store = SettingsStore::new(path.clone());
        assert_eq!(
            store.load().err().expect("invalid settings").code,
            "settings_invalid"
        );
        assert_eq!(
            fs::read_to_string(path).expect("original data"),
            "broken JSON"
        );
    }

    #[test]
    fn upgrades_http_settings_without_enabling_https_or_changing_saved_ports() {
        let existing =
            r#"{"schemaVersion":1,"localPort":8788,"startAtLogin":false,"providers":[]}"#;
        let mut settings: Settings = serde_json::from_str(existing).expect("old HTTP settings");
        settings
            .validate()
            .expect("disabled HTTPS must not invalidate an existing HTTP port");
        assert_eq!(settings.local_port, 8788);
        assert_eq!(settings.https_port, 8788);
        assert!(!settings.https_enabled);
        settings.https_enabled = true;
        assert!(settings.validate().is_err());
        settings.https_port = 8789;
        settings.validate().expect("distinct HTTPS port");
        let round_trip: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).expect("serialize"))
                .expect("deserialize");
        assert!(round_trip.https_enabled);
        assert_eq!(round_trip.https_port, 8789);
    }
}
