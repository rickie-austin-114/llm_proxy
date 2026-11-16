use crate::error::AppError;

fn trust_error() -> AppError {
    AppError::new("https_trust_failed", "The OS could not update this user's certificate trust. Allow the certificate prompt and try again. A managed device may require your administrator's approval.")
}

#[cfg(target_os = "macos")]
pub fn install(der: &[u8]) -> Result<(), AppError> {
    use std::{io::Write, process::Command};

    let mut file = tempfile::NamedTempFile::new().map_err(|_| trust_error())?;
    file.write_all(der).map_err(|_| trust_error())?;
    let keychain = Command::new("/usr/bin/security")
        .args(["default-keychain", "-d", "user"])
        .output()
        .map_err(|_| trust_error())?;
    if !keychain.status.success() {
        return Err(trust_error());
    }
    let keychain = String::from_utf8(keychain.stdout).map_err(|_| trust_error())?;
    let keychain = keychain.trim().trim_matches('"');
    if keychain.is_empty() || keychain.contains('\n') {
        return Err(trust_error());
    }
    // No shell, sudo, admin-domain flag, or private key is passed to this tool.
    let status = Command::new("/usr/bin/security")
        .args(["add-trusted-cert", "-r", "trustRoot", "-p", "ssl"])
        .arg("-k")
        .arg(keychain)
        .arg(file.path())
        .output()
        .map_err(|_| trust_error())?;
    if status.status.success() {
        Ok(())
    } else {
        Err(trust_error())
    }
}

#[cfg(target_os = "macos")]
pub fn remove(_der: &[u8], fingerprint: &str) -> Result<(), AppError> {
    let output = std::process::Command::new("/usr/bin/security")
        .args(["delete-certificate", "-t", "-Z", fingerprint])
        .output()
        .map_err(|_| trust_error())?;
    // errSecItemNotFound (-25300) is returned as exit status 44 by security.
    if output.status.success() || output.status.code() == Some(44) {
        Ok(())
    } else {
        Err(trust_error())
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::{trust_error, AppError};
    use std::ptr;
    use windows_sys::Win32::Foundation::{GetLastError, CRYPT_E_NOT_FOUND};
    use windows_sys::Win32::Security::Cryptography::*;

    struct Store(HCERTSTORE);

    impl Store {
        fn open() -> Result<Self, AppError> {
            let name: Vec<u16> = "ROOT\0".encode_utf16().collect();
            // SAFETY: name is a valid NUL-terminated string for the duration of
            // the call. This API opens the current user's store, not LocalMachine.
            let store = unsafe { CertOpenSystemStoreW(0, name.as_ptr()) };
            if store.is_null() {
                Err(trust_error())
            } else {
                Ok(Self(store))
            }
        }
    }

    impl Drop for Store {
        fn drop(&mut self) {
            // SAFETY: this handle is owned and is closed exactly once.
            unsafe {
                CertCloseStore(self.0, 0);
            }
        }
    }

    pub fn install(der: &[u8]) -> Result<(), AppError> {
        let store = Store::open()?;
        let length = u32::try_from(der.len()).map_err(|_| trust_error())?;
        // SAFETY: the DER buffer is valid for length bytes. Windows copies it;
        // no certificate-context ownership is requested through the null output.
        let result = unsafe {
            CertAddEncodedCertificateToStore(
                store.0,
                X509_ASN_ENCODING,
                der.as_ptr(),
                length,
                CERT_STORE_ADD_REPLACE_EXISTING,
                ptr::null_mut(),
            )
        };
        if result == 0 {
            Err(trust_error())
        } else {
            Ok(())
        }
    }

    pub fn remove(der: &[u8], _fingerprint: &str) -> Result<(), AppError> {
        let store = Store::open()?;
        let mut previous = ptr::null_mut();
        loop {
            // SAFETY: the store is live. Enumeration frees the preceding
            // context; each newly returned context is valid until the next call.
            let certificate = unsafe { CertEnumCertificatesInStore(store.0, previous) };
            if certificate.is_null() {
                // SAFETY: read the thread-local error immediately after the API
                // call; only normal end-of-enumeration means removal is complete.
                let error = unsafe { GetLastError() };
                return if error == CRYPT_E_NOT_FOUND as u32 {
                    Ok(())
                } else {
                    Err(trust_error())
                };
            }
            previous = certificate;
            // SAFETY: Windows owns this context and its encoded buffer for the
            // current iteration. Only an exact DER match can be deleted.
            let encoded = unsafe {
                std::slice::from_raw_parts(
                    (*certificate).pbCertEncoded,
                    (*certificate).cbCertEncoded as usize,
                )
            };
            if encoded == der {
                // SAFETY: deletion consumes this owned enumeration context,
                // including on failure. Do not enumerate or free it afterward.
                let deleted = unsafe { CertDeleteCertificateFromStore(certificate) };
                return if deleted == 0 {
                    Err(trust_error())
                } else {
                    Ok(())
                };
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub use windows::{install, remove};

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn install(_der: &[u8]) -> Result<(), AppError> {
    Err(trust_error())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn remove(_der: &[u8], _fingerprint: &str) -> Result<(), AppError> {
    Err(trust_error())
}
