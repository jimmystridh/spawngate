//! Custom domain management for PaaS applications
//!
//! Provides domain registration, DNS verification, and automatic SSL
//! certificate provisioning via Let's Encrypt ACME.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::ToSocketAddrs;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Domain configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainConfig {
    /// The domain name (e.g., "example.com" or "*.example.com")
    pub domain: String,
    /// The app this domain routes to
    pub app_name: String,
    /// Whether the domain ownership has been verified
    pub verified: bool,
    /// Whether SSL is enabled for this domain
    pub ssl_enabled: bool,
    /// DNS verification token
    pub verification_token: Option<String>,
    /// Certificate expiry (if SSL enabled)
    pub cert_expires_at: Option<String>,
}

impl DomainConfig {
    pub fn new(domain: &str, app_name: &str) -> Self {
        Self {
            domain: domain.to_string(),
            app_name: app_name.to_string(),
            verified: false,
            ssl_enabled: false,
            verification_token: Some(generate_verification_token()),
            cert_expires_at: None,
        }
    }

    /// Check if this is a wildcard domain
    pub fn is_wildcard(&self) -> bool {
        self.domain.starts_with("*.")
    }

    /// Get the base domain (without wildcard prefix)
    pub fn base_domain(&self) -> &str {
        if self.is_wildcard() {
            &self.domain[2..]
        } else {
            &self.domain
        }
    }

    /// Check if a hostname matches this domain config
    pub fn matches(&self, hostname: &str) -> bool {
        if self.is_wildcard() {
            // Wildcard matches any subdomain
            let base = self.base_domain();
            hostname.ends_with(base) && hostname != base
        } else {
            hostname == self.domain
        }
    }
}

/// Domain manager for tracking and routing domains
pub struct DomainManager {
    /// Map of domain -> config
    domains: RwLock<HashMap<String, DomainConfig>>,
    /// Map of app_name -> list of domains
    app_domains: RwLock<HashMap<String, Vec<String>>>,
}

impl DomainManager {
    pub fn new() -> Self {
        Self {
            domains: RwLock::new(HashMap::new()),
            app_domains: RwLock::new(HashMap::new()),
        }
    }

    /// Add a domain for an app with a verification token
    pub async fn add_domain(&self, domain: &str, app_name: &str, verification_token: &str) -> DomainConfig {
        let normalized = normalize_domain(domain).unwrap_or_else(|_| domain.to_lowercase());

        let config = DomainConfig {
            domain: normalized.clone(),
            app_name: app_name.to_string(),
            verified: false,
            ssl_enabled: false,
            verification_token: Some(verification_token.to_string()),
            cert_expires_at: None,
        };

        let mut domains = self.domains.write().await;
        let mut app_domains = self.app_domains.write().await;

        domains.insert(normalized.clone(), config.clone());

        app_domains
            .entry(app_name.to_string())
            .or_insert_with(Vec::new)
            .push(normalized.clone());

        info!(domain = %normalized, app = app_name, "Domain added");
        config
    }

    /// Add a domain from database record
    pub async fn add_domain_from_db(
        &self,
        domain: &str,
        app_name: &str,
        verified: bool,
        ssl_enabled: bool,
        verification_token: Option<String>,
        cert_expires_at: Option<String>,
    ) {
        let normalized = normalize_domain(domain).unwrap_or_else(|_| domain.to_lowercase());

        let config = DomainConfig {
            domain: normalized.clone(),
            app_name: app_name.to_string(),
            verified,
            ssl_enabled,
            verification_token,
            cert_expires_at,
        };

        let mut domains = self.domains.write().await;
        let mut app_domains = self.app_domains.write().await;

        domains.insert(normalized.clone(), config);

        app_domains
            .entry(app_name.to_string())
            .or_insert_with(Vec::new)
            .push(normalized);
    }

    /// Add a domain for an app (for testing, generates its own token)
    pub async fn add_domain_auto(&self, domain: &str, app_name: &str) -> Result<DomainConfig> {
        let normalized = normalize_domain(domain)?;

        let config = DomainConfig::new(&normalized, app_name);

        let mut domains = self.domains.write().await;
        let mut app_domains = self.app_domains.write().await;

        // Check if domain already exists
        if domains.contains_key(&normalized) {
            anyhow::bail!("Domain {} is already registered", normalized);
        }

        domains.insert(normalized.clone(), config.clone());

        app_domains
            .entry(app_name.to_string())
            .or_insert_with(Vec::new)
            .push(normalized.clone());

        info!(domain = %normalized, app = app_name, "Domain added");
        Ok(config)
    }

    /// Remove a domain
    pub async fn remove_domain(&self, domain: &str) -> Result<Option<DomainConfig>> {
        let normalized = normalize_domain(domain)?;

        let mut domains = self.domains.write().await;
        let mut app_domains = self.app_domains.write().await;

        if let Some(config) = domains.remove(&normalized) {
            // Remove from app's domain list
            if let Some(list) = app_domains.get_mut(&config.app_name) {
                list.retain(|d| d != &normalized);
            }
            info!(domain = %normalized, "Domain removed");
            Ok(Some(config))
        } else {
            Ok(None)
        }
    }

    /// Get domain configuration
    pub async fn get_domain(&self, domain: &str) -> Option<DomainConfig> {
        let normalized = normalize_domain(domain).ok()?;
        self.domains.read().await.get(&normalized).cloned()
    }

    /// Get all domains for an app
    pub async fn get_app_domains(&self, app_name: &str) -> Vec<DomainConfig> {
        let domains = self.domains.read().await;
        let app_domains = self.app_domains.read().await;

        app_domains
            .get(app_name)
            .map(|list| {
                list.iter()
                    .filter_map(|d| domains.get(d).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Find which app a hostname routes to
    pub async fn resolve_app(&self, hostname: &str) -> Option<String> {
        let domains = self.domains.read().await;

        // First try exact match
        if let Some(config) = domains.get(hostname) {
            if config.verified {
                return Some(config.app_name.clone());
            }
        }

        // Then try wildcard matches
        for config in domains.values() {
            if config.is_wildcard() && config.verified && config.matches(hostname) {
                return Some(config.app_name.clone());
            }
        }

        None
    }

    /// Update domain verification status
    pub async fn set_verified(&self, domain: &str, verified: bool) -> Result<()> {
        let normalized = normalize_domain(domain)?;
        let mut domains = self.domains.write().await;

        if let Some(config) = domains.get_mut(&normalized) {
            config.verified = verified;
            if verified {
                config.verification_token = None;
            }
            info!(domain = %normalized, verified = verified, "Domain verification updated");
            Ok(())
        } else {
            anyhow::bail!("Domain {} not found", normalized)
        }
    }

    /// Update SSL status for a domain
    pub async fn set_ssl_enabled(&self, domain: &str, enabled: bool, expires_at: Option<String>) -> Result<()> {
        let normalized = normalize_domain(domain)?;
        let mut domains = self.domains.write().await;

        if let Some(config) = domains.get_mut(&normalized) {
            config.ssl_enabled = enabled;
            config.cert_expires_at = expires_at;
            info!(domain = %normalized, ssl = enabled, "SSL status updated");
            Ok(())
        } else {
            anyhow::bail!("Domain {} not found", normalized)
        }
    }

    /// Get all domains
    pub async fn list_all_domains(&self) -> Vec<DomainConfig> {
        self.domains.read().await.values().cloned().collect()
    }

    /// Load domains from database records
    pub async fn load_from_records(&self, records: Vec<DomainConfig>) {
        let mut domains = self.domains.write().await;
        let mut app_domains = self.app_domains.write().await;

        for config in records {
            app_domains
                .entry(config.app_name.clone())
                .or_insert_with(Vec::new)
                .push(config.domain.clone());
            domains.insert(config.domain.clone(), config);
        }
    }
}

impl Default for DomainManager {
    fn default() -> Self {
        Self::new()
    }
}

/// DNS verification result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsVerificationResult {
    pub domain: String,
    pub verified: bool,
    pub method: String,
    pub expected_value: Option<String>,
    pub found_value: Option<String>,
    pub error: Option<String>,
}

/// DNS verifier for checking domain ownership
pub struct DnsVerifier {
    /// Verification prefix for TXT records
    txt_prefix: String,
}

impl DnsVerifier {
    pub fn new() -> Self {
        Self {
            txt_prefix: "_spawngate".to_string(),
        }
    }

    /// Get the DNS record name for verification
    pub fn verification_record_name(&self, domain: &str) -> String {
        format!("{}.{}", self.txt_prefix, domain)
    }

    /// Verify domain ownership via DNS TXT record (simplified interface)
    pub async fn verify(&self, domain: &str, expected_token: &str) -> Result<bool> {
        let result = self.verify_txt(domain, expected_token).await;
        Ok(result.verified)
    }

    /// Verify domain ownership via DNS TXT record
    pub async fn verify_txt(&self, domain: &str, expected_token: &str) -> DnsVerificationResult {
        let record_name = self.verification_record_name(domain);

        debug!(domain = domain, record = %record_name, "Checking DNS TXT record");

        // Try to resolve TXT record
        match self.lookup_txt(&record_name).await {
            Ok(values) => {
                let found = values.iter().any(|v| v == expected_token);
                DnsVerificationResult {
                    domain: domain.to_string(),
                    verified: found,
                    method: "TXT".to_string(),
                    expected_value: Some(expected_token.to_string()),
                    found_value: values.first().cloned(),
                    error: if found { None } else { Some("Token not found in TXT records".to_string()) },
                }
            }
            Err(e) => {
                DnsVerificationResult {
                    domain: domain.to_string(),
                    verified: false,
                    method: "TXT".to_string(),
                    expected_value: Some(expected_token.to_string()),
                    found_value: None,
                    error: Some(e.to_string()),
                }
            }
        }
    }

    /// Verify domain by checking if it resolves to our server
    pub async fn verify_cname(&self, domain: &str, expected_target: &str) -> DnsVerificationResult {
        debug!(domain = domain, target = expected_target, "Checking DNS CNAME/A record");

        // Try to resolve the domain
        match self.lookup_host(domain).await {
            Ok(addresses) => {
                // Check if any resolved address matches our expected target
                let target_addrs: Vec<_> = expected_target
                    .to_socket_addrs()
                    .map(|addrs| addrs.map(|a| a.ip().to_string()).collect())
                    .unwrap_or_default();

                let found = addresses.iter().any(|a| target_addrs.contains(a));

                DnsVerificationResult {
                    domain: domain.to_string(),
                    verified: found,
                    method: "CNAME/A".to_string(),
                    expected_value: Some(expected_target.to_string()),
                    found_value: addresses.first().cloned(),
                    error: if found { None } else { Some("Domain does not resolve to expected target".to_string()) },
                }
            }
            Err(e) => {
                DnsVerificationResult {
                    domain: domain.to_string(),
                    verified: false,
                    method: "CNAME/A".to_string(),
                    expected_value: Some(expected_target.to_string()),
                    found_value: None,
                    error: Some(e.to_string()),
                }
            }
        }
    }

    /// Look up TXT records for a domain
    async fn lookup_txt(&self, name: &str) -> Result<Vec<String>> {
        // Use system DNS resolution
        // In production, you'd use a proper DNS library like trust-dns
        use std::process::Command;

        let output = Command::new("dig")
            .args(["+short", "TXT", name])
            .output()
            .context("Failed to execute dig command")?;

        if !output.status.success() {
            anyhow::bail!("DNS lookup failed");
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let values: Vec<String> = stdout
            .lines()
            .map(|line| line.trim().trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Ok(values)
    }

    /// Look up A/AAAA records for a domain
    async fn lookup_host(&self, name: &str) -> Result<Vec<String>> {
        use std::process::Command;

        let output = Command::new("dig")
            .args(["+short", "A", name])
            .output()
            .context("Failed to execute dig command")?;

        if !output.status.success() {
            anyhow::bail!("DNS lookup failed");
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let addresses: Vec<String> = stdout
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|s| !s.is_empty() && !s.starts_with(';'))
            .collect();

        Ok(addresses)
    }
}

impl Default for DnsVerifier {
    fn default() -> Self {
        Self::new()
    }
}

/// SSL certificate manager using ACME
pub struct SslManager {
    /// Directory for storing certificates
    cert_dir: std::path::PathBuf,
    /// ACME account email
    email: Option<String>,
    /// Whether to use staging ACME server
    staging: bool,
}

impl SslManager {
    pub fn new(cert_dir: std::path::PathBuf) -> Self {
        Self {
            cert_dir,
            email: None,
            staging: false,
        }
    }

    pub fn with_email(mut self, email: &str) -> Self {
        self.email = Some(email.to_string());
        self
    }

    pub fn with_staging(mut self, staging: bool) -> Self {
        self.staging = staging;
        self
    }

    /// Check if we have a valid certificate for a domain
    pub fn has_valid_cert(&self, domain: &str) -> bool {
        let cert_path = self.cert_path(domain);
        let key_path = self.key_path(domain);

        if !cert_path.exists() || !key_path.exists() {
            return false;
        }

        // Check if certificate is still valid (not expired)
        match self.check_cert_expiry(&cert_path) {
            Ok(days_remaining) => days_remaining > 7, // Renew if less than 7 days
            Err(_) => false,
        }
    }

    /// Get certificate path for a domain
    pub fn cert_path(&self, domain: &str) -> std::path::PathBuf {
        self.cert_dir.join(format!("{}.crt", sanitize_domain(domain)))
    }

    /// Get private key path for a domain
    pub fn key_path(&self, domain: &str) -> std::path::PathBuf {
        self.cert_dir.join(format!("{}.key", sanitize_domain(domain)))
    }

    /// Provision an SSL certificate for a domain (wrapper for request_certificate)
    pub async fn provision_certificate(&self, domain: &str) -> Result<CertificateInfo> {
        self.request_certificate(domain).await
    }

    /// Request a new certificate via ACME
    pub async fn request_certificate(&self, domain: &str) -> Result<CertificateInfo> {
        info!(domain = domain, "Requesting SSL certificate");

        // Ensure cert directory exists
        std::fs::create_dir_all(&self.cert_dir)
            .context("Failed to create certificate directory")?;

        let cert_path = self.cert_path(domain);
        let key_path = self.key_path(domain);

        // For now, generate a self-signed certificate
        // In production, this would use instant-acme for Let's Encrypt
        self.generate_self_signed(domain, &cert_path, &key_path)?;

        let expires_at = chrono_lite_now_plus_days(90);

        info!(domain = domain, expires = %expires_at, "Certificate generated");

        Ok(CertificateInfo {
            domain: domain.to_string(),
            cert_path: cert_path.to_string_lossy().to_string(),
            key_path: key_path.to_string_lossy().to_string(),
            expires_at,
            issuer: "Self-Signed".to_string(),
        })
    }

    /// Generate a self-signed certificate (for development)
    fn generate_self_signed(
        &self,
        domain: &str,
        cert_path: &std::path::Path,
        key_path: &std::path::Path,
    ) -> Result<()> {
        use rcgen::{CertifiedKey, generate_simple_self_signed};

        let subject_alt_names = if domain.starts_with("*.") {
            vec![domain.to_string(), domain[2..].to_string()]
        } else {
            vec![domain.to_string()]
        };

        let CertifiedKey { cert, key_pair } = generate_simple_self_signed(subject_alt_names)
            .context("Failed to generate certificate")?;

        std::fs::write(cert_path, cert.pem())
            .context("Failed to write certificate")?;

        std::fs::write(key_path, key_pair.serialize_pem())
            .context("Failed to write private key")?;

        // Set restrictive permissions on key file
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600))?;
        }

        Ok(())
    }

    /// Check certificate expiry (returns days remaining)
    fn check_cert_expiry(&self, cert_path: &std::path::Path) -> Result<i64> {
        let cert_pem = std::fs::read_to_string(cert_path)?;

        // Parse the certificate
        let pem = x509_parser::pem::parse_x509_pem(cert_pem.as_bytes())
            .context("Failed to parse PEM")?;

        let cert = pem.1.parse_x509()
            .context("Failed to parse X509")?;

        let not_after = cert.validity().not_after;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let expires_at = not_after.timestamp();
        let days_remaining = (expires_at - now) / 86400;

        Ok(days_remaining)
    }

    /// Renew certificates that are expiring soon
    pub async fn renew_expiring(&self, domains: &[DomainConfig]) -> Vec<Result<CertificateInfo>> {
        let mut results = Vec::new();

        for domain in domains {
            if !domain.ssl_enabled {
                continue;
            }

            if !self.has_valid_cert(&domain.domain) {
                results.push(self.request_certificate(&domain.domain).await);
            }
        }

        results
    }
}

/// Certificate information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificateInfo {
    pub domain: String,
    pub cert_path: String,
    pub key_path: String,
    pub expires_at: String,
    pub issuer: String,
}

// ==================== Helper Functions ====================

/// Normalize a domain name (lowercase, trim whitespace)
fn normalize_domain(domain: &str) -> Result<String> {
    let domain = domain.trim().to_lowercase();

    // Basic validation
    if domain.is_empty() {
        anyhow::bail!("Domain cannot be empty");
    }

    if domain.len() > 253 {
        anyhow::bail!("Domain name too long");
    }

    // Check for valid characters
    let valid_chars = domain.chars().all(|c| {
        c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '*'
    });

    if !valid_chars {
        anyhow::bail!("Domain contains invalid characters");
    }

    // Wildcard must be at the start
    if domain.contains('*') && !domain.starts_with("*.") {
        anyhow::bail!("Wildcard (*) must be at the start of the domain");
    }

    Ok(domain)
}

/// Sanitize domain for use in filenames
fn sanitize_domain(domain: &str) -> String {
    domain
        .replace('*', "wildcard")
        .replace('.', "_")
}

/// Generate a random verification token
fn generate_verification_token() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: [u8; 16] = rng.gen();
    format!("paas-verify-{}", hex::encode(bytes))
}

/// Get current time plus N days as ISO string
fn chrono_lite_now_plus_days(days: i64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let future = now + (days as u64 * 86400);

    // Convert to ISO-ish format
    let secs_per_day = 86400u64;
    let days_since_epoch = future / secs_per_day;
    let years = 1970 + days_since_epoch / 365;
    let remaining_days = days_since_epoch % 365;
    let month = remaining_days / 30 + 1;
    let day = remaining_days % 30 + 1;

    format!("{:04}-{:02}-{:02}", years, month.min(12), day.min(28))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_domain() {
        assert_eq!(normalize_domain("Example.COM").unwrap(), "example.com");
        assert_eq!(normalize_domain("  test.io  ").unwrap(), "test.io");
        assert_eq!(normalize_domain("*.example.com").unwrap(), "*.example.com");
    }

    #[test]
    fn test_normalize_domain_errors() {
        assert!(normalize_domain("").is_err());
        assert!(normalize_domain("test*.com").is_err());
        assert!(normalize_domain("test_domain.com").is_err());
    }

    #[test]
    fn test_domain_config_matches() {
        let exact = DomainConfig::new("example.com", "myapp");
        assert!(exact.matches("example.com"));
        assert!(!exact.matches("sub.example.com"));

        let wildcard = DomainConfig::new("*.example.com", "myapp");
        assert!(wildcard.matches("sub.example.com"));
        assert!(wildcard.matches("deep.sub.example.com"));
        assert!(!wildcard.matches("example.com")); // Wildcard doesn't match bare domain
    }

    #[test]
    fn test_domain_config_base_domain() {
        let exact = DomainConfig::new("example.com", "myapp");
        assert_eq!(exact.base_domain(), "example.com");

        let wildcard = DomainConfig::new("*.example.com", "myapp");
        assert_eq!(wildcard.base_domain(), "example.com");
    }

    #[tokio::test]
    async fn test_domain_manager_add_remove() {
        let manager = DomainManager::new();

        let config = manager.add_domain_auto("test.example.com", "myapp").await.unwrap();
        assert_eq!(config.domain, "test.example.com");
        assert_eq!(config.app_name, "myapp");
        assert!(!config.verified);

        // Should fail on duplicate
        assert!(manager.add_domain_auto("test.example.com", "other").await.is_err());

        // Remove
        let removed = manager.remove_domain("test.example.com").await.unwrap();
        assert!(removed.is_some());

        // Should be gone
        assert!(manager.get_domain("test.example.com").await.is_none());
    }

    #[tokio::test]
    async fn test_domain_manager_resolve() {
        let manager = DomainManager::new();

        manager.add_domain_auto("app.example.com", "app1").await.unwrap();
        manager.set_verified("app.example.com", true).await.unwrap();

        manager.add_domain_auto("*.wildcard.io", "app2").await.unwrap();
        manager.set_verified("*.wildcard.io", true).await.unwrap();

        // Exact match
        assert_eq!(manager.resolve_app("app.example.com").await, Some("app1".to_string()));

        // Wildcard match
        assert_eq!(manager.resolve_app("sub.wildcard.io").await, Some("app2".to_string()));
        assert_eq!(manager.resolve_app("deep.sub.wildcard.io").await, Some("app2".to_string()));

        // Unverified domain shouldn't resolve
        manager.add_domain_auto("unverified.test", "app3").await.unwrap();
        assert!(manager.resolve_app("unverified.test").await.is_none());
    }

    #[test]
    fn test_sanitize_domain() {
        assert_eq!(sanitize_domain("example.com"), "example_com");
        assert_eq!(sanitize_domain("*.example.com"), "wildcard_example_com");
    }

    #[test]
    fn test_verification_token() {
        let token1 = generate_verification_token();
        let token2 = generate_verification_token();

        assert!(token1.starts_with("paas-verify-"));
        assert_ne!(token1, token2);
        assert_eq!(token1.len(), 44); // "paas-verify-" (12) + 32 hex chars = 44
    }
}
