mod app;
mod certificates;
mod commands;
mod credentials;
mod desktop;
mod error;
mod settings;

use std::sync::{atomic::Ordering, Arc};

use tauri::{Manager, RunEvent, WindowEvent};
#[cfg(target_os = "macos")]
use tauri_plugin_autostart::MacosLauncher;

use crate::{
    app::AppController, certificates::CertificateStore, credentials::CredentialStore,
    settings::SettingsStore,
};

pub fn run() {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            eprintln!("The application runtime could not start.");
            return;
        }
    };
    tauri::async_runtime::set(runtime.handle().clone());
    let background = std::env::args().any(|argument| argument == "--background");
    let autostart = tauri_plugin_autostart::Builder::new().args(["--background"]);
    #[cfg(target_os = "macos")]
    let autostart = autostart.macos_launcher(MacosLauncher::LaunchAgent);
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if !args.iter().any(|argument| argument == "--background") {
                let _ = desktop::open_settings(app);
            }
        }))
        .plugin(autostart.build())
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
            commands::set_start_at_login,
            commands::reveal_provider_key,
            commands::copy_text,
            commands::reset_app_data,
        ])
        .setup(move |app| {
            let config_dir = app.path().app_config_dir()?;
            let path = config_dir.join("settings.json");
            let store = SettingsStore::new(path);
            let loaded = store.load();
            let initialize_startup = matches!(&loaded, Ok(None));
            let settings = loaded.map(Option::unwrap_or_default);
            let should_start = background
                && settings
                    .as_ref()
                    .is_ok_and(|settings| settings.start_at_login);
            let controller = Arc::new(AppController::new(
                app.handle().clone(),
                store,
                settings,
                CredentialStore::new()?,
                CertificateStore::new(config_dir.join("https-identity.json")),
            ));
            app.manage(controller.clone());
            #[cfg(target_os = "macos")]
            desktop::install_app_menu(app.handle())?;
            desktop::install_tray(app.handle())?;
            if !background {
                desktop::open_settings(app.handle())?;
            }
            tauri::async_runtime::spawn(async move {
                controller.refresh_https_status().await;
                controller
                    .sync_startup_registration(initialize_startup)
                    .await;
                if should_start {
                    if let Err(error) = controller.start().await {
                        controller.report(error).await;
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let window = window.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = window.destroy();
                });
            }
        });
    let application = match builder.build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(_) => {
            eprintln!("llm_proxy could not initialize. Check installation and OS permissions.");
            return;
        }
    };
    application.run(|app, event| match event {
        RunEvent::ExitRequested { api, code, .. } => {
            let quitting = app
                .state::<Arc<AppController>>()
                .is_quitting
                .load(Ordering::Relaxed);
            if code.is_none() && !quitting {
                api.prevent_exit();
            }
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => {
            let _ = desktop::open_settings(app);
        }
        _ => {}
    });
}
