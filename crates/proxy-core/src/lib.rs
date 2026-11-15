//! Loopback-only, streaming Claude/OpenAI forwarding without desktop dependencies.

mod cors;
mod endpoint;
mod error;
mod provider;
mod proxy;
mod tls;

pub use cors::BrowserOrigins;
pub use endpoint::validate_endpoint;
pub use error::ProxyError;
pub use provider::{Provider, Secret, Upstream};
pub use proxy::{Limits, ProxyOptions, RunningProxy};
pub use tls::{HttpsOptions, TlsIdentity};
