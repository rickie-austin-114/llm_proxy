use std::{path::PathBuf, sync::Arc, time::Duration};

use proxy_core::Provider;
use proxy_service::ipc::{self, Request, Response};
pub use proxy_service::{
    app::{blocking, AppStatus},
    error::AppError,
};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, OnceCell};

use crate::startup;

pub struct AppController {
    app: AppHandle,
    directory: PathBuf,
    initialized: OnceCell<()>,
    connection: Mutex<()>,
    operations: Mutex<()>,
    startup_state: Mutex<String>,
    startup_error: Mutex<Option<AppError>>,
    cached: Mutex<Option<AppStatus>>,
}

impl AppController {
    pub fn new(app: AppHandle, directory: PathBuf) -> Self {
        Self {
            app,
            directory,
            initialized: OnceCell::new(),
            connection: Mutex::new(()),
            operations: Mutex::new(()),
            startup_state: Mutex::new("unknown".into()),
            startup_error: Mutex::new(None),
            cached: Mutex::new(None),
        }
    }

    async fn registration(&self, new_setup: bool) {
        let directory = self.directory.clone();
        match blocking(move || startup::needs_worker_handoff(&directory, new_setup)).await {
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
            startup::initialize(&directory, &worker, new_setup)
        })
        .await;
        match result {
            Ok(state) => {
                *self.startup_state.lock().await = state;
                *self.startup_error.lock().await = None;
            }
            Err(error) => {
                *self.startup_state.lock().await = "unavailable".into();
                *self.startup_error.lock().await = Some(error);
            }
        }
    }

    async fn connect(&self) -> Result<(), AppError> {
        self.initialized
            .get_or_init(|| async {
                self.registration(false).await;
            })
            .await;
        let _connection = self.connection.lock().await;
        let expected =
            blocking(|| proxy_service::binary_fingerprint(&startup::worker_path()?)).await?;
        let state = self.startup_state.lock().await.clone();
        if let Ok(response) = ipc::call(Request::Identity).await {
            if matches!(&response, Response::Identity { fingerprint, managed, .. } if *fingerprint == expected && (state != "enabled" || *managed))
            {
                return Ok(());
            }
            // Replacing a running worker is an app upgrade; never send new secrets to an old binary.
            proxy_service::stop_worker().await?;
        }
        blocking(move || startup::launch(&startup::worker_path()?, &state)).await?;
        for attempt in 0..40 {
            if let Ok(response) = ipc::call(Request::Identity).await {
                if matches!(&response, Response::Identity { fingerprint, .. } if *fingerprint == expected)
                {
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_millis(if attempt < 10 { 100 } else { 300 })).await;
        }
        Err(AppError::new("worker_start_failed", "The background proxy did not start. Check OS background-app permissions and reopen settings."))
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
    pub async fn start(&self) -> Result<AppStatus, AppError> {
        let _operation = self.operations.lock().await;
        self.connect().await?;
        self.registration(true).await;
        self.status_command(Request::Start).await
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
        let _ = self
            .status_command(Request::SaveProvider {
                id,
                endpoint,
                api_key,
            })
            .await?;
        self.registration(true).await;
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
