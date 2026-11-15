use thiserror::Error;

/// Safe, user-facing errors. Underlying HTTP errors can contain sensitive URLs.
#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("Use a valid HTTPS upstream base URL without credentials, a query, or a fragment.")]
    InvalidEndpoint,
    #[error("HTTP upstreams must use a literal loopback address such as 127.0.0.1 or [::1].")]
    InsecureEndpoint,
    #[error("The upstream cannot point back to this app's listening port.")]
    SelfProxy,
    #[error("Enter a nonempty API key without spaces or control characters.")]
    InvalidKey,
    #[error("Enter at most 16 unique HTTPS origins, such as https://pivot.claude.ai, without paths, trailing slashes, credentials, or wildcards.")]
    InvalidBrowserOrigins,
    #[error("The HTTP client could not be initialized.")]
    ClientInitialization,
    #[error("The connection test failed or timed out. Check the endpoint, TLS certificate, and network.")]
    ConnectionTestFailed,
    #[error("The selected port is unavailable. Choose another port and try again.")]
    PortUnavailable,
    #[error(
        "The local HTTPS certificate or private key is invalid. Set up HTTPS again in settings."
    )]
    InvalidTlsIdentity,
    #[error("The local proxy's configuration lock is unavailable. Restart the app.")]
    StateUnavailable,
}
