use std::sync::Arc;

use proxy_core::Provider;
use tauri::{State, WebviewWindow};

use crate::{
    app::{blocking, clipboard, AppController, AppStatus},
    error::AppError,
};

fn authorize(window: &WebviewWindow) -> Result<(), AppError> {
    if window.label() == "settings" {
        Ok(())
    } else {
        Err(AppError::new(
            "window_denied",
            "This window cannot change application settings.",
        ))
    }
}

#[tauri::command]
pub async fn get_status(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
) -> Result<AppStatus, AppError> {
    authorize(&window)?;
    controller.status().await
}

#[tauri::command]
pub async fn check_background_startup(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
) -> Result<AppStatus, AppError> {
    authorize(&window)?;
    controller.check_background_startup().await
}

#[tauri::command]
pub async fn open_startup_settings(window: WebviewWindow) -> Result<(), AppError> {
    authorize(&window)?;
    blocking(crate::startup::open_settings).await
}

#[tauri::command]
pub async fn save_provider(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
    id: Provider,
    upstream_base_url: String,
    api_key: Option<String>,
) -> Result<AppStatus, AppError> {
    authorize(&window)?;
    controller
        .save_provider(id, upstream_base_url, api_key)
        .await
}

#[tauri::command]
pub async fn delete_provider(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
    id: Provider,
) -> Result<AppStatus, AppError> {
    authorize(&window)?;
    controller.delete_provider(id).await
}

#[tauri::command]
pub async fn test_connection(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
    id: Provider,
    upstream_base_url: String,
    api_key: Option<String>,
) -> Result<String, AppError> {
    authorize(&window)?;
    controller
        .test_connection(id, upstream_base_url, api_key)
        .await
}

#[tauri::command]
pub async fn finish_onboarding(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
) -> Result<AppStatus, AppError> {
    authorize(&window)?;
    controller.finish_onboarding().await
}

#[tauri::command]
pub async fn skip_onboarding(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
    draft: Option<proxy_service::app::ProviderDraft>,
) -> Result<AppStatus, AppError> {
    authorize(&window)?;
    controller.skip_onboarding(draft).await
}

#[tauri::command]
pub async fn start_proxy(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
) -> Result<AppStatus, AppError> {
    authorize(&window)?;
    controller.start().await
}

#[tauri::command]
pub async fn stop_proxy(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
) -> Result<AppStatus, AppError> {
    authorize(&window)?;
    controller.stop().await
}

#[tauri::command]
pub async fn set_local_port(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
    port: u16,
) -> Result<AppStatus, AppError> {
    authorize(&window)?;
    controller.set_port(port).await
}

#[tauri::command]
pub async fn set_browser_origins(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
    origins: Vec<String>,
) -> Result<AppStatus, AppError> {
    authorize(&window)?;
    controller.set_browser_origins(origins).await
}

#[tauri::command]
pub async fn set_https_port(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
    port: u16,
) -> Result<AppStatus, AppError> {
    authorize(&window)?;
    controller.set_https_port(port).await
}

#[tauri::command]
pub async fn setup_https(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
) -> Result<AppStatus, AppError> {
    authorize(&window)?;
    controller.setup_https().await
}

#[tauri::command]
pub async fn trust_https_certificate(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
) -> Result<AppStatus, AppError> {
    authorize(&window)?;
    controller.trust_https_certificate().await
}

#[tauri::command]
pub async fn check_https_trust(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
) -> Result<AppStatus, AppError> {
    authorize(&window)?;
    controller.check_https_trust().await
}

#[tauri::command]
pub async fn remove_https(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
) -> Result<AppStatus, AppError> {
    authorize(&window)?;
    controller.remove_https().await
}

#[tauri::command]
pub async fn reveal_provider_key(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
    id: Provider,
) -> Result<String, AppError> {
    authorize(&window)?;
    controller.provider_key(id).await
}

#[tauri::command]
pub async fn check_saved_key_access(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
) -> Result<(), AppError> {
    authorize(&window)?;
    controller.check_saved_key_access().await
}

#[tauri::command]
pub async fn copy_text(window: WebviewWindow, value: String) -> Result<(), AppError> {
    authorize(&window)?;
    if value.len() > 4096 {
        return Err(AppError::new(
            "clipboard_value_too_large",
            "This clipboard value is too large.",
        ));
    }
    blocking(move || clipboard(&value)).await
}

#[tauri::command]
pub async fn reset_app_data(
    window: WebviewWindow,
    controller: State<'_, Arc<AppController>>,
) -> Result<AppStatus, AppError> {
    authorize(&window)?;
    controller.reset().await
}
