pub mod app;
mod certificates;
mod credentials;
pub mod error;
pub mod ipc;
mod settings;
mod transport;

use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use app::AppController;
use certificates::CertificateStore;
use credentials::CredentialStore;
use error::AppError;
use settings::SettingsStore;
use tokio::sync::{watch, Notify};

static RUNNING_FINGERPRINT: OnceLock<String> = OnceLock::new();

pub fn binary_fingerprint(path: &Path) -> Result<String, AppError> {
    let mut file = std::fs::File::open(path).map_err(|_| fingerprint_error())?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 16 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|_| fingerprint_error())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn fingerprint_error() -> AppError {
    AppError::new(
        "worker_identity_unavailable",
        "The installed background worker could not be verified. Reinstall the complete app.",
    )
}

pub(crate) fn running_fingerprint() -> String {
    RUNNING_FINGERPRINT.get().cloned().unwrap_or_default()
}

pub fn user_id() -> Result<String, AppError> {
    transport::user_id()
}

pub fn config_dir() -> Result<PathBuf, AppError> {
    dirs::config_dir()
        .map(|path| path.join("app.llmproxy.desktop"))
        .ok_or_else(|| {
            AppError::new(
                "config_directory_missing",
                "Your app settings directory is unavailable.",
            )
        })
}

/// Stop the worker and wait for process exit before an installer replaces its executable.
pub async fn stop_worker() -> Result<(), AppError> {
    let response = match ipc::call(ipc::Request::Identity).await {
        Ok(response) => response,
        Err(error) if error.code == "worker_unavailable" => return Ok(()),
        Err(error) => return Err(error),
    };
    let ipc::Response::Identity { process_id, .. } = &response else {
        return Err(fingerprint_error());
    };
    let process_id = *process_id;
    #[cfg(windows)]
    let process = transport::open_process_for_wait(process_id)?;
    ipc::call(ipc::Request::Shutdown).await?;
    #[cfg(windows)]
    {
        transport::wait_process(process).await
    }
    #[cfg(unix)]
    {
        for _ in 0..100 {
            // SAFETY: signal 0 only checks whether this process still exists; it sends no signal.
            if unsafe { libc::kill(process_id as libc::pid_t, 0) } != 0 {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        Err(AppError::new(
            "worker_stop_timeout",
            "The background proxy is still stopping. Wait a moment and try again.",
        ))
    }
}

/// The worker is the only process that reads credentials or owns proxy listeners.
pub async fn run() -> Result<(), AppError> {
    let executable = std::env::current_exe().map_err(|_| fingerprint_error())?;
    let _ = RUNNING_FINGERPRINT.set(binary_fingerprint(&executable)?);
    let directory = config_dir()?;
    let listener = match transport::Listener::bind() {
        Ok(listener) => listener,
        Err(error) => {
            // A manual worker may already be running when the OS grants background approval.
            // Treat a verified existing worker as a successful duplicate launch, not a crash
            // that would trigger the OS restart policy repeatedly.
            if matches!(
                tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    ipc::call(ipc::Request::Identity)
                )
                .await,
                Ok(Ok(ipc::Response::Identity { .. }))
            ) {
                return Ok(());
            }
            return Err(error);
        }
    };
    let store = SettingsStore::new(directory.join("settings.json"));
    let settings = store.load().map(Option::unwrap_or_default);
    let should_start = settings
        .as_ref()
        .is_ok_and(|settings| !settings.providers.is_empty());
    let (changes, receiver) = watch::channel(());
    let controller = Arc::new(AppController::new(
        changes,
        store,
        settings,
        CredentialStore::new()?,
        CertificateStore::new(directory.join("https-identity.json")),
    ));
    let shutdown = Arc::new(Notify::new());
    let initialize = {
        let controller = controller.clone();
        tokio::spawn(async move {
            controller.refresh_https_status().await;
            if should_start {
                if let Err(error) = controller.start().await {
                    controller.report(error).await;
                }
            }
        })
    };
    let outcome = ipc::serve(listener, controller.clone(), receiver, shutdown).await;
    initialize.abort();
    let _ = initialize.await;
    let _ = controller.stop().await;
    outcome
}
