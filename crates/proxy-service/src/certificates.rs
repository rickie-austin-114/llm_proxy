mod platform;

use std::{
    fs,
    io::{Read, Write},
    net::{Ipv4Addr, Ipv6Addr},
    path::PathBuf,
    sync::Arc,
};

use proxy_core::TlsIdentity;
use rcgen::{
    BasicConstraints, CertificateParams, CidrSubnet, DnType, ExtendedKeyUsagePurpose,
    GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose, NameConstraints,
};
use rustls::{
    client::{danger::ServerCertVerifier, WebPkiServerVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
    RootCertStore,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use zeroize::Zeroizing;

use crate::{credentials::CredentialStore, error::AppError};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateStatus {
    pub configured: bool,
    pub trusted: bool,
    pub expires_at: Option<i64>,
    pub fingerprint: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublicIdentity {
    schema_version: u32,
    root_der: Vec<u8>,
    server_der: Vec<u8>,
    expires_at: i64,
}

impl PublicIdentity {
    fn fingerprint(&self) -> String {
        fingerprint(&self.root_der)
    }

    fn verify(&self, roots: RootCertStore, name: &'static str) -> bool {
        let Ok(verifier) = WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build() else {
            return false;
        };
        let Ok(name) = ServerName::try_from(name) else {
            return false;
        };
        verifier
            .verify_server_cert(
                &CertificateDer::from(self.server_der.clone()),
                &[CertificateDer::from(self.root_der.clone())],
                &name,
                &[],
                UnixTime::now(),
            )
            .is_ok()
    }

    fn validate(&self) -> Result<(), AppError> {
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from(self.root_der.clone()))
            .map_err(|_| identity_error())?;
        if self.schema_version != 1
            || self.expires_at <= OffsetDateTime::now_utc().unix_timestamp()
            || !["localhost", "127.0.0.1", "::1"]
                .into_iter()
                .all(|name| self.verify(roots.clone(), name))
        {
            return Err(identity_error());
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct CertificateStore {
    path: PathBuf,
}

impl CertificateStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn load(&self) -> Result<Option<PublicIdentity>, AppError> {
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(identity_error()),
        };
        let mut contents = Vec::new();
        file.take(32 * 1024 + 1)
            .read_to_end(&mut contents)
            .map_err(|_| identity_error())?;
        if contents.len() > 32 * 1024 {
            return Err(identity_error());
        }
        serde_json::from_slice(&contents)
            .map(Some)
            .map_err(|_| identity_error())
    }

    pub fn status(&self) -> Result<CertificateStatus, AppError> {
        let Some(identity) = self.load()? else {
            return Ok(CertificateStatus::default());
        };
        let native = rustls_native_certs::load_native_certs();
        let mut roots = RootCertStore::empty();
        for certificate in native.certs {
            let _ = roots.add(certificate);
        }
        Ok(CertificateStatus {
            configured: true,
            trusted: identity.validate().is_ok() && identity.verify(roots, "localhost"),
            expires_at: Some(identity.expires_at),
            fingerprint: Some(identity.fingerprint()),
        })
    }

    pub fn identity(&self, credentials: &CredentialStore) -> Result<TlsIdentity, AppError> {
        let identity = self.load()?.ok_or_else(identity_error)?;
        identity.validate()?;
        let key = credentials.read_https_key()?.ok_or_else(identity_error)?;
        TlsIdentity::new(vec![identity.server_der, identity.root_der], key).map_err(AppError::from)
    }

    pub fn create(&self, credentials: &CredentialStore) -> Result<CertificateStatus, AppError> {
        let (identity, key) = generate()?;
        let previous = self.load()?;
        let previous_key = credentials.read_https_key()?;
        if let Some(previous) = previous.as_ref() {
            platform::remove(&previous.root_der, &previous.fingerprint())?;
        }
        credentials.save_https_key(&key)?;
        if let Err(error) = self.save(&identity) {
            let restored = match previous_key {
                Some(key) => credentials.save_https_key(&key),
                None => credentials.delete_https_key(),
            };
            restored.map_err(|_| AppError::new("https_rollback_failed", "Certificate setup failed and the previous private key could not be restored. Set up HTTPS again before starting."))?;
            return Err(error);
        }
        self.status()
    }

    fn save(&self, identity: &PublicIdentity) -> Result<(), AppError> {
        identity.validate()?;
        let parent = self.path.parent().ok_or_else(identity_error)?;
        fs::create_dir_all(parent).map_err(|_| identity_error())?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|_| identity_error())?;
        serde_json::to_writer(&mut temporary, identity).map_err(|_| identity_error())?;
        temporary.write_all(b"\n").map_err(|_| identity_error())?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|_| identity_error())?;
        temporary
            .persist(&self.path)
            .map_err(|_| identity_error())?;
        Ok(())
    }

    pub fn trust(&self) -> Result<CertificateStatus, AppError> {
        let identity = self.load()?.ok_or_else(identity_error)?;
        identity.validate()?;
        platform::install(&identity.root_der)?;
        let status = self.status()?;
        if !status.trusted {
            return Err(AppError::new("https_trust_unverified", "The certificate was submitted to the OS, but TLS trust could not be verified. Check certificate permissions, then select Check certificate trust. Restart Office after trusting it."));
        }
        Ok(status)
    }

    pub fn remove(&self, credentials: &CredentialStore) -> Result<(), AppError> {
        if let Some(identity) = self.load()? {
            platform::remove(&identity.root_der, &identity.fingerprint())?;
        }
        credentials.delete_https_key()?;
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(identity_error()),
        }
    }
}

fn generate() -> Result<(PublicIdentity, Zeroizing<Vec<u8>>), AppError> {
    let now = OffsetDateTime::now_utc();
    let ca_key = KeyPair::generate().map_err(|_| identity_error())?;
    let mut ca = CertificateParams::new(Vec::<String>::new()).map_err(|_| identity_error())?;
    ca.distinguished_name.push(
        DnType::CommonName,
        format!(
            "llm_proxy Local HTTPS {}",
            &fingerprint(ca_key.public_key_raw())[..12]
        ),
    );
    ca.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    ca.not_before = now - Duration::minutes(5);
    ca.not_after = now + Duration::days(3650);
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    ca.name_constraints = Some(NameConstraints {
        permitted_subtrees: vec![
            GeneralSubtree::DnsName("localhost".into()),
            GeneralSubtree::IpAddress(CidrSubnet::from_addr_prefix(Ipv4Addr::LOCALHOST.into(), 32)),
            GeneralSubtree::IpAddress(CidrSubnet::from_addr_prefix(
                Ipv6Addr::LOCALHOST.into(),
                128,
            )),
        ],
        excluded_subtrees: Vec::new(),
    });
    let root = ca.self_signed(&ca_key).map_err(|_| identity_error())?;
    let server_key = KeyPair::generate().map_err(|_| identity_error())?;
    let mut server =
        CertificateParams::new(vec!["localhost".into(), "127.0.0.1".into(), "::1".into()])
            .map_err(|_| identity_error())?;
    server
        .distinguished_name
        .push(DnType::CommonName, "localhost");
    server.not_before = ca.not_before;
    server.not_after = now + Duration::days(365);
    server.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    server.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf = server
        .signed_by(&server_key, &Issuer::from_params(&ca, &ca_key))
        .map_err(|_| identity_error())?;
    let identity = PublicIdentity {
        schema_version: 1,
        root_der: root.der().to_vec(),
        server_der: leaf.der().to_vec(),
        expires_at: server.not_after.unix_timestamp(),
    };
    identity.validate()?;
    // Only the server key is saved. The issuing key never leaves this function.
    Ok((identity, Zeroizing::new(server_key.serialize_der())))
}

fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect()
}

fn identity_error() -> AppError {
    AppError::new("https_identity_unavailable", "The local HTTPS certificate is missing, expired, or unavailable. Stop the proxy and set up HTTPS again. Check app-data permissions if it cannot be saved.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_unique_loopback_identities_and_keeps_private_material_out_of_files() {
        let (identity, key) = generate().expect("generate certificate");
        let (other, _) = generate().expect("another identity");
        assert_ne!(identity.fingerprint(), other.fingerprint());
        assert!(TlsIdentity::new(
            vec![identity.server_der.clone(), identity.root_der.clone()],
            key
        )
        .is_ok());
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from(identity.root_der.clone()))
            .expect("root certificate");
        assert!(!identity.verify(roots, "example.com"));
        let directory = tempfile::tempdir().expect("temporary certificate directory");
        let store = CertificateStore::new(directory.path().join("https.json"));
        store.save(&identity).expect("save public metadata");
        let saved = fs::read_to_string(&store.path).expect("saved metadata");
        assert!(!saved.contains("PRIVATE"));
        let loaded = store.load().expect("reload").expect("identity");
        assert_eq!(identity.fingerprint(), loaded.fingerprint());
        assert!(!loaded.verify(RootCertStore::empty(), "localhost"));
    }

    #[test]
    fn rejects_expired_corrupt_and_mismatched_identities() {
        let (mut identity, _) = generate().expect("identity");
        identity.expires_at = 0;
        assert!(identity.validate().is_err());
        let (mut identity, _) = generate().expect("identity");
        let (_, other_key) = generate().expect("other key");
        assert!(TlsIdentity::new(vec![identity.server_der.clone()], other_key).is_err());
        identity.server_der = vec![0];
        assert!(identity.validate().is_err());
    }

    #[test]
    fn bounds_public_metadata_and_preserves_corrupt_files_for_recovery() {
        let directory = tempfile::tempdir().expect("temporary app data");
        let path = directory.path().join("https.json");
        let store = CertificateStore::new(path.clone());
        for contents in [b"invalid JSON".to_vec(), vec![b' '; 32 * 1024 + 1]] {
            fs::write(&path, &contents).expect("invalid certificate metadata");
            assert!(store.load().is_err());
            assert_eq!(fs::read(&path).expect("original file"), contents);
        }
    }
}
