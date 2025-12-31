use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use spawngate::acme::AcmeManager;
use spawngate::admin::{AdminServer, PKG_NAME, VERSION};
use spawngate::config::{AcmeChallengeType, Config};
use spawngate::pool::PoolConfig;
use spawngate::process::ProcessManager;
use spawngate::proxy::ProxyServer;
use std::fs::File;
use std::io::BufReader;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("spawngate=debug".parse().expect("valid log directive")),
        )
        .init();

    // Load configuration
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config.toml"));

    let config = Config::load(&config_path).map_err(|e| {
        error!(path = %config_path.display(), error = %e, "Failed to load configuration");
        e
    })?;

    info!(path = %config_path.display(), "Configuration loaded");

    // Print startup banner
    print_startup_banner(&config);

    // Write PID file if configured (with exclusive lock on Unix)
    let pid_file_path = config.server.pid_file.as_ref().map(PathBuf::from);
    let _pid_file = if let Some(ref path) = pid_file_path {
        let pid_file = write_pid_file(path)?;
        info!(path = %path.display(), "PID file written and locked");
        Some(pid_file)
    } else {
        None
    };

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Build admin API URL
    let admin_url = format!("http://127.0.0.1:{}", config.server.admin_port);

    // Create process manager
    let process_manager = ProcessManager::new(
        config.backends.clone(),
        config.defaults.clone(),
        admin_url,
    );

    let pool_config = PoolConfig {
        max_idle_per_host: config.server.pool_max_idle_per_host,
        idle_timeout: Duration::from_secs(config.server.pool_idle_timeout_secs),
    };

    info!(
        max_idle = pool_config.max_idle_per_host,
        idle_timeout_secs = pool_config.idle_timeout.as_secs(),
        "Connection pool configured"
    );

    // Get shared defaults reference for ProxyServer instances
    let shared_defaults = process_manager.shared_defaults();

    // Load TLS configuration if enabled
    // Priority: ACME > file-based certs > self-signed
    let (tls_acceptor, acme_manager) = if config.server.acme_enabled() {
        // ACME/Let's Encrypt automatic certificate provisioning
        let acme_config = config.server.acme.clone();

        // Create cache directory if it doesn't exist
        std::fs::create_dir_all(&acme_config.cache_dir).map_err(|e| {
            anyhow::anyhow!("Failed to create ACME cache directory '{}': {}", acme_config.cache_dir, e)
        })?;

        info!(
            domains = ?acme_config.domains,
            email = ?acme_config.email,
            cache_dir = %acme_config.cache_dir,
            challenge_type = ?acme_config.challenge_type,
            "ACME/Let's Encrypt certificate provisioning enabled"
        );

        let manager = Arc::new(AcmeManager::new(acme_config.clone())?);

        // For TLS-ALPN-01, we use the resolver directly
        // For HTTP-01, we need to wait for initial certificate
        let tls_acceptor = if acme_config.challenge_type == AcmeChallengeType::TlsAlpn01 {
            let resolver = manager.tls_alpn01_resolver();
            let rustls_config = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_cert_resolver(resolver);
            Some(TlsAcceptor::from(Arc::new(rustls_config)))
        } else {
            // For HTTP-01, we'll set up TLS after getting the certificate
            None
        };

        (tls_acceptor, Some(manager))
    } else if config.server.tls_enabled() {
        let (certs, key) = if config.server.has_tls_files() {
            let cert_path = config.server.tls_cert.as_ref().unwrap();
            let key_path = config.server.tls_key.as_ref().unwrap();
            let certs = load_certs(cert_path)?;
            let key = load_key(key_path)?;
            info!(cert = %cert_path, key = %key_path, "TLS enabled with provided certificates");
            (certs, key)
        } else {
            let (certs, key) = generate_self_signed_cert()?;
            warn!("TLS enabled with auto-generated self-signed certificate (not for production)");
            (certs, key)
        };

        let tls_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| anyhow::anyhow!("TLS configuration error: {}", e))?;

        (Some(TlsAcceptor::from(Arc::new(tls_config))), None::<Arc<AcmeManager>>)
    } else {
        (None, None::<Arc<AcmeManager>>)
    };

    // Get ACME HTTP-01 challenges if using HTTP-01 challenge type
    let acme_http01_challenges = acme_manager.as_ref().and_then(|m| {
        if config.server.acme.challenge_type == AcmeChallengeType::Http01 {
            Some(m.http01_challenges())
        } else {
            None
        }
    });

    // Create HTTP proxy server (if port > 0)
    let http_port = config.server.http_port();
    let https_port = config.server.https_port();
    let http_proxy_handle = if http_port > 0 {
        let http_addr: SocketAddr = format!("{}:{}", config.server.bind, http_port)
            .parse()
            .map_err(|e| {
                error!(bind = %config.server.bind, port = http_port, error = %e, "Invalid HTTP bind address");
                anyhow::anyhow!("Invalid HTTP bind address: {}", e)
            })?;

        let mut http_proxy = ProxyServer::with_pool_config(
            http_addr,
            Arc::clone(&process_manager),
            Arc::clone(&shared_defaults),
            shutdown_rx.clone(),
            pool_config.clone(),
        );

        // Add ACME HTTP-01 challenge handler if configured
        if let Some(challenges) = acme_http01_challenges.clone() {
            http_proxy = http_proxy.with_acme_challenges(challenges);
            info!("ACME HTTP-01 challenge handler enabled on HTTP port");
        }

        // If force_https is enabled and HTTPS is available, redirect HTTP to HTTPS
        // Note: ACME challenges are handled before redirect
        if config.server.force_https && https_port > 0 {
            http_proxy = http_proxy.with_https_redirect(https_port);
            info!(http_port, https_port, "HTTP to HTTPS redirect enabled");
        }

        Some(tokio::spawn(async move {
            if let Err(e) = http_proxy.run().await {
                error!(error = %e, "HTTP proxy server error");
            }
        }))
    } else {
        None
    };

    // Create HTTPS proxy server (if TLS enabled and port > 0)
    let https_proxy_handle = if https_port > 0 && tls_acceptor.is_some() {
        let https_addr: SocketAddr = format!("{}:{}", config.server.bind, https_port)
            .parse()
            .map_err(|e| {
                error!(bind = %config.server.bind, port = https_port, error = %e, "Invalid HTTPS bind address");
                anyhow::anyhow!("Invalid HTTPS bind address: {}", e)
            })?;

        let https_proxy = ProxyServer::with_pool_config(
            https_addr,
            Arc::clone(&process_manager),
            Arc::clone(&shared_defaults),
            shutdown_rx.clone(),
            pool_config,
        )
        .with_tls(tls_acceptor.clone().expect("TLS acceptor required for HTTPS"));

        Some(tokio::spawn(async move {
            if let Err(e) = https_proxy.run().await {
                error!(error = %e, "HTTPS proxy server error");
            }
        }))
    } else {
        None
    };

    // Spawn ACME manager task if configured
    let acme_task = if let Some(ref manager) = acme_manager {
        let mgr = Arc::clone(manager);
        let shutdown = shutdown_rx.clone();
        Some(tokio::spawn(async move {
            if let Err(e) = mgr.run(shutdown).await {
                error!(error = %e, "ACME manager error");
            }
        }))
    } else {
        None
    };

    // Create admin server (always HTTP for internal use)
    let admin_addr: SocketAddr = format!("127.0.0.1:{}", config.server.admin_port)
        .parse()
        .map_err(|e| {
            error!(admin_port = config.server.admin_port, error = %e, "Invalid admin bind address");
            anyhow::anyhow!("Invalid admin bind address: {}", e)
        })?;

    // Generate or use configured admin token
    let admin_token = config.server.admin_token.clone().unwrap_or_else(|| {
        let token = uuid::Uuid::new_v4().to_string();
        info!(token = %token, "Generated admin API token (configure admin_token to set a fixed value)");
        token
    });

    let admin_server = AdminServer::new(admin_addr, Arc::clone(&process_manager), shutdown_rx.clone(), admin_token);

    // Spawn idle cleanup task
    let cleanup_manager = Arc::clone(&process_manager);
    let cleanup_shutdown_rx = shutdown_rx.clone();
    tokio::spawn(async move {
        idle_cleanup_loop(cleanup_manager, cleanup_shutdown_rx).await;
    });

    // Spawn admin server
    let admin_handle = tokio::spawn(async move {
        if let Err(e) = admin_server.run().await {
            error!(error = %e, "Admin server error");
        }
    });

    // Wait for shutdown signal (Ctrl+C or SIGTERM) or config reload (SIGHUP)
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())
            .expect("Failed to install SIGTERM handler");
        let mut sighup = signal(SignalKind::hangup())
            .expect("Failed to install SIGHUP handler");

        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    info!("Received SIGINT (Ctrl+C), shutting down...");
                    break;
                }
                _ = sigterm.recv() => {
                    info!("Received SIGTERM, shutting down...");
                    break;
                }
                _ = sighup.recv() => {
                    info!(path = %config_path.display(), "Received SIGHUP, reloading configuration...");
                    match process_manager.reload_config(&config_path).await {
                        Ok(result) => {
                            info!(
                                added = result.added.len(),
                                removed = result.removed.len(),
                                updated = result.updated.len(),
                                "Configuration reloaded successfully"
                            );
                            if !result.added.is_empty() {
                                info!(backends = ?result.added, "New backends available");
                            }
                            if !result.removed.is_empty() {
                                info!(backends = ?result.removed, "Backends removed");
                            }
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to reload configuration");
                        }
                    }
                }
            }
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
        info!("Received Ctrl+C, shutting down...");
    }

    // Signal shutdown
    let _ = shutdown_tx.send(true);

    // Stop all backends
    info!("Stopping all backends...");
    process_manager.stop_all().await;

    // Stop ACME task if running
    if let Some(handle) = acme_task {
        handle.abort();
    }

    // Wait for servers to stop (with timeout)
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        if let Some(handle) = http_proxy_handle {
            let _ = handle.await;
        }
        if let Some(handle) = https_proxy_handle {
            let _ = handle.await;
        }
        let _ = admin_handle.await;
    })
    .await;

    // Clean up PID file
    if let Some(ref path) = pid_file_path {
        if let Err(e) = std::fs::remove_file(path) {
            warn!(path = %path.display(), error = %e, "Failed to remove PID file");
        }
    }

    info!("Shutdown complete");
    Ok(())
}

async fn idle_cleanup_loop(process_manager: Arc<ProcessManager>, mut shutdown_rx: watch::Receiver<bool>) {
    let interval = Duration::from_secs(10); // Check every 10 seconds

    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {
                process_manager.cleanup_idle_backends().await;
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
        }
    }
}

/// PID file handle that maintains an exclusive lock
#[cfg(unix)]
struct PidFile {
    _file: std::fs::File,
}

#[cfg(unix)]
impl PidFile {
    fn create(path: &Path) -> anyhow::Result<Self> {
        use std::os::unix::io::AsRawFd;

        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;

        // Try to acquire exclusive lock (non-blocking)
        let fd = file.as_raw_fd();
        let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };

        if result != 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::WouldBlock {
                anyhow::bail!("Another instance is already running (PID file is locked)");
            }
            return Err(err.into());
        }

        // Write PID
        let pid = std::process::id();
        use std::io::Write;
        writeln!(&file, "{}", pid)?;

        // Keep the file handle open to maintain the lock
        Ok(Self { _file: file })
    }
}

#[cfg(not(unix))]
struct PidFile;

#[cfg(not(unix))]
impl PidFile {
    fn create(path: &Path) -> anyhow::Result<Self> {
        let pid = std::process::id();
        let mut file = std::fs::File::create(path)?;
        use std::io::Write;
        writeln!(file, "{}", pid)?;
        Ok(Self)
    }
}

fn write_pid_file(path: &Path) -> anyhow::Result<PidFile> {
    PidFile::create(path)
}

fn print_startup_banner(config: &Config) {
    info!(
        name = PKG_NAME,
        version = VERSION,
        "Starting proxy server"
    );
    let http_port = config.server.http_port();
    let https_port = config.server.https_port();
    info!(
        bind = %config.server.bind,
        http_port = if http_port > 0 { Some(http_port) } else { None },
        https_port = if https_port > 0 { Some(https_port) } else { None },
        admin_port = config.server.admin_port,
        tls = config.server.tls_enabled(),
        acme = config.server.acme_enabled(),
        "Server configuration"
    );
    info!(
        pool_max_idle = config.server.pool_max_idle_per_host,
        pool_idle_timeout_secs = config.server.pool_idle_timeout_secs,
        "Connection pool settings"
    );
    info!(
        idle_timeout_secs = config.defaults.idle_timeout_secs,
        startup_timeout_secs = config.defaults.startup_timeout_secs,
        request_timeout_secs = config.defaults.request_timeout_secs,
        "Request handling defaults"
    );
    info!(
        health_path = %config.defaults.health_path,
        health_check_interval_ms = config.defaults.health_check_interval_ms,
        ready_check_interval_ms = config.defaults.ready_health_check_interval_ms,
        unhealthy_threshold = config.defaults.unhealthy_threshold,
        "Health check settings"
    );
    info!(
        shutdown_grace_period_secs = config.defaults.shutdown_grace_period_secs,
        drain_timeout_secs = config.defaults.drain_timeout_secs,
        "Shutdown settings"
    );
    info!(
        backend_count = config.backends.len(),
        backends = ?config.backends.keys().collect::<Vec<_>>(),
        "Configured backends"
    );
}

fn load_certs(path: &str) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let file = File::open(path)
        .map_err(|e| anyhow::anyhow!("Failed to open certificate file {}: {}", path, e))?;
    let mut reader = BufReader::new(file);
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("Failed to parse certificates from {}: {}", path, e))?;

    if certs.is_empty() {
        anyhow::bail!("No certificates found in {}", path);
    }

    Ok(certs)
}

fn load_key(path: &str) -> anyhow::Result<PrivateKeyDer<'static>> {
    let file = File::open(path)
        .map_err(|e| anyhow::anyhow!("Failed to open key file {}: {}", path, e))?;
    let mut reader = BufReader::new(file);

    loop {
        match rustls_pemfile::read_one(&mut reader)
            .map_err(|e| anyhow::anyhow!("Failed to parse key from {}: {}", path, e))?
        {
            Some(rustls_pemfile::Item::Pkcs1Key(key)) => return Ok(key.into()),
            Some(rustls_pemfile::Item::Pkcs8Key(key)) => return Ok(key.into()),
            Some(rustls_pemfile::Item::Sec1Key(key)) => return Ok(key.into()),
            None => break,
            _ => continue,
        }
    }

    anyhow::bail!("No private key found in {}", path)
}

fn generate_self_signed_cert() -> anyhow::Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let subject_alt_names = vec!["localhost".to_string(), "127.0.0.1".to_string()];

    let CertifiedKey { cert, key_pair } = generate_simple_self_signed(subject_alt_names)
        .map_err(|e| anyhow::anyhow!("Failed to generate self-signed certificate: {}", e))?;

    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der = PrivateKeyDer::try_from(key_pair.serialize_der())
        .map_err(|e| anyhow::anyhow!("Failed to serialize private key: {}", e))?;

    Ok((vec![cert_der], key_der))
}
