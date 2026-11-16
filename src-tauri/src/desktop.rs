use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

#[cfg(target_os = "macos")]
use tauri::menu::{Menu, MenuItem};

use crate::error::AppError;

#[cfg(target_os = "macos")]
pub fn install_app_menu(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::menu::{PredefinedMenuItem, Submenu};

    let application = Submenu::with_items(
        app,
        "ICA proxy",
        true,
        &[
            &MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Quit ICA proxy", true, Some("CmdOrCtrl+Q"))?,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    app.set_menu(Menu::with_items(app, &[&application, &edit])?)?;
    app.on_menu_event(|app, event| match event.id.as_ref() {
        "settings" => {
            let _ = open_settings(app);
        }
        "quit" => app.exit(0),
        _ => {}
    });
    Ok(())
}

pub fn open_settings(app: &AppHandle) -> Result<(), AppError> {
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Regular)
        .map_err(|_| settings_window_error())?;
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return Ok(());
    }
    WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("index.html".into()))
        .title("ICA proxy")
        .inner_size(960.0, 840.0)
        .min_inner_size(640.0, 600.0)
        .on_navigation(|url| {
            url.scheme() == "tauri"
                || url.host_str() == Some("tauri.localhost")
                || (cfg!(debug_assertions)
                    && url.host_str() == Some("127.0.0.1")
                    && url.port() == Some(1420))
        })
        .build()
        .map_err(|_| settings_window_error())?;
    Ok(())
}

fn settings_window_error() -> AppError {
    AppError::new(
        "settings_window_failed",
        "The settings window could not be opened. Restart the app.",
    )
}
