#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => std::process::exit(1),
    };
    let stopping = std::env::args().any(|argument| argument == "--stop");
    let result = runtime.block_on(async {
        if stopping {
            proxy_service::stop_worker().await
        } else {
            proxy_service::run().await
        }
    });
    // A blocked native approval dialog must not prevent worker replacement or sign-out forever.
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    if result.is_err() {
        eprintln!("The background proxy could not complete this operation.");
        std::process::exit(1);
    }
}
