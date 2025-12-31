//! ACME (Let's Encrypt) certificate management
//!
//! Supports automatic certificate provisioning using:
//! - HTTP-01 challenge (serves token at /.well-known/acme-challenge/)
//! - TLS-ALPN-01 challenge (serves certificate with acme-tls/1 ALPN)
//!
//! # Security Considerations
//!
//! ## Key Storage
//! ACME account keys and certificate private keys are stored in the cache directory
//! with restrictive file permissions (0600 on Unix). However, the keys are stored
//! unencrypted on disk. For production deployments:
//!
//! - Ensure the cache directory is on an encrypted filesystem
//! - Restrict access to the cache directory to the service user only
//! - Consider using a secrets manager for high-security environments
//! - Back up the cache directory securely (it contains your ACME account key)

use crate::config::{AcmeChallengeType, AcmeConfig};
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, LetsEncrypt,
    NewAccount, NewOrder, OrderStatus,
};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::ResolvesServerCert;
use rustls::sign::CertifiedKey;
use std::collections::HashMap;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, RwLock};
use tracing::{debug, error, info};

const ACME_TLS_ALPN_NAME: &[u8] = b"acme-tls/1";
const ACME_ALPN_OID: &[u64] = &[1, 3, 6, 1, 5, 5, 7, 1, 31];

/// Pending ACME challenges for HTTP-01 validation
#[derive(Clone, Default)]
pub struct Http01Challenges {
    inner: Arc<RwLock<HashMap<String, String>>>,
}

impl Http01Challenges {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn set(&self, token: String, key_authorization: String) {
        self.inner.write().await.insert(token, key_authorization);
    }

    pub async fn get(&self, token: &str) -> Option<String> {
        self.inner.read().await.get(token).cloned()
    }

    pub async fn remove(&self, token: &str) {
        self.inner.write().await.remove(token);
    }
}

/// TLS-ALPN-01 challenge certificate resolver
pub struct TlsAlpn01Resolver {
    challenge_certs: Arc<RwLock<HashMap<String, Arc<CertifiedKey>>>>,
    regular_cert: Arc<RwLock<Option<Arc<CertifiedKey>>>>,
}

impl std::fmt::Debug for TlsAlpn01Resolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsAlpn01Resolver")
            .field("challenge_certs", &"<RwLock<HashMap>>")
            .field("regular_cert", &"<RwLock<Option>>")
            .finish()
    }
}

impl TlsAlpn01Resolver {
    pub fn new() -> Self {
        Self {
            challenge_certs: Arc::new(RwLock::new(HashMap::new())),
            regular_cert: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn set_challenge_cert(&self, domain: &str, cert: Arc<CertifiedKey>) {
        self.challenge_certs
            .write()
            .await
            .insert(domain.to_string(), cert);
    }

    pub async fn remove_challenge_cert(&self, domain: &str) {
        self.challenge_certs.write().await.remove(domain);
    }

    pub async fn set_regular_cert(&self, cert: Arc<CertifiedKey>) {
        *self.regular_cert.write().await = Some(cert);
    }

    fn get_challenge_cert_sync(&self, domain: &str) -> Option<Arc<CertifiedKey>> {
        self.challenge_certs.blocking_read().get(domain).cloned()
    }

    fn get_regular_cert_sync(&self) -> Option<Arc<CertifiedKey>> {
        self.regular_cert.blocking_read().clone()
    }
}

impl ResolvesServerCert for TlsAlpn01Resolver {
    fn resolve(
        &self,
        client_hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        let is_acme_challenge = client_hello
            .alpn()
            .map(|mut alpn| alpn.any(|p| p == ACME_TLS_ALPN_NAME))
            .unwrap_or(false);

        if is_acme_challenge {
            if let Some(sni) = client_hello.server_name() {
                return self.get_challenge_cert_sync(sni);
            }
        }

        self.get_regular_cert_sync()
    }
}

/// ACME certificate manager
pub struct AcmeManager {
    config: AcmeConfig,
    cache_dir: PathBuf,
    http01_challenges: Http01Challenges,
    tls_alpn01_resolver: Arc<TlsAlpn01Resolver>,
    current_cert: Arc<RwLock<Option<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)>>>,
    cert_tx: watch::Sender<Option<Arc<CertifiedKey>>>,
    cert_rx: watch::Receiver<Option<Arc<CertifiedKey>>>,
}

impl AcmeManager {
    pub fn new(config: AcmeConfig) -> Result<Self, anyhow::Error> {
        let cache_dir = validate_cache_dir(&config.cache_dir)?;
        let (cert_tx, cert_rx) = watch::channel(None);
        Ok(Self {
            config,
            cache_dir,
            http01_challenges: Http01Challenges::new(),
            tls_alpn01_resolver: Arc::new(TlsAlpn01Resolver::new()),
            current_cert: Arc::new(RwLock::new(None)),
            cert_tx,
            cert_rx,
        })
    }

    pub fn http01_challenges(&self) -> Http01Challenges {
        self.http01_challenges.clone()
    }

    pub fn tls_alpn01_resolver(&self) -> Arc<TlsAlpn01Resolver> {
        Arc::clone(&self.tls_alpn01_resolver)
    }

    pub fn cert_receiver(&self) -> watch::Receiver<Option<Arc<CertifiedKey>>> {
        self.cert_rx.clone()
    }

    /// Load or create an ACME account
    async fn get_or_create_account(&self) -> anyhow::Result<Account> {
        let account_path = self.cache_dir.join("account.json");

        if account_path.exists() {
            debug!(path = %account_path.display(), "Loading existing ACME account");
            let data = std::fs::read_to_string(&account_path)?;
            let credentials: AccountCredentials = serde_json::from_str(&data)?;
            let account = Account::from_credentials(credentials).await?;
            return Ok(account);
        }

        info!("Creating new ACME account");
        let email = self.config.email.as_ref().ok_or_else(|| {
            anyhow::anyhow!("ACME email is required for account creation")
        })?;

        let directory_url = self
            .config
            .directory_url
            .as_deref()
            .unwrap_or(LetsEncrypt::Production.url());

        let (account, credentials) = Account::create(
            &NewAccount {
                contact: &[&format!("mailto:{}", email)],
                terms_of_service_agreed: true,
                only_return_existing: false,
            },
            directory_url,
            None,
        )
        .await?;

        // Save credentials for future use
        std::fs::create_dir_all(&self.cache_dir)?;
        let data = serde_json::to_string_pretty(&credentials)?;
        std::fs::write(&account_path, data)?;
        info!(path = %account_path.display(), "ACME account credentials saved");

        Ok(account)
    }

    /// Load cached certificate if valid
    fn load_cached_cert(&self) -> Option<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
        let cert_path = self.cache_dir.join("cert.pem");
        let key_path = self.cache_dir.join("key.pem");

        if !cert_path.exists() || !key_path.exists() {
            return None;
        }

        let cert_data = std::fs::read(&cert_path).ok()?;
        let key_data = std::fs::read(&key_path).ok()?;

        let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut BufReader::new(&cert_data[..]))
            .filter_map(|c| c.ok())
            .collect();

        if certs.is_empty() {
            return None;
        }

        let key = load_private_key(&key_data)?;

        // Check if certificate is still valid (at least 30 days remaining)
        if let Some(cert) = certs.first() {
            if !is_cert_valid_for_days(cert, 30) {
                info!("Cached certificate expires within 30 days, will renew");
                return None;
            }
        }

        info!(path = %cert_path.display(), "Loaded cached certificate");
        Some((certs, key))
    }

    /// Save certificate to cache with restricted permissions
    fn save_cert(&self, cert_chain_pem: &str, private_key_pem: &str) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.cache_dir)?;

        let cert_path = self.cache_dir.join("cert.pem");
        let key_path = self.cache_dir.join("key.pem");

        // Write certificate (can be world-readable)
        std::fs::write(&cert_path, cert_chain_pem)?;

        // Write private key with restricted permissions (0600)
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&key_path)?;
            std::io::Write::write_all(&mut file, private_key_pem.as_bytes())?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&key_path, private_key_pem)?;
        }

        info!(path = %cert_path.display(), "Certificate saved to cache");
        Ok(())
    }

    /// Obtain a new certificate via ACME
    async fn obtain_certificate(
        &self,
        account: &Account,
    ) -> anyhow::Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>, String, String)> {
        let identifiers: Vec<Identifier> = self
            .config
            .domains
            .iter()
            .map(|d| Identifier::Dns(d.clone()))
            .collect();

        info!(domains = ?self.config.domains, "Requesting new certificate");

        let mut order = account
            .new_order(&NewOrder {
                identifiers: &identifiers,
            })
            .await?;

        let authorizations = order.authorizations().await?;

        // Process each authorization
        for authz in authorizations {
            if authz.status == AuthorizationStatus::Valid {
                continue;
            }

            let identifier = match &authz.identifier {
                Identifier::Dns(domain) => domain.clone(),
            };

            let challenge_type = match self.config.challenge_type {
                AcmeChallengeType::Http01 => ChallengeType::Http01,
                AcmeChallengeType::TlsAlpn01 => ChallengeType::TlsAlpn01,
            };

            let challenge = authz
                .challenges
                .iter()
                .find(|c| c.r#type == challenge_type)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Challenge type {:?} not available for {}",
                        self.config.challenge_type,
                        identifier
                    )
                })?;

            let key_auth = order.key_authorization(challenge);
            let key_auth_str = key_auth.as_str().to_string();
            let digest: Vec<u8> = key_auth.digest().as_ref().to_vec();

            match self.config.challenge_type {
                AcmeChallengeType::Http01 => {
                    debug!(domain = %identifier, token = %challenge.token, "Setting up HTTP-01 challenge");
                    self.http01_challenges
                        .set(challenge.token.clone(), key_auth_str)
                        .await;
                }
                AcmeChallengeType::TlsAlpn01 => {
                    debug!(domain = %identifier, "Setting up TLS-ALPN-01 challenge");
                    let challenge_cert = create_tls_alpn01_cert(&identifier, &digest)?;
                    self.tls_alpn01_resolver
                        .set_challenge_cert(&identifier, challenge_cert)
                        .await;
                }
            }

            // Notify ACME server we're ready
            order.set_challenge_ready(&challenge.url).await?;

            // Wait for authorization to become valid
            let mut attempts = 0;
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;

                // Refresh the order and get authorizations again
                order.refresh().await?;
                let auths = order.authorizations().await?;
                let current_auth = auths.iter().find(|a| {
                    matches!(&a.identifier, Identifier::Dns(d) if d == &identifier)
                });

                match current_auth.map(|a| &a.status) {
                    Some(AuthorizationStatus::Valid) => {
                        info!(domain = %identifier, "Authorization valid");
                        break;
                    }
                    Some(AuthorizationStatus::Pending) => {
                        attempts += 1;
                        if attempts > 30 {
                            anyhow::bail!("Authorization timeout for {}", identifier);
                        }
                        debug!(domain = %identifier, attempt = attempts, "Waiting for authorization");
                    }
                    Some(AuthorizationStatus::Invalid) => {
                        anyhow::bail!("Authorization failed for {}", identifier);
                    }
                    Some(status) => {
                        debug!(domain = %identifier, status = ?status, "Authorization status");
                    }
                    None => {
                        anyhow::bail!("Authorization not found for {}", identifier);
                    }
                }
            }

            // Clean up challenge
            match self.config.challenge_type {
                AcmeChallengeType::Http01 => {
                    self.http01_challenges.remove(&challenge.token).await;
                }
                AcmeChallengeType::TlsAlpn01 => {
                    self.tls_alpn01_resolver.remove_challenge_cert(&identifier).await;
                }
            }
        }

        // Wait for order to be ready
        let mut attempts = 0;
        loop {
            let state = order.state();
            match state.status {
                OrderStatus::Ready => break,
                OrderStatus::Pending => {
                    attempts += 1;
                    if attempts > 30 {
                        anyhow::bail!("Order timeout");
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    order.refresh().await?;
                }
                OrderStatus::Invalid => {
                    anyhow::bail!("Order invalid");
                }
                OrderStatus::Valid => break,
                OrderStatus::Processing => {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    order.refresh().await?;
                }
            }
        }

        // Generate CSR and finalize order
        let mut params = CertificateParams::new(self.config.domains.clone())?;
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::CommonName, self.config.domains[0].clone());

        let private_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
        let csr = params.serialize_request(&private_key)?;

        order.finalize(csr.der()).await?;

        // Wait for certificate
        let mut attempts = 0;
        let cert_chain_pem: String = loop {
            order.refresh().await?;
            let state = order.state();

            match state.status {
                OrderStatus::Valid => {
                    if let Some(cert) = order.certificate().await? {
                        break cert;
                    }
                    anyhow::bail!("Order valid but no certificate returned");
                }
                OrderStatus::Processing => {
                    attempts += 1;
                    if attempts > 30 {
                        anyhow::bail!("Certificate timeout");
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                _ => anyhow::bail!("Unexpected order status: {:?}", state.status),
            }
        };

        let private_key_pem = private_key.serialize_pem();

        // Parse the certificate chain
        let certs: Vec<CertificateDer<'static>> =
            rustls_pemfile::certs(&mut BufReader::new(cert_chain_pem.as_bytes()))
                .filter_map(|c| c.ok())
                .collect();

        let key = PrivateKeyDer::try_from(private_key.serialize_der())
            .map_err(|e| anyhow::anyhow!("Failed to parse private key: {}", e))?;

        info!(domains = ?self.config.domains, "Certificate obtained successfully");

        Ok((certs, key, cert_chain_pem, private_key_pem))
    }

    /// Update the current certificate and notify watchers
    async fn update_cert(
        &self,
        certs: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
    ) -> anyhow::Result<()> {
        let signing_key = rustls::crypto::ring::sign::any_supported_type(&key)
            .map_err(|e| anyhow::anyhow!("Failed to create signing key: {}", e))?;

        let certified_key = Arc::new(CertifiedKey::new(certs.clone(), signing_key));

        // Update the resolver for TLS-ALPN-01
        self.tls_alpn01_resolver
            .set_regular_cert(Arc::clone(&certified_key))
            .await;

        // Store cert for direct access
        *self.current_cert.write().await = Some((certs, key));

        // Notify watchers
        let _ = self.cert_tx.send(Some(certified_key));

        Ok(())
    }

    /// Get the current certificate if available
    pub async fn get_current_cert(
        &self,
    ) -> Option<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
        let guard = self.current_cert.read().await;
        guard.as_ref().map(|(certs, key)| {
            (certs.clone(), key.clone_key())
        })
    }

    /// Run the ACME manager - obtains and renews certificates
    pub async fn run(&self, mut shutdown_rx: watch::Receiver<bool>) -> anyhow::Result<()> {
        // Try to load cached certificate first
        if let Some((certs, key)) = self.load_cached_cert() {
            self.update_cert(certs, key).await?;
        }

        let account = self.get_or_create_account().await?;

        // Initial certificate if not cached
        if self.current_cert.read().await.is_none() {
            match self.obtain_certificate(&account).await {
                Ok((certs, key, cert_pem, key_pem)) => {
                    self.save_cert(&cert_pem, &key_pem)?;
                    self.update_cert(certs, key).await?;
                }
                Err(e) => {
                    error!(error = %e, "Failed to obtain initial certificate");
                    return Err(e);
                }
            }
        }

        // Renewal loop - check every 12 hours
        let renewal_interval = Duration::from_secs(12 * 60 * 60);

        loop {
            tokio::select! {
                _ = tokio::time::sleep(renewal_interval) => {
                    // Check if renewal is needed (30 days before expiry)
                    let needs_renewal = {
                        let cert = self.current_cert.read().await;
                        cert.as_ref()
                            .and_then(|(certs, _)| certs.first())
                            .map(|c| !is_cert_valid_for_days(c, 30))
                            .unwrap_or(true)
                    };

                    if needs_renewal {
                        info!("Certificate renewal needed");
                        match self.obtain_certificate(&account).await {
                            Ok((certs, key, cert_pem, key_pem)) => {
                                self.save_cert(&cert_pem, &key_pem)?;
                                self.update_cert(certs, key).await?;
                                info!("Certificate renewed successfully");
                            }
                            Err(e) => {
                                error!(error = %e, "Failed to renew certificate");
                            }
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("ACME manager shutting down");
                        break;
                    }
                }
            }
        }

        Ok(())
    }
}

/// Create a TLS-ALPN-01 challenge certificate
fn create_tls_alpn01_cert(domain: &str, digest: &[u8]) -> anyhow::Result<Arc<CertifiedKey>> {
    use rcgen::{CustomExtension, IsCa, KeyUsagePurpose};

    let mut params = CertificateParams::new(vec![domain.to_string()])?;
    params.is_ca = IsCa::NoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];

    // Add the acmeIdentifier extension with the key authorization digest
    let mut ext_value = vec![0x04, 0x20]; // OCTET STRING of 32 bytes
    ext_value.extend_from_slice(digest);

    let extension = CustomExtension::from_oid_content(ACME_ALPN_OID, ext_value);
    params.custom_extensions.push(extension);

    let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
    let cert = params.self_signed(&key_pair)?;

    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der = PrivateKeyDer::try_from(key_pair.serialize_der())
        .map_err(|e| anyhow::anyhow!("Failed to serialize private key: {}", e))?;

    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)
        .map_err(|e| anyhow::anyhow!("Failed to create signing key: {}", e))?;

    Ok(Arc::new(CertifiedKey::new(vec![cert_der], signing_key)))
}

fn load_private_key(data: &[u8]) -> Option<PrivateKeyDer<'static>> {
    let mut reader = BufReader::new(data);

    loop {
        match rustls_pemfile::read_one(&mut reader) {
            Ok(Some(rustls_pemfile::Item::Pkcs1Key(key))) => return Some(key.into()),
            Ok(Some(rustls_pemfile::Item::Pkcs8Key(key))) => return Some(key.into()),
            Ok(Some(rustls_pemfile::Item::Sec1Key(key))) => return Some(key.into()),
            Ok(None) => return None,
            Ok(_) => continue,
            Err(_) => return None,
        }
    }
}

fn is_cert_valid_for_days(cert: &CertificateDer<'_>, days: u64) -> bool {
    use x509_parser::prelude::*;

    let (_, parsed) = match X509Certificate::from_der(cert.as_ref()) {
        Ok(result) => result,
        Err(e) => {
            error!(error = %e, "Failed to parse X.509 certificate");
            return false;
        }
    };

    let not_after = parsed.validity().not_after;

    // Get current time as Unix timestamp
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // Get certificate expiry as Unix timestamp
    let expiry = not_after.timestamp();

    // Check if certificate is valid for at least the required number of days
    let remaining_secs = expiry - now;
    if remaining_secs < 0 {
        info!("Certificate has already expired");
        return false;
    }

    let remaining_days = remaining_secs as u64 / (24 * 60 * 60);
    if remaining_days < days {
        info!(
            remaining_days,
            required_days = days,
            "Certificate expires soon, renewal needed"
        );
        return false;
    }

    debug!(
        remaining_days,
        expiry_timestamp = expiry,
        "Certificate validity check passed"
    );
    true
}

/// Validate and canonicalize the ACME cache directory path
fn validate_cache_dir(path: &str) -> anyhow::Result<PathBuf> {
    // Check for path traversal attempts
    if path.contains("..") {
        anyhow::bail!("ACME cache directory path must not contain '..'");
    }

    // Convert to PathBuf and canonicalize if it exists
    let path_buf = PathBuf::from(path);

    // If path exists, canonicalize it to resolve symlinks
    if path_buf.exists() {
        let canonical = path_buf.canonicalize().map_err(|e| {
            anyhow::anyhow!("Failed to canonicalize ACME cache directory '{}': {}", path, e)
        })?;

        // Verify it's a directory
        if !canonical.is_dir() {
            anyhow::bail!("ACME cache path '{}' exists but is not a directory", path);
        }

        return Ok(canonical);
    }

    // Path doesn't exist - validate the parent exists and is safe
    if let Some(parent) = path_buf.parent() {
        if parent.as_os_str().is_empty() {
            // Relative path with no parent (e.g., "acme_cache")
            return Ok(path_buf);
        }

        if parent.exists() {
            let canonical_parent = parent.canonicalize().map_err(|e| {
                anyhow::anyhow!("Failed to canonicalize parent directory: {}", e)
            })?;

            // Rebuild path with canonical parent
            if let Some(file_name) = path_buf.file_name() {
                return Ok(canonical_parent.join(file_name));
            }
        }
    }

    // Return as-is if parent doesn't exist (will fail later on create)
    Ok(path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http01_challenges() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let challenges = Http01Challenges::new();

            challenges
                .set("token123".to_string(), "key_auth_123".to_string())
                .await;

            assert_eq!(
                challenges.get("token123").await,
                Some("key_auth_123".to_string())
            );
            assert_eq!(challenges.get("nonexistent").await, None);

            challenges.remove("token123").await;
            assert_eq!(challenges.get("token123").await, None);
        });
    }

    #[test]
    fn test_acme_manager_creation() {
        let config = AcmeConfig {
            enabled: true,
            domains: vec!["example.com".to_string()],
            email: Some("admin@example.com".to_string()),
            directory_url: None,
            cache_dir: "/tmp/acme_test".to_string(),
            challenge_type: AcmeChallengeType::Http01,
        };

        let manager = AcmeManager::new(config).unwrap();
        assert!(manager.http01_challenges.inner.try_read().is_ok());
    }

    #[test]
    fn test_validate_cache_dir_rejects_traversal() {
        assert!(validate_cache_dir("../etc/passwd").is_err());
        assert!(validate_cache_dir("/tmp/../etc").is_err());
        assert!(validate_cache_dir("foo/../../bar").is_err());
    }

    #[test]
    fn test_validate_cache_dir_accepts_valid_paths() {
        assert!(validate_cache_dir("/tmp/acme").is_ok());
        assert!(validate_cache_dir("./acme_cache").is_ok());
        assert!(validate_cache_dir("acme_cache").is_ok());
    }
}
