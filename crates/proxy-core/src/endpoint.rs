use percent_encoding::percent_decode_str;
use url::{Host, Url};

use crate::ProxyError;

/// Validate an endpoint without resolving remote names or making a network call.
/// HTTP is allowed only for literal loopback IPs, avoiding DNS-rebinding exceptions.
pub fn validate_endpoint(value: &str) -> Result<Url, ProxyError> {
    let mut url = Url::parse(value.trim()).map_err(|_| ProxyError::InvalidEndpoint)?;
    if url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.scheme(), "http" | "https")
    {
        return Err(ProxyError::InvalidEndpoint);
    }
    let literal_loopback = match url.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    if url.scheme() == "http" && !literal_loopback {
        return Err(ProxyError::InsecureEndpoint);
    }
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&path);
    Ok(url)
}

pub(crate) fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

pub(crate) fn destination(base: &Url, suffix: &str, query: Option<&str>) -> Option<Url> {
    let decoded = percent_decode_str(suffix).decode_utf8().ok()?;
    if decoded.contains(['\\', '%', '\0'])
        || suffix.starts_with("//")
        || decoded.split('/').any(|part| part == "." || part == "..")
    {
        return None;
    }
    let mut url = base.clone();
    let path = format!("{}{}", base.path().trim_end_matches('/'), suffix);
    url.set_path(&path);
    url.set_query(query);
    Some(url)
}
