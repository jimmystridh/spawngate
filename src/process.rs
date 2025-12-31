use crate::config::{BackendConfig, BackendDefaults, BackendType, Config};
use crate::docker::{DockerManager, SharedDockerManager};
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

/// Interval for polling drain status during shutdown (in milliseconds)
const DRAIN_POLL_INTERVAL_MS: u64 = 50;

/// State of a backend process
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendState {
    /// Process is not running
    Stopped,
    /// Process is starting up, waiting for health check
    Starting,
    /// Process is running and ready to accept traffic
    Ready,
    /// Process is running but health checks are failing
    Unhealthy,
    /// Process is shutting down
    Stopping,
}

/// Handle to a running backend (local process or Docker container)
pub enum ProcessHandle {
    /// Local process spawned directly
    Local(Child),
    /// Docker container
    Docker {
        container_id: String,
        docker: SharedDockerManager,
        /// Sender to stop log streaming when container is stopped
        log_shutdown: Option<tokio::sync::watch::Sender<bool>>,
    },
}

/// Information about a running backend
pub struct BackendProcess {
    /// The process or container handle
    handle: ProcessHandle,
    /// Current state of the backend
    state: BackendState,
    /// Last time traffic was received
    last_activity: Instant,
    /// Channel to notify when state changes to Ready
    ready_tx: broadcast::Sender<()>,
    /// Number of in-flight requests currently being processed
    in_flight: Arc<AtomicUsize>,
    /// Consecutive health check failures
    consecutive_failures: u32,
}

/// Shared reference to backend defaults (for hot reload support)
pub type SharedDefaults = Arc<RwLock<BackendDefaults>>;

/// Manages all backend processes.
///
/// # Usage
///
/// `ProcessManager` is designed to be used behind an `Arc` for shared ownership
/// across async tasks. The [`new`](ProcessManager::new) constructor returns
/// `Arc<Self>` directly to enforce this pattern.
///
/// ```ignore
/// let manager = ProcessManager::new(configs, defaults, admin_url);
/// // manager is already Arc<ProcessManager>
/// ```
///
/// Methods that spawn background tasks (like `start_backend`) require `&Arc<Self>`
/// to clone the Arc for the spawned task.
///
/// # Hot Reload
///
/// The `reload_config` method allows updating backend configurations without
/// restarting the proxy. New backends are added, removed backends are stopped
/// gracefully, and modified backends take effect on their next restart.
pub struct ProcessManager {
    /// Running processes keyed by hostname
    processes: DashMap<String, Mutex<BackendProcess>>,
    /// Configuration for each backend (supports hot reload)
    configs: Arc<RwLock<HashMap<String, BackendConfig>>>,
    /// Default settings (supports hot reload)
    defaults: SharedDefaults,
    /// Admin API URL for callback notifications
    admin_url: String,
    /// Docker manager (lazily initialized when needed)
    docker: tokio::sync::OnceCell<SharedDockerManager>,
}

impl ProcessManager {
    /// Create a new process manager.
    ///
    /// Returns `Arc<Self>` because the manager is designed to be shared
    /// across multiple async tasks for health monitoring and request handling.
    pub fn new(
        configs: HashMap<String, BackendConfig>,
        defaults: BackendDefaults,
        admin_url: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            processes: DashMap::new(),
            configs: Arc::new(RwLock::new(configs)),
            defaults: Arc::new(RwLock::new(defaults)),
            admin_url,
            docker: tokio::sync::OnceCell::new(),
        })
    }

    /// Get a shared reference to the defaults (for ProxyServer)
    pub fn shared_defaults(&self) -> SharedDefaults {
        Arc::clone(&self.defaults)
    }

    /// Get or initialize the Docker manager
    async fn get_docker(&self, docker_host: Option<&str>) -> anyhow::Result<SharedDockerManager> {
        self.docker
            .get_or_try_init(|| async {
                let manager = DockerManager::new(docker_host).await?;
                Ok(Arc::new(manager))
            })
            .await
            .cloned()
    }

    /// Get the configuration for a hostname (cloned for thread safety)
    pub fn get_config(&self, hostname: &str) -> Option<BackendConfig> {
        self.configs.read().get(hostname).cloned()
    }

    /// Check if a backend exists in configuration
    pub fn has_backend(&self, hostname: &str) -> bool {
        self.configs.read().contains_key(hostname)
    }

    /// Get the current defaults (cloned for thread safety)
    pub fn get_defaults(&self) -> BackendDefaults {
        self.defaults.read().clone()
    }

    /// Get the current state of a backend
    pub fn get_state(&self, hostname: &str) -> BackendState {
        self.processes
            .get(hostname)
            .map(|p| p.lock().state)
            .unwrap_or(BackendState::Stopped)
    }

    /// Check if a backend is ready to accept traffic
    pub fn is_ready(&self, hostname: &str) -> bool {
        self.get_state(hostname) == BackendState::Ready
    }

    /// Update the last activity timestamp for a backend
    pub fn touch(&self, hostname: &str) {
        if let Some(process) = self.processes.get(hostname) {
            process.lock().last_activity = Instant::now();
        }
    }

    /// Get a receiver that will be notified when the backend becomes ready
    pub fn subscribe_ready(&self, hostname: &str) -> Option<broadcast::Receiver<()>> {
        self.processes
            .get(hostname)
            .map(|p| p.lock().ready_tx.subscribe())
    }

    /// Increment the in-flight request count for a backend
    /// Returns true if the backend is in a valid state to accept requests
    pub fn increment_in_flight(&self, hostname: &str) -> bool {
        if let Some(process) = self.processes.get(hostname) {
            let guard = process.lock();
            // Only accept new requests if backend is Ready
            if guard.state == BackendState::Ready {
                guard.in_flight.fetch_add(1, Ordering::SeqCst);
                return true;
            }
        }
        false
    }

    /// Decrement the in-flight request count for a backend
    pub fn decrement_in_flight(&self, hostname: &str) {
        if let Some(process) = self.processes.get(hostname) {
            process.lock().in_flight.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Get the in-flight request count for a backend
    pub fn get_in_flight(&self, hostname: &str) -> usize {
        self.processes
            .get(hostname)
            .map(|p| p.lock().in_flight.load(Ordering::SeqCst))
            .unwrap_or(0)
    }

    /// Mark a backend as ready (called from health check or callback)
    pub fn mark_ready(&self, hostname: &str) -> bool {
        if let Some(process) = self.processes.get(hostname) {
            let mut guard = process.lock();
            if guard.state == BackendState::Starting || guard.state == BackendState::Unhealthy {
                let was_unhealthy = guard.state == BackendState::Unhealthy;
                guard.state = BackendState::Ready;
                guard.last_activity = Instant::now();
                guard.consecutive_failures = 0;
                // Notify all waiting requests
                let _ = guard.ready_tx.send(());
                if was_unhealthy {
                    info!(hostname, "Backend recovered and is now ready");
                } else {
                    info!(hostname, "Backend is now ready");
                }
                return true;
            }
        }
        false
    }

    /// Mark a backend as unhealthy
    pub fn mark_unhealthy(&self, hostname: &str) {
        if let Some(process) = self.processes.get(hostname) {
            let mut guard = process.lock();
            if guard.state == BackendState::Ready {
                guard.state = BackendState::Unhealthy;
                warn!(hostname, "Backend marked as unhealthy");
            }
        }
    }

    /// Record a health check failure, returns true if backend should be marked unhealthy
    pub fn record_health_failure(&self, hostname: &str, threshold: u32) -> bool {
        if let Some(process) = self.processes.get(hostname) {
            let mut guard = process.lock();
            guard.consecutive_failures += 1;
            if guard.consecutive_failures >= threshold && guard.state == BackendState::Ready {
                guard.state = BackendState::Unhealthy;
                warn!(
                    hostname,
                    failures = guard.consecutive_failures,
                    "Backend marked as unhealthy after consecutive failures"
                );
                return true;
            }
        }
        false
    }

    /// Reset health check failure count (on successful health check)
    pub fn reset_health_failures(&self, hostname: &str) {
        if let Some(process) = self.processes.get(hostname) {
            let mut guard = process.lock();
            if guard.consecutive_failures > 0 {
                guard.consecutive_failures = 0;
                if guard.state == BackendState::Unhealthy {
                    guard.state = BackendState::Ready;
                    info!(hostname, "Backend recovered and is now healthy");
                }
            }
        }
    }

    /// Check if a backend is healthy (Ready state)
    pub fn is_healthy(&self, hostname: &str) -> bool {
        self.get_state(hostname) == BackendState::Ready
    }

    /// Start a backend process or container
    pub async fn start_backend(self: &Arc<Self>, hostname: &str) -> anyhow::Result<()> {
        let config = self
            .get_config(hostname)
            .ok_or_else(|| anyhow::anyhow!("Unknown backend: {}", hostname))?;

        // Check if already running or starting
        if let Some(process) = self.processes.get(hostname) {
            let state = process.lock().state;
            if state == BackendState::Starting || state == BackendState::Ready {
                debug!(hostname, "Backend already running or starting");
                return Ok(());
            }
        }

        let handle = match config.backend_type {
            BackendType::Local => self.start_local_backend(hostname, &config).await?,
            BackendType::Docker => self.start_docker_backend(hostname, &config).await?,
        };

        let (ready_tx, _) = broadcast::channel(16);
        let now = Instant::now();

        let process = BackendProcess {
            handle,
            state: BackendState::Starting,
            last_activity: now,
            ready_tx,
            in_flight: Arc::new(AtomicUsize::new(0)),
            consecutive_failures: 0,
        };

        self.processes.insert(hostname.to_string(), Mutex::new(process));

        // Start health check polling
        let manager = Arc::clone(self);
        let hostname_owned = hostname.to_string();
        let config_clone = config.clone();
        let defaults = self.get_defaults();

        tokio::spawn(async move {
            manager
                .poll_health(&hostname_owned, &config_clone, &defaults)
                .await;
        });

        Ok(())
    }

    /// Start a local process backend
    async fn start_local_backend(
        &self,
        hostname: &str,
        config: &BackendConfig,
    ) -> anyhow::Result<ProcessHandle> {
        let command = config.command.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Local backend requires 'command' field")
        })?;

        info!(hostname, command = %command, "Starting local backend");

        let mut cmd = Command::new(command);
        cmd.args(&config.args);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Set working directory if specified
        if let Some(ref working_dir) = config.working_dir {
            cmd.current_dir(working_dir);
        }

        // Set environment variables
        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        // Set the PORT environment variable
        cmd.env("PORT", config.port.to_string());

        // Set the callback URL for ready notification
        let callback_url = format!("{}/ready/{}", self.admin_url, hostname);
        cmd.env("SERVERLESS_PROXY_READY_URL", &callback_url);

        // Spawn the process
        let child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);
        info!(hostname, pid, "Backend process spawned");

        Ok(ProcessHandle::Local(child))
    }

    /// Start a Docker container backend
    async fn start_docker_backend(
        &self,
        hostname: &str,
        config: &BackendConfig,
    ) -> anyhow::Result<ProcessHandle> {
        let image = config.image.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Docker backend requires 'image' field")
        })?;

        info!(hostname, image = %image, "Starting Docker backend");

        let docker = self.get_docker(config.docker_host.as_deref()).await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Cannot start Docker backend '{}': {}",
                    hostname, e
                )
            })?;

        let container_id = docker
            .start_container(config, hostname, &self.admin_url)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to start Docker container for backend '{}' (image: {}): {}",
                    hostname, image, e
                )
            })?;

        // Start streaming container logs
        let log_shutdown = docker.stream_logs(container_id.clone(), hostname.to_string());

        Ok(ProcessHandle::Docker {
            container_id,
            docker,
            log_shutdown: Some(log_shutdown),
        })
    }

    /// Spawn an auto-restart for an unhealthy backend
    fn spawn_auto_restart(self: &Arc<Self>, hostname: &str) {
        let manager = Arc::clone(self);
        let hostname_owned = hostname.to_string();
        tokio::spawn(async move {
            // Stop the unhealthy backend
            manager.stop_backend(&hostname_owned).await;

            // Small delay before restart
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Restart the backend
            if let Err(e) = manager.start_backend(&hostname_owned).await {
                error!(hostname = %hostname_owned, error = %e, "Failed to auto-restart backend");
            }
        });
    }

    /// Poll the health endpoint until the backend is ready, then continue monitoring
    async fn poll_health(
        self: &Arc<Self>,
        hostname: &str,
        config: &BackendConfig,
        defaults: &BackendDefaults,
    ) {
        let health_path = config.health_path(defaults);
        let health_url = format!("http://127.0.0.1:{}{}", config.port, health_path);
        let startup_interval = config.health_check_interval(defaults);
        let ready_interval = config.ready_health_check_interval(defaults);
        let timeout = config.startup_timeout(defaults);
        let unhealthy_threshold = config.unhealthy_threshold(defaults);
        let start = Instant::now();

        debug!(hostname, %health_url, "Starting health check polling");

        // Phase 1: Wait for backend to become ready
        loop {
            let state = self.get_state(hostname);
            if state != BackendState::Starting {
                if state == BackendState::Ready {
                    break; // Continue to phase 2
                }
                debug!(hostname, ?state, "Stopping health polling, state changed");
                return;
            }

            // Check startup timeout
            if start.elapsed() > timeout {
                error!(hostname, "Backend startup timeout exceeded");
                self.stop_backend(hostname).await;
                return;
            }

            // Try to connect to the health endpoint
            match self.check_health(&health_url).await {
                Ok(true) => {
                    if self.mark_ready(hostname) {
                        break; // Continue to phase 2
                    }
                }
                Ok(false) => {
                    debug!(hostname, "Health check returned unhealthy");
                }
                Err(e) => {
                    debug!(hostname, error = %e, "Health check failed");
                }
            }

            tokio::time::sleep(startup_interval).await;
        }

        // Phase 2: Continuous health monitoring
        debug!(
            hostname,
            interval_ms = ready_interval.as_millis(),
            "Starting continuous health monitoring"
        );

        loop {
            tokio::time::sleep(ready_interval).await;

            let state = self.get_state(hostname);
            match state {
                BackendState::Ready | BackendState::Unhealthy => {
                    // Continue monitoring
                }
                BackendState::Stopping | BackendState::Stopped => {
                    debug!(hostname, ?state, "Stopping health monitoring, backend shutting down");
                    return;
                }
                BackendState::Starting => {
                    // Shouldn't happen, but handle gracefully
                    debug!(hostname, "Backend unexpectedly in Starting state during monitoring");
                    return;
                }
            }

            // Perform health check
            match self.check_health(&health_url).await {
                Ok(true) => {
                    // Health check passed
                    self.reset_health_failures(hostname);
                }
                Ok(false) | Err(_) => {
                    // Health check failed
                    let became_unhealthy =
                        self.record_health_failure(hostname, unhealthy_threshold);
                    if became_unhealthy {
                        // Attempt auto-restart
                        info!(hostname, "Attempting auto-restart of unhealthy backend");
                        self.spawn_auto_restart(hostname);
                        return; // New poll_health task will be spawned by start_backend
                    }
                }
            }
        }
    }

    /// Check the health endpoint with actual HTTP request
    async fn check_health(&self, url: &str) -> anyhow::Result<bool> {
        // Parse URL to extract host:port and path
        let url_without_scheme = url.strip_prefix("http://").unwrap_or(url);
        let (host_port, path) = url_without_scheme
            .split_once('/')
            .map(|(h, p)| (h, format!("/{}", p)))
            .unwrap_or((url_without_scheme, "/".to_string()));

        // Connect with a short timeout
        let connect_result = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::TcpStream::connect(host_port),
        )
        .await;

        let mut stream = match connect_result {
            Ok(Ok(s)) => s,
            Ok(Err(_)) | Err(_) => return Ok(false),
        };

        // Send HTTP GET request
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            path, host_port
        );

        if stream.write_all(request.as_bytes()).await.is_err() {
            return Ok(false);
        }

        // Read response with timeout
        let read_result = tokio::time::timeout(Duration::from_secs(2), async {
            let mut reader = BufReader::new(stream);
            let mut status_line = String::new();
            reader.read_line(&mut status_line).await?;
            Ok::<_, std::io::Error>(status_line)
        })
        .await;

        match read_result {
            Ok(Ok(status_line)) => {
                // Check for 2xx status code
                // Format: "HTTP/1.1 200 OK\r\n"
                let is_healthy = status_line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|code| code.parse::<u16>().ok())
                    .map(|code| (200..300).contains(&code))
                    .unwrap_or(false);
                Ok(is_healthy)
            }
            _ => Ok(false),
        }
    }

    /// Stop a backend process/container with graceful shutdown
    /// 1. Mark as Stopping (stops accepting new requests)
    /// 2. Wait for in-flight requests to drain (with timeout)
    /// 3. Send SIGTERM / docker stop
    /// 4. Wait for graceful shutdown (with timeout)
    /// 5. Send SIGKILL / docker kill if still running
    pub async fn stop_backend(&self, hostname: &str) {
        // Get config for timeouts
        let defaults = self.get_defaults();
        let (drain_timeout, grace_period) = self
            .get_config(hostname)
            .map(|c| (c.drain_timeout(&defaults), c.shutdown_grace_period(&defaults)))
            .unwrap_or((
                Duration::from_secs(defaults.drain_timeout_secs),
                Duration::from_secs(defaults.shutdown_grace_period_secs),
            ));

        // Get the in-flight counter before removing the process
        let in_flight_counter = self.processes.get(hostname).map(|p| {
            let guard = p.lock();
            guard.in_flight.clone()
        });

        // Mark as stopping (if present)
        if let Some(process) = self.processes.get(hostname) {
            process.lock().state = BackendState::Stopping;
        }

        // Wait for in-flight requests to drain
        if let Some(counter) = in_flight_counter {
            let drain_start = Instant::now();
            while counter.load(Ordering::SeqCst) > 0 {
                if drain_start.elapsed() > drain_timeout {
                    let remaining = counter.load(Ordering::SeqCst);
                    warn!(
                        hostname,
                        remaining,
                        "Drain timeout exceeded, proceeding with shutdown"
                    );
                    break;
                }
                tokio::time::sleep(Duration::from_millis(DRAIN_POLL_INTERVAL_MS)).await;
            }
            let drained_in = drain_start.elapsed();
            if drained_in > Duration::from_millis(100) {
                info!(hostname, drained_in_ms = drained_in.as_millis(), "Drained in-flight requests");
            }
        }

        // Remove and extract the process handle
        let backend = {
            let Some((_, process)) = self.processes.remove(hostname) else {
                return;
            };
            process.into_inner()
        };

        match backend.handle {
            ProcessHandle::Local(mut child) => {
                self.stop_local_process(hostname, &mut child, grace_period).await;
            }
            ProcessHandle::Docker { container_id, docker, log_shutdown } => {
                // Stop log streaming first
                if let Some(shutdown) = log_shutdown {
                    let _ = shutdown.send(true);
                }
                self.stop_docker_container(hostname, &container_id, &docker, grace_period).await;
            }
        }
    }

    /// Stop a local process
    async fn stop_local_process(&self, hostname: &str, child: &mut Child, grace_period: Duration) {
        if let Some(pid) = child.id() {
            info!(hostname, pid, "Sending SIGTERM to backend");

            // Send SIGTERM on Unix, or kill on other platforms
            #[cfg(unix)]
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }

            #[cfg(not(unix))]
            {
                let _ = child.start_kill();
            }
        }

        // Wait for the process to exit (with configurable grace period)
        let wait_result = tokio::time::timeout(grace_period, child.wait()).await;

        match wait_result {
            Ok(Ok(status)) => {
                info!(hostname, ?status, "Backend process exited gracefully");
            }
            Ok(Err(e)) => {
                warn!(hostname, error = %e, "Error waiting for backend to exit");
            }
            Err(_) => {
                warn!(
                    hostname,
                    grace_period_secs = grace_period.as_secs(),
                    "Grace period exceeded, sending SIGKILL"
                );
                let _ = child.kill().await;
            }
        }
    }

    /// Stop a Docker container
    async fn stop_docker_container(
        &self,
        hostname: &str,
        container_id: &str,
        docker: &DockerManager,
        grace_period: Duration,
    ) {
        info!(hostname, container_id, "Stopping Docker container");

        // docker stop sends SIGTERM and waits
        if let Err(e) = docker.stop_container(container_id, grace_period).await {
            warn!(hostname, container_id, error = %e, "Error stopping container, forcing kill");
            let _ = docker.kill_container(container_id).await;
        }

        // Remove the container
        if let Err(e) = docker.remove_container(container_id).await {
            warn!(hostname, container_id, error = %e, "Error removing container");
        }
    }

    /// Check for idle backends and stop them
    pub async fn cleanup_idle_backends(&self) {
        let mut to_stop = Vec::new();
        let defaults = self.get_defaults();

        for entry in self.processes.iter() {
            let hostname = entry.key();
            let guard = entry.value().lock();

            if guard.state != BackendState::Ready {
                continue;
            }

            let config = match self.get_config(hostname) {
                Some(c) => c,
                None => continue,
            };

            let idle_timeout = config.idle_timeout(&defaults);
            let idle_duration = guard.last_activity.elapsed();

            if idle_duration > idle_timeout {
                info!(
                    hostname,
                    idle_secs = idle_duration.as_secs(),
                    "Backend idle timeout reached"
                );
                to_stop.push(hostname.clone());
            }
        }

        for hostname in to_stop {
            self.stop_backend(&hostname).await;
        }
    }

    /// Stop all backends
    pub async fn stop_all(&self) {
        let hostnames: Vec<String> = self.processes.iter().map(|e| e.key().clone()).collect();
        for hostname in hostnames {
            self.stop_backend(&hostname).await;
        }
    }

    /// Get the port for a backend
    pub fn get_backend_port(&self, hostname: &str) -> Option<u16> {
        self.get_config(hostname).map(|c| c.port)
    }

    /// List all backends and their current status
    pub fn list_backends(&self) -> Vec<BackendStatus> {
        let configs = self.configs.read();
        configs
            .keys()
            .map(|hostname| {
                let (state, in_flight) = self
                    .processes
                    .get(hostname)
                    .map(|p| {
                        let guard = p.lock();
                        (guard.state, guard.in_flight.load(Ordering::SeqCst))
                    })
                    .unwrap_or((BackendState::Stopped, 0));

                let config = configs.get(hostname).expect("key exists");
                BackendStatus {
                    hostname: hostname.clone(),
                    state,
                    port: config.port,
                    in_flight,
                }
            })
            .collect()
    }

    /// Reload configuration from a file
    ///
    /// This updates backend configurations without restarting the proxy.
    /// - New backends are added
    /// - Removed backends are stopped gracefully
    /// - Modified backends take effect on their next restart
    ///
    /// Note: Server settings (ports, TLS, ACME) cannot be changed via hot reload.
    pub async fn reload_config<P: AsRef<Path>>(&self, path: P) -> anyhow::Result<ReloadResult> {
        let new_config = Config::load(path)?;
        self.apply_config(new_config.backends, new_config.defaults).await
    }

    /// Apply new configuration
    pub async fn apply_config(
        &self,
        new_backends: HashMap<String, BackendConfig>,
        new_defaults: BackendDefaults,
    ) -> anyhow::Result<ReloadResult> {
        let mut result = ReloadResult::default();

        // Get current backend hostnames
        let current_hostnames: Vec<String> = {
            let configs = self.configs.read();
            configs.keys().cloned().collect()
        };

        let new_hostnames: std::collections::HashSet<&String> = new_backends.keys().collect();

        // Find backends to remove (in current but not in new)
        let to_remove: Vec<String> = current_hostnames
            .iter()
            .filter(|h| !new_hostnames.contains(h))
            .cloned()
            .collect();

        // Stop removed backends
        for hostname in &to_remove {
            info!(hostname, "Removing backend (config reload)");
            self.stop_backend(hostname).await;
            result.removed.push(hostname.clone());
        }

        // Find new backends (in new but not in current)
        for hostname in new_backends.keys() {
            if !current_hostnames.contains(hostname) {
                result.added.push(hostname.clone());
                info!(hostname, "Adding backend (config reload)");
            } else {
                result.updated.push(hostname.clone());
            }
        }

        // Update configs atomically
        {
            let mut configs = self.configs.write();
            *configs = new_backends;
        }

        // Update defaults
        {
            let mut defaults = self.defaults.write();
            *defaults = new_defaults;
        }

        info!(
            added = result.added.len(),
            removed = result.removed.len(),
            updated = result.updated.len(),
            "Configuration reloaded"
        );

        Ok(result)
    }
}

/// Result of a configuration reload operation
#[derive(Debug, Clone, Default)]
pub struct ReloadResult {
    /// Newly added backends
    pub added: Vec<String>,
    /// Removed backends (stopped)
    pub removed: Vec<String>,
    /// Updated backends (config changed, takes effect on next restart)
    pub updated: Vec<String>,
}

/// Status information for a backend
#[derive(Debug, Clone)]
pub struct BackendStatus {
    /// The hostname for this backend
    pub hostname: String,
    /// Current state of the backend
    pub state: BackendState,
    /// Port the backend listens on
    pub port: u16,
    /// Number of in-flight requests
    pub in_flight: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> BackendConfig {
        BackendConfig::local("echo", 3000).with_args(vec!["hello".to_string()])
    }

    fn create_test_manager() -> Arc<ProcessManager> {
        let mut configs = HashMap::new();
        configs.insert("example.com".to_string(), create_test_config());
        let mut api_config = BackendConfig::local("echo", 4000);
        api_config.health_path = Some("/healthz".to_string());
        api_config.idle_timeout_secs = Some(120);
        configs.insert("api.example.com".to_string(), api_config);

        ProcessManager::new(
            configs,
            BackendDefaults::default(),
            "http://127.0.0.1:9999".to_string(),
        )
    }

    #[test]
    fn test_has_backend() {
        let manager = create_test_manager();

        assert!(manager.has_backend("example.com"));
        assert!(manager.has_backend("api.example.com"));
        assert!(!manager.has_backend("unknown.com"));
    }

    #[test]
    fn test_get_config() {
        let manager = create_test_manager();

        let config = manager.get_config("example.com").unwrap();
        assert_eq!(config.command, Some("echo".to_string()));
        assert_eq!(config.port, 3000);

        let config = manager.get_config("api.example.com").unwrap();
        assert_eq!(config.port, 4000);
        assert_eq!(config.health_path, Some("/healthz".to_string()));

        assert!(manager.get_config("unknown.com").is_none());
    }

    #[test]
    fn test_get_backend_port() {
        let manager = create_test_manager();

        assert_eq!(manager.get_backend_port("example.com"), Some(3000));
        assert_eq!(manager.get_backend_port("api.example.com"), Some(4000));
        assert_eq!(manager.get_backend_port("unknown.com"), None);
    }

    #[test]
    fn test_initial_state_is_stopped() {
        let manager = create_test_manager();

        assert_eq!(manager.get_state("example.com"), BackendState::Stopped);
        assert!(!manager.is_ready("example.com"));
    }

    #[test]
    fn test_subscribe_ready_returns_none_when_not_running() {
        let manager = create_test_manager();

        assert!(manager.subscribe_ready("example.com").is_none());
    }

    #[test]
    fn test_mark_ready_returns_false_when_not_starting() {
        let manager = create_test_manager();

        // Returns false when no process exists
        assert!(!manager.mark_ready("example.com"));
    }

    #[tokio::test]
    async fn test_start_backend_unknown_host() {
        let manager = create_test_manager();

        let result = manager.start_backend("unknown.com").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown backend"));
    }

    #[tokio::test]
    async fn test_start_and_stop_backend() {
        // Start the backend (using 'sleep' which exists on most systems)
        let mut configs = HashMap::new();
        let mut cfg = BackendConfig::local("sleep", 5000);
        cfg.args = vec!["60".to_string()];
        cfg.startup_timeout_secs = Some(1); // Short timeout for test
        cfg.health_check_interval_ms = Some(50);
        cfg.shutdown_grace_period_secs = Some(1);
        cfg.drain_timeout_secs = Some(1);
        configs.insert("test.com".to_string(), cfg);

        let manager = ProcessManager::new(
            configs,
            BackendDefaults::default(),
            "http://127.0.0.1:9999".to_string(),
        );

        // Start the backend
        manager.start_backend("test.com").await.unwrap();

        // Should be in Starting state
        assert_eq!(manager.get_state("test.com"), BackendState::Starting);

        // Can subscribe to ready notifications
        assert!(manager.subscribe_ready("test.com").is_some());

        // Stop the backend
        manager.stop_backend("test.com").await;

        // Should be stopped (removed from map)
        assert_eq!(manager.get_state("test.com"), BackendState::Stopped);
    }

    #[tokio::test]
    async fn test_stop_all_backends() {
        let mut configs = HashMap::new();
        let mut cfg_a = BackendConfig::local("sleep", 5001);
        cfg_a.args = vec!["60".to_string()];
        cfg_a.startup_timeout_secs = Some(1);
        cfg_a.health_check_interval_ms = Some(50);
        cfg_a.shutdown_grace_period_secs = Some(1);
        cfg_a.drain_timeout_secs = Some(1);
        configs.insert("a.com".to_string(), cfg_a);

        let mut cfg_b = BackendConfig::local("sleep", 5002);
        cfg_b.args = vec!["60".to_string()];
        cfg_b.startup_timeout_secs = Some(1);
        cfg_b.health_check_interval_ms = Some(50);
        cfg_b.shutdown_grace_period_secs = Some(1);
        cfg_b.drain_timeout_secs = Some(1);
        configs.insert("b.com".to_string(), cfg_b);

        let manager = ProcessManager::new(
            configs,
            BackendDefaults::default(),
            "http://127.0.0.1:9999".to_string(),
        );

        // Start both backends
        manager.start_backend("a.com").await.unwrap();
        manager.start_backend("b.com").await.unwrap();

        assert_eq!(manager.get_state("a.com"), BackendState::Starting);
        assert_eq!(manager.get_state("b.com"), BackendState::Starting);

        // Stop all
        manager.stop_all().await;

        assert_eq!(manager.get_state("a.com"), BackendState::Stopped);
        assert_eq!(manager.get_state("b.com"), BackendState::Stopped);
    }

    #[test]
    fn test_touch_updates_activity() {
        // This test needs a running process to work, so we'll test the mechanics
        let manager = create_test_manager();

        // Touch on non-existent process should not panic
        manager.touch("example.com");
    }

    #[test]
    fn test_backend_state_enum() {
        // Test that all states are distinct
        assert_ne!(BackendState::Stopped, BackendState::Starting);
        assert_ne!(BackendState::Starting, BackendState::Ready);
        assert_ne!(BackendState::Ready, BackendState::Unhealthy);
        assert_ne!(BackendState::Unhealthy, BackendState::Stopping);
        assert_ne!(BackendState::Stopping, BackendState::Stopped);

        // Test Debug trait
        assert_eq!(format!("{:?}", BackendState::Stopped), "Stopped");
        assert_eq!(format!("{:?}", BackendState::Starting), "Starting");
        assert_eq!(format!("{:?}", BackendState::Ready), "Ready");
        assert_eq!(format!("{:?}", BackendState::Unhealthy), "Unhealthy");
        assert_eq!(format!("{:?}", BackendState::Stopping), "Stopping");

        // Test Clone
        let state = BackendState::Ready;
        let cloned = state;
        assert_eq!(state, cloned);
    }

    #[test]
    fn test_in_flight_request_tracking_no_process() {
        let manager = create_test_manager();

        // Should return 0 when no process is running
        assert_eq!(manager.get_in_flight("example.com"), 0);

        // Increment on non-existent process returns false (no process to track)
        assert!(!manager.increment_in_flight("example.com"));
        // Decrement should not panic
        manager.decrement_in_flight("example.com");
        assert_eq!(manager.get_in_flight("example.com"), 0);
    }

    #[tokio::test]
    async fn test_in_flight_request_tracking_with_process() {
        let mut configs = HashMap::new();
        let mut cfg = BackendConfig::local("sleep", 5010);
        cfg.args = vec!["60".to_string()];
        cfg.startup_timeout_secs = Some(1);
        cfg.health_check_interval_ms = Some(50);
        cfg.shutdown_grace_period_secs = Some(1);
        cfg.drain_timeout_secs = Some(1);
        configs.insert("test.com".to_string(), cfg);

        let manager = ProcessManager::new(
            configs,
            BackendDefaults::default(),
            "http://127.0.0.1:9999".to_string(),
        );

        // Start the backend
        manager.start_backend("test.com").await.unwrap();

        // Initially no in-flight requests
        assert_eq!(manager.get_in_flight("test.com"), 0);

        // Backend is in Starting state, so increment_in_flight should return false
        assert!(!manager.increment_in_flight("test.com"));
        assert_eq!(manager.get_in_flight("test.com"), 0);

        // Manually mark as ready to test in-flight tracking
        manager.mark_ready("test.com");
        assert_eq!(manager.get_state("test.com"), BackendState::Ready);

        // Now increment should work
        assert!(manager.increment_in_flight("test.com"));
        assert_eq!(manager.get_in_flight("test.com"), 1);

        assert!(manager.increment_in_flight("test.com"));
        assert!(manager.increment_in_flight("test.com"));
        assert_eq!(manager.get_in_flight("test.com"), 3);

        // Decrement back down
        manager.decrement_in_flight("test.com");
        assert_eq!(manager.get_in_flight("test.com"), 2);

        manager.decrement_in_flight("test.com");
        manager.decrement_in_flight("test.com");
        assert_eq!(manager.get_in_flight("test.com"), 0);

        // Cleanup
        manager.stop_backend("test.com").await;
    }

    #[tokio::test]
    async fn test_graceful_shutdown_config() {
        let mut configs = HashMap::new();
        let mut cfg = BackendConfig::local("sleep", 5011);
        cfg.args = vec!["60".to_string()];
        cfg.startup_timeout_secs = Some(1);
        cfg.health_check_interval_ms = Some(50);
        cfg.shutdown_grace_period_secs = Some(2); // 2 seconds grace period
        cfg.drain_timeout_secs = Some(5);          // 5 seconds drain timeout
        configs.insert("test.com".to_string(), cfg);

        let defaults = BackendDefaults::default();
        let config = configs.get("test.com").unwrap();

        // Verify custom values are used
        assert_eq!(config.shutdown_grace_period(&defaults), Duration::from_secs(2));
        assert_eq!(config.drain_timeout(&defaults), Duration::from_secs(5));
    }
}
