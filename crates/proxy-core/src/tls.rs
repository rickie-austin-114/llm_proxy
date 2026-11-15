use std::sync::Arc;

use rustls::{pki_types::PrivatePkcs8KeyDer, ServerConfig};
use zeroize::Zeroizing;

use crate::ProxyError;

/// A validated server identity. No private material is exposed through Debug or IPC.
#[derive(Clone)]
pub struct TlsIdentity(pub(crate) Arc<ServerConfig>);

impl TlsIdentity {
    pub fn new(chain: Vec<Vec<u8>>, key: Zeroizing<Vec<u8>>) -> Result<Self, ProxyError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|_| ProxyError::InvalidTlsIdentity)?
            .with_no_client_auth()
            .with_single_cert(
                chain.into_iter().map(Into::into).collect(),
                PrivatePkcs8KeyDer::from(key.to_vec()).into(),
            )
            .map_err(|_| ProxyError::InvalidTlsIdentity)?;
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Ok(Self(Arc::new(config)))
    }
}

pub struct HttpsOptions {
    /// Zero chooses an ephemeral port for isolated integration tests.
    pub port: u16,
    pub identity: TlsIdentity,
}
