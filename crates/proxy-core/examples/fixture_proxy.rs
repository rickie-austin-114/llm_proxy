//! Developer fixture runner. Never reads or persists production credentials.
use proxy_core::{Limits, Provider, ProxyOptions, RunningProxy, Secret, Upstream};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proxy = RunningProxy::start(ProxyOptions {
        https: None,
        port: 4011,
        browser_origins: Default::default(),
        upstreams: vec![
            Upstream::new(
                Provider::Openai,
                "http://127.0.0.1:4010/openai/v1",
                Secret::new("fixture-openai-key".into())?,
            )?,
            Upstream::new(
                Provider::Claude,
                "http://127.0.0.1:4010/claude",
                Secret::new("fixture-claude-key".into())?,
            )?,
        ],
        limits: Limits::default(),
    })
    .await?;
    println!("Fixture proxy listening on 127.0.0.1:{}", proxy.port());
    tokio::signal::ctrl_c().await?;
    proxy.stop().await;
    Ok(())
}
