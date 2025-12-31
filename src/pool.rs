//! Connection pool for backend HTTP connections
//!
//! This module provides connection pooling for efficient reuse of HTTP connections
//! to backend servers, reducing latency and resource usage.

use http_body_util::{combinators::BoxBody, BodyExt, Empty};
use hyper::body::{Bytes, Incoming};
use hyper::{Request, Response};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::debug;

/// Error type for connection pool operations
#[derive(Debug)]
pub enum PoolError {
    /// Error from the HTTP client
    Client(hyper_util::client::legacy::Error),
    /// Error building a request
    RequestBuild(String),
}

impl std::fmt::Display for PoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PoolError::Client(e) => write!(f, "Client error: {}", e),
            PoolError::RequestBuild(s) => write!(f, "Request build error: {}", s),
        }
    }
}

impl std::error::Error for PoolError {}

impl From<hyper_util::client::legacy::Error> for PoolError {
    fn from(err: hyper_util::client::legacy::Error) -> Self {
        PoolError::Client(err)
    }
}

/// Statistics for the connection pool
#[derive(Debug, Default)]
pub struct PoolStats {
    /// Total number of requests made through the pool
    pub total_requests: AtomicU64,
    /// Total number of health check requests
    pub health_checks: AtomicU64,
}

impl PoolStats {
    /// Record a regular request
    pub fn record_request(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a health check request
    pub fn record_health_check(&self) {
        self.health_checks.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get_total_requests(&self) -> u64 {
        self.total_requests.load(Ordering::Relaxed)
    }

    pub fn get_health_checks(&self) -> u64 {
        self.health_checks.load(Ordering::Relaxed)
    }
}

/// Configuration for the connection pool
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Maximum idle connections per host
    pub max_idle_per_host: usize,
    /// Idle connection timeout
    pub idle_timeout: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_idle_per_host: 10,
            idle_timeout: Duration::from_secs(90),
        }
    }
}

/// A connection pool for HTTP connections to backend servers
pub struct ConnectionPool {
    /// Main client for proxying requests
    client: Client<HttpConnector, Incoming>,
    /// Dedicated client for health checks (uses Empty body type)
    health_client: Client<HttpConnector, Empty<Bytes>>,
    stats: Arc<PoolStats>,
    config: PoolConfig,
}

impl ConnectionPool {
    /// Create a new connection pool with the given configuration
    pub fn new(config: PoolConfig) -> Self {
        let mut connector = HttpConnector::new();
        connector.set_nodelay(true);
        connector.enforce_http(true);

        // Build the main client with connection pooling
        let client = Client::builder(TokioExecutor::new())
            .pool_max_idle_per_host(config.max_idle_per_host)
            .pool_idle_timeout(config.idle_timeout)
            .build(connector.clone());

        // Build a dedicated health check client (reused across health checks)
        let health_client = Client::builder(TokioExecutor::new())
            .pool_max_idle_per_host(config.max_idle_per_host)
            .pool_idle_timeout(config.idle_timeout)
            .build(connector);

        debug!(
            max_idle = config.max_idle_per_host,
            idle_timeout_secs = config.idle_timeout.as_secs(),
            "Connection pool initialized"
        );

        Self {
            client,
            health_client,
            stats: Arc::new(PoolStats::default()),
            config,
        }
    }

    /// Get the pool configuration
    pub fn config(&self) -> &PoolConfig {
        &self.config
    }

    /// Get pool statistics
    pub fn stats(&self) -> Arc<PoolStats> {
        Arc::clone(&self.stats)
    }

    /// Send a request through the connection pool
    pub async fn send_request(
        &self,
        req: Request<Incoming>,
        port: u16,
    ) -> Result<Response<BoxBody<Bytes, hyper::Error>>, PoolError> {
        // Build the URI for the backend
        let uri = format!("http://127.0.0.1:{}{}", port, req.uri().path_and_query().map(|pq| pq.as_str()).unwrap_or("/"));

        // Create a new request with the backend URI
        let (parts, body) = req.into_parts();
        let mut builder = Request::builder()
            .method(parts.method)
            .uri(&uri);

        // Copy headers
        for (key, value) in parts.headers.iter() {
            builder = builder.header(key, value);
        }

        let backend_req = builder
            .body(body)
            .map_err(|e| PoolError::RequestBuild(e.to_string()))?;

        // Record statistics
        self.stats.record_request();

        // Send the request through the pooled client
        let response = self.client.request(backend_req).await?;

        // Convert the response body to BoxBody
        let (parts, body) = response.into_parts();
        let boxed_body = body.boxed();

        Ok(Response::from_parts(parts, boxed_body))
    }

    /// Check if a backend is reachable (useful for health checks)
    /// Uses the dedicated health check client for connection reuse
    pub async fn check_backend(&self, port: u16, path: &str) -> bool {
        let uri = format!("http://127.0.0.1:{}{}", port, path);

        let req = match Request::builder()
            .method("GET")
            .uri(&uri)
            .header("Connection", "keep-alive")
            .body(Empty::<Bytes>::new())
        {
            Ok(r) => r,
            Err(_) => return false,
        };

        // Record health check
        self.stats.record_health_check();

        // Use the dedicated health client (reused across checks)
        match self.health_client.request(req).await {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_config_default() {
        let config = PoolConfig::default();
        assert_eq!(config.max_idle_per_host, 10);
        assert_eq!(config.idle_timeout, Duration::from_secs(90));
    }

    #[test]
    fn test_pool_stats() {
        let stats = PoolStats::default();

        assert_eq!(stats.get_total_requests(), 0);
        assert_eq!(stats.get_health_checks(), 0);

        stats.record_request();
        assert_eq!(stats.get_total_requests(), 1);
        assert_eq!(stats.get_health_checks(), 0);

        stats.record_request();
        stats.record_health_check();
        assert_eq!(stats.get_total_requests(), 2);
        assert_eq!(stats.get_health_checks(), 1);
    }

    #[test]
    fn test_pool_creation() {
        let config = PoolConfig {
            max_idle_per_host: 5,
            idle_timeout: Duration::from_secs(30),
        };

        let pool = ConnectionPool::new(config.clone());
        assert_eq!(pool.config().max_idle_per_host, 5);
        assert_eq!(pool.config().idle_timeout, Duration::from_secs(30));
        assert_eq!(pool.stats().get_total_requests(), 0);
    }
}
