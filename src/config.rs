use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// Global configuration for the proxy
#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    /// Server configuration
    #[serde(default)]
    pub server: ServerConfig,

    /// Global default settings for backends
    #[serde(default)]
    pub defaults: BackendDefaults,

    /// Virtual host configurations
    #[serde(default)]
    pub backends: HashMap<String, BackendConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    /// HTTP port (default: 80, set to 0 to disable)
    #[serde(default = "default_listen_port")]
    pub port: u16,

    /// HTTPS port (default: 443 when TLS enabled, set to 0 to disable)
    pub tls_port: Option<u16>,

    /// Bind address (default: 0.0.0.0)
    #[serde(default = "default_bind_address")]
    pub bind: String,

    /// Port for the internal admin API (for backend callbacks)
    #[serde(default = "default_admin_port")]
    pub admin_port: u16,

    /// Authentication token for admin API (required for write operations)
    /// If not set, a random token is generated at startup and logged
    pub admin_token: Option<String>,

    /// Maximum idle connections per backend host (default: 10)
    #[serde(default = "default_pool_max_idle_per_host")]
    pub pool_max_idle_per_host: usize,

    /// Idle connection timeout in seconds (default: 90)
    #[serde(default = "default_pool_idle_timeout")]
    pub pool_idle_timeout_secs: u64,

    /// Path to PID file (optional)
    pub pid_file: Option<String>,

    /// Enable TLS (default: false). If true without cert/key, generates self-signed.
    #[serde(default)]
    pub tls: bool,

    /// Path to TLS certificate file (PEM format)
    pub tls_cert: Option<String>,

    /// Path to TLS private key file (PEM format)
    pub tls_key: Option<String>,

    /// Force redirect from HTTP to HTTPS (default: false)
    #[serde(default)]
    pub force_https: bool,

    /// ACME/Let's Encrypt configuration
    #[serde(default)]
    pub acme: AcmeConfig,
}

/// Challenge type for ACME domain validation
#[derive(Debug, Deserialize, Clone, Default, PartialEq)]
pub enum AcmeChallengeType {
    /// HTTP-01: Serves challenge response on port 80 at /.well-known/acme-challenge/
    #[default]
    #[serde(alias = "http01", alias = "HTTP-01")]
    #[serde(rename = "http-01")]
    Http01,
    /// TLS-ALPN-01: Serves challenge via TLS on port 443 with special ALPN protocol
    #[serde(alias = "tls-alpn01", alias = "TLS-ALPN-01")]
    #[serde(rename = "tls-alpn-01")]
    TlsAlpn01,
}

/// ACME (Let's Encrypt) configuration for automatic certificate provisioning
#[derive(Debug, Deserialize, Clone)]
pub struct AcmeConfig {
    /// Enable ACME certificate provisioning
    #[serde(default)]
    pub enabled: bool,

    /// Domains to obtain certificates for
    #[serde(default)]
    pub domains: Vec<String>,

    /// Contact email for Let's Encrypt notifications (required when enabled)
    pub email: Option<String>,

    /// ACME directory URL (defaults to Let's Encrypt production)
    /// Use "https://acme-staging-v02.api.letsencrypt.org/directory" for testing
    pub directory_url: Option<String>,

    /// Local directory for certificate and account cache
    #[serde(default = "default_acme_cache_dir")]
    pub cache_dir: String,

    /// Challenge type for domain validation (default: http-01)
    #[serde(default)]
    pub challenge_type: AcmeChallengeType,
}

impl Default for AcmeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            domains: Vec::new(),
            email: None,
            directory_url: None,
            cache_dir: default_acme_cache_dir(),
            challenge_type: AcmeChallengeType::default(),
        }
    }
}

fn default_acme_cache_dir() -> String {
    "./acme_cache".to_string()
}

impl ServerConfig {
    pub fn tls_enabled(&self) -> bool {
        self.acme_enabled() || self.tls || self.tls_cert.is_some() && self.tls_key.is_some()
    }

    pub fn has_tls_files(&self) -> bool {
        self.tls_cert.is_some() && self.tls_key.is_some()
    }

    pub fn acme_enabled(&self) -> bool {
        self.acme.enabled && !self.acme.domains.is_empty()
    }

    /// Get HTTP port (0 means disabled)
    pub fn http_port(&self) -> u16 {
        self.port
    }

    /// Get HTTPS port (0 means disabled)
    pub fn https_port(&self) -> u16 {
        if !self.tls_enabled() {
            return 0;
        }
        self.tls_port.unwrap_or(443)
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: default_listen_port(),
            tls_port: None,
            bind: default_bind_address(),
            admin_port: default_admin_port(),
            admin_token: None,
            pool_max_idle_per_host: default_pool_max_idle_per_host(),
            pool_idle_timeout_secs: default_pool_idle_timeout(),
            pid_file: None,
            tls: false,
            tls_cert: None,
            tls_key: None,
            force_https: false,
            acme: AcmeConfig::default(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct BackendDefaults {
    /// Default idle timeout in seconds before shutting down a backend
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_secs: u64,

    /// Default startup timeout in seconds
    #[serde(default = "default_startup_timeout")]
    pub startup_timeout_secs: u64,

    /// Default health check interval in milliseconds
    #[serde(default = "default_health_interval")]
    pub health_check_interval_ms: u64,

    /// Default health endpoint path
    #[serde(default = "default_health_path")]
    pub health_path: String,

    /// Default grace period in seconds between SIGTERM and SIGKILL
    #[serde(default = "default_shutdown_grace_period")]
    pub shutdown_grace_period_secs: u64,

    /// Default drain timeout in seconds (wait for in-flight requests before SIGTERM)
    #[serde(default = "default_drain_timeout")]
    pub drain_timeout_secs: u64,

    /// Default request timeout in seconds (max time to wait for backend response)
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,

    /// Default health check interval for ready backends in milliseconds
    #[serde(default = "default_ready_health_interval")]
    pub ready_health_check_interval_ms: u64,

    /// Number of consecutive health check failures before marking backend unhealthy
    #[serde(default = "default_unhealthy_threshold")]
    pub unhealthy_threshold: u32,
}

impl Default for BackendDefaults {
    fn default() -> Self {
        Self {
            idle_timeout_secs: default_idle_timeout(),
            startup_timeout_secs: default_startup_timeout(),
            health_check_interval_ms: default_health_interval(),
            health_path: default_health_path(),
            shutdown_grace_period_secs: default_shutdown_grace_period(),
            drain_timeout_secs: default_drain_timeout(),
            request_timeout_secs: default_request_timeout(),
            ready_health_check_interval_ms: default_ready_health_interval(),
            unhealthy_threshold: default_unhealthy_threshold(),
        }
    }
}


/// Backend type: local process or Docker container
#[derive(Debug, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BackendType {
    /// Local process spawned directly (default)
    #[default]
    Local,
    /// Docker container managed via Docker API
    Docker,
}

/// Image pull policy for Docker backends
#[derive(Debug, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PullPolicy {
    /// Pull if image doesn't exist locally (default)
    #[default]
    #[serde(alias = "if-not-present")]
    IfNotPresent,
    /// Always pull before starting
    Always,
    /// Never pull, fail if image doesn't exist
    Never,
}

/// Configuration for a single backend
///
/// # Security Warning
///
/// For local backends, the `command` and `args` fields allow arbitrary command execution.
/// For Docker backends, container images are pulled and run.
/// Configuration files must be protected with appropriate file permissions
/// (e.g., readable only by the service user). Malicious configuration files
/// could execute arbitrary code with the permissions of the proxy process.
#[derive(Debug, Deserialize, Clone)]
pub struct BackendConfig {
    /// Backend type: "local" (default) or "docker"
    #[serde(default, rename = "type")]
    pub backend_type: BackendType,

    // === Local process fields ===
    /// Command to execute to start the backend (local only)
    ///
    /// **Security:** This command is executed directly. Ensure config files
    /// are protected and commands come from trusted sources only.
    pub command: Option<String>,

    /// Arguments to pass to the command (local only)
    #[serde(default)]
    pub args: Vec<String>,

    /// Working directory for the command (local only)
    pub working_dir: Option<String>,

    // === Docker-specific fields ===
    /// Docker image to run (required for Docker backends)
    pub image: Option<String>,

    /// Container name (default: spawngate-{hostname})
    pub container_name: Option<String>,

    /// Docker host URL (default: unix:///var/run/docker.sock)
    pub docker_host: Option<String>,

    /// Docker network to connect to (default: bridge)
    pub network: Option<String>,

    /// Image pull policy: "always", "never", or "if-not-present" (default)
    #[serde(default)]
    pub pull_policy: PullPolicy,

    /// Memory limit (e.g., "512m", "1g")
    pub memory: Option<String>,

    /// CPU limit (e.g., "0.5", "2")
    pub cpus: Option<String>,

    // === Common fields ===
    /// Environment variables to set
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Port the backend will listen on
    pub port: u16,

    /// Health check endpoint path (overrides default)
    pub health_path: Option<String>,

    /// Idle timeout in seconds (overrides default)
    pub idle_timeout_secs: Option<u64>,

    /// Startup timeout in seconds (overrides default)
    pub startup_timeout_secs: Option<u64>,

    /// Health check interval in milliseconds (overrides default)
    pub health_check_interval_ms: Option<u64>,

    /// Grace period in seconds between SIGTERM and SIGKILL (overrides default)
    pub shutdown_grace_period_secs: Option<u64>,

    /// Drain timeout in seconds (wait for in-flight requests before SIGTERM, overrides default)
    pub drain_timeout_secs: Option<u64>,

    /// Request timeout in seconds (overrides default)
    pub request_timeout_secs: Option<u64>,

    /// Health check interval for ready backends in milliseconds (overrides default)
    pub ready_health_check_interval_ms: Option<u64>,

    /// Number of consecutive health check failures before marking backend unhealthy (overrides default)
    pub unhealthy_threshold: Option<u32>,
}

impl BackendConfig {
    /// Create a new local backend config with defaults
    pub fn local(command: &str, port: u16) -> Self {
        Self {
            backend_type: BackendType::Local,
            command: Some(command.to_string()),
            args: Vec::new(),
            working_dir: None,
            image: None,
            container_name: None,
            docker_host: None,
            network: None,
            pull_policy: PullPolicy::default(),
            memory: None,
            cpus: None,
            env: HashMap::new(),
            port,
            health_path: None,
            idle_timeout_secs: None,
            startup_timeout_secs: None,
            health_check_interval_ms: None,
            shutdown_grace_period_secs: None,
            drain_timeout_secs: None,
            request_timeout_secs: None,
            ready_health_check_interval_ms: None,
            unhealthy_threshold: None,
        }
    }

    /// Create a new Docker backend config with defaults
    pub fn docker(image: &str, port: u16) -> Self {
        Self {
            backend_type: BackendType::Docker,
            command: None,
            args: Vec::new(),
            working_dir: None,
            image: Some(image.to_string()),
            container_name: None,
            docker_host: None,
            network: None,
            pull_policy: PullPolicy::default(),
            memory: None,
            cpus: None,
            env: HashMap::new(),
            port,
            health_path: None,
            idle_timeout_secs: None,
            startup_timeout_secs: None,
            health_check_interval_ms: None,
            shutdown_grace_period_secs: None,
            drain_timeout_secs: None,
            request_timeout_secs: None,
            ready_health_check_interval_ms: None,
            unhealthy_threshold: None,
        }
    }

    /// Set arguments for this backend config (builder pattern)
    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    /// Set environment variables (builder pattern)
    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = env;
        self
    }

    /// Set working directory (builder pattern)
    pub fn with_working_dir(mut self, dir: &str) -> Self {
        self.working_dir = Some(dir.to_string());
        self
    }

    pub fn idle_timeout(&self, defaults: &BackendDefaults) -> Duration {
        Duration::from_secs(self.idle_timeout_secs.unwrap_or(defaults.idle_timeout_secs))
    }

    pub fn startup_timeout(&self, defaults: &BackendDefaults) -> Duration {
        Duration::from_secs(self.startup_timeout_secs.unwrap_or(defaults.startup_timeout_secs))
    }

    pub fn health_check_interval(&self, defaults: &BackendDefaults) -> Duration {
        Duration::from_millis(
            self.health_check_interval_ms
                .unwrap_or(defaults.health_check_interval_ms),
        )
    }

    pub fn health_path<'a>(&'a self, defaults: &'a BackendDefaults) -> &'a str {
        self.health_path
            .as_deref()
            .unwrap_or(&defaults.health_path)
    }

    pub fn shutdown_grace_period(&self, defaults: &BackendDefaults) -> Duration {
        Duration::from_secs(
            self.shutdown_grace_period_secs
                .unwrap_or(defaults.shutdown_grace_period_secs),
        )
    }

    pub fn drain_timeout(&self, defaults: &BackendDefaults) -> Duration {
        Duration::from_secs(
            self.drain_timeout_secs
                .unwrap_or(defaults.drain_timeout_secs),
        )
    }

    pub fn request_timeout(&self, defaults: &BackendDefaults) -> Duration {
        Duration::from_secs(
            self.request_timeout_secs
                .unwrap_or(defaults.request_timeout_secs),
        )
    }

    pub fn ready_health_check_interval(&self, defaults: &BackendDefaults) -> Duration {
        Duration::from_millis(
            self.ready_health_check_interval_ms
                .unwrap_or(defaults.ready_health_check_interval_ms),
        )
    }

    pub fn unhealthy_threshold(&self, defaults: &BackendDefaults) -> u32 {
        self.unhealthy_threshold
            .unwrap_or(defaults.unhealthy_threshold)
    }

    /// Validate the backend configuration
    pub fn validate(&self, hostname: &str) -> Result<(), String> {
        match self.backend_type {
            BackendType::Local => {
                if self.command.is_none() {
                    return Err(format!(
                        "Backend '{}': local backend requires 'command' field",
                        hostname
                    ));
                }
            }
            BackendType::Docker => {
                if self.image.is_none() {
                    return Err(format!(
                        "Backend '{}': Docker backend requires 'image' field",
                        hostname
                    ));
                }
            }
        }

        if self.port == 0 {
            return Err(format!(
                "Backend '{}': 'port' must be greater than 0",
                hostname
            ));
        }

        Ok(())
    }
}

// Default value functions
fn default_listen_port() -> u16 {
    80
}

fn default_bind_address() -> String {
    "0.0.0.0".to_string()
}

fn default_admin_port() -> u16 {
    9999
}

fn default_pool_max_idle_per_host() -> usize {
    10 // Keep up to 10 idle connections per backend
}

fn default_pool_idle_timeout() -> u64 {
    90 // Close idle connections after 90 seconds
}

fn default_idle_timeout() -> u64 {
    600 // 10 minutes
}

fn default_startup_timeout() -> u64 {
    30 // 30 seconds
}

fn default_health_interval() -> u64 {
    100 // 100ms
}

fn default_health_path() -> String {
    "/health".to_string()
}

fn default_shutdown_grace_period() -> u64 {
    10 // 10 seconds between SIGTERM and SIGKILL
}

fn default_drain_timeout() -> u64 {
    30 // 30 seconds to wait for in-flight requests to complete
}

fn default_request_timeout() -> u64 {
    30 // 30 seconds max for backend to respond
}

fn default_ready_health_interval() -> u64 {
    5000 // 5 seconds between health checks for ready backends
}

fn default_unhealthy_threshold() -> u32 {
    3 // 3 consecutive failures before marking unhealthy
}

impl Config {
    pub fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    /// Validate all configuration
    pub fn validate(&self) -> anyhow::Result<()> {
        let mut errors = Vec::new();

        for (hostname, backend) in &self.backends {
            if let Err(e) = backend.validate(hostname) {
                errors.push(e);
            }
        }

        if !errors.is_empty() {
            anyhow::bail!("Configuration errors:\n  - {}", errors.join("\n  - "));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let toml = r#"
[server]
port = 8080
bind = "127.0.0.1"
admin_port = 9000

[defaults]
idle_timeout_secs = 300
startup_timeout_secs = 60
health_check_interval_ms = 200
health_path = "/healthz"

[backends."example.com"]
command = "node"
args = ["server.js"]
port = 3000
working_dir = "/app"

[backends."api.example.com"]
command = "python"
args = ["-m", "http.server", "8000"]
port = 8000
idle_timeout_secs = 120
"#;

        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.defaults.idle_timeout_secs, 300);
        assert_eq!(config.backends.len(), 2);
        assert!(config.backends.contains_key("example.com"));
    }

    #[test]
    fn test_default_server_config() {
        let config = ServerConfig::default();
        assert_eq!(config.port, 80);
        assert_eq!(config.bind, "0.0.0.0");
        assert_eq!(config.admin_port, 9999);
        assert_eq!(config.pool_max_idle_per_host, 10);
        assert_eq!(config.pool_idle_timeout_secs, 90);
    }

    #[test]
    fn test_default_backend_defaults() {
        let defaults = BackendDefaults::default();
        assert_eq!(defaults.idle_timeout_secs, 600);
        assert_eq!(defaults.startup_timeout_secs, 30);
        assert_eq!(defaults.health_check_interval_ms, 100);
        assert_eq!(defaults.health_path, "/health");
        assert_eq!(defaults.shutdown_grace_period_secs, 10);
        assert_eq!(defaults.drain_timeout_secs, 30);
        assert_eq!(defaults.request_timeout_secs, 30);
        assert_eq!(defaults.ready_health_check_interval_ms, 5000);
        assert_eq!(defaults.unhealthy_threshold, 3);
    }

    #[test]
    fn test_backend_config_uses_defaults() {
        let defaults = BackendDefaults::default();
        let backend: BackendConfig = toml::from_str(r#"
command = "node"
port = 3000
"#)
        .unwrap();

        assert_eq!(backend.idle_timeout(&defaults), Duration::from_secs(600));
        assert_eq!(backend.startup_timeout(&defaults), Duration::from_secs(30));
        assert_eq!(
            backend.health_check_interval(&defaults),
            Duration::from_millis(100)
        );
        assert_eq!(backend.health_path(&defaults), "/health");
        assert_eq!(
            backend.shutdown_grace_period(&defaults),
            Duration::from_secs(10)
        );
        assert_eq!(backend.drain_timeout(&defaults), Duration::from_secs(30));
        assert_eq!(backend.request_timeout(&defaults), Duration::from_secs(30));
        assert_eq!(
            backend.ready_health_check_interval(&defaults),
            Duration::from_millis(5000)
        );
        assert_eq!(backend.unhealthy_threshold(&defaults), 3);
    }

    #[test]
    fn test_backend_config_overrides_defaults() {
        let defaults = BackendDefaults::default();
        let backend: BackendConfig = toml::from_str(r#"
command = "node"
port = 3000
idle_timeout_secs = 120
startup_timeout_secs = 60
health_check_interval_ms = 500
health_path = "/ready"
shutdown_grace_period_secs = 5
drain_timeout_secs = 15
request_timeout_secs = 10
ready_health_check_interval_ms = 10000
unhealthy_threshold = 5
"#)
        .unwrap();

        assert_eq!(backend.idle_timeout(&defaults), Duration::from_secs(120));
        assert_eq!(backend.startup_timeout(&defaults), Duration::from_secs(60));
        assert_eq!(
            backend.health_check_interval(&defaults),
            Duration::from_millis(500)
        );
        assert_eq!(backend.health_path(&defaults), "/ready");
        assert_eq!(
            backend.shutdown_grace_period(&defaults),
            Duration::from_secs(5)
        );
        assert_eq!(backend.drain_timeout(&defaults), Duration::from_secs(15));
        assert_eq!(backend.request_timeout(&defaults), Duration::from_secs(10));
        assert_eq!(
            backend.ready_health_check_interval(&defaults),
            Duration::from_millis(10000)
        );
        assert_eq!(backend.unhealthy_threshold(&defaults), 5);
    }

    #[test]
    fn test_minimal_config() {
        let toml = r#"
[backends."example.com"]
command = "node"
port = 3000
"#;
        let config: Config = toml::from_str(toml).unwrap();

        // Should use defaults for server
        assert_eq!(config.server.port, 80);
        assert_eq!(config.server.bind, "0.0.0.0");

        // Should use defaults for backend defaults
        assert_eq!(config.defaults.idle_timeout_secs, 600);

        // Backend should be present
        assert!(config.backends.contains_key("example.com"));
        let backend = config.backends.get("example.com").unwrap();
        assert_eq!(backend.command, Some("node".to_string()));
        assert_eq!(backend.port, 3000);
    }

    #[test]
    fn test_backend_with_env_vars() {
        let toml = r#"
command = "node"
port = 3000

[env]
NODE_ENV = "production"
DEBUG = "true"
"#;
        let backend: BackendConfig = toml::from_str(toml).unwrap();

        assert_eq!(backend.env.len(), 2);
        assert_eq!(backend.env.get("NODE_ENV"), Some(&"production".to_string()));
        assert_eq!(backend.env.get("DEBUG"), Some(&"true".to_string()));
    }

    #[test]
    fn test_backend_with_args() {
        let toml = r#"
command = "python"
args = ["-m", "http.server", "8000"]
port = 8000
"#;
        let backend: BackendConfig = toml::from_str(toml).unwrap();

        assert_eq!(backend.args, vec!["-m", "http.server", "8000"]);
    }

    #[test]
    fn test_empty_config() {
        let toml = "";
        let config: Config = toml::from_str(toml).unwrap();

        // Should use all defaults
        assert_eq!(config.server.port, 80);
        assert_eq!(config.defaults.idle_timeout_secs, 600);
        assert!(config.backends.is_empty());
    }

    #[test]
    fn test_acme_config_defaults() {
        let toml = "";
        let config: Config = toml::from_str(toml).unwrap();

        // ACME should be disabled by default
        assert!(!config.server.acme.enabled);
        assert!(config.server.acme.domains.is_empty());
        assert!(config.server.acme.email.is_none());
        assert!(config.server.acme.directory_url.is_none());
        assert_eq!(config.server.acme.cache_dir, "./acme_cache");
        assert!(!config.server.acme_enabled());
    }

    #[test]
    fn test_acme_config_enabled() {
        let toml = r#"
[server.acme]
enabled = true
domains = ["example.com", "api.example.com"]
email = "admin@example.com"
cache_dir = "/var/lib/acme"
"#;
        let config: Config = toml::from_str(toml).unwrap();

        assert!(config.server.acme.enabled);
        assert_eq!(config.server.acme.domains, vec!["example.com", "api.example.com"]);
        assert_eq!(config.server.acme.email, Some("admin@example.com".to_string()));
        assert_eq!(config.server.acme.cache_dir, "/var/lib/acme");
        assert!(config.server.acme_enabled());
    }

    #[test]
    fn test_acme_config_with_staging() {
        let toml = r#"
[server.acme]
enabled = true
domains = ["test.example.com"]
email = "test@example.com"
directory_url = "https://acme-staging-v02.api.letsencrypt.org/directory"
"#;
        let config: Config = toml::from_str(toml).unwrap();

        assert!(config.server.acme.enabled);
        assert_eq!(
            config.server.acme.directory_url,
            Some("https://acme-staging-v02.api.letsencrypt.org/directory".to_string())
        );
    }

    #[test]
    fn test_acme_enabled_requires_domains() {
        let toml = r#"
[server.acme]
enabled = true
email = "admin@example.com"
"#;
        let config: Config = toml::from_str(toml).unwrap();

        // ACME enabled but no domains - acme_enabled() should return false
        assert!(config.server.acme.enabled);
        assert!(config.server.acme.domains.is_empty());
        assert!(!config.server.acme_enabled());
    }

    #[test]
    fn test_tls_enabled_includes_acme() {
        // Without ACME
        let toml = "";
        let config: Config = toml::from_str(toml).unwrap();
        assert!(!config.server.tls_enabled());

        // With ACME
        let toml = r#"
[server.acme]
enabled = true
domains = ["example.com"]
email = "admin@example.com"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.server.tls_enabled());
        assert!(config.server.acme_enabled());
    }

    #[test]
    fn test_acme_challenge_type_defaults_to_http01() {
        let toml = r#"
[server.acme]
enabled = true
domains = ["example.com"]
email = "admin@example.com"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.server.acme.challenge_type, AcmeChallengeType::Http01);
    }

    #[test]
    fn test_acme_challenge_type_tls_alpn01() {
        let toml = r#"
[server.acme]
enabled = true
domains = ["example.com"]
email = "admin@example.com"
challenge_type = "tls-alpn-01"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.server.acme.challenge_type, AcmeChallengeType::TlsAlpn01);
    }

    #[test]
    fn test_acme_challenge_type_http01_explicit() {
        let toml = r#"
[server.acme]
enabled = true
domains = ["example.com"]
email = "admin@example.com"
challenge_type = "http-01"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.server.acme.challenge_type, AcmeChallengeType::Http01);
    }

    #[test]
    fn test_docker_backend_config() {
        let toml = r#"
[backends."app.example.com"]
type = "docker"
image = "myapp:latest"
port = 3000
container_name = "myapp-container"
network = "host"
pull_policy = "always"
memory = "512m"
cpus = "1.0"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let backend = config.backends.get("app.example.com").unwrap();

        assert_eq!(backend.backend_type, BackendType::Docker);
        assert_eq!(backend.image, Some("myapp:latest".to_string()));
        assert_eq!(backend.port, 3000);
        assert_eq!(backend.container_name, Some("myapp-container".to_string()));
        assert_eq!(backend.network, Some("host".to_string()));
        assert_eq!(backend.pull_policy, PullPolicy::Always);
        assert_eq!(backend.memory, Some("512m".to_string()));
        assert_eq!(backend.cpus, Some("1.0".to_string()));
        assert!(backend.command.is_none());
    }

    #[test]
    fn test_local_backend_is_default() {
        let toml = r#"
[backends."app.local"]
command = "node"
args = ["server.js"]
port = 3000
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let backend = config.backends.get("app.local").unwrap();

        assert_eq!(backend.backend_type, BackendType::Local);
        assert_eq!(backend.command, Some("node".to_string()));
        assert!(backend.image.is_none());
    }

    #[test]
    fn test_pull_policy_if_not_present() {
        let toml = r#"
[backends."app.example.com"]
type = "docker"
image = "myapp:latest"
port = 3000
pull_policy = "if-not-present"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let backend = config.backends.get("app.example.com").unwrap();
        assert_eq!(backend.pull_policy, PullPolicy::IfNotPresent);
    }

    #[test]
    fn test_pull_policy_never() {
        let toml = r#"
[backends."app.example.com"]
type = "docker"
image = "myapp:latest"
port = 3000
pull_policy = "never"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let backend = config.backends.get("app.example.com").unwrap();
        assert_eq!(backend.pull_policy, PullPolicy::Never);
    }

    #[test]
    fn test_backend_config_helpers() {
        let local = BackendConfig::local("node", 3000);
        assert_eq!(local.backend_type, BackendType::Local);
        assert_eq!(local.command, Some("node".to_string()));
        assert_eq!(local.port, 3000);

        let docker = BackendConfig::docker("myapp:latest", 8080);
        assert_eq!(docker.backend_type, BackendType::Docker);
        assert_eq!(docker.image, Some("myapp:latest".to_string()));
        assert_eq!(docker.port, 8080);
        assert!(docker.command.is_none());
    }

    #[test]
    fn test_mixed_backend_types() {
        let toml = r#"
[backends."local.app"]
command = "node"
port = 3000

[backends."docker.app"]
type = "docker"
image = "nginx:latest"
port = 8080
"#;
        let config: Config = toml::from_str(toml).unwrap();

        let local = config.backends.get("local.app").unwrap();
        assert_eq!(local.backend_type, BackendType::Local);

        let docker = config.backends.get("docker.app").unwrap();
        assert_eq!(docker.backend_type, BackendType::Docker);
    }

    #[test]
    fn test_validate_docker_requires_image() {
        let toml = r#"
[backends."app.example.com"]
type = "docker"
port = 3000
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Docker backend requires 'image' field"));
        assert!(err.contains("app.example.com"));
    }

    #[test]
    fn test_validate_local_requires_command() {
        let toml = r#"
[backends."app.local"]
port = 3000
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("local backend requires 'command' field"));
        assert!(err.contains("app.local"));
    }

    #[test]
    fn test_validate_port_nonzero() {
        let toml = r#"
[backends."app.example.com"]
command = "node"
port = 0
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("'port' must be greater than 0"));
    }

    #[test]
    fn test_validate_multiple_errors() {
        let toml = r#"
[backends."local.app"]
port = 3000

[backends."docker.app"]
type = "docker"
port = 8080
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        // Should report both errors
        assert!(err.contains("local backend requires 'command' field"));
        assert!(err.contains("Docker backend requires 'image' field"));
    }

    #[test]
    fn test_validate_valid_configs() {
        let local = BackendConfig::local("node", 3000);
        assert!(local.validate("test.local").is_ok());

        let docker = BackendConfig::docker("nginx:latest", 8080);
        assert!(docker.validate("test.docker").is_ok());
    }
}
