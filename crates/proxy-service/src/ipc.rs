use std::{sync::Arc, time::Duration};

use proxy_core::Provider;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{watch, Notify},
    task::JoinSet,
    time::timeout,
};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    app::{AppController, AppStatus},
    error::AppError,
    transport::{self, Listener, Stream},
};

const PROTOCOL_VERSION: u32 = 1;
const MAX_FRAME_BYTES: usize = 128 * 1024;
const MAX_CLIENTS: usize = 8;

// Deliberately no Debug: requests and text replies may contain credentials.
#[derive(Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Identity,
    Status,
    Subscribe,
    Start,
    StartAfterSkip,
    SetRunInBackground {
        enabled: bool,
    },
    CompleteOnboarding,
    SkipOnboarding {
        draft: Option<crate::app::ProviderDraft>,
    },
    Stop,
    SaveProvider {
        id: Provider,
        endpoint: String,
        api_key: Option<String>,
    },
    DeleteProvider {
        id: Provider,
    },
    TestConnection {
        id: Provider,
        endpoint: String,
        api_key: Option<String>,
    },
    SetPort {
        port: u16,
    },
    SetHttpsPort {
        port: u16,
    },
    SetBrowserOrigins {
        origins: Vec<String>,
    },
    SetupHttps,
    TrustHttps,
    CheckHttps,
    RemoveHttps,
    ProviderKey {
        id: Provider,
    },
    CheckSavedKeys,
    Reset,
    Shutdown,
}

impl Drop for Request {
    fn drop(&mut self) {
        if let Self::SaveProvider {
            api_key: Some(key), ..
        }
        | Self::TestConnection {
            api_key: Some(key), ..
        } = self
        {
            key.zeroize();
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Response {
    Identity {
        process_id: u32,
        fingerprint: String,
        managed: bool,
        #[serde(default)]
        foreground: bool,
    },
    Status(Box<AppStatus>),
    Text(String),
    Unit,
    Error(AppError),
}

impl Drop for Response {
    fn drop(&mut self) {
        if let Self::Text(text) = self {
            text.zeroize();
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    version: u32,
    payload: T,
}

fn invalid() -> AppError {
    AppError::new("worker_protocol_error", "The settings app and background worker could not communicate. Quit settings and reopen the installed app.")
}

pub(crate) async fn write_frame<T: Serialize>(
    stream: &mut (impl AsyncWrite + Unpin + ?Sized),
    payload: T,
) -> Result<(), AppError> {
    let bytes = Zeroizing::new(
        serde_json::to_vec(&Envelope {
            version: PROTOCOL_VERSION,
            payload,
        })
        .map_err(|_| invalid())?,
    );
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(invalid());
    }
    timeout(Duration::from_secs(10), async {
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await?;
        stream.flush().await
    })
    .await
    .map_err(|_| invalid())?
    .map_err(|_| invalid())
}

pub(crate) async fn read_frame<T: DeserializeOwned>(
    stream: &mut (impl AsyncRead + Unpin + ?Sized),
) -> Result<T, AppError> {
    let length = stream
        .read_u32()
        .await
        .map_err(|_| transport::unavailable())? as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(invalid());
    }
    let mut bytes = Zeroizing::new(vec![0; length]);
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(|_| transport::unavailable())?;
    let envelope: Envelope<T> = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if envelope.version != PROTOCOL_VERSION {
        return Err(invalid());
    }
    Ok(envelope.payload)
}

pub async fn call(request: Request) -> Result<Response, AppError> {
    let mut stream = transport::connect().await?;
    write_frame(&mut stream, request).await?;
    // Allow time for a deliberate OS trust/Keychain approval. No request is retried after writing.
    let mut response = timeout(Duration::from_secs(300), read_frame(&mut stream))
        .await
        .map_err(|_| transport::unavailable())??;
    if let Response::Error(error) = &mut response {
        return Err(error.clone());
    }
    Ok(response)
}

pub struct Subscription(Stream);
impl Subscription {
    pub async fn next(&mut self) -> Result<AppStatus, AppError> {
        let mut response: Response = read_frame(&mut self.0).await?;
        match &mut response {
            Response::Status(status) => Ok((**status).clone()),
            _ => Err(invalid()),
        }
    }
}

pub async fn subscribe() -> Result<Subscription, AppError> {
    let mut stream = transport::connect().await?;
    write_frame(&mut stream, Request::Subscribe).await?;
    Ok(Subscription(stream))
}

async fn execute(controller: &AppController, request: &mut Request) -> Result<Response, AppError> {
    let status = match request {
        Request::Identity => {
            return Ok(Response::Identity {
                process_id: std::process::id(),
                fingerprint: crate::running_fingerprint(),
                managed: std::env::args().any(|argument| argument == "--managed"),
                foreground: std::env::args().any(|argument| argument == "--foreground"),
            })
        }
        Request::Status => controller.status().await,
        Request::Start => controller.start().await?,
        Request::StartAfterSkip => controller.start_after_skip().await?,
        Request::SetRunInBackground { enabled } => {
            controller.set_run_in_background(*enabled).await?
        }
        Request::CompleteOnboarding => controller.complete_onboarding().await?,
        Request::SkipOnboarding { draft } => controller.skip_onboarding(draft.take()).await?,
        Request::Stop => controller.stop().await?,
        Request::SaveProvider {
            id,
            endpoint,
            api_key,
        } => {
            controller
                .save_provider(*id, std::mem::take(endpoint), api_key.take())
                .await?
        }
        Request::DeleteProvider { id } => controller.delete_provider(*id).await?,
        Request::TestConnection {
            id,
            endpoint,
            api_key,
        } => {
            return controller
                .test_connection(*id, std::mem::take(endpoint), api_key.take())
                .await
                .map(Response::Text)
        }
        Request::SetPort { port } => controller.set_port(*port).await?,
        Request::SetHttpsPort { port } => controller.set_https_port(*port).await?,
        Request::SetBrowserOrigins { origins } => {
            controller
                .set_browser_origins(std::mem::take(origins))
                .await?
        }
        Request::SetupHttps => controller.setup_https().await?,
        Request::TrustHttps => controller.trust_https_certificate().await?,
        Request::CheckHttps => controller.check_https_trust().await?,
        Request::RemoveHttps => controller.remove_https().await?,
        Request::ProviderKey { id } => {
            return controller.provider_key(*id).await.map(Response::Text)
        }
        Request::CheckSavedKeys => {
            controller.check_saved_key_access().await?;
            return Ok(Response::Unit);
        }
        Request::Reset => controller.reset().await?,
        Request::Shutdown => return Ok(Response::Unit),
        Request::Subscribe => return Err(invalid()),
    };
    Ok(Response::Status(Box::new(status)))
}

pub(crate) async fn handle(
    mut stream: Stream,
    controller: Arc<AppController>,
    mut changes: watch::Receiver<()>,
    shutdown: Arc<Notify>,
) -> Result<(), AppError> {
    let mut request: Request = timeout(Duration::from_secs(10), read_frame(&mut stream))
        .await
        .map_err(|_| invalid())??;
    if matches!(request, Request::Subscribe) {
        loop {
            changes.borrow_and_update();
            write_frame(
                &mut stream,
                Response::Status(Box::new(controller.status().await)),
            )
            .await?;
            // Observe disconnected settings immediately, including while the proxy is idle.
            let mut byte = [0];
            tokio::select! {
                result = changes.changed() => if result.is_err() { return Ok(()); },
                _ = stream.read(&mut byte) => return Ok(()),
            }
        }
    }
    let response = execute(&controller, &mut request)
        .await
        .unwrap_or_else(Response::Error);
    let result = write_frame(&mut stream, response).await;
    if matches!(request, Request::Shutdown) {
        shutdown.notify_one();
    }
    result
}

pub(crate) async fn serve(
    mut listener: Listener,
    controller: Arc<AppController>,
    changes: watch::Receiver<()>,
    shutdown: Arc<Notify>,
) -> Result<(), AppError> {
    let mut clients = JoinSet::new();
    let signal = shutdown_signal();
    tokio::pin!(signal);
    let outcome = loop {
        tokio::select! {
            _ = shutdown.notified() => break Ok(()),
            _ = &mut signal => break Ok(()),
            Some(_) = clients.join_next(), if !clients.is_empty() => {},
            accepted = listener.accept(), if clients.len() < MAX_CLIENTS => {
                match accepted {
                    Ok(stream) => { clients.spawn(handle(stream, controller.clone(), changes.clone(), shutdown.clone())); },
                    Err(error) => break Err(error),
                }
            }
        }
    };
    clients.abort_all();
    while clients.join_next().await.is_some() {}
    outcome
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        if let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _ = terminate.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_older_worker_identity_for_upgrade_handoff() {
        let response: Response = serde_json::from_str(
            r#"{"kind":"identity","value":{"process_id":1,"fingerprint":"old-worker","managed":false}}"#,
        )
        .expect("older identity still decodes");
        assert!(matches!(
            &response,
            Response::Identity {
                foreground: false,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn rejects_oversized_frames_before_allocating_or_reading_the_body() {
        let (mut client, mut server) = tokio::io::duplex(32);
        client
            .write_u32(MAX_FRAME_BYTES as u32 + 1)
            .await
            .expect("length header");
        let error = read_frame::<Request>(&mut server)
            .await
            .err()
            .expect("oversized request rejected");
        assert_eq!(error.code, "worker_protocol_error");
    }

    #[tokio::test]
    async fn rejects_incompatible_versions_and_unknown_commands_without_echoing_secrets() {
        for json in [
            r#"{"version":999,"payload":{"command":"status"}}"#,
            r#"{"version":1,"payload":{"command":"shell","api_key":"fake-secret"}}"#,
        ] {
            let (mut client, mut server) = tokio::io::duplex(1024);
            client.write_u32(json.len() as u32).await.expect("length");
            client.write_all(json.as_bytes()).await.expect("body");
            let error = read_frame::<Request>(&mut server)
                .await
                .err()
                .expect("invalid request rejected");
            assert_eq!(error.code, "worker_protocol_error");
            assert!(!error.message.contains("fake-secret"));
        }
    }

    #[tokio::test]
    async fn transports_provider_keys_without_changing_the_api_key_auth_contract() {
        let (mut client, mut server) = tokio::io::duplex(2048);
        write_frame(
            &mut client,
            Request::SaveProvider {
                id: Provider::Claude,
                endpoint: "https://example.com".into(),
                api_key: Some("fake-provider-key".into()),
            },
        )
        .await
        .expect("request");
        let request: Request = read_frame(&mut server).await.expect("decoded request");
        assert!(
            matches!(&request, Request::SaveProvider { id: Provider::Claude, api_key: Some(key), .. } if key == "fake-provider-key")
        );
    }
}
