use std::sync::Mutex;

use keyring_core::Entry;
use proxy_core::{Provider, Secret};
use zeroize::Zeroizing;

use crate::error::AppError;

const SERVICE: &str = "app.llmproxy.desktop";
// Kept only so reset can remove credentials created by earlier builds.
const LEGACY_LOCAL_TOKEN_ID: &str = "local-access-token";
const HTTPS_KEY_ID: &str = "https-server-key";

/// Serialize native store operations, including reads, across blocking workers.
pub struct CredentialStore {
    operation: Mutex<()>,
}

impl CredentialStore {
    pub fn new() -> Result<Self, AppError> {
        #[cfg(target_os = "macos")]
        keyring_core::set_default_store(
            apple_native_keyring_store::keychain::Store::new().map_err(|_| credential_error())?,
        );
        #[cfg(target_os = "windows")]
        keyring_core::set_default_store(
            windows_native_keyring_store::Store::new().map_err(|_| credential_error())?,
        );
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        return Err(AppError::new(
            "unsupported_platform",
            "This app currently supports macOS and Windows.",
        ));
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        Ok(Self {
            operation: Mutex::new(()),
        })
    }

    pub fn read_provider(&self, provider: Provider) -> Result<Option<Secret>, AppError> {
        let _guard = self.operation.lock().map_err(|_| credential_error())?;
        read(provider.credential_id())
    }

    pub fn save_provider(&self, provider: Provider, value: Secret) -> Result<(), AppError> {
        let _guard = self.operation.lock().map_err(|_| credential_error())?;
        entry(provider.credential_id())?
            .set_password(value.expose())
            .map_err(|_| credential_error())
    }

    pub fn delete_provider(&self, provider: Provider) -> Result<(), AppError> {
        let _guard = self.operation.lock().map_err(|_| credential_error())?;
        remove(provider.credential_id())
    }

    pub fn clear(&self) -> Result<(), AppError> {
        let _guard = self.operation.lock().map_err(|_| credential_error())?;
        for id in [
            Provider::Claude.credential_id(),
            Provider::Openai.credential_id(),
            LEGACY_LOCAL_TOKEN_ID,
            HTTPS_KEY_ID,
        ] {
            remove(id)?;
        }
        Ok(())
    }

    pub fn read_https_key(&self) -> Result<Option<Zeroizing<Vec<u8>>>, AppError> {
        let _guard = self.operation.lock().map_err(|_| credential_error())?;
        match entry(HTTPS_KEY_ID)?.get_secret() {
            Ok(key) => Ok(Some(Zeroizing::new(key))),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(_) => Err(credential_error()),
        }
    }

    pub fn save_https_key(&self, key: &[u8]) -> Result<(), AppError> {
        let _guard = self.operation.lock().map_err(|_| credential_error())?;
        entry(HTTPS_KEY_ID)?
            .set_secret(key)
            .map_err(|_| credential_error())
    }

    pub fn delete_https_key(&self) -> Result<(), AppError> {
        let _guard = self.operation.lock().map_err(|_| credential_error())?;
        remove(HTTPS_KEY_ID)
    }
}

fn entry(id: &str) -> Result<Entry, AppError> {
    Entry::new(SERVICE, id).map_err(|_| credential_error())
}

fn read(id: &str) -> Result<Option<Secret>, AppError> {
    match entry(id)?.get_password() {
        Ok(value) => Ok(Some(Secret::new(value)?)),
        Err(keyring_core::Error::NoEntry) => Ok(None),
        Err(_) => Err(credential_error()),
    }
}

fn remove(id: &str) -> Result<(), AppError> {
    match entry(id)?.delete_credential() {
        Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
        Err(_) => Err(credential_error()),
    }
}

fn credential_error() -> AppError {
    #[cfg(target_os = "macos")]
    let message = "Keychain access was denied or is unavailable. Unlock your login Keychain, then try again. When macOS asks llm-proxy-worker for access, enter your login Keychain password and choose Always Allow to remember approval for this background worker.";
    #[cfg(not(target_os = "macos"))]
    let message = "The OS credential store is locked or unavailable. Unlock it or allow access, then try again. No plaintext fallback is used.";
    AppError::new("credential_store_unavailable", message)
}
