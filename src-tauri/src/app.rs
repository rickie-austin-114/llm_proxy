use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use proxy_core::{
    BrowserOrigins, HttpsOptions, Limits, Provider, ProxyOptions, RunningProxy, Secret, Upstream,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_autostart::ManagerExt;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use crate::{
    certificates::{CertificateStatus, CertificateStore},
    credentials::CredentialStore,
    error::AppError,
    settings::{validate_ports, SavedProvider, Settings, SettingsStore},
};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub id: u64,
    pub timestamp: u64,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub id: Provider,
    pub upstream_base_url: String,
    pub local_base_url: String,
    pub https_base_url: Option<String>,
    pub has_key: bool,
    pub credential_state: String,
    pub last_test: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    pub phase: Phase,
    pub local_port: u16,
    pub https_port: u16,
    pub https_enabled: bool,
    pub certificate: CertificateStatus,
    pub start_at_login: bool,
    pub browser_origins: Vec<String>,
    pub providers: Vec<ProviderStatus>,
    pub last_error: Option<AppError>,
    pub diagnostics: Vec<Diagnostic>,
    pub settings_need_reset: bool,
}

struct Model {
    settings: Settings,
    certificate: CertificateStatus,
    proxy: Option<RunningProxy>,
    phase: Phase,
    key_states: HashMap<Provider, String>,
    last_tests: HashMap<Provider, String>,
    last_error: Option<AppError>,
    settings_need_reset: bool,
    diagnostics: VecDeque<Diagnostic>,
    next_diagnostic_id: u64,
}

pub struct AppController {
    app: AppHandle,
    model: Mutex<Model>,
    operations: Mutex<()>,
    credentials: Arc<CredentialStore>,
    store: SettingsStore,
    certificates: CertificateStore,
    pub is_quitting: AtomicBool,
}

impl AppController {
    pub fn new(
        app: AppHandle,
        store: SettingsStore,
        settings: Result<Settings, AppError>,
        credentials: CredentialStore,
        certificates: CertificateStore,
    ) -> Self {
        let (settings, last_error, settings_need_reset) = match settings {
            Ok(settings) => (settings, None, false),
            Err(error) => (Settings::default(), Some(error), true),
        };
        Self {
            app,
            model: Mutex::new(Model {
                settings,
                certificate: CertificateStatus::default(),
                proxy: None,
                phase: Phase::Stopped,
                key_states: HashMap::new(),
                last_tests: HashMap::new(),
                last_error,
                settings_need_reset,
                diagnostics: VecDeque::new(),
                next_diagnostic_id: 0,
            }),
            operations: Mutex::new(()),
            credentials: Arc::new(credentials),
            store,
            certificates,
            is_quitting: AtomicBool::new(false),
        }
    }

    pub async fn status(&self) -> AppStatus {
        let model = self.model.lock().await;
        let providers = [Provider::Claude, Provider::Openai]
            .into_iter()
            .map(|id| {
                let saved = model.settings.providers.iter().find(|p| p.id == id);
                let key_state =
                    model.key_states.get(&id).cloned().unwrap_or_else(|| {
                        if saved.is_some() { "saved" } else { "missing" }.into()
                    });
                ProviderStatus {
                    id,
                    upstream_base_url: saved.map_or_else(
                        || id.default_endpoint().to_owned(),
                        |p| p.upstream_base_url.clone(),
                    ),
                    local_base_url: format!(
                        "http://127.0.0.1:{}{}",
                        model.settings.local_port,
                        id.mount()
                    ),
                    https_base_url: (model.settings.https_enabled && model.certificate.configured)
                        .then(|| {
                            format!(
                                "https://localhost:{}{}",
                                model.settings.https_port,
                                id.mount()
                            )
                        }),
                    has_key: saved.is_some() && key_state != "missing",
                    credential_state: key_state,
                    last_test: model.last_tests.get(&id).cloned(),
                }
            })
            .collect();
        AppStatus {
            phase: model.phase,
            local_port: model.settings.local_port,
            https_port: model.settings.https_port,
            https_enabled: model.settings.https_enabled,
            certificate: model.certificate.clone(),
            start_at_login: model.settings.start_at_login,
            browser_origins: model.settings.browser_origins.clone(),
            providers,
            last_error: model.last_error.clone(),
            diagnostics: model.diagnostics.iter().rev().cloned().collect(),
            settings_need_reset: model.settings_need_reset,
        }
    }

    async fn publish(&self) {
        let status = self.status().await;
        let _ = self.app.emit("app-status", &status);
        if let Some(tray) = self.app.tray_by_id("main") {
            let label = match status.phase {
                Phase::Running => "Running",
                Phase::Starting => "Starting",
                Phase::Stopping => "Stopping",
                Phase::Stopped => "Stopped",
                Phase::Error => "Needs attention",
            };
            let _ = tray.set_tooltip(Some(format!("llm_proxy: {label}")));
        }
    }

    pub async fn report(&self, error: AppError) {
        self.record(error.code, &error.message).await;
        self.model.lock().await.last_error = Some(error);
        self.publish().await;
    }

    async fn record(&self, code: &str, message: &str) {
        let mut model = self.model.lock().await;
        if model.diagnostics.len() == 64 {
            model.diagnostics.pop_front();
        }
        let id = model.next_diagnostic_id;
        model.next_diagnostic_id = model.next_diagnostic_id.wrapping_add(1);
        model.diagnostics.push_back(Diagnostic {
            id,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            code: code.into(),
            message: message.into(),
        });
    }

    async fn editable_settings(&self) -> Result<Settings, AppError> {
        if self.is_quitting.load(Ordering::Relaxed) {
            return Err(AppError::new("app_quitting", "The app is shutting down."));
        }
        let model = self.model.lock().await;
        if model.settings_need_reset {
            return Err(AppError::new(
                "settings_invalid",
                "Reset app data before saving or starting the proxy.",
            ));
        }
        Ok(model.settings.clone())
    }

    async fn persist(&self, settings: Settings) -> Result<(), AppError> {
        let store = self.store.clone();
        blocking(move || store.save(&settings)).await
    }

    async fn upstreams(&self, settings: &Settings) -> Vec<Upstream> {
        let mut upstreams = Vec::new();
        for provider in &settings.providers {
            let credentials = self.credentials.clone();
            let id = provider.id;
            let key = blocking(move || credentials.read_provider(id)).await;
            let (state, result) = match key {
                Ok(Some(key)) => (
                    "available",
                    Upstream::new(id, &provider.upstream_base_url, key).map_err(AppError::from),
                ),
                Ok(None) => (
                    "missing",
                    Err(AppError::new(
                        "missing_credential",
                        "A saved provider key is missing. Re-enter it in settings.",
                    )),
                ),
                Err(error) => ("locked", Err(error)),
            };
            self.model.lock().await.key_states.insert(id, state.into());
            match result {
                Ok(upstream) => upstreams.push(upstream),
                Err(error) => self.report(error).await,
            }
        }
        upstreams
    }

    async fn refresh_running(&self, settings: &Settings) -> Result<(), AppError> {
        let is_running = self.model.lock().await.proxy.is_some();
        if is_running {
            let upstreams = self.upstreams(settings).await;
            if let Some(proxy) = self.model.lock().await.proxy.as_ref() {
                proxy.replace_upstreams(upstreams)?;
            }
        }
        Ok(())
    }

    pub async fn start(&self) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        let settings = self.editable_settings().await?;
        let browser_origins = BrowserOrigins::new(&settings.browser_origins)?;
        if self.model.lock().await.proxy.is_some() {
            return Ok(self.status().await);
        }
        if settings.providers.is_empty() {
            return Err(AppError::new(
                "no_providers",
                "Save a Claude or OpenAI API key before starting the proxy.",
            ));
        }
        let https = if settings.https_enabled {
            let certificates = self.certificates.clone();
            let credentials = self.credentials.clone();
            Some(HttpsOptions {
                port: settings.https_port,
                identity: blocking(move || certificates.identity(&credentials)).await?,
            })
        } else {
            None
        };
        {
            let mut model = self.model.lock().await;
            model.phase = Phase::Starting;
            model.last_error = None;
        }
        self.publish().await;
        let upstreams = self.upstreams(&settings).await;
        let result = if upstreams.is_empty() {
            Err(AppError::new("no_available_providers", "No provider credentials are available. Unlock the OS credential store or re-enter a key, then start again."))
        } else {
            RunningProxy::start(ProxyOptions {
                port: settings.local_port,
                https,
                browser_origins,
                upstreams,
                limits: Limits::default(),
            })
            .await
            .map_err(AppError::from)
        };
        match result {
            Ok(proxy) => {
                let mut model = self.model.lock().await;
                model.proxy = Some(proxy);
                model.phase = Phase::Running;
            }
            Err(error) => {
                self.model.lock().await.phase = Phase::Error;
                self.report(error.clone()).await;
                return Err(error);
            }
        }
        self.record(
            "proxy_started",
            "Local proxy started on loopback interfaces.",
        )
        .await;
        self.publish().await;
        Ok(self.status().await)
    }

    pub async fn stop(&self) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        self.stop_inner().await;
        Ok(self.status().await)
    }

    async fn stop_inner(&self) {
        let proxy = {
            let mut model = self.model.lock().await;
            model.phase = Phase::Stopping;
            model.proxy.take()
        };
        self.publish().await;
        if let Some(proxy) = proxy {
            proxy.stop().await;
        }
        self.model.lock().await.phase = Phase::Stopped;
        self.record(
            "proxy_stopped",
            "Local listeners and active requests stopped.",
        )
        .await;
        self.publish().await;
    }

    pub async fn save_provider(
        &self,
        id: Provider,
        endpoint: String,
        api_key: Option<String>,
    ) -> Result<AppStatus, AppError> {
        let new_key = supplied_key(api_key)?;
        let _operation = self.operations.lock().await;
        let mut settings = self.editable_settings().await?;
        let endpoint = proxy_core::validate_endpoint(&endpoint)?
            .to_string()
            .trim_end_matches('/')
            .to_owned();
        let credentials = self.credentials.clone();
        let previous = blocking(move || credentials.read_provider(id)).await?;
        let selected = new_key
            .as_ref()
            .or(previous.as_ref())
            .ok_or_else(|| AppError::new("missing_key", "Enter an API key for this provider."))?;
        // Construct the client before committing either persistence operation.
        let upstream = Upstream::new(id, &endpoint, Secret::new(selected.expose().to_owned())?)?;
        upstream.validate_proxy_port(settings.local_port)?;
        if settings.https_enabled {
            upstream.validate_proxy_port(settings.https_port)?;
        }
        let changed_key = new_key.is_some();
        if let Some(key) = new_key {
            let credentials = self.credentials.clone();
            blocking(move || credentials.save_provider(id, key)).await?;
        }
        settings.providers.retain(|provider| provider.id != id);
        settings.providers.push(SavedProvider {
            id,
            upstream_base_url: endpoint,
        });
        if let Err(error) = self.persist(settings.clone()).await {
            if changed_key {
                let credentials = self.credentials.clone();
                blocking(move || match previous { Some(key) => credentials.save_provider(id, key), None => credentials.delete_provider(id) }).await.map_err(|_| AppError::new("credential_rollback_failed", "Settings could not be saved and the previous key could not be restored. Re-enter the correct key before restarting."))?;
            }
            return Err(error);
        }
        {
            let mut model = self.model.lock().await;
            model.settings = settings.clone();
            model.last_error = None;
            model.last_tests.remove(&id);
            model.key_states.insert(id, "available".into());
        }
        self.refresh_running(&settings).await?;
        self.record("provider_saved", "Provider endpoint and credential saved.")
            .await;
        self.publish().await;
        Ok(self.status().await)
    }

    pub async fn delete_provider(&self, id: Provider) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        let mut settings = self.editable_settings().await?;
        let credentials = self.credentials.clone();
        let previous = blocking(move || {
            let previous = credentials.read_provider(id)?;
            credentials.delete_provider(id)?;
            Ok(previous)
        })
        .await?;
        settings.providers.retain(|provider| provider.id != id);
        if let Err(error) = self.persist(settings.clone()).await {
            if let Some(key) = previous {
                let credentials = self.credentials.clone();
                blocking(move || credentials.save_provider(id, key)).await?;
            }
            return Err(error);
        }
        {
            let mut model = self.model.lock().await;
            model.settings = settings.clone();
            model.key_states.insert(id, "missing".into());
            model.last_tests.remove(&id);
            model.last_error = None;
        }
        self.refresh_running(&settings).await?;
        self.record(
            "provider_removed",
            "Provider configuration and credential removed.",
        )
        .await;
        self.publish().await;
        Ok(self.status().await)
    }

    pub async fn test_connection(
        &self,
        id: Provider,
        endpoint: String,
        api_key: Option<String>,
    ) -> Result<String, AppError> {
        let supplied = supplied_key(api_key)?;
        let key = match supplied {
            Some(key) => key,
            None => {
                let credentials = self.credentials.clone();
                blocking(move || credentials.read_provider(id))
                    .await?
                    .ok_or_else(|| {
                        AppError::new("missing_key", "Enter or save an API key before testing.")
                    })?
            }
        };
        let upstream = Upstream::new(id, &endpoint, key)?;
        let settings = self.model.lock().await.settings.clone();
        upstream.validate_proxy_port(settings.local_port)?;
        if settings.https_enabled {
            upstream.validate_proxy_port(settings.https_port)?;
        }
        let status = upstream.test_connection().await?;
        let message = match status {
            200..=299 => "Connection verified using model listing. No generation request was made."
                .to_owned(),
            401 | 403 => return Err(AppError::new(
                "upstream_authentication",
                "The upstream rejected this key or its permissions. Check the key and endpoint.",
            )),
            404 => {
                return Err(AppError::new(
                    "upstream_path",
                    "Model listing was not found. Check the base URL and its version prefix.",
                ))
            }
            429 => {
                return Err(AppError::new(
                    "upstream_rate_limit",
                    "The upstream is rate-limiting requests. Wait and try again.",
                ))
            }
            _ => {
                return Err(AppError::new(
                    "upstream_error",
                    format!(
                        "The model-list test returned HTTP {status}. Check the upstream service."
                    ),
                ))
            }
        };
        self.model
            .lock()
            .await
            .last_tests
            .insert(id, message.clone());
        self.publish().await;
        Ok(message)
    }

    pub async fn set_port(&self, port: u16) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        let mut settings = self.editable_settings().await?;
        if self.model.lock().await.proxy.is_some() {
            return Err(AppError::new(
                "stop_before_port_change",
                "Stop the proxy before changing its local port.",
            ));
        }
        validate_ports(port, settings.https_port)?;
        settings.local_port = port;
        self.persist(settings.clone()).await?;
        self.model.lock().await.settings = settings;
        self.publish().await;
        Ok(self.status().await)
    }

    async fn stopped_settings(&self) -> Result<Settings, AppError> {
        let settings = self.editable_settings().await?;
        if self.model.lock().await.proxy.is_some() {
            return Err(AppError::new(
                "stop_before_https_change",
                "Stop the proxy before changing HTTPS settings.",
            ));
        }
        Ok(settings)
    }

    pub async fn refresh_https_status(&self) {
        let store = self.certificates.clone();
        match blocking(move || store.status()).await {
            Ok(status) => self.model.lock().await.certificate = status,
            Err(error) => self.report(error).await,
        }
        self.publish().await;
    }

    pub async fn check_https_trust(&self) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        let store = self.certificates.clone();
        let certificate = blocking(move || store.status()).await?;
        self.model.lock().await.certificate = certificate;
        self.publish().await;
        Ok(self.status().await)
    }

    pub async fn set_https_port(&self, port: u16) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        let mut settings = self.stopped_settings().await?;
        validate_ports(settings.local_port, port)?;
        settings.https_port = port;
        self.persist(settings.clone()).await?;
        self.model.lock().await.settings = settings;
        self.publish().await;
        Ok(self.status().await)
    }

    pub async fn setup_https(&self) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        let mut settings = self.stopped_settings().await?;
        validate_ports(settings.local_port, settings.https_port)?;
        let store = self.certificates.clone();
        let credentials = self.credentials.clone();
        let certificate = blocking(move || store.create(&credentials)).await?;
        settings.https_enabled = true;
        self.persist(settings.clone()).await?;
        {
            let mut model = self.model.lock().await;
            model.settings = settings;
            model.certificate = certificate;
            model.last_error = None;
        }
        self.record(
            "https_configured",
            "Local HTTPS certificate created. Trust it before connecting Office.",
        )
        .await;
        self.publish().await;
        Ok(self.status().await)
    }

    pub async fn trust_https_certificate(&self) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        self.editable_settings().await?;
        let store = self.certificates.clone();
        let credentials = self.credentials.clone();
        let certificate = blocking(move || {
            store.identity(&credentials)?;
            store.trust()
        })
        .await?;
        {
            let mut model = self.model.lock().await;
            model.certificate = certificate;
            model.last_error = None;
        }
        self.record(
            "https_trusted",
            "Local HTTPS certificate trust verified. Restart Office before reconnecting.",
        )
        .await;
        self.publish().await;
        Ok(self.status().await)
    }

    pub async fn remove_https(&self) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        let mut settings = self.stopped_settings().await?;
        let store = self.certificates.clone();
        let credentials = self.credentials.clone();
        blocking(move || store.remove(&credentials)).await?;
        settings.https_enabled = false;
        self.persist(settings.clone()).await?;
        {
            let mut model = self.model.lock().await;
            model.settings = settings;
            model.certificate = CertificateStatus::default();
        }
        self.record(
            "https_removed",
            "HTTPS disabled; local certificate trust and private key removed.",
        )
        .await;
        self.publish().await;
        Ok(self.status().await)
    }

    pub async fn set_browser_origins(&self, origins: Vec<String>) -> Result<AppStatus, AppError> {
        BrowserOrigins::new(&origins)?;
        let _operation = self.operations.lock().await;
        let mut settings = self.editable_settings().await?;
        if self.model.lock().await.proxy.is_some() {
            return Err(AppError::new(
                "stop_before_origin_change",
                "Stop the proxy before changing trusted browser origins.",
            ));
        }
        settings.browser_origins = origins;
        self.persist(settings.clone()).await?;
        self.model.lock().await.settings = settings;
        self.publish().await;
        Ok(self.status().await)
    }

    pub async fn set_start_at_login(&self, enabled: bool) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        let mut settings = self.editable_settings().await?;
        let app = self.app.clone();
        let previous = blocking(move || {
            let previous = app.autolaunch().is_enabled().map_err(|_| startup_error())?;
            update_startup(&app, enabled)?;
            Ok(previous)
        })
        .await?;
        settings.start_at_login = enabled;
        if let Err(error) = self.persist(settings.clone()).await {
            let app = self.app.clone();
            if blocking(move || update_startup(&app, previous))
                .await
                .is_err()
            {
                return Err(AppError::new("startup_rollback_failed", "Settings could not be saved. Check the startup entry in OS settings before trying again."));
            }
            return Err(error);
        }
        self.model.lock().await.settings = settings;
        self.publish().await;
        Ok(self.status().await)
    }

    pub async fn sync_startup_registration(&self, initialize: bool) {
        if initialize {
            let enabled = self.model.lock().await.settings.start_at_login;
            if let Err(error) = self.set_start_at_login(enabled).await {
                self.report(error).await;
            }
        }
        let app = self.app.clone();
        match blocking(move || app.autolaunch().is_enabled().map_err(|_| startup_error())).await {
            Ok(enabled) => {
                self.model.lock().await.settings.start_at_login = enabled;
            }
            Err(_) => self.report(startup_error()).await,
        }
        self.publish().await;
    }

    pub async fn provider_key(&self, id: Provider) -> Result<String, AppError> {
        let credentials = self.credentials.clone();
        let key = blocking(move || credentials.read_provider(id))
            .await?
            .ok_or_else(|| AppError::new("missing_key", "No key is stored for this provider."))?;
        Ok(key.expose().to_owned())
    }

    pub async fn reset(&self) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        self.stop_inner().await;
        let app = self.app.clone();
        blocking(move || update_startup(&app, false)).await?;
        let certificates = self.certificates.clone();
        let credentials = self.credentials.clone();
        blocking(move || certificates.remove(&credentials)).await?;
        let credentials = self.credentials.clone();
        blocking(move || credentials.clear()).await?;
        let settings = Settings {
            start_at_login: false,
            ..Settings::default()
        };
        self.persist(settings.clone()).await?;
        {
            let mut model = self.model.lock().await;
            model.settings = settings;
            model.certificate = CertificateStatus::default();
            model.key_states.clear();
            model.last_tests.clear();
            model.last_error = None;
            model.settings_need_reset = false;
            model.diagnostics.clear();
        }
        self.publish().await;
        Ok(self.status().await)
    }

    pub async fn quit(&self) {
        if self.is_quitting.swap(true, Ordering::Relaxed) {
            return;
        }
        // An OS credential prompt must not keep an explicit Quit waiting forever.
        let _ = tokio::time::timeout(Duration::from_secs(6), async {
            let _operation = self.operations.lock().await;
            self.stop_inner().await;
        })
        .await;
        self.app.exit(0);
    }
}

fn supplied_key(value: Option<String>) -> Result<Option<Secret>, AppError> {
    value
        .map(|value| {
            let value = Zeroizing::new(value);
            Secret::new(value.trim().to_owned()).map_err(AppError::from)
        })
        .transpose()
}

fn update_startup(app: &AppHandle, enabled: bool) -> Result<(), AppError> {
    if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    }
    .map_err(|_| startup_error())
}

pub async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(work).await.map_err(|_| {
        AppError::new(
            "background_operation_failed",
            "An OS operation failed. Try again or restart the app.",
        )
    })?
}

pub fn clipboard(value: &str) -> Result<(), AppError> {
    let mut clipboard = arboard::Clipboard::new().map_err(|_| clipboard_error())?;
    clipboard.set_text(value).map_err(|_| clipboard_error())
}

fn clipboard_error() -> AppError {
    AppError::new(
        "clipboard_unavailable",
        "The clipboard is unavailable. Select and copy the value manually.",
    )
}
fn startup_error() -> AppError {
    AppError::new("startup_registration_failed", "Startup registration could not be updated. Check this app's permissions and the OS login/startup settings.")
}
