mod app;
mod commands;
mod desktop;
mod error;
mod startup;

use crate::app::AppController;
use std::sync::Arc;
#[cfg(target_os = "macos")]
use tauri::RunEvent;
use tauri::{Manager, WindowEvent};

pub fn run() {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            eprintln!("The settings runtime could not start.");
            return;
        }
    };
    if std::env::args().any(|argument| argument == "--uninstall") {
        let _ = proxy_service::config_dir().and_then(|directory| startup::disable(&directory));
        let _ = runtime.block_on(proxy_service::ipc::call(
            proxy_service::ipc::Request::Shutdown,
        ));
        return;
    }
    tauri::async_runtime::set(runtime.handle().clone());
    // Migrate an old login item without opening a window if it invokes the former flag.
    let legacy_background = std::env::args().any(|argument| argument == "--background");
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if !args.iter().any(|argument| argument == "--background") {
                let _ = desktop::open_settings(app);
            }
        }))
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::save_provider,
            commands::delete_provider,
            commands::test_connection,
            commands::start_proxy,
            commands::stop_proxy,
            commands::set_local_port,
            commands::set_browser_origins,
            commands::set_https_port,
            commands::setup_https,
            commands::trust_https_certificate,
            commands::check_https_trust,
            commands::remove_https,
            commands::reveal_provider_key,
            commands::check_saved_key_access,
            commands::copy_text,
            commands::reset_app_data,
        ])
        .setup(move |app| {
            let controller = Arc::new(AppController::new(
                app.handle().clone(),
                app.path().app_config_dir()?,
            ));
            app.manage(controller.clone());
            #[cfg(target_os = "macos")]
            desktop::install_app_menu(app.handle())?;
            if legacy_background {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let _ = controller.status().await;
                    handle.exit(0);
                });
            } else {
                desktop::open_settings(app.handle())?;
                tauri::async_runtime::spawn(controller.listen());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, WindowEvent::CloseRequested { .. }) && window.label() == "settings" {
                window.app_handle().exit(0);
            }
        });
    let application = match builder.build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(_) => {
            eprintln!(
                "llm_proxy settings could not initialize. Check installation and OS permissions."
            );
            return;
        }
    };
    application.run(|app, event| {
        #[cfg(target_os = "macos")]
        if matches!(event, RunEvent::Reopen { .. }) {
            let _ = desktop::open_settings(app);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (app, event);
    });
}
