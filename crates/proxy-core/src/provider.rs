use std::{sync::Arc, time::Duration};

use reqwest::{header::HeaderValue, redirect::Policy, Client};
use serde::{Deserialize, Serialize};
use url::Url;
use zeroize::Zeroizing;

use crate::{validate_endpoint, ProxyError};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Openai,
}

impl Provider {
    pub fn default_endpoint(self) -> &'static str {
        match self {
            Self::Claude => "https://api.servicesessentials.ibm.com",
            Self::Openai => "https://api.servicesessentials.ibm.com/v1",
        }
    }

    pub fn mount(self) -> &'static str {
        match self {
            Self::Claude => "/claude",
            Self::Openai => "/openai/v1",
        }
    }

    pub fn credential_id(self) -> &'static str {
        match self {
            Self::Claude => "provider-claude",
            Self::Openai => "provider-openai",
        }
    }
}

/// A secret deliberately has no Debug or Serialize implementation.
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: String) -> Result<Self, ProxyError> {
        let value = Zeroizing::new(value);
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(ProxyError::InvalidKey);
        }
        Ok(Self(value))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

#[derive(Clone)]
pub struct Upstream {
    pub(crate) provider: Provider,
    pub(crate) base: Url,
    pub(crate) key: Arc<Secret>,
    pub(crate) client: Client,
}

impl Upstream {
    pub fn new(provider: Provider, base: &str, key: Secret) -> Result<Self, ProxyError> {
        let base = validate_endpoint(base)?;
        // A body is streamed once and must never be replayed after partial generation.
        let client = Client::builder()
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(120))
            .pool_max_idle_per_host(2)
            .pool_idle_timeout(Duration::from_secs(30))
            .http2_initial_stream_window_size(65_535)
            .http2_initial_connection_window_size(262_144)
            .build()
            .map_err(|_| ProxyError::ClientInitialization)?;
        Ok(Self {
            provider,
            base,
            key: Arc::new(key),
            client,
        })
    }

    pub fn provider(&self) -> Provider {
        self.provider
    }

    pub fn validate_proxy_port(&self, port: u16) -> Result<(), ProxyError> {
        if crate::endpoint::is_loopback(&self.base)
            && self.base.port_or_known_default() == Some(port)
        {
            return Err(ProxyError::SelfProxy);
        }
        Ok(())
    }

    pub(crate) fn auth_value(&self) -> Result<HeaderValue, ProxyError> {
        let value = Zeroizing::new(match self.provider {
            Provider::Claude => self.key.expose().to_owned(),
            Provider::Openai => format!("Bearer {}", self.key.expose()),
        });
        let mut header = HeaderValue::from_str(&value).map_err(|_| ProxyError::InvalidKey)?;
        header.set_sensitive(true);
        Ok(header)
    }

    /// A non-generation model-list request; response bodies are never retained.
    pub async fn test_connection(&self) -> Result<u16, ProxyError> {
        let suffix = match self.provider {
            Provider::Claude => "/v1/models",
            Provider::Openai => "/models",
        };
        let query = match self.provider {
            Provider::Claude => Some("limit=1"),
            Provider::Openai => None,
        };
        let url = crate::endpoint::destination(&self.base, suffix, query)
            .ok_or(ProxyError::InvalidEndpoint)?;
        let mut request = self.client.get(url);
        request = match self.provider {
            Provider::Claude => request
                .header("x-api-key", self.auth_value()?)
                .header("anthropic-version", "2023-06-01"),
            Provider::Openai => request.header("authorization", self.auth_value()?),
        };
        let response = tokio::time::timeout(Duration::from_secs(15), request.send())
            .await
            .map_err(|_| ProxyError::ConnectionTestFailed)?
            .map_err(|_| ProxyError::ConnectionTestFailed)?;
        Ok(response.status().as_u16())
    }
}
