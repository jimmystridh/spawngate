use crate::acme::Http01Challenges;
use crate::error::{json_error_response, ProxyErrorCode};
use crate::pool::{ConnectionPool, PoolConfig};
use crate::process::{BackendState, ProcessManager, SharedDefaults};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::HeaderValue;
use hyper::service::service_fn;
use hyper::upgrade::Upgraded;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as AutoBuilder;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

const ACME_CHALLENGE_PREFIX: &str = "/.well-known/acme-challenge/";

/// Header name for request ID
const X_REQUEST_ID: &str = "x-request-id";
/// Header name for forwarded-for
const X_FORWARDED_FOR: &str = "x-forwarded-for";
/// Header name for forwarded host
const X_FORWARDED_HOST: &str = "x-forwarded-host";
/// Header name for forwarded proto
const X_FORWARDED_PROTO: &str = "x-forwarded-proto";

/// The main reverse proxy server
pub struct ProxyServer {
    bind_addr: SocketAddr,
    process_manager: Arc<ProcessManager>,
    defaults: SharedDefaults,
    shutdown_rx: watch::Receiver<bool>,
    pool: Arc<ConnectionPool>,
    tls_acceptor: Option<TlsAcceptor>,
    /// If set, redirect all HTTP requests to this HTTPS port
    https_redirect_port: Option<u16>,
    /// ACME HTTP-01 challenges
    acme_challenges: Option<Http01Challenges>,
}

impl ProxyServer {
    pub fn new(
        bind_addr: SocketAddr,
        process_manager: Arc<ProcessManager>,
        defaults: SharedDefaults,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Self {
        Self::with_pool_config(
            bind_addr,
            process_manager,
            defaults,
            shutdown_rx,
            PoolConfig::default(),
        )
    }

    pub fn with_pool_config(
        bind_addr: SocketAddr,
        process_manager: Arc<ProcessManager>,
        defaults: SharedDefaults,
        shutdown_rx: watch::Receiver<bool>,
        pool_config: PoolConfig,
    ) -> Self {
        let pool = Arc::new(ConnectionPool::new(pool_config));
        Self {
            bind_addr,
            process_manager,
            defaults,
            shutdown_rx,
            pool,
            tls_acceptor: None,
            https_redirect_port: None,
            acme_challenges: None,
        }
    }

    pub fn with_tls(mut self, acceptor: TlsAcceptor) -> Self {
        self.tls_acceptor = Some(acceptor);
        self
    }

    /// Enable HTTPS redirect: all HTTP requests will be redirected to HTTPS on the given port
    pub fn with_https_redirect(mut self, port: u16) -> Self {
        self.https_redirect_port = Some(port);
        self
    }

    /// Set ACME HTTP-01 challenge handler
    pub fn with_acme_challenges(mut self, challenges: Http01Challenges) -> Self {
        self.acme_challenges = Some(challenges);
        self
    }

    /// Get the connection pool (for statistics)
    pub fn pool(&self) -> &Arc<ConnectionPool> {
        &self.pool
    }

    pub fn tls_enabled(&self) -> bool {
        self.tls_acceptor.is_some()
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let listener = TcpListener::bind(self.bind_addr).await?;
        let protocol = if self.tls_acceptor.is_some() { "HTTPS" } else { "HTTP" };
        info!(addr = %self.bind_addr, protocol, "Proxy server listening (HTTP/1.1 and HTTP/2)");

        let mut shutdown_rx = self.shutdown_rx.clone();
        let tls_acceptor = self.tls_acceptor.clone();
        let https_redirect_port = self.https_redirect_port;
        let acme_challenges = self.acme_challenges.clone();

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            let process_manager = Arc::clone(&self.process_manager);
                            let defaults = Arc::clone(&self.defaults);
                            let pool = Arc::clone(&self.pool);
                            let tls_acceptor = tls_acceptor.clone();
                            let acme_challenges = acme_challenges.clone();

                            tokio::spawn(async move {
                                if let Some(acceptor) = tls_acceptor {
                                    match acceptor.accept(stream).await {
                                        Ok(tls_stream) => {
                                            if let Err(e) = handle_connection(tls_stream, addr, process_manager, defaults, pool, true, None, None).await {
                                                debug!(addr = %addr, error = %e, "TLS connection error");
                                            }
                                        }
                                        Err(e) => {
                                            debug!(addr = %addr, error = %e, "TLS handshake failed");
                                        }
                                    }
                                } else if let Err(e) = handle_connection(stream, addr, process_manager, defaults, pool, false, https_redirect_port, acme_challenges).await {
                                    debug!(addr = %addr, error = %e, "Connection error");
                                }
                            });
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to accept connection");
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Proxy server shutting down");
                        break;
                    }
                }
            }
        }

        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_connection<S>(
    stream: S,
    addr: SocketAddr,
    process_manager: Arc<ProcessManager>,
    defaults: SharedDefaults,
    pool: Arc<ConnectionPool>,
    is_tls: bool,
    https_redirect_port: Option<u16>,
    acme_challenges: Option<Http01Challenges>,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let io = TokioIo::new(stream);

    let service = service_fn(move |req: Request<Incoming>| {
        let pm = Arc::clone(&process_manager);
        let defs = Arc::clone(&defaults);
        let pool = Arc::clone(&pool);
        let client_addr = addr;
        let acme = acme_challenges.clone();
        async move { handle_request(req, pm, defs, pool, client_addr, is_tls, https_redirect_port, acme).await }
    });

    // Use auto::Builder to support both HTTP/1.1 and HTTP/2
    // HTTP/2 uses h2c (HTTP/2 cleartext) or h2 over TLS
    // HTTP/1.1 connections can still use WebSocket upgrades
    AutoBuilder::new(TokioExecutor::new())
        .http1()
        .preserve_header_case(true)
        .http2()
        .max_concurrent_streams(250)
        .serve_connection_with_upgrades(io, service)
        .await
        .map_err(|e| anyhow::anyhow!("Connection error: {}", e))?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_request(
    mut req: Request<Incoming>,
    process_manager: Arc<ProcessManager>,
    defaults: SharedDefaults,
    pool: Arc<ConnectionPool>,
    client_addr: SocketAddr,
    is_tls: bool,
    https_redirect_port: Option<u16>,
    acme_challenges: Option<Http01Challenges>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    // Handle ACME HTTP-01 challenges first (before HTTPS redirect)
    if let Some(ref challenges) = acme_challenges {
        let path = req.uri().path();
        if let Some(token) = path.strip_prefix(ACME_CHALLENGE_PREFIX) {
            if let Some(key_auth) = challenges.get(token).await {
                debug!(token, "Responding to ACME HTTP-01 challenge");
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header(hyper::header::CONTENT_TYPE, "text/plain")
                    .body(Full::new(Bytes::from(key_auth)).map_err(|never| match never {}).boxed())
                    .expect("valid response builder"));
            }
        }
    }

    // Handle HTTPS redirect if configured (for non-TLS connections)
    if let Some(redirect_port) = https_redirect_port {
        if !is_tls {
            return Ok(build_https_redirect(&req, redirect_port));
        }
    }

    // Generate or propagate request ID
    let request_id = req
        .headers()
        .get(X_REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    // Extract hostname from Host header
    let hostname = match extract_hostname(&req) {
        Some(h) => h,
        None => {
            return Ok(json_error_response(
                ProxyErrorCode::MissingHostHeader,
                "Missing or invalid Host header",
            ));
        }
    };

    // Add proxy headers
    // Security: We overwrite X-Forwarded-* headers rather than appending to prevent
    // client spoofing. This proxy is assumed to be the first trusted hop.
    let headers = req.headers_mut();

    // Set X-Request-ID
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        headers.insert(X_REQUEST_ID, value);
    }

    // Set X-Forwarded-For to the actual client IP (overwrites any client-provided value)
    if let Ok(value) = HeaderValue::from_str(&client_addr.ip().to_string()) {
        headers.insert(X_FORWARDED_FOR, value);
    }

    // Set X-Forwarded-Host (original Host header, overwrites any client-provided value)
    if let Some(host) = headers.get(hyper::header::HOST).cloned() {
        headers.insert(X_FORWARDED_HOST, host);
    }

    // Set X-Forwarded-Proto (overwrites any client-provided value)
    let proto = if is_tls { "https" } else { "http" };
    headers.insert(X_FORWARDED_PROTO, HeaderValue::from_static(proto));

    debug!(hostname, method = %req.method(), uri = %req.uri(), request_id, "Incoming request");

    // Check if we have a backend configured for this host
    if !process_manager.has_backend(&hostname) {
        // Don't reveal whether host exists - use generic message
        return Ok(json_error_response(
            ProxyErrorCode::UnknownHost,
            "Unknown or unconfigured host",
        ));
    }

    // Check if backend is in draining mode (stopping)
    let state = process_manager.get_state(&hostname);
    if state == BackendState::Stopping {
        return Ok(json_error_response(
            ProxyErrorCode::BackendShuttingDown,
            "Backend is shutting down, please retry later",
        ));
    }

    // Check if backend is unhealthy
    if state == BackendState::Unhealthy {
        return Ok(json_error_response(
            ProxyErrorCode::BackendUnhealthy,
            "Backend is currently unhealthy, auto-restart in progress",
        ));
    }

    // Ensure backend is running and ready
    match ensure_backend_ready(&hostname, &process_manager, &defaults).await {
        Ok(()) => {}
        Err(e) => {
            // Log detailed error internally, return generic message externally
            error!(hostname, error = %e, "Failed to start backend");
            return Ok(json_error_response(
                ProxyErrorCode::BackendStartFailed,
                "Backend unavailable",
            ));
        }
    }

    // Update activity timestamp
    process_manager.touch(&hostname);

    // Get the backend port and request timeout
    let (port, request_timeout) = match process_manager.get_config(&hostname) {
        Some(config) => {
            let defaults_ref = defaults.read();
            (config.port, config.request_timeout(&defaults_ref))
        }
        None => {
            return Ok(json_error_response(
                ProxyErrorCode::BackendConfigError,
                "Backend configuration not found",
            ));
        }
    };

    // Check for WebSocket/HTTP upgrade request
    if is_upgrade_request(&req) {
        return handle_upgrade(req, process_manager, hostname, port, request_id).await;
    }

    // Track in-flight request - also atomically verifies backend is still Ready
    if !process_manager.increment_in_flight(&hostname) {
        // Backend state changed between ensure_backend_ready and now
        return Ok(json_error_response(
            ProxyErrorCode::BackendShuttingDown,
            "Backend state changed, please retry",
        ));
    }

    // Forward the request through the connection pool with timeout
    let result = tokio::time::timeout(request_timeout, pool.send_request(req, port)).await;

    // Decrement in-flight counter when done
    process_manager.decrement_in_flight(&hostname);

    match result {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(e)) => {
            // Log detailed error internally, return generic message externally
            error!(hostname, port, error = %e, "Failed to forward request via pool");
            Ok(json_error_response(
                ProxyErrorCode::ConnectionFailed,
                "Failed to connect to backend",
            ))
        }
        Err(_) => {
            warn!(
                hostname,
                port,
                timeout_secs = request_timeout.as_secs(),
                "Request timed out"
            );
            Ok(json_error_response(
                ProxyErrorCode::RequestTimeout,
                format!(
                    "Request timed out after {} seconds",
                    request_timeout.as_secs()
                ),
            ))
        }
    }
}

/// Maximum hostname length per DNS specification
const MAX_HOSTNAME_LEN: usize = 253;

fn extract_hostname(req: &Request<Incoming>) -> Option<String> {
    req.headers()
        .get(hyper::header::HOST)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| {
            // Strip port if present
            let hostname = h.split(':').next()?;

            // Validate length (DNS max is 253 characters)
            if hostname.len() > MAX_HOSTNAME_LEN {
                return None;
            }

            // Validate characters: alphanumeric, hyphen, and dot only
            // This prevents log injection and other attacks
            if !hostname.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.') {
                return None;
            }

            Some(hostname.to_lowercase())
        })
}

/// Build an HTTPS redirect response (301 Moved Permanently)
fn build_https_redirect(req: &Request<Incoming>, https_port: u16) -> Response<BoxBody<Bytes, hyper::Error>> {
    let host = req
        .headers()
        .get(hyper::header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(|h| h.split(':').next().unwrap_or(h))
        .unwrap_or("localhost");

    let path = req.uri().path_and_query().map(|pq| pq.as_str()).unwrap_or("/");

    let location = if https_port == 443 {
        format!("https://{}{}", host, path)
    } else {
        format!("https://{}:{}{}", host, https_port, path)
    };

    Response::builder()
        .status(StatusCode::MOVED_PERMANENTLY)
        .header(hyper::header::LOCATION, location)
        .header(hyper::header::CONTENT_TYPE, "text/plain")
        .body(
            http_body_util::Full::new(Bytes::from("Redirecting to HTTPS"))
                .map_err(|never| match never {})
                .boxed(),
        )
        .expect("valid response builder")
}

async fn ensure_backend_ready(
    hostname: &str,
    process_manager: &Arc<ProcessManager>,
    defaults: &SharedDefaults,
) -> anyhow::Result<()> {
    let state = process_manager.get_state(hostname);

    match state {
        BackendState::Ready => {
            // Already running and ready
            return Ok(());
        }
        BackendState::Starting => {
            // Wait for it to become ready
            return wait_for_ready(hostname, process_manager, defaults).await;
        }
        BackendState::Stopping => {
            // Wait a bit and then try to start
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        BackendState::Unhealthy => {
            // Backend is unhealthy - auto-restart should be in progress
            // Return error so proxy reports unhealthy state
            return Err(anyhow::anyhow!("Backend is unhealthy"));
        }
        BackendState::Stopped => {
            // Need to start it
        }
    }

    // Start the backend
    process_manager.start_backend(hostname).await?;

    // Wait for it to become ready
    wait_for_ready(hostname, process_manager, defaults).await
}

async fn wait_for_ready(
    hostname: &str,
    process_manager: &Arc<ProcessManager>,
    defaults: &SharedDefaults,
) -> anyhow::Result<()> {
    let config = process_manager
        .get_config(hostname)
        .ok_or_else(|| anyhow::anyhow!("Backend not found"))?;

    let timeout = config.startup_timeout(&defaults.read());

    // Subscribe to ready notifications
    let mut ready_rx = process_manager
        .subscribe_ready(hostname)
        .ok_or_else(|| anyhow::anyhow!("Backend not starting"))?;

    // Wait for ready signal or timeout
    let result = tokio::time::timeout(timeout, async {
        loop {
            // Check if already ready
            if process_manager.is_ready(hostname) {
                return Ok(());
            }

            // Wait for notification
            match ready_rx.recv().await {
                Ok(()) => return Ok(()),
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    return Err(anyhow::anyhow!("Backend failed to start"));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Check state again
                    if process_manager.is_ready(hostname) {
                        return Ok(());
                    }
                }
            }
        }
    })
    .await;

    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(anyhow::anyhow!("Timeout waiting for backend to start")),
    }
}

/// Check if a request is a WebSocket upgrade request
fn is_upgrade_request(req: &Request<Incoming>) -> bool {
    // Check for Connection: Upgrade header (case-insensitive value check)
    let has_upgrade_connection = req
        .headers()
        .get(hyper::header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_lowercase().contains("upgrade"))
        .unwrap_or(false);

    // Check for Upgrade header present
    let has_upgrade_header = req.headers().contains_key(hyper::header::UPGRADE);

    has_upgrade_connection && has_upgrade_header
}

/// Get the value of the Upgrade header
fn get_upgrade_type(req: &Request<Incoming>) -> Option<String> {
    req.headers()
        .get(hyper::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_lowercase())
}

/// Forward bytes bidirectionally between client and backend connections
async fn forward_bidirectional(
    client: Upgraded,
    backend: TcpStream,
    hostname: &str,
    request_id: &str,
) {
    let mut client_io = TokioIo::new(client);
    let mut backend_io = backend;

    // Use tokio's copy_bidirectional for efficient forwarding
    match tokio::io::copy_bidirectional(&mut client_io, &mut backend_io).await {
        Ok((client_to_backend, backend_to_client)) => {
            debug!(
                hostname,
                request_id,
                client_to_backend,
                backend_to_client,
                "WebSocket connection closed normally"
            );
        }
        Err(e) => {
            debug!(hostname, request_id, error = %e, "WebSocket connection closed with error");
        }
    }
}

/// Build the raw HTTP upgrade request to send to the backend
fn build_upgrade_request(req: &Request<Incoming>, port: u16) -> Vec<u8> {
    let path = req.uri().path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    let mut request = format!(
        "{} {} HTTP/1.1\r\n",
        req.method(),
        path
    );

    // Forward all headers
    for (name, value) in req.headers() {
        if let Ok(v) = value.to_str() {
            request.push_str(&format!("{}: {}\r\n", name, v));
        }
    }

    // Update Host header to point to backend
    request.push_str(&format!("Host: 127.0.0.1:{}\r\n", port));
    request.push_str("\r\n");

    request.into_bytes()
}

/// Parse the HTTP response from the backend to check for 101 Switching Protocols
fn parse_upgrade_response(data: &[u8]) -> Option<(StatusCode, Vec<(String, String)>)> {
    let response_str = std::str::from_utf8(data).ok()?;
    let mut lines = response_str.lines();

    // Parse status line: HTTP/1.1 101 Switching Protocols
    let status_line = lines.next()?;
    let parts: Vec<&str> = status_line.splitn(3, ' ').collect();
    if parts.len() < 2 {
        return None;
    }

    let status_code: u16 = parts[1].parse().ok()?;
    let status = StatusCode::from_u16(status_code).ok()?;

    // Parse headers
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }

    Some((status, headers))
}

/// Handle a WebSocket upgrade request
async fn handle_upgrade(
    req: Request<Incoming>,
    process_manager: Arc<ProcessManager>,
    hostname: String,
    port: u16,
    request_id: String,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let upgrade_type = get_upgrade_type(&req).unwrap_or_else(|| "unknown".to_string());
    debug!(hostname, request_id, upgrade_type, "Handling upgrade request");

    // Build the raw HTTP request to send to the backend
    let raw_request = build_upgrade_request(&req, port);

    // Connect to the backend
    let backend_addr = format!("127.0.0.1:{}", port);
    let mut backend_stream = match TcpStream::connect(&backend_addr).await {
        Ok(stream) => stream,
        Err(e) => {
            error!(hostname, port, error = %e, "Failed to connect to backend for upgrade");
            return Ok(json_error_response(
                ProxyErrorCode::ConnectionFailed,
                format!("Failed to connect to backend: {}", e),
            ));
        }
    };

    // Send the upgrade request to the backend
    if let Err(e) = backend_stream.write_all(&raw_request).await {
        error!(hostname, error = %e, "Failed to send upgrade request to backend");
        return Ok(json_error_response(
            ProxyErrorCode::ConnectionFailed,
            format!("Failed to send upgrade request: {}", e),
        ));
    }

    // Read the backend's response
    let mut response_buf = vec![0u8; 4096];
    let n = match backend_stream.read(&mut response_buf).await {
        Ok(n) if n > 0 => n,
        Ok(_) => {
            error!(hostname, "Backend closed connection before responding to upgrade");
            return Ok(json_error_response(
                ProxyErrorCode::ConnectionFailed,
                "Backend closed connection",
            ));
        }
        Err(e) => {
            error!(hostname, error = %e, "Failed to read upgrade response from backend");
            return Ok(json_error_response(
                ProxyErrorCode::ConnectionFailed,
                format!("Failed to read backend response: {}", e),
            ));
        }
    };

    // Parse the backend's response
    let (status, response_headers) = match parse_upgrade_response(&response_buf[..n]) {
        Some(parsed) => parsed,
        None => {
            error!(hostname, "Failed to parse backend upgrade response");
            return Ok(json_error_response(
                ProxyErrorCode::ConnectionFailed,
                "Invalid upgrade response from backend",
            ));
        }
    };

    // Check if backend accepted the upgrade
    if status != StatusCode::SWITCHING_PROTOCOLS {
        warn!(hostname, status = %status, "Backend rejected upgrade request");
        // Return the backend's non-101 response as-is
        let mut response = Response::builder().status(status);
        for (name, value) in &response_headers {
            if let Ok(hv) = HeaderValue::from_str(value) {
                response = response.header(name.as_str(), hv);
            }
        }
        return Ok(response
            .body(Empty::<Bytes>::new().map_err(|never| match never {}).boxed())
            .expect("valid response builder"));
    }

    info!(hostname, request_id, upgrade_type, "WebSocket upgrade successful");

    // Track the WebSocket connection as in-flight - atomically verifies backend is Ready
    if !process_manager.increment_in_flight(&hostname) {
        return Ok(json_error_response(
            ProxyErrorCode::BackendShuttingDown,
            "Backend state changed, please retry",
        ));
    }

    // Build the 101 response to send to the client
    let mut response = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
    for (name, value) in &response_headers {
        // Skip hop-by-hop headers that hyper handles
        let name_lower = name.to_lowercase();
        if name_lower == "content-length" || name_lower == "transfer-encoding" {
            continue;
        }
        if let Ok(hv) = HeaderValue::from_str(value) {
            response = response.header(name.as_str(), hv);
        }
    }

    let response = response
        .body(Empty::<Bytes>::new().map_err(|never| match never {}).boxed())
        .expect("valid response builder");

    // Spawn the bidirectional forwarding task
    let pm = process_manager.clone();
    let hostname_clone = hostname.clone();
    let request_id_clone = request_id.clone();
    tokio::spawn(async move {
        // Wait for the client upgrade to complete
        match hyper::upgrade::on(req).await {
            Ok(upgraded) => {
                debug!(hostname = hostname_clone, request_id = request_id_clone, "Client upgrade complete, starting forwarding");
                forward_bidirectional(upgraded, backend_stream, &hostname_clone, &request_id_clone).await;
            }
            Err(e) => {
                error!(hostname = hostname_clone, error = %e, "Failed to upgrade client connection");
            }
        }
        // Decrement in-flight when done
        pm.decrement_in_flight(&hostname_clone);
        debug!(hostname = hostname_clone, request_id = request_id_clone, "WebSocket connection closed");
    });

    Ok(response)
}
