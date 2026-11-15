use axum::{
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
};
use url::Url;

use crate::ProxyError;

// Keep the browser trust boundary explicit when adding future Office origins.
const OFFICE_ORIGIN: &str = "https://pivot.claude.ai";
const MAX_BROWSER_ORIGINS: usize = 16;
const MAX_ORIGIN_BYTES: usize = 2048;

/// Validated exact HTTPS origins. An empty list disables browser access.
#[derive(Clone)]
pub struct BrowserOrigins(Vec<HeaderValue>);

impl Default for BrowserOrigins {
    fn default() -> Self {
        Self(vec![HeaderValue::from_static(OFFICE_ORIGIN)])
    }
}

impl BrowserOrigins {
    /// Reject wildcards, credentials, paths, duplicates, and noncanonical origins.
    pub fn new(origins: &[String]) -> Result<Self, ProxyError> {
        if origins.len() > MAX_BROWSER_ORIGINS {
            return Err(ProxyError::InvalidBrowserOrigins);
        }
        let mut allowed = Vec::new();
        for origin in origins {
            if origin.len() > MAX_ORIGIN_BYTES || origin.contains('*') {
                return Err(ProxyError::InvalidBrowserOrigins);
            }
            let url = Url::parse(origin).map_err(|_| ProxyError::InvalidBrowserOrigins)?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || url.origin().ascii_serialization() != *origin
            {
                return Err(ProxyError::InvalidBrowserOrigins);
            }
            let value =
                HeaderValue::from_str(origin).map_err(|_| ProxyError::InvalidBrowserOrigins)?;
            if allowed.contains(&value) {
                return Err(ProxyError::InvalidBrowserOrigins);
            }
            allowed.push(value);
        }
        Ok(Self(allowed))
    }

    pub fn to_vec(&self) -> Vec<String> {
        self.0
            .iter()
            .filter_map(|value| value.to_str().ok().map(str::to_owned))
            .collect()
    }

    pub(crate) fn allowed_origin(&self, headers: &HeaderMap) -> Option<HeaderValue> {
        let mut origins = headers.get_all(header::ORIGIN).iter();
        let (Some(origin), None) = (origins.next(), origins.next()) else {
            return None;
        };
        self.0.iter().find(|allowed| *allowed == origin).cloned()
    }
}

pub(crate) fn method_allowed(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::POST | Method::OPTIONS)
}

pub(crate) fn preflight(headers: &HeaderMap) -> Result<Response, ()> {
    let mut methods = headers
        .get_all(header::ACCESS_CONTROL_REQUEST_METHOD)
        .iter();
    let (Some(method), None) = (methods.next(), methods.next()) else {
        return Err(());
    };
    let method = Method::from_bytes(method.as_bytes()).map_err(|_| ())?;
    if !method_allowed(&method) {
        return Err(());
    }

    // Echo validated names for this trusted origin, including Authorization and
    // future SDK headers. A wildcard does not cover Authorization in preflights.
    let mut requested = Vec::new();
    for value in headers.get_all(header::ACCESS_CONTROL_REQUEST_HEADERS) {
        for name in value.to_str().map_err(|_| ())?.split(',') {
            let name = HeaderName::from_bytes(name.trim().as_bytes()).map_err(|_| ())?;
            if name == "*" {
                return Err(());
            }
            requested.push(name);
        }
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    if !requested.is_empty() {
        let names = requested
            .iter()
            .map(HeaderName::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_str(&names).map_err(|_| ())?,
        );
    }
    headers.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
    headers.insert(
        header::VARY,
        HeaderValue::from_static("access-control-request-method, access-control-request-headers"),
    );
    Ok(response)
}

pub(crate) fn strip_headers(headers: &mut HeaderMap) {
    let names: Vec<_> = headers
        .keys()
        .filter(|name| name.as_str().starts_with("access-control-"))
        .cloned()
        .collect();
    for name in names {
        headers.remove(name);
    }
}

pub(crate) fn finish(response: &mut Response, allowed_origin: Option<HeaderValue>) {
    let headers = response.headers_mut();
    headers.append(header::VARY, HeaderValue::from_static("Origin"));
    if let Some(origin) = allowed_origin {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        // Explicit names also expose future rate-limit and request-ID metadata.
        let exposed = headers
            .keys()
            .filter(|name| !name.as_str().starts_with("access-control-"))
            .map(HeaderName::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        if let Ok(exposed) = HeaderValue::from_str(&exposed) {
            headers.insert(header::ACCESS_CONTROL_EXPOSE_HEADERS, exposed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_bounded_exact_https_origins() {
        for origin in [
            "*",
            "https://*.claude.ai",
            "http://pivot.claude.ai",
            "null",
            "https://user:password@pivot.claude.ai",
            "https://pivot.claude.ai/",
            "https://pivot.claude.ai/path",
            "https://pivot.claude.ai?query=true",
            "https://pivot.claude.ai#fragment",
            "https://pivot.claude.ai:443",
            "https://PIVOT.claude.ai",
            " https://pivot.claude.ai",
        ] {
            assert!(
                BrowserOrigins::new(&[origin.to_owned()]).is_err(),
                "{origin}"
            );
        }
        assert!(
            BrowserOrigins::new(&[OFFICE_ORIGIN.to_owned(), OFFICE_ORIGIN.to_owned()]).is_err()
        );
        let excessive: Vec<_> = (0..17)
            .map(|index| format!("https://office{index}.example.com"))
            .collect();
        assert!(BrowserOrigins::new(&excessive).is_err());
        assert!(
            BrowserOrigins::new(&[format!("https://{}.example.com", "a".repeat(2048))]).is_err()
        );
        let allowed = vec![
            OFFICE_ORIGIN.to_owned(),
            "https://office.example.com:8443".to_owned(),
        ];
        assert_eq!(
            BrowserOrigins::new(&allowed)
                .expect("exact origins")
                .to_vec(),
            allowed
        );
        assert!(BrowserOrigins::new(&[])
            .expect("disabled browser access")
            .to_vec()
            .is_empty());
    }
}
