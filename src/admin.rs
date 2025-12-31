use crate::process::ProcessManager;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::header::AUTHORIZATION;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as AutoBuilder;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, error, info, warn};

/// Version information for the proxy
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PKG_NAME: &str = env!("CARGO_PKG_NAME");

/// Helper to create a simple response - infallible with valid StatusCode
fn response(status: StatusCode, body: impl Into<Bytes>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .body(Full::new(body.into()))
        .expect("valid response with StatusCode enum")
}

/// Helper to create a JSON response
fn json_response(status: StatusCode, body: impl Into<Bytes>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(body.into()))
        .expect("valid response with StatusCode enum and static header")
}

/// Admin API server for backend callbacks
pub struct AdminServer {
    bind_addr: SocketAddr,
    process_manager: Arc<ProcessManager>,
    shutdown_rx: watch::Receiver<bool>,
    tls_acceptor: Option<TlsAcceptor>,
    auth_token: Arc<String>,
}

impl AdminServer {
    pub fn new(
        bind_addr: SocketAddr,
        process_manager: Arc<ProcessManager>,
        shutdown_rx: watch::Receiver<bool>,
        auth_token: String,
    ) -> Self {
        Self {
            bind_addr,
            process_manager,
            shutdown_rx,
            tls_acceptor: None,
            auth_token: Arc::new(auth_token),
        }
    }

    pub fn with_tls(mut self, acceptor: TlsAcceptor) -> Self {
        self.tls_acceptor = Some(acceptor);
        self
    }

    pub fn tls_enabled(&self) -> bool {
        self.tls_acceptor.is_some()
    }

    pub fn auth_token(&self) -> &str {
        &self.auth_token
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let listener = TcpListener::bind(self.bind_addr).await?;
        let protocol = if self.tls_acceptor.is_some() { "HTTPS" } else { "HTTP" };
        info!(addr = %self.bind_addr, protocol, "Admin API server listening (HTTP/1.1 and HTTP/2)");

        let mut shutdown_rx = self.shutdown_rx.clone();
        let tls_acceptor = self.tls_acceptor.clone();
        let auth_token = Arc::clone(&self.auth_token);

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            let process_manager = Arc::clone(&self.process_manager);
                            let tls_acceptor = tls_acceptor.clone();
                            let auth_token = Arc::clone(&auth_token);

                            tokio::spawn(async move {
                                if let Some(acceptor) = tls_acceptor {
                                    match acceptor.accept(stream).await {
                                        Ok(tls_stream) => {
                                            if let Err(e) = serve_admin_connection(tls_stream, addr, process_manager, auth_token).await {
                                                debug!(addr = %addr, error = %e, "Admin TLS connection error");
                                            }
                                        }
                                        Err(e) => {
                                            debug!(addr = %addr, error = %e, "Admin TLS handshake failed");
                                        }
                                    }
                                } else if let Err(e) = serve_admin_connection(stream, addr, process_manager, auth_token).await {
                                    debug!(addr = %addr, error = %e, "Admin connection error");
                                }
                            });
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to accept admin connection");
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Admin server shutting down");
                        break;
                    }
                }
            }
        }

        Ok(())
    }
}

async fn serve_admin_connection<S>(
    stream: S,
    _addr: SocketAddr,
    process_manager: Arc<ProcessManager>,
    auth_token: Arc<String>,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let io = TokioIo::new(stream);
    let service = service_fn(move |req| {
        let pm = Arc::clone(&process_manager);
        let token = Arc::clone(&auth_token);
        async move { handle_admin_request(req, pm, token).await }
    });

    AutoBuilder::new(TokioExecutor::new())
        .serve_connection(io, service)
        .await
        .map_err(|e| anyhow::anyhow!("Admin connection error: {}", e))?;

    Ok(())
}

fn check_auth(req: &Request<hyper::body::Incoming>, expected_token: &str) -> bool {
    req.headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|auth| {
            // Support "Bearer <token>" format
            auth.strip_prefix("Bearer ")
                .unwrap_or(auth)
                .eq(expected_token)
        })
        .unwrap_or(false)
}

async fn handle_admin_request(
    req: Request<hyper::body::Incoming>,
    process_manager: Arc<ProcessManager>,
    auth_token: Arc<String>,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let path = req.uri().path();
    let method = req.method();

    debug!(%method, %path, "Admin API request");

    let response = match (method, path) {
        // Health check for the admin API itself (no auth required)
        (&Method::GET, "/health") => response(StatusCode::OK, "ok"),

        // Version endpoint: GET /version (no auth required)
        (&Method::GET, "/version") => {
            let version_info = serde_json::json!({
                "name": PKG_NAME,
                "version": VERSION,
            });
            json_response(StatusCode::OK, version_info.to_string())
        }

        // Backend ready callback: POST /ready/{hostname} (auth required)
        (&Method::POST, path) if path.starts_with("/ready/") => {
            if !check_auth(&req, &auth_token) {
                warn!(path, "Unauthorized admin API request");
                response(StatusCode::UNAUTHORIZED, "unauthorized")
            } else {
                let hostname = path.strip_prefix("/ready/").unwrap_or("");
                if hostname.is_empty() {
                    response(StatusCode::BAD_REQUEST, "missing hostname")
                } else if process_manager.mark_ready(hostname) {
                    info!(hostname, "Backend marked ready via callback");
                    response(StatusCode::OK, "ok")
                } else {
                    response(StatusCode::NOT_FOUND, "backend not starting")
                }
            }
        }

        // List backends and their status: GET /backends (auth required)
        (&Method::GET, "/backends") => {
            if !check_auth(&req, &auth_token) {
                warn!(path, "Unauthorized admin API request");
                response(StatusCode::UNAUTHORIZED, "unauthorized")
            } else {
                let backends = process_manager.list_backends();
                let backend_list: Vec<serde_json::Value> = backends
                    .into_iter()
                    .map(|b| {
                        serde_json::json!({
                            "hostname": b.hostname,
                            "state": b.state,
                            "port": b.port,
                            "in_flight": b.in_flight
                        })
                    })
                    .collect();
                let response_body = serde_json::json!({
                    "backends": backend_list,
                    "count": backend_list.len()
                });
                json_response(StatusCode::OK, response_body.to_string())
            }
        }

        // 404 for everything else
        _ => response(StatusCode::NOT_FOUND, "not found"),
    };

    Ok(response)
}
