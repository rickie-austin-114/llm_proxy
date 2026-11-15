use std::sync::Arc;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, WebviewUrl, WebviewWindowBuilder,
};

use crate::{app::AppController, error::AppError};

#[cfg(target_os = "macos")]
pub fn install_app_menu(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::menu::{PredefinedMenuItem, Submenu};

    let application = Submenu::with_items(
        app,
        "llm_proxy",
        true,
        &[
            &MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Quit llm_proxy", true, Some("CmdOrCtrl+Q"))?,
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
    Ok(())
}

fn handle_menu(app: &AppHandle, id: &str) {
    let controller = app.state::<Arc<AppController>>().inner().clone();
    match id {
        "settings" => {
            let _ = open_settings(app);
        }
        "start" => {
            tauri::async_runtime::spawn(async move {
                if let Err(error) = controller.start().await {
                    controller.report(error).await;
                }
            });
        }
        "stop" => {
            tauri::async_runtime::spawn(async move {
                let _ = controller.stop().await;
            });
        }
        "quit" => {
            tauri::async_runtime::spawn(async move {
                controller.quit().await;
            });
        }
        _ => {}
    }
}

pub fn open_settings(app: &AppHandle) -> Result<(), AppError> {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return Ok(());
    }
    WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("index.html".into()))
        .title("llm_proxy")
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
        .map_err(|_| {
            AppError::new(
                "settings_window_failed",
                "The settings window could not be opened. Restart the app.",
            )
        })?;
    Ok(())
}

pub fn install_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    app.on_menu_event(|app, event| handle_menu(app, event.id.as_ref()));
    let settings = MenuItem::with_id(app, "settings", "Open settings", true, None::<&str>)?;
    let start = MenuItem::with_id(app, "start", "Start proxy", true, None::<&str>)?;
    let stop = MenuItem::with_id(app, "stop", "Stop proxy", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit llm_proxy", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&settings, &start, &stop, &quit])?;
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?;
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("llm_proxy: Stopped")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                let _ = open_settings(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}
