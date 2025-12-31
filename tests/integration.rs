//! Integration tests for Spawngate

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use spawngate::admin::AdminServer;
use spawngate::config::{BackendConfig, BackendDefaults, Config};
use spawngate::pool::PoolConfig;
use spawngate::process::{BackendState, ProcessManager};
use spawngate::proxy::ProxyServer;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::watch;

/// Get the path to the mock server binary
fn mock_server_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    #[cfg(windows)]
    path.push("tests/mock_server/target/release/mock-server.exe");
    #[cfg(not(windows))]
    path.push("tests/mock_server/target/release/mock-server");
    path
}

/// Helper to create a backend config using the mock server
fn mock_backend_config(port: u16) -> BackendConfig {
    let mut config = BackendConfig::local(&mock_server_path().to_string_lossy(), port);
    config.health_path = Some("/health".to_string());
    config.idle_timeout_secs = Some(5); // Short for testing
    config.startup_timeout_secs = Some(10);
    config.health_check_interval_ms = Some(50);
    config.shutdown_grace_period_secs = Some(2); // Short for testing
    config.drain_timeout_secs = Some(5);
    config.request_timeout_secs = Some(30);
    config.ready_health_check_interval_ms = Some(1000); // 1 second for testing
    config.unhealthy_threshold = Some(3);
    config
}

/// Helper to create a backend config with startup delay
fn mock_backend_config_with_delay(port: u16, delay_ms: u64) -> BackendConfig {
    let mut env = HashMap::new();
    env.insert("STARTUP_DELAY_MS".to_string(), delay_ms.to_string());

    let mut config = BackendConfig::local(&mock_server_path().to_string_lossy(), port);
    config.env = env;
    config.health_path = Some("/health".to_string());
    config.idle_timeout_secs = Some(5);
    config.startup_timeout_secs = Some(10);
    config.health_check_interval_ms = Some(50);
    config.shutdown_grace_period_secs = Some(2);
    config.drain_timeout_secs = Some(5);
    config.request_timeout_secs = Some(30);
    config.ready_health_check_interval_ms = Some(1000);
    config.unhealthy_threshold = Some(3);
    config
}

/// Wait for a port to become available (server listening)
async fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .is_ok()
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// Send a simple HTTP request and get response
async fn http_get(port: u16, path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port)).await?;

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        path, port
    );
    stream.write_all(request.as_bytes()).await?;

    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    Ok(response)
}

/// Send authenticated HTTP GET request (for admin API testing)
async fn http_get_with_auth(port: u16, path: &str, token: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port)).await?;

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        path, port, token
    );
    stream.write_all(request.as_bytes()).await?;

    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    Ok(response)
}

/// Send HTTP request with custom Host header (for proxy testing)
async fn http_get_with_host(
    port: u16,
    path: &str,
    host: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port)).await?;

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        path, host
    );
    stream.write_all(request.as_bytes()).await?;

    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    Ok(response)
}

// ============================================================================
// Basic Configuration Tests
// ============================================================================

#[test]
fn test_full_config_parsing() {
    let toml = r#"
[server]
port = 8080
bind = "127.0.0.1"
admin_port = 9000

[defaults]
idle_timeout_secs = 300
startup_timeout_secs = 45
health_check_interval_ms = 250
health_path = "/ready"

[backends."frontend.example.com"]
command = "npm"
args = ["start"]
port = 3000
working_dir = "/app/frontend"
idle_timeout_secs = 600

[backends."frontend.example.com".env]
NODE_ENV = "production"

[backends."api.example.com"]
command = "python"
args = ["-m", "uvicorn", "main:app"]
port = 8000
health_path = "/health"
startup_timeout_secs = 60
"#;

    let config: Config = toml::from_str(toml).unwrap();

    assert_eq!(config.server.port, 8080);
    assert_eq!(config.server.bind, "127.0.0.1");
    assert_eq!(config.defaults.idle_timeout_secs, 300);

    let frontend = config.backends.get("frontend.example.com").unwrap();
    assert_eq!(frontend.command, Some("npm".to_string()));
    assert_eq!(frontend.port, 3000);
}

#[test]
fn test_backend_config_default_fallback() {
    let defaults = BackendDefaults {
        idle_timeout_secs: 300,
        startup_timeout_secs: 45,
        health_check_interval_ms: 200,
        health_path: "/health".to_string(),
        shutdown_grace_period_secs: 10,
        drain_timeout_secs: 30,
        request_timeout_secs: 30,
        ready_health_check_interval_ms: 5000,
        unhealthy_threshold: 3,
    };

    let mut backend = BackendConfig::local("node", 3000);
    backend.idle_timeout_secs = Some(60);

    assert_eq!(backend.idle_timeout(&defaults), Duration::from_secs(60));
    assert_eq!(backend.startup_timeout(&defaults), Duration::from_secs(45));
    assert_eq!(backend.health_path(&defaults), "/health");
    assert_eq!(
        backend.shutdown_grace_period(&defaults),
        Duration::from_secs(10)
    );
    assert_eq!(backend.drain_timeout(&defaults), Duration::from_secs(30));
}

// ============================================================================
// Process Manager Tests
// ============================================================================

#[tokio::test]
async fn test_process_manager_backend_lookup() {
    let mut configs = HashMap::new();
    configs.insert("app.example.com".to_string(), mock_backend_config(30000));

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    assert!(manager.has_backend("app.example.com"));
    assert!(!manager.has_backend("other.example.com"));
    assert_eq!(manager.get_backend_port("app.example.com"), Some(30000));
}

// ============================================================================
// Mock Server Lifecycle Tests
// ============================================================================

#[tokio::test]
async fn test_mock_server_starts_and_responds() {
    // Skip if mock server not built
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let port = 30001;
    let mut configs = HashMap::new();
    configs.insert("test.local".to_string(), mock_backend_config(port));

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start the backend
    manager.start_backend("test.local").await.unwrap();
    assert_eq!(manager.get_state("test.local"), BackendState::Starting);

    // Wait for it to be listening
    assert!(
        wait_for_port(port, Duration::from_secs(5)).await,
        "Mock server did not start in time"
    );

    // Backend should become ready (health check will mark it)
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(manager.get_state("test.local"), BackendState::Ready);

    // Make a request to verify it's working
    let response = http_get(port, "/echo").await.unwrap();
    assert!(response.contains("200 OK"));
    assert!(response.contains("echo response"));

    // Cleanup
    manager.stop_backend("test.local").await;
    assert_eq!(manager.get_state("test.local"), BackendState::Stopped);
}

#[tokio::test]
async fn test_mock_server_health_check_polling() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let port = 30002;

    // Backend with startup delay - health polling should wait
    let mut configs = HashMap::new();
    configs.insert(
        "delayed.local".to_string(),
        mock_backend_config_with_delay(port, 500), // 500ms delay
    );

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    let start = std::time::Instant::now();
    manager.start_backend("delayed.local").await.unwrap();

    // Should be starting
    assert_eq!(manager.get_state("delayed.local"), BackendState::Starting);

    // Wait for ready
    tokio::time::sleep(Duration::from_millis(800)).await;

    // Should now be ready (after delay + health check)
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(500),
        "Should have waited for startup delay"
    );

    // Verify it's ready
    assert!(wait_for_port(port, Duration::from_secs(2)).await);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(manager.get_state("delayed.local"), BackendState::Ready);

    // Cleanup
    manager.stop_backend("delayed.local").await;
}

#[tokio::test]
async fn test_mock_server_restart_after_stop() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let port = 30003;
    let mut configs = HashMap::new();
    configs.insert("restart.local".to_string(), mock_backend_config(port));

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // First start
    manager.start_backend("restart.local").await.unwrap();
    assert!(wait_for_port(port, Duration::from_secs(5)).await);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(manager.get_state("restart.local"), BackendState::Ready);

    // Stop
    manager.stop_backend("restart.local").await;
    assert_eq!(manager.get_state("restart.local"), BackendState::Stopped);

    // Wait for port to be released
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Restart
    manager.start_backend("restart.local").await.unwrap();
    assert!(wait_for_port(port, Duration::from_secs(5)).await);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(manager.get_state("restart.local"), BackendState::Ready);

    // Verify it responds
    let response = http_get(port, "/health").await.unwrap();
    assert!(response.contains("200 OK"));

    // Cleanup
    manager.stop_backend("restart.local").await;
}

#[tokio::test]
async fn test_stop_all_backends() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let mut configs = HashMap::new();
    configs.insert("a.local".to_string(), mock_backend_config(30004));
    configs.insert("b.local".to_string(), mock_backend_config(30005));

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start both
    manager.start_backend("a.local").await.unwrap();
    manager.start_backend("b.local").await.unwrap();

    // Wait for both to be ready
    assert!(wait_for_port(30004, Duration::from_secs(5)).await);
    assert!(wait_for_port(30005, Duration::from_secs(5)).await);

    // Stop all
    manager.stop_all().await;

    assert_eq!(manager.get_state("a.local"), BackendState::Stopped);
    assert_eq!(manager.get_state("b.local"), BackendState::Stopped);
}

// ============================================================================
// Admin API Callback Tests
// ============================================================================

#[tokio::test]
async fn test_admin_api_ready_callback() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let backend_port = 30006;
    let admin_port = 30007;

    let mut configs = HashMap::new();
    configs.insert("callback.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    // Start admin server
    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());

    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    // Wait for admin server to start
    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);

    // Start backend - it will call back to admin API
    manager.start_backend("callback.local").await.unwrap();

    // Wait for ready via callback or health check
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Should be ready
    assert_eq!(manager.get_state("callback.local"), BackendState::Ready);

    // Cleanup
    manager.stop_backend("callback.local").await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
}

#[tokio::test]
async fn test_admin_api_manual_ready_call() {
    let admin_port = 30008;
    let backend_port = 30009;

    let mut configs = HashMap::new();
    // Use sleep instead of mock server so we control when it becomes ready
    configs.insert(
        "manual.local".to_string(),
{
            let mut cfg = BackendConfig::local("sleep", backend_port);
            cfg.args = vec!["60".to_string()];
            cfg.health_path = Some("/health".to_string());
            cfg.startup_timeout_secs = Some(60); // Long timeout
            cfg.health_check_interval_ms = Some(1000); // Slow polling
            cfg.shutdown_grace_period_secs = Some(1);
            cfg.drain_timeout_secs = Some(1);
            cfg
        },
    );

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    // Start admin server
    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());

    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    // Wait for admin server
    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);

    // Start backend
    manager.start_backend("manual.local").await.unwrap();
    assert_eq!(manager.get_state("manual.local"), BackendState::Starting);

    // Manually call the ready endpoint
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", admin_port))
        .await
        .unwrap();
    let request = format!(
        "POST /ready/manual.local HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer test-token\r\nContent-Length: 0\r\n\r\n",
        admin_port
    );
    stream.write_all(request.as_bytes()).await.unwrap();

    let mut response = vec![0u8; 1024];
    let n = stream.read(&mut response).await.unwrap();
    let response_str = String::from_utf8_lossy(&response[..n]);
    assert!(response_str.contains("200 OK"));

    // Should now be ready
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(manager.get_state("manual.local"), BackendState::Ready);

    // Cleanup
    manager.stop_backend("manual.local").await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
}

// ============================================================================
// Full Proxy Tests
// ============================================================================

#[tokio::test]
async fn test_full_proxy_flow() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30010;
    let admin_port = 30011;
    let backend_port = 30012;

    let mut configs = HashMap::new();
    configs.insert("proxy.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    // Start admin server
    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    // Start proxy server
    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    // Wait for servers to start
    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Backend should be stopped initially
    assert_eq!(manager.get_state("proxy.local"), BackendState::Stopped);

    // Make request through proxy - this should start the backend
    let response = http_get_with_host(proxy_port, "/echo", "proxy.local")
        .await
        .unwrap();

    // Should get response from backend
    assert!(response.contains("200 OK"), "Response: {}", response);
    assert!(
        response.contains("echo response"),
        "Response: {}",
        response
    );

    // Backend should now be ready
    assert_eq!(manager.get_state("proxy.local"), BackendState::Ready);

    // Make another request - should work immediately
    let response2 = http_get_with_host(proxy_port, "/health", "proxy.local")
        .await
        .unwrap();
    assert!(response2.contains("200 OK"));

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

#[tokio::test]
async fn test_proxy_unknown_host_returns_404() {
    let proxy_port = 30013;

    let configs = HashMap::new(); // No backends configured

    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start proxy server
    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    // Wait for proxy to start
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Request to unknown host should return 404
    let response = http_get_with_host(proxy_port, "/", "unknown.host").await;
    assert!(response.is_ok());
    let response = response.unwrap();
    assert!(
        response.contains("404") || response.contains("Not Found"),
        "Response: {}",
        response
    );

    proxy_handle.abort();
}

// ============================================================================
// Idle Timeout Tests
// ============================================================================

#[tokio::test]
async fn test_idle_timeout_cleanup() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let port = 30014;

    let mut configs = HashMap::new();
    configs.insert(
        "idle.local".to_string(),
        {
            let mut cfg = BackendConfig::local(&mock_server_path().to_string_lossy(), port);
            cfg.health_path = Some("/health".to_string());
            cfg.idle_timeout_secs = Some(1); // 1 second idle timeout
            cfg.startup_timeout_secs = Some(10);
            cfg.health_check_interval_ms = Some(50);
            cfg.shutdown_grace_period_secs = Some(2);
            cfg.drain_timeout_secs = Some(5);
            cfg.ready_health_check_interval_ms = Some(60000); // Long to avoid interference
            cfg
        },
    );

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start backend
    manager.start_backend("idle.local").await.unwrap();
    assert!(wait_for_port(port, Duration::from_secs(5)).await);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(manager.get_state("idle.local"), BackendState::Ready);

    // Wait for idle timeout
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Run cleanup
    manager.cleanup_idle_backends().await;

    // Should be stopped
    assert_eq!(manager.get_state("idle.local"), BackendState::Stopped);
}

#[tokio::test]
async fn test_activity_resets_idle_timeout() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let port = 30015;

    let mut configs = HashMap::new();
    configs.insert(
        "active.local".to_string(),
        {
            let mut cfg = BackendConfig::local(&mock_server_path().to_string_lossy(), port);
            cfg.health_path = Some("/health".to_string());
            cfg.idle_timeout_secs = Some(2); // 2 second idle timeout
            cfg.startup_timeout_secs = Some(10);
            cfg.health_check_interval_ms = Some(50);
            cfg.shutdown_grace_period_secs = Some(2);
            cfg.drain_timeout_secs = Some(5);
            cfg.ready_health_check_interval_ms = Some(60000); // Long to avoid interference
            cfg
        },
    );

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start backend
    manager.start_backend("active.local").await.unwrap();
    assert!(wait_for_port(port, Duration::from_secs(5)).await);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Touch periodically to keep it alive
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        manager.touch("active.local");
    }

    // Run cleanup - should NOT stop because we've been touching it
    manager.cleanup_idle_backends().await;
    assert_eq!(manager.get_state("active.local"), BackendState::Ready);

    // Now wait without touching
    tokio::time::sleep(Duration::from_secs(3)).await;
    manager.cleanup_idle_backends().await;

    // Now it should be stopped
    assert_eq!(manager.get_state("active.local"), BackendState::Stopped);
}

// ============================================================================
// Startup Timeout Tests
// ============================================================================

#[tokio::test]
#[cfg(unix)] // Uses Unix 'sleep' command
async fn test_startup_timeout_stops_backend() {
    // Backend that will never become healthy (no server listening)
    let port = 30016;

    let mut configs = HashMap::new();
    configs.insert(
        "timeout.local".to_string(),
        {
            let mut cfg = BackendConfig::local("sleep", port); // sleep doesn't listen on any port
            cfg.args = vec!["60".to_string()];
            cfg.health_path = Some("/health".to_string());
            cfg.startup_timeout_secs = Some(1); // 1 second timeout
            cfg.health_check_interval_ms = Some(100);
            cfg.shutdown_grace_period_secs = Some(1);
            cfg.drain_timeout_secs = Some(1);
            cfg
        },
    );

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start backend
    manager.start_backend("timeout.local").await.unwrap();
    assert_eq!(manager.get_state("timeout.local"), BackendState::Starting);

    // Wait for startup timeout to trigger
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Should be stopped due to timeout
    assert_eq!(manager.get_state("timeout.local"), BackendState::Stopped);
}

// ============================================================================
// Concurrent Request Tests
// ============================================================================

#[tokio::test]
async fn test_concurrent_requests_while_starting() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30017;
    let admin_port = 30018;
    let backend_port = 30019;

    let mut configs = HashMap::new();
    // Backend with startup delay
    configs.insert(
        "concurrent.local".to_string(),
        mock_backend_config_with_delay(backend_port, 300),
    );

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    // Start servers
    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Fire multiple concurrent requests
    let mut handles = vec![];
    for i in 0..5 {
        let port = proxy_port;
        handles.push(tokio::spawn(async move {
            let result = http_get_with_host(port, "/echo", "concurrent.local").await;
            let response = result.map_err(|e| e.to_string());
            (i, response)
        }));
    }

    // All requests should succeed
    for handle in handles {
        let (i, result) = handle.await.unwrap();
        assert!(
            result.is_ok(),
            "Request {} failed: {:?}",
            i,
            result.err()
        );
        let response = result.unwrap();
        assert!(
            response.contains("200 OK"),
            "Request {} got: {}",
            i,
            response
        );
    }

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

// ============================================================================
// Multiple Backend Tests
// ============================================================================

#[tokio::test]
async fn test_multiple_backends_through_proxy() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30020;
    let admin_port = 30021;
    let backend_a_port = 30022;
    let backend_b_port = 30023;

    let mut configs = HashMap::new();
    configs.insert("backend-a.local".to_string(), mock_backend_config(backend_a_port));
    configs.insert("backend-b.local".to_string(), mock_backend_config(backend_b_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    // Start servers
    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Both backends should be stopped initially
    assert_eq!(manager.get_state("backend-a.local"), BackendState::Stopped);
    assert_eq!(manager.get_state("backend-b.local"), BackendState::Stopped);

    // Request to backend A
    let response_a = http_get_with_host(proxy_port, "/echo", "backend-a.local")
        .await
        .unwrap();
    assert!(response_a.contains("200 OK"));

    // Only backend A should be running
    assert_eq!(manager.get_state("backend-a.local"), BackendState::Ready);
    assert_eq!(manager.get_state("backend-b.local"), BackendState::Stopped);

    // Request to backend B
    let response_b = http_get_with_host(proxy_port, "/echo", "backend-b.local")
        .await
        .unwrap();
    assert!(response_b.contains("200 OK"));

    // Now both should be running
    assert_eq!(manager.get_state("backend-a.local"), BackendState::Ready);
    assert_eq!(manager.get_state("backend-b.local"), BackendState::Ready);

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

// ============================================================================
// Request/Response Tests
// ============================================================================

#[tokio::test]
async fn test_proxy_missing_host_header() {
    let proxy_port = 30024;

    let configs = HashMap::new();
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        "http://127.0.0.1:9999".to_string(),
    );

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Send request without Host header
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", proxy_port))
        .await
        .unwrap();
    let request = "GET / HTTP/1.1\r\nConnection: close\r\n\r\n";
    stream.write_all(request.as_bytes()).await.unwrap();

    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();

    // Should return 400 Bad Request
    assert!(
        response.contains("400") || response.contains("Bad Request"),
        "Response: {}",
        response
    );

    proxy_handle.abort();
}

#[tokio::test]
async fn test_post_request_with_body() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30025;
    let admin_port = 30026;
    let backend_port = 30027;

    let mut configs = HashMap::new();
    configs.insert("post.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Send POST request with body
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", proxy_port))
        .await
        .unwrap();
    let body = r#"{"test": "data"}"#;
    let request = format!(
        "POST /echo HTTP/1.1\r\nHost: post.local\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(request.as_bytes()).await.unwrap();

    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();

    // Should get successful response
    assert!(response.contains("200 OK"), "Response: {}", response);

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

#[tokio::test]
async fn test_response_headers_preserved() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30128;
    let admin_port = 30129;
    let backend_port = 30130;

    let mut configs = HashMap::new();
    configs.insert("headers.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Request and check for custom header from mock server
    let response = http_get_with_host(proxy_port, "/echo", "headers.local")
        .await
        .unwrap();

    // Mock server adds X-Mock-Server header (case-insensitive check)
    let response_lower = response.to_lowercase();
    assert!(
        response_lower.contains("x-mock-server: true"),
        "Response missing custom header: {}",
        response
    );

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

// ============================================================================
// Admin API Tests
// ============================================================================

#[tokio::test]
async fn test_admin_api_health_endpoint() {
    let admin_port = 30031;

    let configs = HashMap::new();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx, "test-token".to_string());

    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);

    // Call admin health endpoint
    let response = http_get(admin_port, "/health").await.unwrap();
    assert!(response.contains("200 OK"), "Response: {}", response);
    assert!(response.contains("ok"), "Response: {}", response);

    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
}

#[tokio::test]
async fn test_admin_api_ready_unknown_backend() {
    let admin_port = 30032;

    let configs = HashMap::new();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx, "test-token".to_string());

    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);

    // Call ready for unknown backend
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", admin_port))
        .await
        .unwrap();
    let request = format!(
        "POST /ready/unknown.backend HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer test-token\r\nContent-Length: 0\r\n\r\n",
        admin_port
    );
    stream.write_all(request.as_bytes()).await.unwrap();

    let mut response = vec![0u8; 1024];
    let n = stream.read(&mut response).await.unwrap();
    let response_str = String::from_utf8_lossy(&response[..n]);

    // Should return 404
    assert!(
        response_str.contains("404") || response_str.contains("not starting"),
        "Response: {}",
        response_str
    );

    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[tokio::test]
async fn test_backend_returns_error_status() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30033;
    let admin_port = 30034;
    let backend_port = 30035;

    let mut configs = HashMap::new();
    configs.insert("error.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Request /error endpoint which returns 500
    let response = http_get_with_host(proxy_port, "/error", "error.local")
        .await
        .unwrap();

    // Should pass through the 500 error
    assert!(
        response.contains("500") || response.contains("Internal Server Error"),
        "Response: {}",
        response
    );

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

// ============================================================================
// Graceful Shutdown Tests
// ============================================================================

#[tokio::test]
async fn test_in_flight_request_tracking() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let port = 30040;
    let mut configs = HashMap::new();
    configs.insert("inflight.local".to_string(), mock_backend_config(port));

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start backend
    manager.start_backend("inflight.local").await.unwrap();
    assert!(wait_for_port(port, Duration::from_secs(5)).await);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(manager.get_state("inflight.local"), BackendState::Ready);

    // Initially no in-flight requests
    assert_eq!(manager.get_in_flight("inflight.local"), 0);

    // Increment in-flight count (simulating requests)
    manager.increment_in_flight("inflight.local");
    manager.increment_in_flight("inflight.local");
    assert_eq!(manager.get_in_flight("inflight.local"), 2);

    // Decrement back
    manager.decrement_in_flight("inflight.local");
    assert_eq!(manager.get_in_flight("inflight.local"), 1);

    manager.decrement_in_flight("inflight.local");
    assert_eq!(manager.get_in_flight("inflight.local"), 0);

    // Cleanup
    manager.stop_backend("inflight.local").await;
}

#[tokio::test]
async fn test_drain_mode_rejects_new_requests() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30041;
    let admin_port = 30042;
    let backend_port = 30043;

    let mut configs = HashMap::new();
    configs.insert("drain.local".to_string(), {
        let mut cfg = BackendConfig::local(&mock_server_path().to_string_lossy(), backend_port);
        cfg.health_path = Some("/health".to_string());
        cfg.idle_timeout_secs = Some(60);
        cfg.startup_timeout_secs = Some(10);
        cfg.health_check_interval_ms = Some(50);
        cfg.shutdown_grace_period_secs = Some(5);
        cfg.drain_timeout_secs = Some(10); // Long drain timeout
        cfg.ready_health_check_interval_ms = Some(60000);
        cfg
    });

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    // Start servers
    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Make first request to start backend
    let response = http_get_with_host(proxy_port, "/health", "drain.local")
        .await
        .unwrap();
    assert!(response.contains("200 OK"));
    assert_eq!(manager.get_state("drain.local"), BackendState::Ready);

    // Simulate in-flight request
    manager.increment_in_flight("drain.local");
    assert_eq!(manager.get_in_flight("drain.local"), 1);

    // Start stopping the backend (this will put it in Stopping state)
    let manager_clone = Arc::clone(&manager);
    let stop_handle = tokio::spawn(async move {
        manager_clone.stop_backend("drain.local").await;
    });

    // Give a moment for the state to change to Stopping
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Backend should be in Stopping state (draining)
    assert_eq!(manager.get_state("drain.local"), BackendState::Stopping);

    // New request during drain should be rejected with 503
    let response = http_get_with_host(proxy_port, "/health", "drain.local")
        .await
        .unwrap();
    assert!(
        response.contains("503") || response.contains("Service Unavailable") || response.contains("shutting down"),
        "Expected 503, got: {}",
        response
    );

    // Complete the in-flight request
    manager.decrement_in_flight("drain.local");

    // Wait for stop to complete
    let _ = stop_handle.await;
    assert_eq!(manager.get_state("drain.local"), BackendState::Stopped);

    // Cleanup
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

#[tokio::test]
async fn test_graceful_shutdown_waits_for_drain() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let port = 30044;
    let mut configs = HashMap::new();
    configs.insert("graceful.local".to_string(), {
        let mut cfg = BackendConfig::local(&mock_server_path().to_string_lossy(), port);
        cfg.health_path = Some("/health".to_string());
        cfg.idle_timeout_secs = Some(60);
        cfg.startup_timeout_secs = Some(10);
        cfg.health_check_interval_ms = Some(50);
        cfg.shutdown_grace_period_secs = Some(5);
        cfg.drain_timeout_secs = Some(5); // 5 second drain timeout
        cfg.ready_health_check_interval_ms = Some(60000);
        cfg
    });

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start backend
    manager.start_backend("graceful.local").await.unwrap();
    assert!(wait_for_port(port, Duration::from_secs(5)).await);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(manager.get_state("graceful.local"), BackendState::Ready);

    // Simulate in-flight requests
    manager.increment_in_flight("graceful.local");
    manager.increment_in_flight("graceful.local");
    assert_eq!(manager.get_in_flight("graceful.local"), 2);

    // Start shutdown in background
    let manager_clone = Arc::clone(&manager);
    let shutdown_start = std::time::Instant::now();
    let stop_handle = tokio::spawn(async move {
        manager_clone.stop_backend("graceful.local").await;
        std::time::Instant::now()
    });

    // Wait a bit then complete in-flight requests
    tokio::time::sleep(Duration::from_millis(200)).await;
    manager.decrement_in_flight("graceful.local");
    manager.decrement_in_flight("graceful.local");

    // Wait for stop to complete
    let stop_end = stop_handle.await.unwrap();
    let shutdown_duration = stop_end.duration_since(shutdown_start);

    // Shutdown should have waited for in-flight requests
    // (at least 200ms for our sleep, but less than drain timeout)
    assert!(
        shutdown_duration >= Duration::from_millis(200),
        "Shutdown should have waited for drain: {:?}",
        shutdown_duration
    );
    assert!(
        shutdown_duration < Duration::from_secs(5),
        "Shutdown took too long: {:?}",
        shutdown_duration
    );

    assert_eq!(manager.get_state("graceful.local"), BackendState::Stopped);
}

#[tokio::test]
async fn test_shutdown_grace_period_config() {
    let defaults = BackendDefaults::default();

    // Default values
    assert_eq!(defaults.shutdown_grace_period_secs, 10);
    assert_eq!(defaults.drain_timeout_secs, 30);

    // Custom config
    let mut backend = BackendConfig::local("test", 3000);
    backend.shutdown_grace_period_secs = Some(3);
    backend.drain_timeout_secs = Some(15);

    assert_eq!(backend.shutdown_grace_period(&defaults), Duration::from_secs(3));
    assert_eq!(backend.drain_timeout(&defaults), Duration::from_secs(15));

    // Fallback to defaults
    let backend_default = BackendConfig::local("test", 3000);

    assert_eq!(backend_default.shutdown_grace_period(&defaults), Duration::from_secs(10));
    assert_eq!(backend_default.drain_timeout(&defaults), Duration::from_secs(30));
}

// ============================================================================
// Connection Pool Tests
// ============================================================================

#[test]
fn test_pool_config_parsing() {
    let toml = r#"
[server]
port = 8080
bind = "127.0.0.1"
admin_port = 9000
pool_max_idle_per_host = 20
pool_idle_timeout_secs = 120

[defaults]
idle_timeout_secs = 300

[backends."test.example.com"]
command = "node"
args = ["server.js"]
port = 3000
"#;

    let config: Config = toml::from_str(toml).unwrap();

    assert_eq!(config.server.pool_max_idle_per_host, 20);
    assert_eq!(config.server.pool_idle_timeout_secs, 120);
}

#[test]
fn test_pool_config_defaults() {
    let toml = r#"
[server]
port = 8080
bind = "127.0.0.1"
admin_port = 9000

[backends."test.example.com"]
command = "node"
args = ["server.js"]
port = 3000
"#;

    let config: Config = toml::from_str(toml).unwrap();

    // Should use defaults
    assert_eq!(config.server.pool_max_idle_per_host, 10);
    assert_eq!(config.server.pool_idle_timeout_secs, 90);
}

#[test]
fn test_pool_config_construction() {
    let pool_config = PoolConfig {
        max_idle_per_host: 15,
        idle_timeout: Duration::from_secs(60),
    };

    assert_eq!(pool_config.max_idle_per_host, 15);
    assert_eq!(pool_config.idle_timeout, Duration::from_secs(60));
}

#[tokio::test]
async fn test_proxy_with_custom_pool_config() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30050;
    let admin_port = 30051;
    let backend_port = 30052;

    let mut configs = HashMap::new();
    configs.insert("pooltest.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    // Start admin server
    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    // Start proxy server with custom pool config
    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let pool_config = PoolConfig {
        max_idle_per_host: 5,
        idle_timeout: Duration::from_secs(30),
    };
    let proxy_server = ProxyServer::with_pool_config(
        proxy_addr,
        Arc::clone(&manager),
        manager.shared_defaults(),
        shutdown_rx,
        pool_config,
    );

    // Verify pool config was applied
    let pool = proxy_server.pool();
    assert_eq!(pool.config().max_idle_per_host, 5);
    assert_eq!(pool.config().idle_timeout, Duration::from_secs(30));

    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    // Wait for servers to start
    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Make request through proxy
    let response = http_get_with_host(proxy_port, "/echo", "pooltest.local")
        .await
        .unwrap();

    // Should get response from backend
    assert!(response.contains("200 OK"), "Response: {}", response);

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

#[tokio::test]
async fn test_connection_pool_stats() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30053;
    let admin_port = 30054;
    let backend_port = 30055;

    let mut configs = HashMap::new();
    configs.insert("stats.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    // Start admin server
    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    // Start proxy server
    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let pool_config = PoolConfig::default();
    let proxy_server = ProxyServer::with_pool_config(
        proxy_addr,
        Arc::clone(&manager),
        manager.shared_defaults(),
        shutdown_rx,
        pool_config,
    );

    // Get pool stats reference before starting server
    let pool = proxy_server.pool().clone();
    let stats = pool.stats();

    // Initially no requests
    assert_eq!(stats.get_total_requests(), 0);

    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    // Wait for servers to start
    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Make multiple requests
    for _ in 0..5 {
        let response = http_get_with_host(proxy_port, "/echo", "stats.local")
            .await
            .unwrap();
        assert!(response.contains("200 OK"));
    }

    // Stats should have recorded requests
    assert_eq!(stats.get_total_requests(), 5);

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

#[tokio::test]
async fn test_multiple_backends_use_pool() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30056;
    let admin_port = 30057;
    let backend_a_port = 30058;
    let backend_b_port = 30059;

    let mut configs = HashMap::new();
    configs.insert("pool-a.local".to_string(), mock_backend_config(backend_a_port));
    configs.insert("pool-b.local".to_string(), mock_backend_config(backend_b_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    // Start admin server
    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    // Start proxy with pool
    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let pool_config = PoolConfig {
        max_idle_per_host: 5,
        idle_timeout: Duration::from_secs(60),
    };
    let proxy_server = ProxyServer::with_pool_config(
        proxy_addr,
        Arc::clone(&manager),
        manager.shared_defaults(),
        shutdown_rx,
        pool_config,
    );

    let pool = proxy_server.pool().clone();
    let stats = pool.stats();

    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    // Wait for servers to start
    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Make requests to both backends
    let response_a = http_get_with_host(proxy_port, "/echo", "pool-a.local")
        .await
        .unwrap();
    assert!(response_a.contains("200 OK"));

    let response_b = http_get_with_host(proxy_port, "/echo", "pool-b.local")
        .await
        .unwrap();
    assert!(response_b.contains("200 OK"));

    // Make more requests to verify pool works for both
    for _ in 0..3 {
        let _ = http_get_with_host(proxy_port, "/health", "pool-a.local").await;
        let _ = http_get_with_host(proxy_port, "/health", "pool-b.local").await;
    }

    // Stats should reflect all requests
    assert_eq!(stats.get_total_requests(), 8); // 2 initial + 6 more

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

#[tokio::test]
async fn test_concurrent_requests_through_pool() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30060;
    let admin_port = 30061;
    let backend_port = 30062;

    let mut configs = HashMap::new();
    configs.insert("concurrent-pool.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    // Start admin server
    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    // Start proxy with pool configured for concurrent connections
    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let pool_config = PoolConfig {
        max_idle_per_host: 10,
        idle_timeout: Duration::from_secs(30),
    };
    let proxy_server = ProxyServer::with_pool_config(
        proxy_addr,
        Arc::clone(&manager),
        manager.shared_defaults(),
        shutdown_rx,
        pool_config,
    );

    let pool = proxy_server.pool().clone();
    let stats = pool.stats();

    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    // Wait for servers to start
    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Fire concurrent requests
    let mut handles = vec![];
    for i in 0..10 {
        let port = proxy_port;
        handles.push(tokio::spawn(async move {
            let result = http_get_with_host(port, "/echo", "concurrent-pool.local").await;
            (i, result.map_err(|e| e.to_string()))
        }));
    }

    // All requests should succeed
    for handle in handles {
        let (i, result) = handle.await.unwrap();
        assert!(result.is_ok(), "Request {} failed: {:?}", i, result.err());
        let response = result.unwrap();
        assert!(response.contains("200 OK"), "Request {} got: {}", i, response);
    }

    // All requests should be tracked
    assert_eq!(stats.get_total_requests(), 10);

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

// ============================================================================
// JSON Error Response Tests
// ============================================================================

#[tokio::test]
async fn test_json_error_response_unknown_host() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30070;
    let admin_port = 30071;
    let backend_port = 30072;

    let mut configs = HashMap::new();
    configs.insert("known.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Request to unknown host
    let response = http_get_with_host(proxy_port, "/test", "unknown.local")
        .await
        .unwrap();

    // Should get JSON error with 404
    assert!(response.contains("404"), "Response: {}", response);
    assert!(response.contains("application/json"), "Response: {}", response);
    // Check for X-Proxy-Error header (case-insensitive)
    assert!(
        response.contains("X-Proxy-Error") || response.contains("x-proxy-error"),
        "Response should contain X-Proxy-Error header: {}",
        response
    );
    assert!(response.contains("UNKNOWN_HOST"), "Response: {}", response);

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

#[tokio::test]
async fn test_json_error_response_missing_host() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30073;
    let admin_port = 30074;
    let backend_port = 30075;

    let mut configs = HashMap::new();
    configs.insert("known.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Request without Host header using Connection: close to get proper EOF
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", proxy_port))
        .await
        .unwrap();

    // Send HTTP request without Host header
    let request = "GET /test HTTP/1.0\r\n\r\n";
    stream.write_all(request.as_bytes()).await.unwrap();

    // Read response with timeout
    let mut response = vec![0u8; 4096];
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        stream.read(&mut response)
    ).await;

    let n = result.expect("Timeout reading response").expect("Failed to read response");
    let response = String::from_utf8_lossy(&response[..n]).to_string();

    // Should get JSON error with 400
    assert!(response.contains("400"), "Response: {}", response);
    assert!(response.contains("application/json"), "Response: {}", response);
    assert!(response.contains("MISSING_HOST_HEADER"), "Response: {}", response);

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

// ============================================================================
// Request Timeout Tests
// ============================================================================

#[test]
fn test_request_timeout_config() {
    let defaults = BackendDefaults::default();
    assert_eq!(defaults.request_timeout_secs, 30);

    let mut backend = BackendConfig::local("test", 3000);
    backend.request_timeout_secs = Some(10);

    assert_eq!(backend.request_timeout(&defaults), Duration::from_secs(10));
}

#[test]
fn test_request_timeout_uses_default() {
    let defaults = BackendDefaults::default();

    let backend = BackendConfig::local("test", 3000);

    assert_eq!(backend.request_timeout(&defaults), Duration::from_secs(30));
}

// ============================================================================
// Health Check and Unhealthy State Tests
// ============================================================================

#[test]
fn test_unhealthy_threshold_config() {
    let defaults = BackendDefaults::default();
    assert_eq!(defaults.unhealthy_threshold, 3);
    assert_eq!(defaults.ready_health_check_interval_ms, 5000);

    let mut backend = BackendConfig::local("test", 3000);
    backend.ready_health_check_interval_ms = Some(1000);
    backend.unhealthy_threshold = Some(5);

    assert_eq!(backend.unhealthy_threshold(&defaults), 5);
    assert_eq!(
        backend.ready_health_check_interval(&defaults),
        Duration::from_millis(1000)
    );
}

#[test]
fn test_backend_state_includes_unhealthy() {
    // Test that Unhealthy state exists and is distinct
    assert_ne!(BackendState::Unhealthy, BackendState::Ready);
    assert_ne!(BackendState::Unhealthy, BackendState::Stopping);
    assert_ne!(BackendState::Unhealthy, BackendState::Stopped);
    assert_ne!(BackendState::Unhealthy, BackendState::Starting);

    // Test Debug
    assert_eq!(format!("{:?}", BackendState::Unhealthy), "Unhealthy");
}

#[tokio::test]
async fn test_unhealthy_backend_rejected() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30076;
    let admin_port = 30077;
    let backend_port = 30078;

    let mut configs = HashMap::new();
    configs.insert("unhealthy.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // First, make a normal request to start the backend
    let response = http_get_with_host(proxy_port, "/echo", "unhealthy.local")
        .await
        .unwrap();
    assert!(response.contains("200 OK"));

    // Mark the backend as unhealthy
    manager.mark_unhealthy("unhealthy.local");
    assert_eq!(manager.get_state("unhealthy.local"), BackendState::Unhealthy);

    // Request to unhealthy backend should fail with 503
    let response = http_get_with_host(proxy_port, "/echo", "unhealthy.local")
        .await
        .unwrap();
    assert!(response.contains("503"), "Response: {}", response);
    assert!(response.contains("BACKEND_UNHEALTHY"), "Response: {}", response);

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}


// ============================================================================
// Request Header Tests
// ============================================================================

/// Helper to send HTTP request with timeout for reading
async fn http_get_with_timeout(
    port: u16,
    path: &str,
    host: &str,
    extra_headers: &[(&str, &str)],
) -> Result<String, Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port)).await?;

    let mut header_str = String::new();
    for (name, value) in extra_headers {
        header_str.push_str(&format!("{}: {}\r\n", name, value));
    }

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\n{}Connection: close\r\n\r\n",
        path, host, header_str
    );
    stream.write_all(request.as_bytes()).await?;

    // Read with timeout to avoid hanging
    let mut response = vec![0u8; 8192];
    let result = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut response)).await;

    match result {
        Ok(Ok(n)) => Ok(String::from_utf8_lossy(&response[..n]).to_string()),
        Ok(Err(e)) => Err(Box::new(e)),
        Err(_) => Err("Timeout reading response".into()),
    }
}

/// Test that X-Request-ID header is generated and forwarded
#[tokio::test]
async fn test_x_request_id_header_generated() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30110;
    let admin_port = 30111;
    let backend_port = 30112;

    let mut configs = HashMap::new();
    configs.insert("header-test.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Request to /headers endpoint - mock server returns headers as JSON
    let response = http_get_with_timeout(proxy_port, "/headers", "header-test.local", &[])
        .await
        .unwrap();

    assert!(response.contains("200 OK"), "Response: {}", response);
    // Backend should receive x-request-id header
    assert!(
        response.to_lowercase().contains("x-request-id"),
        "Response should contain x-request-id: {}",
        response
    );

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

/// Test that X-Request-ID is propagated when provided by client
#[tokio::test]
async fn test_x_request_id_header_propagated() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30113;
    let admin_port = 30114;
    let backend_port = 30115;

    let mut configs = HashMap::new();
    configs.insert("header-test.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Send request with custom X-Request-ID
    let custom_id = "custom-request-id-12345";
    let response = http_get_with_timeout(
        proxy_port,
        "/headers",
        "header-test.local",
        &[("X-Request-ID", custom_id)],
    )
    .await
    .unwrap();

    assert!(response.contains("200 OK"), "Response: {}", response);
    // Backend should receive our custom x-request-id
    assert!(
        response.contains(custom_id),
        "Response should contain custom request id '{}': {}",
        custom_id,
        response
    );

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

/// Test that X-Forwarded-For header is added with client IP
#[tokio::test]
async fn test_x_forwarded_for_header() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30116;
    let admin_port = 30117;
    let backend_port = 30118;

    let mut configs = HashMap::new();
    configs.insert("forward-test.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Request to /headers endpoint
    let response = http_get_with_timeout(proxy_port, "/headers", "forward-test.local", &[])
        .await
        .unwrap();

    assert!(response.contains("200 OK"), "Response: {}", response);
    // Backend should receive x-forwarded-for with client IP (127.0.0.1)
    assert!(
        response.to_lowercase().contains("x-forwarded-for"),
        "Response should contain x-forwarded-for: {}",
        response
    );
    assert!(
        response.contains("127.0.0.1"),
        "x-forwarded-for should contain client IP 127.0.0.1: {}",
        response
    );

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

/// Test that X-Forwarded-Host header contains original Host
#[tokio::test]
async fn test_x_forwarded_host_header() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30119;
    let admin_port = 30120;
    let backend_port = 30121;

    let mut configs = HashMap::new();
    configs.insert("myapp.example.com".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Request to /headers with specific host
    let response = http_get_with_timeout(proxy_port, "/headers", "myapp.example.com", &[])
        .await
        .unwrap();

    assert!(response.contains("200 OK"), "Response: {}", response);
    // Backend should receive x-forwarded-host with original host
    assert!(
        response.to_lowercase().contains("x-forwarded-host"),
        "Response should contain x-forwarded-host: {}",
        response
    );
    assert!(
        response.contains("myapp.example.com"),
        "x-forwarded-host should contain original host: {}",
        response
    );

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

/// Test that X-Forwarded-Proto header is set to http
#[tokio::test]
async fn test_x_forwarded_proto_header() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30122;
    let admin_port = 30123;
    let backend_port = 30124;

    let mut configs = HashMap::new();
    configs.insert("proto-test.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Request to /headers endpoint
    let response = http_get_with_timeout(proxy_port, "/headers", "proto-test.local", &[])
        .await
        .unwrap();

    assert!(response.contains("200 OK"), "Response: {}", response);
    // Backend should receive x-forwarded-proto set to http
    let response_lower = response.to_lowercase();
    assert!(
        response_lower.contains("x-forwarded-proto"),
        "Response should contain x-forwarded-proto: {}",
        response
    );
    // The JSON format will be "x-forwarded-proto":"http"
    assert!(
        response_lower.contains("x-forwarded-proto\":\"http"),
        "x-forwarded-proto should be 'http': {}",
        response
    );

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

/// Test that X-Forwarded-For is overwritten (not appended) to prevent spoofing
#[tokio::test]
async fn test_x_forwarded_for_appends() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30125;
    let admin_port = 30126;
    let backend_port = 30127;

    let mut configs = HashMap::new();
    configs.insert("append-test.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Send request with spoofed X-Forwarded-For
    let response = http_get_with_timeout(
        proxy_port,
        "/headers",
        "append-test.local",
        &[("X-Forwarded-For", "10.0.0.1")],
    )
    .await
    .unwrap();

    assert!(response.contains("200 OK"), "Response: {}", response);
    // Security: The client-provided IP should be overwritten, not appended
    // Only the actual client IP (127.0.0.1) should be present
    assert!(
        response.contains("127.0.0.1"),
        "Response should contain actual client IP: {}",
        response
    );
    // The spoofed IP should NOT be in the header (it was overwritten)
    assert!(
        !response.contains("x-forwarded-for: 10.0.0.1"),
        "Spoofed X-Forwarded-For should be overwritten: {}",
        response
    );

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

// ============================================================================
// Admin API Tests
// ============================================================================

/// Test /version endpoint returns version info
#[tokio::test]
async fn test_admin_version_endpoint() {
    let admin_port = 30105;

    let configs = HashMap::new();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults,
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx, "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);

    // Request /version
    let response = http_get(admin_port, "/version").await.unwrap();
    assert!(response.contains("200 OK"), "Response: {}", response);
    assert!(
        response.contains("application/json"),
        "Should return JSON: {}",
        response
    );
    assert!(
        response.contains("spawngate"),
        "Should contain package name: {}",
        response
    );
    assert!(
        response.contains("version"),
        "Should contain version field: {}",
        response
    );

    // Cleanup
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
}

// ============================================================================
// WebSocket Proxy Tests
// ============================================================================

/// WebSocket magic GUID for computing accept key
const WS_MAGIC_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Compute Sec-WebSocket-Accept from client key
fn compute_ws_accept(key: &str) -> String {
    use sha1::{Sha1, Digest};
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WS_MAGIC_GUID.as_bytes());
    let hash = hasher.finalize();
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, hash)
}

/// Perform a WebSocket handshake through the proxy
async fn websocket_handshake(
    port: u16,
    host: &str,
    path: &str,
) -> Result<TcpStream, Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port)).await?;

    // Generate a random key for the WebSocket handshake
    let key = "dGhlIHNhbXBsZSBub25jZQ==";

    let request = format!(
        "GET {} HTTP/1.1\r\n\
         Host: {}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {}\r\n\
         Sec-WebSocket-Version: 13\r\n\
         \r\n",
        path, host, key
    );

    stream.write_all(request.as_bytes()).await?;

    // Read response
    let mut response = vec![0u8; 4096];
    let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut response))
        .await??;

    let response_str = String::from_utf8_lossy(&response[..n]);

    if !response_str.contains("101 Switching Protocols") {
        return Err(format!("WebSocket handshake failed: {}", response_str).into());
    }

    // Verify the accept key
    let expected_accept = compute_ws_accept(key);
    if !response_str.contains(&expected_accept) {
        return Err(format!(
            "Invalid Sec-WebSocket-Accept. Expected '{}', got: {}",
            expected_accept, response_str
        ).into());
    }

    Ok(stream)
}

/// Send a WebSocket text frame
async fn send_ws_text(stream: &mut TcpStream, text: &str) -> Result<(), Box<dyn std::error::Error>> {
    let payload = text.as_bytes();
    let mut frame = Vec::new();

    // FIN bit + text opcode
    frame.push(0x81);

    // Length with mask bit set (client must mask)
    if payload.len() < 126 {
        frame.push(0x80 | payload.len() as u8);
    } else if payload.len() < 65536 {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }

    // Mask key (can be any 4 bytes)
    let mask = [0x12, 0x34, 0x56, 0x78u8];
    frame.extend_from_slice(&mask);

    // Masked payload
    for (i, byte) in payload.iter().enumerate() {
        frame.push(byte ^ mask[i % 4]);
    }

    stream.write_all(&frame).await?;
    Ok(())
}

/// Receive a WebSocket text frame
async fn recv_ws_text(stream: &mut TcpStream) -> Result<String, Box<dyn std::error::Error>> {
    let mut header = [0u8; 2];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut header)).await??;

    let opcode = header[0] & 0x0F;
    if opcode != 0x1 {
        return Err(format!("Expected text frame (opcode 1), got {}", opcode).into());
    }

    let mut payload_len = (header[1] & 0x7F) as u64;

    if payload_len == 126 {
        let mut ext = [0u8; 2];
        stream.read_exact(&mut ext).await?;
        payload_len = u16::from_be_bytes(ext) as u64;
    } else if payload_len == 127 {
        let mut ext = [0u8; 8];
        stream.read_exact(&mut ext).await?;
        payload_len = u64::from_be_bytes(ext);
    }

    let mut payload = vec![0u8; payload_len as usize];
    if !payload.is_empty() {
        stream.read_exact(&mut payload).await?;
    }

    Ok(String::from_utf8(payload)?)
}

/// Send a WebSocket close frame
async fn send_ws_close(stream: &mut TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    // Close frame with mask
    let frame = [0x88, 0x80, 0x00, 0x00, 0x00, 0x00]; // FIN + close opcode, masked, no payload
    stream.write_all(&frame).await?;
    Ok(())
}

/// Test WebSocket upgrade through proxy
#[tokio::test]
async fn test_websocket_upgrade_through_proxy() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30200;
    let admin_port = 30201;
    let backend_port = 30202;

    let mut configs = HashMap::new();
    configs.insert("ws.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Perform WebSocket handshake through proxy
    let result = websocket_handshake(proxy_port, "ws.local", "/ws").await;
    assert!(result.is_ok(), "WebSocket handshake failed: {:?}", result.err());

    let mut ws_stream = result.unwrap();

    // Send a message
    send_ws_text(&mut ws_stream, "Hello WebSocket!").await.unwrap();

    // Receive echo
    let response = recv_ws_text(&mut ws_stream).await.unwrap();
    assert_eq!(response, "Hello WebSocket!");

    // Close connection
    send_ws_close(&mut ws_stream).await.unwrap();

    // Small delay to let connection close
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

/// Test multiple WebSocket messages
#[tokio::test]
async fn test_websocket_multiple_messages() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30203;
    let admin_port = 30204;
    let backend_port = 30205;

    let mut configs = HashMap::new();
    configs.insert("ws-multi.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Perform WebSocket handshake
    let mut ws_stream = websocket_handshake(proxy_port, "ws-multi.local", "/ws")
        .await
        .expect("WebSocket handshake failed");

    // Send multiple messages
    let messages = ["Message 1", "Message 2", "Hello World", "Final Message"];

    for msg in &messages {
        send_ws_text(&mut ws_stream, msg).await.unwrap();
        let response = recv_ws_text(&mut ws_stream).await.unwrap();
        assert_eq!(&response, *msg, "Echo mismatch for message: {}", msg);
    }

    // Close connection
    send_ws_close(&mut ws_stream).await.unwrap();

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

/// Test that WebSocket connections are tracked as in-flight
#[tokio::test]
async fn test_websocket_in_flight_tracking() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30206;
    let admin_port = 30207;
    let backend_port = 30208;

    let mut configs = HashMap::new();
    configs.insert("ws-track.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Initial in-flight should be 0
    assert_eq!(manager.get_in_flight("ws-track.local"), 0);

    // Perform WebSocket handshake (this will start the backend)
    let mut ws_stream = websocket_handshake(proxy_port, "ws-track.local", "/ws")
        .await
        .expect("WebSocket handshake failed");

    // Give a moment for the upgrade task to increment in-flight
    tokio::time::sleep(Duration::from_millis(100)).await;

    // In-flight should be 1 while WebSocket is connected
    assert_eq!(manager.get_in_flight("ws-track.local"), 1, "WebSocket should be tracked as in-flight");

    // Close connection
    send_ws_close(&mut ws_stream).await.unwrap();
    drop(ws_stream);

    // Give time for connection close to be processed
    tokio::time::sleep(Duration::from_millis(200)).await;

    // In-flight should be back to 0
    assert_eq!(manager.get_in_flight("ws-track.local"), 0, "In-flight should be 0 after WebSocket closes");

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

// ============================================================================
// HTTP/2 Tests
// ============================================================================

/// Send an HTTP/2 request with prior knowledge (h2c)
async fn http2_request(
    port: u16,
    host: &str,
    path: &str,
) -> Result<(u16, String), Box<dyn std::error::Error + Send + Sync>> {
    use h2::client;

    let stream = TcpStream::connect(format!("127.0.0.1:{}", port)).await?;

    let (h2, connection) = client::handshake(stream).await?;

    // Spawn connection driver
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("HTTP/2 connection error: {}", e);
        }
    });

    let mut h2 = h2.ready().await?;

    let request = http::Request::builder()
        .method(http::Method::GET)
        .uri(format!("http://{}{}", host, path))
        .header("host", host)
        .body(())?;

    let (response, _send_stream) = h2.send_request(request, true)?;
    let response = response.await?;

    let status = response.status().as_u16();

    // Read body from response
    let mut body_stream = response.into_body();
    let mut body = String::new();
    while let Some(chunk) = body_stream.data().await {
        let chunk = chunk?;
        body.push_str(&String::from_utf8_lossy(&chunk));
        body_stream.flow_control().release_capacity(chunk.len())?;
    }

    Ok((status, body))
}

/// Test HTTP/2 request through proxy with prior knowledge
#[tokio::test]
async fn test_http2_request_through_proxy() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30300;
    let admin_port = 30301;
    let backend_port = 30302;

    let mut configs = HashMap::new();
    configs.insert("h2.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Send HTTP/2 request
    let result = http2_request(proxy_port, "h2.local", "/echo").await;
    assert!(result.is_ok(), "HTTP/2 request failed: {:?}", result.err());

    let (status, body) = result.unwrap();
    assert_eq!(status, 200, "Expected 200, got {}", status);
    assert!(body.contains("echo"), "Unexpected body: {}", body);

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

/// Test HTTP/2 multiplexed requests (multiple streams on same connection)
#[tokio::test]
async fn test_http2_multiplexed_requests() {
    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 30303;
    let admin_port = 30304;
    let backend_port = 30305;

    let mut configs = HashMap::new();
    configs.insert("h2-multi.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Send multiple HTTP/2 requests on the same connection
    use h2::client;

    let stream = TcpStream::connect(format!("127.0.0.1:{}", proxy_port))
        .await
        .unwrap();

    let (h2, connection) = client::handshake(stream).await.unwrap();

    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("HTTP/2 connection error: {}", e);
        }
    });

    let mut h2 = h2.ready().await.unwrap();

    // Send 5 concurrent requests on same connection
    let mut handles = vec![];
    for i in 0..5 {
        let request = http::Request::builder()
            .method(http::Method::GET)
            .uri(format!("http://h2-multi.local/echo?n={}", i))
            .header("host", "h2-multi.local")
            .body(())
            .unwrap();

        let (response_future, _send_stream) = h2.send_request(request, true).unwrap();

        handles.push(tokio::spawn(async move {
            let response = response_future.await.unwrap();
            let status = response.status().as_u16();

            let mut body_stream = response.into_body();
            let mut body = String::new();
            while let Some(chunk) = body_stream.data().await {
                let chunk = chunk.unwrap();
                body.push_str(&String::from_utf8_lossy(&chunk));
                body_stream.flow_control().release_capacity(chunk.len()).unwrap();
            }

            (i, status, body)
        }));

        // Get next ready handle
        h2 = h2.ready().await.unwrap();
    }

    // All requests should succeed
    for handle in handles {
        let (i, status, body) = handle.await.unwrap();
        assert_eq!(status, 200, "Request {} got status {}", i, status);
        assert!(body.contains("Hello") || body.contains("echo") || body.contains("mock"), "Request {} got body: {}", i, body);
    }

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

/// Test HTTP/2 to admin API
#[tokio::test]
async fn test_http2_admin_api() {
    let admin_port = 30306;

    let configs = HashMap::new();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx, "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);

    // Send HTTP/2 request to admin health endpoint
    use h2::client;

    let stream = TcpStream::connect(format!("127.0.0.1:{}", admin_port))
        .await
        .unwrap();

    let (h2, connection) = client::handshake(stream).await.unwrap();

    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("HTTP/2 connection error: {}", e);
        }
    });

    let mut h2 = h2.ready().await.unwrap();

    let request = http::Request::builder()
        .method(http::Method::GET)
        .uri("http://localhost/health")
        .header("host", "localhost")
        .body(())
        .unwrap();

    let (response_future, _send_stream) = h2.send_request(request, true).unwrap();
    let response = response_future.await.unwrap();

    assert_eq!(response.status().as_u16(), 200);

    let mut body_stream = response.into_body();
    let mut body = String::new();
    while let Some(chunk) = body_stream.data().await {
        let chunk = chunk.unwrap();
        body.push_str(&String::from_utf8_lossy(&chunk));
        body_stream.flow_control().release_capacity(chunk.len()).unwrap();
    }

    assert_eq!(body, "ok");

    // Cleanup
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
}

/// Test the /backends endpoint returns JSON with all configured backends
#[tokio::test]
async fn test_backends_endpoint() {
    // Start admin server with test configuration
    let admin_port = 19370;

    let mut backends = HashMap::new();
    backends.insert("app1.test".to_string(), mock_backend_config(19371));
    backends.insert("app2.test".to_string(), mock_backend_config(19372));

    let process_manager = ProcessManager::new(
        backends,
        BackendDefaults::default(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let admin = AdminServer::new(
        SocketAddr::from(([127, 0, 0, 1], admin_port)),
        Arc::clone(&process_manager),
        shutdown_rx,
        "test-token".to_string(),
    );

    let admin_handle = tokio::spawn(async move { admin.run().await });

    // Wait for admin to be ready
    assert!(
        wait_for_port(admin_port, Duration::from_secs(5)).await,
        "Admin server failed to start"
    );

    // Request the backends endpoint (requires auth)
    let response = http_get_with_auth(admin_port, "/backends", "test-token").await.unwrap();

    // Parse response
    let body_start = response.find("\r\n\r\n").unwrap() + 4;
    let body = &response[body_start..];

    // Verify JSON response
    let json: serde_json::Value = serde_json::from_str(body).expect("Valid JSON response");

    assert_eq!(json["count"], 2);
    assert!(json["backends"].is_array());

    let backends = json["backends"].as_array().unwrap();
    assert_eq!(backends.len(), 2);

    // Find each backend and verify state is "stopped" (not started)
    for backend in backends {
        assert!(backend["hostname"].is_string());
        assert_eq!(backend["state"], "stopped");
        assert!(backend["port"].is_u64());
        assert_eq!(backend["in_flight"], 0);
    }

    // Cleanup
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
}

/// Test the /backends endpoint shows running backend state
#[tokio::test]
async fn test_backends_endpoint_with_running_backend() {
    // Start proxy and admin servers
    let proxy_port = 19375;
    let admin_port = 19376;
    let backend_port = 19377;

    let mut backends = HashMap::new();
    backends.insert("test.local".to_string(), mock_backend_config(backend_port));

    let process_manager = ProcessManager::new(
        backends,
        BackendDefaults::default(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let _defaults = BackendDefaults::default();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Start admin server
    let admin = AdminServer::new(
        SocketAddr::from(([127, 0, 0, 1], admin_port)),
        Arc::clone(&process_manager),
        shutdown_rx.clone(),
        "test-token".to_string(),
    );
    let admin_handle = tokio::spawn(async move { admin.run().await });

    // Start proxy
    let proxy = ProxyServer::new(
        SocketAddr::from(([127, 0, 0, 1], proxy_port)),
        Arc::clone(&process_manager),
        process_manager.shared_defaults(),
        shutdown_rx,
    );
    let proxy_handle = tokio::spawn(async move { proxy.run().await });

    // Wait for servers to be ready
    assert!(
        wait_for_port(proxy_port, Duration::from_secs(5)).await,
        "Proxy server failed to start"
    );
    assert!(
        wait_for_port(admin_port, Duration::from_secs(5)).await,
        "Admin server failed to start"
    );

    // Make a request to start the backend
    let _response = http_get_with_host(proxy_port, "/", "test.local").await.unwrap();

    // Small delay to ensure state is updated
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Check backends endpoint
    let response = http_get_with_auth(admin_port, "/backends", "test-token").await.unwrap();
    let body_start = response.find("\r\n\r\n").unwrap() + 4;
    let body = &response[body_start..];

    let json: serde_json::Value = serde_json::from_str(body).expect("Valid JSON response");

    let backends = json["backends"].as_array().unwrap();
    let backend = &backends[0];

    assert_eq!(backend["hostname"], "test.local");
    assert!(backend["state"] == "ready" || backend["state"] == "starting");
    assert_eq!(backend["port"], backend_port);

    // Cleanup
    let _ = shutdown_tx.send(true);
    process_manager.stop_all().await;
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

/// Test TLS proxy with HTTPS
#[tokio::test]
async fn test_tls_proxy() {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    use std::fs::File;
    use std::io::BufReader;
    use tokio_rustls::TlsAcceptor;

    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    // Load test certificates
    let cert_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/certs/cert.pem");
    let key_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/certs/key.pem");

    if !cert_path.exists() || !key_path.exists() {
        eprintln!("Skipping test: test certificates not found");
        return;
    }

    let certs: Vec<CertificateDer<'static>> = {
        let file = File::open(&cert_path).unwrap();
        let mut reader = BufReader::new(file);
        rustls_pemfile::certs(&mut reader)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };

    let key: PrivateKeyDer<'static> = {
        let file = File::open(&key_path).unwrap();
        let mut reader = BufReader::new(file);
        loop {
            match rustls_pemfile::read_one(&mut reader).unwrap() {
                Some(rustls_pemfile::Item::Pkcs8Key(key)) => break key.into(),
                Some(rustls_pemfile::Item::Pkcs1Key(key)) => break key.into(),
                Some(rustls_pemfile::Item::Sec1Key(key)) => break key.into(),
                None => panic!("No private key found"),
                _ => continue,
            }
        }
    };

    let tls_config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    let tls_acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let proxy_port = 30400;
    let admin_port = 30401;
    let backend_port = 30402;

    let mut configs = HashMap::new();
    configs.insert("tls-test.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx)
        .with_tls(tls_acceptor);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Create a TLS client
    let mut root_store = rustls::RootCertStore::empty();
    let cert_file = File::open(&cert_path).unwrap();
    let mut cert_reader = BufReader::new(cert_file);
    for cert in rustls_pemfile::certs(&mut cert_reader) {
        root_store.add(cert.unwrap()).unwrap();
    }

    let client_config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));

    let stream = TcpStream::connect(format!("127.0.0.1:{}", proxy_port))
        .await
        .unwrap();

    let domain = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let mut tls_stream = connector.connect(domain, stream).await.unwrap();

    // Send HTTPS request
    let request = "GET / HTTP/1.1\r\nHost: tls-test.local\r\nConnection: close\r\n\r\n";
    tls_stream.write_all(request.as_bytes()).await.unwrap();

    let mut response = String::new();
    tls_stream.read_to_string(&mut response).await.unwrap();

    assert!(response.contains("200 OK"), "Expected 200 OK, got: {}", response);
    assert!(response.contains("Hello"), "Expected Hello in response, got: {}", response);

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}

/// Test HTTP to HTTPS redirect
#[tokio::test]
async fn test_https_redirect() {
    let http_port = 30500;
    let https_port = 30501;
    let admin_port = 30502;
    let backend_port = 30503;

    let mut configs = HashMap::new();
    configs.insert("redirect-test.local".to_string(), mock_backend_config(backend_port));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    // Create HTTP proxy with HTTPS redirect enabled
    let http_addr: SocketAddr = format!("127.0.0.1:{}", http_port).parse().unwrap();
    let http_proxy = ProxyServer::new(http_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx.clone())
        .with_https_redirect(https_port);
    let http_proxy_handle = tokio::spawn(async move {
        let _ = http_proxy.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(http_port, Duration::from_secs(2)).await);

    // Send HTTP request - should get 301 redirect
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", http_port))
        .await
        .unwrap();

    let request = "GET /some/path?query=value HTTP/1.1\r\nHost: redirect-test.local\r\nConnection: close\r\n\r\n";
    stream.write_all(request.as_bytes()).await.unwrap();

    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();

    assert!(response.contains("301 Moved Permanently"), "Expected 301, got: {}", response);
    assert!(
        response.contains(&format!("https://redirect-test.local:{}/some/path?query=value", https_port)),
        "Expected redirect to HTTPS with path preserved, got: {}",
        response
    );

    // Cleanup
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = http_proxy_handle.await;
}

/// Test HTTP to HTTPS redirect with standard port 443 (no port in URL)
#[tokio::test]
async fn test_https_redirect_standard_port() {
    let http_port = 30600;
    let admin_port = 30601;

    let configs = HashMap::new();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    // Create HTTP proxy with HTTPS redirect to standard port 443
    let http_addr: SocketAddr = format!("127.0.0.1:{}", http_port).parse().unwrap();
    let http_proxy = ProxyServer::new(http_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx)
        .with_https_redirect(443);
    let http_proxy_handle = tokio::spawn(async move {
        let _ = http_proxy.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(http_port, Duration::from_secs(2)).await);

    // Send HTTP request - should get 301 redirect without port in URL
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", http_port))
        .await
        .unwrap();

    let request = "GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n";
    stream.write_all(request.as_bytes()).await.unwrap();

    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();

    assert!(response.contains("301 Moved Permanently"), "Expected 301, got: {}", response);
    // Port 443 should not appear in the redirect URL
    assert!(
        response.contains("https://example.com/") && !response.contains(":443"),
        "Expected redirect to https://example.com/ without port, got: {}",
        response
    );

    // Cleanup
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = http_proxy_handle.await;
}

// ============================================================================
// Docker Backend Integration Tests
// ============================================================================

use spawngate::docker::DockerManager;

/// Check if Docker is available for testing
/// Only runs on Linux since macOS and Windows Docker support in CI is inconsistent
#[cfg(not(target_os = "linux"))]
async fn docker_available() -> bool {
    eprintln!("Docker tests only run on Linux");
    false
}

/// Check if Docker is available for testing
#[cfg(target_os = "linux")]
async fn docker_available() -> bool {
    match DockerManager::new(None).await {
        Ok(_) => true,
        Err(e) => {
            eprintln!("Docker not available: {}", e);
            false
        }
    }
}

/// Helper to create a Docker backend config using http-echo image
fn docker_backend_config(port: u16) -> BackendConfig {
    let mut config = BackendConfig::docker("hashicorp/http-echo:latest", port);
    config.health_path = Some("/health".to_string());
    config.idle_timeout_secs = Some(60);
    config.startup_timeout_secs = Some(60); // Docker pull can be slow
    config.health_check_interval_ms = Some(100);
    config.shutdown_grace_period_secs = Some(2);
    config.drain_timeout_secs = Some(5);
    config.ready_health_check_interval_ms = Some(1000);
    config
}

/// Helper to clean up Docker containers after tests
async fn cleanup_docker_container(name: &str) {
    if let Ok(docker) = DockerManager::new(None).await {
        let _ = docker.remove_container(name).await;
    }
}

#[tokio::test]
async fn test_docker_manager_connection() {
    if !docker_available().await {
        eprintln!("Skipping test: Docker not available");
        return;
    }

    let manager = DockerManager::new(None).await.unwrap();
    // If we got here, connection succeeded
    assert!(!manager.is_running("nonexistent-container-id").await);
}

#[tokio::test]
async fn test_docker_backend_start_stop() {
    if !docker_available().await {
        eprintln!("Skipping test: Docker not available");
        return;
    }

    let port = 31000;
    let container_name = "spawngate-test-start-stop";

    // Ensure clean state
    cleanup_docker_container(container_name).await;

    let mut config = docker_backend_config(port);
    config.container_name = Some(container_name.to_string());
    // http-echo needs -text argument and -listen for port
    config.args = vec!["-text=hello".to_string(), format!("-listen=:{}", port)];

    let mut configs = HashMap::new();
    configs.insert("docker.local".to_string(), config);

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start the Docker backend
    let result = manager.start_backend("docker.local").await;
    assert!(result.is_ok(), "Failed to start Docker backend: {:?}", result.err());

    // Should be in Starting state
    assert_eq!(manager.get_state("docker.local"), BackendState::Starting);

    // Wait for container to be ready (health check should pass)
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if manager.get_state("docker.local") == BackendState::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Verify it became ready
    assert_eq!(
        manager.get_state("docker.local"),
        BackendState::Ready,
        "Docker backend did not become ready within timeout"
    );

    // Stop the backend
    manager.stop_backend("docker.local").await;
    assert_eq!(manager.get_state("docker.local"), BackendState::Stopped);

    // Cleanup
    cleanup_docker_container(container_name).await;
}

#[tokio::test]
async fn test_docker_backend_proxy_request() {
    if !docker_available().await {
        eprintln!("Skipping test: Docker not available");
        return;
    }

    let proxy_port = 31010;
    let admin_port = 31011;
    let backend_port = 31012;
    let container_name = "spawngate-test-proxy";

    // Ensure clean state
    cleanup_docker_container(container_name).await;

    let mut config = docker_backend_config(backend_port);
    config.container_name = Some(container_name.to_string());
    config.args = vec!["-text=docker-response".to_string(), format!("-listen=:{}", backend_port)];

    let mut configs = HashMap::new();
    configs.insert("docker.proxy.local".to_string(), config);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    // Start admin server
    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    // Start proxy server
    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    // Wait for servers to start
    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Make request through proxy - this should spawn the Docker backend
    let response = http_get_with_host(proxy_port, "/", "docker.proxy.local").await;

    match response {
        Ok(resp) => {
            assert!(resp.contains("200 OK"), "Expected 200 OK, got: {}", resp);
            assert!(resp.contains("docker-response"), "Expected docker-response in body, got: {}", resp);
        }
        Err(e) => {
            // Clean up before failing
            let _ = shutdown_tx.send(true);
            cleanup_docker_container(container_name).await;
            panic!("Request failed: {}", e);
        }
    }

    // Verify backend is ready
    assert_eq!(manager.get_state("docker.proxy.local"), BackendState::Ready);

    // Cleanup
    let _ = shutdown_tx.send(true);
    manager.stop_all().await;
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
    cleanup_docker_container(container_name).await;
}

#[tokio::test]
async fn test_docker_backend_multiple_requests() {
    if !docker_available().await {
        eprintln!("Skipping test: Docker not available");
        return;
    }

    let proxy_port = 31020;
    let admin_port = 31021;
    let backend_port = 31022;
    let container_name = "spawngate-test-multi";

    cleanup_docker_container(container_name).await;

    let mut config = docker_backend_config(backend_port);
    config.container_name = Some(container_name.to_string());
    config.args = vec!["-text=multi-test".to_string(), format!("-listen=:{}", backend_port)];

    let mut configs = HashMap::new();
    configs.insert("docker.multi.local".to_string(), config);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // First request starts the container
    let response1 = http_get_with_host(proxy_port, "/first", "docker.multi.local").await.unwrap();
    assert!(response1.contains("200 OK"), "First request failed: {}", response1);

    // Subsequent requests should use the same container
    for i in 0..5 {
        let response = http_get_with_host(proxy_port, &format!("/request-{}", i), "docker.multi.local").await.unwrap();
        assert!(response.contains("200 OK"), "Request {} failed: {}", i, response);
        assert!(response.contains("multi-test"), "Request {} missing expected body", i);
    }

    // Cleanup
    let _ = shutdown_tx.send(true);
    manager.stop_all().await;
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
    cleanup_docker_container(container_name).await;
}

#[tokio::test]
async fn test_docker_backend_stop_removes_container() {
    if !docker_available().await {
        eprintln!("Skipping test: Docker not available");
        return;
    }

    let port = 31030;
    let container_name = "spawngate-test-removal";

    cleanup_docker_container(container_name).await;

    let mut config = docker_backend_config(port);
    config.container_name = Some(container_name.to_string());
    config.args = vec!["-text=removal-test".to_string(), format!("-listen=:{}", port)];

    let mut configs = HashMap::new();
    configs.insert("docker.remove.local".to_string(), config);

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start and wait for ready
    manager.start_backend("docker.remove.local").await.unwrap();

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if manager.get_state("docker.remove.local") == BackendState::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(manager.get_state("docker.remove.local"), BackendState::Ready);

    // Verify container is running
    let docker = DockerManager::new(None).await.unwrap();
    assert!(docker.is_running(container_name).await, "Container should be running");

    // Stop the backend
    manager.stop_backend("docker.remove.local").await;

    // Container should be removed
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!docker.is_running(container_name).await, "Container should be stopped");

    cleanup_docker_container(container_name).await;
}

#[tokio::test]
async fn test_docker_and_local_backends_mixed() {
    if !docker_available().await {
        eprintln!("Skipping test: Docker not available");
        return;
    }

    if !mock_server_path().exists() {
        eprintln!("Skipping test: mock server not built");
        return;
    }

    let proxy_port = 31040;
    let admin_port = 31041;
    let docker_port = 31042;
    let local_port = 31043;
    let container_name = "spawngate-test-mixed";

    cleanup_docker_container(container_name).await;

    // Docker backend config
    let mut docker_config = docker_backend_config(docker_port);
    docker_config.container_name = Some(container_name.to_string());
    docker_config.args = vec!["-text=from-docker".to_string(), format!("-listen=:{}", docker_port)];

    // Local backend config
    let local_config = mock_backend_config(local_port);

    let mut configs = HashMap::new();
    configs.insert("docker.mixed.local".to_string(), docker_config);
    configs.insert("local.mixed.local".to_string(), local_config);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let defaults = BackendDefaults::default();

    let manager = ProcessManager::new(
        configs,
        defaults.clone(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Request to Docker backend
    let docker_response = http_get_with_host(proxy_port, "/", "docker.mixed.local").await.unwrap();
    assert!(docker_response.contains("200 OK"), "Docker request failed: {}", docker_response);
    assert!(docker_response.contains("from-docker"), "Docker response missing expected body");

    // Request to local backend
    let local_response = http_get_with_host(proxy_port, "/health", "local.mixed.local").await.unwrap();
    assert!(local_response.contains("200 OK"), "Local request failed: {}", local_response);

    // Both should be ready
    assert_eq!(manager.get_state("docker.mixed.local"), BackendState::Ready);
    assert_eq!(manager.get_state("local.mixed.local"), BackendState::Ready);

    // Cleanup
    let _ = shutdown_tx.send(true);
    manager.stop_all().await;
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
    cleanup_docker_container(container_name).await;
}

#[tokio::test]
async fn test_docker_pull_policy_if_not_present() {
    if !docker_available().await {
        eprintln!("Skipping test: Docker not available");
        return;
    }

    let port = 31050;
    let container_name = "spawngate-test-pull-ifnot";

    cleanup_docker_container(container_name).await;

    // Use if-not-present policy (default) - should work since image exists or will be pulled
    let mut config = docker_backend_config(port);
    config.container_name = Some(container_name.to_string());
    config.pull_policy = spawngate::config::PullPolicy::IfNotPresent;
    config.args = vec!["-text=pull-test".to_string(), format!("-listen=:{}", port)];

    let mut configs = HashMap::new();
    configs.insert("docker.pull.local".to_string(), config);

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start should succeed
    let result = manager.start_backend("docker.pull.local").await;
    assert!(result.is_ok(), "Failed to start with if-not-present policy: {:?}", result.err());

    // Wait for ready
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if manager.get_state("docker.pull.local") == BackendState::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(manager.get_state("docker.pull.local"), BackendState::Ready);

    // Cleanup
    manager.stop_backend("docker.pull.local").await;
    cleanup_docker_container(container_name).await;
}

#[tokio::test]
async fn test_docker_pull_policy_never_missing_image() {
    if !docker_available().await {
        eprintln!("Skipping test: Docker not available");
        return;
    }

    let port = 31051;
    let container_name = "spawngate-test-pull-never";

    cleanup_docker_container(container_name).await;

    // Use never policy with a non-existent image - should fail
    let mut config = BackendConfig::docker("nonexistent-image-that-does-not-exist:v999", port);
    config.container_name = Some(container_name.to_string());
    config.pull_policy = spawngate::config::PullPolicy::Never;
    config.startup_timeout_secs = Some(5);
    config.health_check_interval_ms = Some(100);
    config.args = vec!["-text=test".to_string()];

    let mut configs = HashMap::new();
    configs.insert("docker.never.local".to_string(), config);

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start should fail because image doesn't exist and we won't pull
    let result = manager.start_backend("docker.never.local").await;
    assert!(result.is_err(), "Expected failure with never policy on missing image");

    cleanup_docker_container(container_name).await;
}

#[tokio::test]
async fn test_docker_pull_policy_always() {
    if !docker_available().await {
        eprintln!("Skipping test: Docker not available");
        return;
    }

    let port = 31052;
    let container_name = "spawngate-test-pull-always";

    cleanup_docker_container(container_name).await;

    // Use always policy - should pull even if image exists
    let mut config = docker_backend_config(port);
    config.container_name = Some(container_name.to_string());
    config.pull_policy = spawngate::config::PullPolicy::Always;
    config.args = vec!["-text=always-pull".to_string(), format!("-listen=:{}", port)];

    let mut configs = HashMap::new();
    configs.insert("docker.always.local".to_string(), config);

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start should succeed (will pull image)
    let result = manager.start_backend("docker.always.local").await;
    assert!(result.is_ok(), "Failed to start with always policy: {:?}", result.err());

    // Wait for ready
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(60) {
        if manager.get_state("docker.always.local") == BackendState::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(manager.get_state("docker.always.local"), BackendState::Ready);

    // Cleanup
    manager.stop_backend("docker.always.local").await;
    cleanup_docker_container(container_name).await;
}

#[tokio::test]
async fn test_docker_idle_timeout_cleanup() {
    if !docker_available().await {
        eprintln!("Skipping test: Docker not available");
        return;
    }

    let port = 31060;
    let container_name = "spawngate-test-idle";

    cleanup_docker_container(container_name).await;

    let mut config = docker_backend_config(port);
    config.container_name = Some(container_name.to_string());
    config.idle_timeout_secs = Some(2); // 2 second idle timeout
    config.args = vec!["-text=idle-test".to_string(), format!("-listen=:{}", port)];
    config.ready_health_check_interval_ms = Some(60000); // Long to avoid interference

    let mut configs = HashMap::new();
    configs.insert("docker.idle.local".to_string(), config);

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start and wait for ready
    manager.start_backend("docker.idle.local").await.unwrap();

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if manager.get_state("docker.idle.local") == BackendState::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(manager.get_state("docker.idle.local"), BackendState::Ready);

    // Verify container is running
    let docker = DockerManager::new(None).await.unwrap();
    assert!(docker.is_running(container_name).await, "Container should be running");

    // Wait for idle timeout
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Run cleanup
    manager.cleanup_idle_backends().await;

    // Should be stopped
    assert_eq!(manager.get_state("docker.idle.local"), BackendState::Stopped);

    // Container should be removed
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!docker.is_running(container_name).await, "Container should be stopped after idle timeout");

    cleanup_docker_container(container_name).await;
}

#[tokio::test]
async fn test_docker_activity_resets_idle_timeout() {
    if !docker_available().await {
        eprintln!("Skipping test: Docker not available");
        return;
    }

    let port = 31061;
    let container_name = "spawngate-test-active";

    cleanup_docker_container(container_name).await;

    let mut config = docker_backend_config(port);
    config.container_name = Some(container_name.to_string());
    config.idle_timeout_secs = Some(2); // 2 second idle timeout
    config.args = vec!["-text=active-test".to_string(), format!("-listen=:{}", port)];
    config.ready_health_check_interval_ms = Some(60000);

    let mut configs = HashMap::new();
    configs.insert("docker.active.local".to_string(), config);

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start and wait for ready
    manager.start_backend("docker.active.local").await.unwrap();

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if manager.get_state("docker.active.local") == BackendState::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(manager.get_state("docker.active.local"), BackendState::Ready);

    // Touch periodically to keep it alive
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        manager.touch("docker.active.local");
        manager.cleanup_idle_backends().await;
        // Should still be ready
        assert_eq!(
            manager.get_state("docker.active.local"),
            BackendState::Ready,
            "Backend should still be ready after touch"
        );
    }

    // Cleanup
    manager.stop_backend("docker.active.local").await;
    cleanup_docker_container(container_name).await;
}

#[tokio::test]
async fn test_docker_startup_timeout() {
    if !docker_available().await {
        eprintln!("Skipping test: Docker not available");
        return;
    }

    let port = 31070;
    let container_name = "spawngate-test-timeout";

    cleanup_docker_container(container_name).await;

    // Use sleep image which won't respond to health checks
    let mut config = BackendConfig::docker("alpine:latest", port);
    config.container_name = Some(container_name.to_string());
    config.startup_timeout_secs = Some(2); // 2 second timeout
    config.health_check_interval_ms = Some(100);
    config.args = vec!["sleep".to_string(), "60".to_string()]; // Just sleep, no HTTP server

    let mut configs = HashMap::new();
    configs.insert("docker.timeout.local".to_string(), config);

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start the backend
    manager.start_backend("docker.timeout.local").await.unwrap();
    assert_eq!(manager.get_state("docker.timeout.local"), BackendState::Starting);

    // Wait for startup timeout
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Should be stopped due to timeout (health checks will fail)
    assert_eq!(
        manager.get_state("docker.timeout.local"),
        BackendState::Stopped,
        "Backend should be stopped after startup timeout"
    );

    cleanup_docker_container(container_name).await;
}

#[tokio::test]
async fn test_docker_container_env_vars() {
    if !docker_available().await {
        eprintln!("Skipping test: Docker not available");
        return;
    }

    let port = 31080;
    let container_name = "spawngate-test-env";

    cleanup_docker_container(container_name).await;

    let mut config = docker_backend_config(port);
    config.container_name = Some(container_name.to_string());
    config.args = vec!["-text=env-test".to_string(), format!("-listen=:{}", port)];
    // Add custom env vars
    config.env.insert("CUSTOM_VAR".to_string(), "custom_value".to_string());
    config.env.insert("ANOTHER_VAR".to_string(), "another_value".to_string());

    let mut configs = HashMap::new();
    configs.insert("docker.env.local".to_string(), config);

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start and verify it works
    manager.start_backend("docker.env.local").await.unwrap();

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if manager.get_state("docker.env.local") == BackendState::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(manager.get_state("docker.env.local"), BackendState::Ready);

    // Verify container is running and responding
    let response = http_get(port, "/").await.unwrap();
    assert!(response.contains("env-test"), "Container should respond: {}", response);

    // Cleanup
    manager.stop_backend("docker.env.local").await;
    cleanup_docker_container(container_name).await;
}

#[tokio::test]
async fn test_docker_resource_limits() {
    if !docker_available().await {
        eprintln!("Skipping test: Docker not available");
        return;
    }

    let port = 31090;
    let container_name = "spawngate-test-resources";

    cleanup_docker_container(container_name).await;

    let mut config = docker_backend_config(port);
    config.container_name = Some(container_name.to_string());
    config.args = vec!["-text=resource-test".to_string(), format!("-listen=:{}", port)];
    // Set resource limits
    config.memory = Some("64m".to_string());
    config.cpus = Some("0.5".to_string());

    let mut configs = HashMap::new();
    configs.insert("docker.resources.local".to_string(), config);

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start and verify it works with resource limits
    manager.start_backend("docker.resources.local").await.unwrap();

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if manager.get_state("docker.resources.local") == BackendState::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(manager.get_state("docker.resources.local"), BackendState::Ready);

    // Verify container is running
    let docker = DockerManager::new(None).await.unwrap();
    assert!(docker.is_running(container_name).await, "Container should be running with resource limits");

    // Cleanup
    manager.stop_backend("docker.resources.local").await;
    cleanup_docker_container(container_name).await;
}

#[tokio::test]
async fn test_docker_restart_after_stop() {
    if !docker_available().await {
        eprintln!("Skipping test: Docker not available");
        return;
    }

    let port = 31100;
    let container_name = "spawngate-test-restart";

    cleanup_docker_container(container_name).await;

    let mut config = docker_backend_config(port);
    config.container_name = Some(container_name.to_string());
    config.args = vec!["-text=restart-test".to_string(), format!("-listen=:{}", port)];

    let mut configs = HashMap::new();
    configs.insert("docker.restart.local".to_string(), config);

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // First start
    manager.start_backend("docker.restart.local").await.unwrap();

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if manager.get_state("docker.restart.local") == BackendState::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(manager.get_state("docker.restart.local"), BackendState::Ready);

    // Verify it's responding
    let response1 = http_get(port, "/").await.unwrap();
    assert!(response1.contains("restart-test"), "First start should work");

    // Stop
    manager.stop_backend("docker.restart.local").await;
    assert_eq!(manager.get_state("docker.restart.local"), BackendState::Stopped);

    // Wait a moment
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Second start - should work again
    manager.start_backend("docker.restart.local").await.unwrap();

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if manager.get_state("docker.restart.local") == BackendState::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(manager.get_state("docker.restart.local"), BackendState::Ready);

    // Verify it's responding again
    let response2 = http_get(port, "/").await.unwrap();
    assert!(response2.contains("restart-test"), "Second start should work");

    // Cleanup
    manager.stop_backend("docker.restart.local").await;
    cleanup_docker_container(container_name).await;
}

#[tokio::test]
async fn test_docker_concurrent_starts() {
    if !docker_available().await {
        eprintln!("Skipping test: Docker not available");
        return;
    }

    let base_port = 31110;
    let container_names: Vec<String> = (0..3)
        .map(|i| format!("spawngate-test-concurrent-{}", i))
        .collect();

    // Cleanup first
    for name in &container_names {
        cleanup_docker_container(name).await;
    }

    let mut configs = HashMap::new();
    for (i, name) in container_names.iter().enumerate() {
        let port = base_port + i as u16;
        let mut config = docker_backend_config(port);
        config.container_name = Some(name.clone());
        config.args = vec![format!("-text=concurrent-{}", i), format!("-listen=:{}", port)];
        configs.insert(format!("docker.concurrent{}.local", i), config);
    }

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        "http://127.0.0.1:9999".to_string(),
    );

    // Start all backends concurrently
    let hostnames: Vec<String> = (0..3)
        .map(|i| format!("docker.concurrent{}.local", i))
        .collect();

    let mut handles = vec![];
    for hostname in &hostnames {
        let m = Arc::clone(&manager);
        let h = hostname.clone();
        handles.push(tokio::spawn(async move {
            m.start_backend(&h).await
        }));
    }

    // Wait for all starts to complete
    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok(), "Concurrent start failed: {:?}", result.err());
    }

    // Wait for all to become ready
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(60) {
        let all_ready = hostnames.iter().all(|h| manager.get_state(h) == BackendState::Ready);
        if all_ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Verify all are ready
    for hostname in &hostnames {
        assert_eq!(
            manager.get_state(hostname),
            BackendState::Ready,
            "{} should be ready",
            hostname
        );
    }

    // Cleanup
    manager.stop_all().await;
    for name in &container_names {
        cleanup_docker_container(name).await;
    }
}

// ============================================================================
// Hot Reload Tests
// ============================================================================

/// Test hot reload adds new backends
#[tokio::test]
async fn test_hot_reload_add_backend() {
    let port_a = 31500;
    let port_b = 31501;
    let admin_port = 31502;

    let mut configs = HashMap::new();
    configs.insert("app-a.local".to_string(), mock_backend_config(port_a));

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    // Initially only has app-a
    assert!(manager.has_backend("app-a.local"));
    assert!(!manager.has_backend("app-b.local"));

    // Reload with new backend
    let mut new_configs = HashMap::new();
    new_configs.insert("app-a.local".to_string(), mock_backend_config(port_a));
    new_configs.insert("app-b.local".to_string(), mock_backend_config(port_b));

    let result = manager.apply_config(new_configs, BackendDefaults::default()).await.unwrap();

    // Check reload result
    assert!(result.added.contains(&"app-b.local".to_string()));
    assert!(result.updated.contains(&"app-a.local".to_string()));
    assert!(result.removed.is_empty());

    // Now has both backends
    assert!(manager.has_backend("app-a.local"));
    assert!(manager.has_backend("app-b.local"));
}

/// Test hot reload removes backends
#[tokio::test]
async fn test_hot_reload_remove_backend() {
    let port_a = 31510;
    let port_b = 31511;
    let admin_port = 31512;

    let mut configs = HashMap::new();
    configs.insert("app-a.local".to_string(), mock_backend_config(port_a));
    configs.insert("app-b.local".to_string(), mock_backend_config(port_b));

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    // Start app-b
    manager.start_backend("app-b.local").await.unwrap();

    // Wait for ready
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if manager.get_state("app-b.local") == BackendState::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(manager.get_state("app-b.local"), BackendState::Ready);

    // Reload with app-b removed
    let mut new_configs = HashMap::new();
    new_configs.insert("app-a.local".to_string(), mock_backend_config(port_a));

    let result = manager.apply_config(new_configs, BackendDefaults::default()).await.unwrap();

    // Check reload result
    assert!(result.removed.contains(&"app-b.local".to_string()));
    assert!(result.updated.contains(&"app-a.local".to_string()));

    // app-b should be stopped and removed from config
    assert!(manager.has_backend("app-a.local"));
    assert!(!manager.has_backend("app-b.local"));
    assert_eq!(manager.get_state("app-b.local"), BackendState::Stopped);

    manager.stop_all().await;
}

/// Test hot reload updates defaults
#[tokio::test]
async fn test_hot_reload_updates_defaults() {
    let port = 31520;
    let admin_port = 31521;

    let mut configs = HashMap::new();
    configs.insert("app.local".to_string(), mock_backend_config(port));

    let defaults = BackendDefaults {
        idle_timeout_secs: 300,
        ..Default::default()
    };

    let manager = ProcessManager::new(
        configs.clone(),
        defaults,
        format!("http://127.0.0.1:{}", admin_port),
    );

    // Check initial defaults
    assert_eq!(manager.get_defaults().idle_timeout_secs, 300);

    // Reload with new defaults
    let new_defaults = BackendDefaults {
        idle_timeout_secs: 600,
        ..Default::default()
    };

    manager.apply_config(configs, new_defaults).await.unwrap();

    // Check updated defaults
    assert_eq!(manager.get_defaults().idle_timeout_secs, 600);
}

/// Test hot reload with running backend continues to work
#[tokio::test]
async fn test_hot_reload_running_backend_continues() {
    let port_a = 31530;
    let port_b = 31531;
    let admin_port = 31532;
    let proxy_port = 31533;

    let mut configs = HashMap::new();
    configs.insert("app.local".to_string(), mock_backend_config(port_a));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let manager = ProcessManager::new(
        configs,
        BackendDefaults::default(),
        format!("http://127.0.0.1:{}", admin_port),
    );

    // Start admin server
    let admin_addr: SocketAddr = format!("127.0.0.1:{}", admin_port).parse().unwrap();
    let admin_server = AdminServer::new(admin_addr, Arc::clone(&manager), shutdown_rx.clone(), "test-token".to_string());
    let admin_handle = tokio::spawn(async move {
        let _ = admin_server.run().await;
    });

    // Start proxy
    let proxy_addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let proxy_server = ProxyServer::new(proxy_addr, Arc::clone(&manager), manager.shared_defaults(), shutdown_rx);
    let proxy_handle = tokio::spawn(async move {
        let _ = proxy_server.run().await;
    });

    assert!(wait_for_port(admin_port, Duration::from_secs(2)).await);
    assert!(wait_for_port(proxy_port, Duration::from_secs(2)).await);

    // Make request to start backend
    let response = http_get_with_host(proxy_port, "/echo", "app.local").await.unwrap();
    assert!(response.contains("200 OK"), "Initial request failed: {}", response);

    // Hot reload with new backend added (existing backend config unchanged)
    let mut new_configs = HashMap::new();
    new_configs.insert("app.local".to_string(), mock_backend_config(port_a));
    new_configs.insert("new-app.local".to_string(), mock_backend_config(port_b));

    let result = manager.apply_config(new_configs, BackendDefaults::default()).await.unwrap();
    assert!(result.added.contains(&"new-app.local".to_string()));

    // Original backend should still work
    let response = http_get_with_host(proxy_port, "/echo", "app.local").await.unwrap();
    assert!(response.contains("200 OK"), "Request after reload failed: {}", response);

    // New backend should be routable (will start on first request)
    assert!(manager.has_backend("new-app.local"));

    // Cleanup
    manager.stop_all().await;
    let _ = shutdown_tx.send(true);
    let _ = admin_handle.await;
    let _ = proxy_handle.await;
}
