use std::{path::PathBuf, sync::Arc, time::Duration};

use proxy_core::Provider;
use proxy_service::ipc::{self, Request, Response};
pub use proxy_service::{
    app::{blocking, AppStatus},
    error::AppError,
};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, OnceCell};

use crate::{startup, worker};

pub struct AppController {
    app: AppHandle,
    directory: PathBuf,
    initialized: OnceCell<()>,
    connection: Mutex<worker::Connection>,
    operations: Mutex<()>,
    startup_state: Mutex<String>,
    startup_operations: Mutex<()>,
    startup_error: Mutex<Option<AppError>>,
    cached: Mutex<Option<AppStatus>>,
    foreground_owner: Mutex<Option<std::process::ChildStdin>>,
}

impl AppController {
    pub fn new(app: AppHandle, directory: PathBuf) -> Self {
        Self {
            app,
            directory,
            initialized: OnceCell::new(),
            connection: Mutex::new(worker::Connection::default()),
            operations: Mutex::new(()),
            startup_state: Mutex::new("unknown".into()),
            startup_operations: Mutex::new(()),
            startup_error: Mutex::new(None),
            cached: Mutex::new(None),
            foreground_owner: Mutex::new(None),
        }
    }

    async fn registration(&self, intent: startup::Intent) {
        let _startup = self.startup_operations.lock().await;
        let directory = self.directory.clone();
        match blocking(move || startup::needs_worker_handoff(&directory, intent)).await {
            Ok(true) => {
                if let Err(error) = proxy_service::stop_worker().await {
                    *self.startup_error.lock().await = Some(error);
                    return;
                }
            }
            Ok(false) => {}
            Err(error) => {
                *self.startup_error.lock().await = Some(error);
                return;
            }
        }
        let directory = self.directory.clone();
        let result = blocking(move || {
            let worker = startup::worker_path()?;
            startup::initialize(&directory, &worker, intent)
        })
        .await;
        match result {
            Ok(state) => {
                *self.startup_state.lock().await = state;
                *self.startup_error.lock().await = None;
            }
            Err(error) => {
                *self.startup_state.lock().await = if error.code == "startup_approval_required" {
                    "approval_required"
                } else {
                    "unavailable"
                }
                .into();
                *self.startup_error.lock().await = Some(error);
            }
        }
    }

    async fn connect(&self) -> Result<(), AppError> {
        self.initialized
            .get_or_init(|| async {
                self.registration(startup::Intent::Inspect).await;
            })
            .await;
        let mut connection = self.connection.lock().await;
        let expected =
            blocking(|| proxy_service::binary_fingerprint(&startup::worker_path()?)).await?;
        let state = self.startup_state.lock().await.clone();
        let owns_foreground = self.foreground_owner.lock().await.is_some();
        match connection
            .connect(
                self,
                &expected,
                &state,
                owns_foreground,
                cfg!(target_os = "macos") && !cfg!(debug_assertions),
            )
            .await
        {
            Err(error) if error.code == "worker_start_failed" => {
                Err(self.startup_error.lock().await.clone().unwrap_or(error))
            }
            result => result,
        }
    }

    async fn decorate(&self, mut status: AppStatus) -> AppStatus {
        status.background_startup = self.startup_state.lock().await.clone();
        if status.last_error.is_none() {
            status.last_error = self.startup_error.lock().await.clone();
        }
        *self.cached.lock().await = Some(status.clone());
        status
    }

    async fn status_command(&self, request: Request) -> Result<AppStatus, AppError> {
        self.connect().await?;
        let response = ipc::call(request).await?;
        if let Response::Status(status) = &response {
            Ok(self.decorate((**status).clone()).await)
        } else {
            Err(protocol_error())
        }
    }

    async fn text_command(&self, request: Request) -> Result<String, AppError> {
        self.connect().await?;
        let mut response = ipc::call(request).await?;
        if let Response::Text(text) = &mut response {
            Ok(std::mem::take(text))
        } else {
            Err(protocol_error())
        }
    }

    pub async fn status(&self) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        self.status_command(Request::Status).await
    }

    pub async fn check_background_startup(&self) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        // This is an explicit retry. Ordinary status reads keep using the cached registration
        // state, avoiding repeated OS registration attempts or settings prompts.
        // Wait for an in-flight first inspection, or initialize before this repair.
        // The explicit retry must still run when the subscription won the initialization race.
        self.initialized
            .get_or_init(|| async { self.registration(startup::Intent::Inspect).await })
            .await;
        self.registration(startup::Intent::Repair).await;
        self.status_command(Request::Status).await
    }

    pub async fn start(&self) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        let status = self.status_command(Request::Status).await?;
        if !status.onboarding_complete && !status.onboarding_skipped {
            return self.finish_onboarding_inner(true).await;
        }
        self.connect().await?;
        self.registration(if status.onboarding_skipped {
            startup::Intent::Enroll
        } else {
            startup::Intent::Inspect
        })
        .await;
        self.status_command(if status.onboarding_skipped {
            Request::StartAfterSkip
        } else {
            Request::Start
        })
        .await
    }

    pub async fn skip_onboarding(
        &self,
        draft: Option<proxy_service::app::ProviderDraft>,
    ) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        self.status_command(Request::SkipOnboarding { draft }).await
    }

    pub async fn finish_onboarding(&self, run_in_background: bool) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        self.finish_onboarding_inner(run_in_background).await
    }

    async fn finish_onboarding_inner(
        &self,
        run_in_background: bool,
    ) -> Result<AppStatus, AppError> {
        let status = self.status_command(Request::CheckHttps).await?;
        let setup_pending = !status.onboarding_complete;
        // Completed users can enable background operation with their current settings,
        // including HTTP-only settings chosen after onboarding.
        if setup_pending {
            status.validate_onboarding_prerequisites()?;
        }
        if run_in_background {
            self.status_command(Request::SetRunInBackground { enabled: true })
                .await?;
            self.registration(startup::Intent::Enroll).await;
        } else {
            let directory = self.directory.clone();
            blocking(move || startup::disable_enrolled(&directory)).await?;
            proxy_service::stop_worker().await?;
            *self.startup_state.lock().await = "foreground".into();
            *self.startup_error.lock().await = None;
            self.status_command(Request::SetRunInBackground { enabled: false })
                .await?;
        }
        let status = self.status_command(Request::Start).await?;
        if !setup_pending {
            Ok(status)
        } else if !run_in_background
            || matches!(
                status.background_startup.as_str(),
                "enabled" | "development"
            )
        {
            self.status_command(Request::CompleteOnboarding).await
        } else {
            // Keep the final step resumable while the OS requires approval or repair.
            Ok(status)
        }
    }
    pub async fn stop(&self) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        self.status_command(Request::Stop).await
    }
    pub async fn save_provider(
        &self,
        id: Provider,
        endpoint: String,
        api_key: Option<String>,
    ) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        let status = self
            .status_command(Request::SaveProvider {
                id,
                endpoint,
                api_key,
            })
            .await?;
        if status.onboarding_complete {
            self.registration(startup::Intent::Inspect).await;
        }
        self.status_command(Request::Status).await
    }
    pub async fn delete_provider(&self, id: Provider) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        self.status_command(Request::DeleteProvider { id }).await
    }
    pub async fn test_connection(
        &self,
        id: Provider,
        endpoint: String,
        api_key: Option<String>,
    ) -> Result<String, AppError> {
        self.text_command(Request::TestConnection {
            id,
            endpoint,
            api_key,
        })
        .await
    }
    pub async fn set_port(&self, port: u16) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        self.status_command(Request::SetPort { port }).await
    }
    pub async fn set_https_port(&self, port: u16) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        self.status_command(Request::SetHttpsPort { port }).await
    }
    pub async fn set_browser_origins(&self, origins: Vec<String>) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        self.status_command(Request::SetBrowserOrigins { origins })
            .await
    }
    pub async fn setup_https(&self) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        self.status_command(Request::SetupHttps).await
    }
    pub async fn trust_https_certificate(&self) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        self.status_command(Request::TrustHttps).await
    }
    pub async fn check_https_trust(&self) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        self.status_command(Request::CheckHttps).await
    }
    pub async fn remove_https(&self) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        self.status_command(Request::RemoveHttps).await
    }
    pub async fn provider_key(&self, id: Provider) -> Result<String, AppError> {
        self.text_command(Request::ProviderKey { id }).await
    }
    pub async fn check_saved_key_access(&self) -> Result<(), AppError> {
        self.connect().await?;
        match ipc::call(Request::CheckSavedKeys).await? {
            Response::Unit => Ok(()),
            _ => Err(protocol_error()),
        }
    }

    pub async fn reset(&self) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        // Delete secrets/trust first. SMAppService unregister may immediately terminate the worker.
        let mut status = self.status_command(Request::Reset).await?;
        let directory = self.directory.clone();
        blocking(move || startup::disable(&directory)).await?;
        proxy_service::stop_worker().await?;
        *self.startup_state.lock().await = "disabled".into();
        *self.startup_error.lock().await = None;
        status.background_startup = "disabled".into();
        *self.cached.lock().await = Some(status.clone());
        Ok(status)
    }

    pub async fn report(&self, error: AppError) {
        if let Some(mut status) = self.cached.lock().await.clone() {
            if status.background_startup == "disabled"
                && !status.providers.iter().any(|provider| provider.has_key)
            {
                return;
            }
            status.phase = proxy_service::app::Phase::Error;
            status.last_error = Some(error);
            let _ = self.app.emit("app-status", status);
        }
    }

    pub async fn listen(self: Arc<Self>) {
        if let Err(error) = self.connect().await {
            self.report(error).await;
            return;
        }
        let mut failures = 0;
        loop {
            match ipc::subscribe().await {
                Ok(mut subscription) => loop {
                    match subscription.next().await {
                        Ok(status) => {
                            failures = 0;
                            let status = self.decorate(status).await;
                            let _ = self.app.emit("app-status", status);
                        }
                        Err(error) => {
                            self.report(error).await;
                            break;
                        }
                    }
                },
                Err(error) => self.report(error).await,
            }
            failures = (failures + 1).min(6);
            tokio::time::sleep(Duration::from_secs(1 << (failures - 1))).await;
        }
    }
}

impl worker::Host for AppController {
    async fn identity(&self) -> Result<worker::Identity, AppError> {
        let response = tokio::time::timeout(Duration::from_secs(1), ipc::call(Request::Identity))
            .await
            .map_err(|_| {
                AppError::new("worker_unavailable", "The proxy worker did not respond.")
            })??;
        if let Response::Identity {
            fingerprint,
            managed,
            foreground,
            ..
        } = &response
        {
            Ok(worker::Identity {
                fingerprint: fingerprint.clone(),
                managed: *managed,
                foreground: *foreground,
            })
        } else {
            Err(protocol_error())
        }
    }

    async fn stop(&self) -> Result<(), AppError> {
        proxy_service::stop_worker().await
    }

    async fn launch(&self, state: &str) -> Result<(), AppError> {
        let state = state.to_owned();
        *self.foreground_owner.lock().await =
            blocking(move || startup::launch(&startup::worker_path()?, &state)).await?;
        Ok(())
    }

    async fn recover(&self) -> Result<String, AppError> {
        self.registration(startup::Intent::Recover).await;
        if let Some(error) = self.startup_error.lock().await.clone() {
            return Err(error);
        }
        Ok(self.startup_state.lock().await.clone())
    }

    async fn pause(&self, attempt: usize) {
        tokio::time::sleep(Duration::from_millis(if attempt < 10 { 100 } else { 300 })).await;
    }
}

fn protocol_error() -> AppError {
    AppError::new(
        "worker_protocol_error",
        "The background worker returned an incompatible response. Reinstall the complete app.",
    )
}

pub fn clipboard(value: &str) -> Result<(), AppError> {
    let error = || {
        AppError::new(
            "clipboard_unavailable",
            "The clipboard is unavailable. Select and copy the value manually.",
        )
    };
    let mut clipboard = arboard::Clipboard::new().map_err(|_| error())?;
    clipboard.set_text(value).map_err(|_| error())
}
