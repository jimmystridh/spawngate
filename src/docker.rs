//! Docker container management for Docker-based backends

use crate::config::{BackendConfig, PullPolicy};
use bollard::container::{
    Config, CreateContainerOptions, LogOutput, LogsOptions, RemoveContainerOptions,
    StartContainerOptions, StopContainerOptions,
};
use bollard::image::CreateImageOptions;
use bollard::models::{HostConfig, PortBinding};
use bollard::Docker;
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{debug, info, warn};

/// Manages Docker containers for backends
pub struct DockerManager {
    client: Docker,
}

impl DockerManager {
    /// Create a new DockerManager connecting to the Docker daemon
    ///
    /// Connection priority:
    /// 1. Explicit docker_host parameter
    /// 2. DOCKER_HOST environment variable
    /// 3. Common socket paths (platform-specific)
    pub async fn new(docker_host: Option<&str>) -> anyhow::Result<Self> {
        let client = if let Some(host) = docker_host {
            Self::connect_to_host(host).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to connect to Docker at '{}': {}. \
                     Ensure Docker is running and the socket path is correct.",
                    host, e
                )
            })?
        } else if let Ok(host) = std::env::var("DOCKER_HOST") {
            Self::connect_to_host(&host).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to connect to Docker via DOCKER_HOST='{}': {}. \
                     Ensure Docker is running and accessible.",
                    host, e
                )
            })?
        } else {
            Self::connect_with_defaults().await?
        };

        // Verify connection
        client.ping().await.map_err(|e| {
            anyhow::anyhow!(
                "Docker daemon is not responding: {}. \
                 Ensure Docker Desktop, Colima, or dockerd is running.",
                e
            )
        })?;

        debug!("Connected to Docker daemon");
        Ok(Self { client })
    }

    fn connect_to_host(host: &str) -> anyhow::Result<Docker> {
        if host.starts_with("unix://") {
            let socket_path = host.trim_start_matches("unix://");
            Docker::connect_with_socket(socket_path, 120, bollard::API_DEFAULT_VERSION)
                .map_err(|e| anyhow::anyhow!("Cannot connect to Unix socket '{}': {}", socket_path, e))
        } else if host.starts_with("tcp://") || host.starts_with("http://") {
            Docker::connect_with_http(host, 120, bollard::API_DEFAULT_VERSION)
                .map_err(|e| anyhow::anyhow!("Cannot connect to TCP endpoint '{}': {}", host, e))
        } else {
            anyhow::bail!(
                "Invalid docker_host format: '{}'. Expected 'unix:///path/to/socket' or 'tcp://host:port'",
                host
            )
        }
    }

    async fn connect_with_defaults() -> anyhow::Result<Docker> {
        // Try common socket paths
        let home = std::env::var("HOME").unwrap_or_default();
        let xdg_runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();

        let socket_paths: Vec<(&str, String)> = vec![
            ("Linux default", "/var/run/docker.sock".to_string()),
            ("Docker Desktop (macOS)", format!("{}/.docker/run/docker.sock", home)),
            ("Colima (macOS)", format!("{}/.colima/default/docker.sock", home)),
            ("Rancher Desktop", format!("{}/.rd/docker.sock", home)),
            ("Podman (Linux)", format!("{}/podman/podman.sock", xdg_runtime)),
        ];

        let mut tried_paths = Vec::new();

        for (name, path) in &socket_paths {
            if path.is_empty() || path.contains("//") {
                continue; // Skip invalid paths from empty env vars
            }

            if std::path::Path::new(path).exists() {
                debug!(path, name, "Found Docker socket");
                match Docker::connect_with_socket(path, 120, bollard::API_DEFAULT_VERSION) {
                    Ok(client) => {
                        // Verify this socket works
                        if client.ping().await.is_ok() {
                            return Ok(client);
                        }
                        tried_paths.push(format!("{} ({}) - socket exists but daemon not responding", path, name));
                    }
                    Err(e) => {
                        tried_paths.push(format!("{} ({}) - connection failed: {}", path, name, e));
                    }
                }
            }
        }

        // Fall back to bollard's default
        match Docker::connect_with_socket_defaults() {
            Ok(client) => Ok(client),
            Err(e) => {
                let tried_info = if tried_paths.is_empty() {
                    "No Docker socket found at common locations".to_string()
                } else {
                    format!("Tried:\n  - {}", tried_paths.join("\n  - "))
                };

                anyhow::bail!(
                    "Cannot connect to Docker daemon. {}\n\n\
                     To fix this:\n\
                     - Start Docker Desktop, Colima, or dockerd\n\
                     - Or set DOCKER_HOST environment variable\n\
                     - Or specify docker_host in the backend configuration\n\n\
                     Underlying error: {}",
                    tried_info, e
                )
            }
        }
    }

    /// Pull a Docker image if needed based on pull policy
    pub async fn pull_image_if_needed(
        &self,
        image: &str,
        policy: &PullPolicy,
    ) -> anyhow::Result<()> {
        let should_pull = match policy {
            PullPolicy::Always => true,
            PullPolicy::Never => {
                // Check if image exists, fail if not
                if self.client.inspect_image(image).await.is_err() {
                    anyhow::bail!(
                        "Image '{}' not found locally and pull_policy is 'never'. \
                         Pull the image manually with 'docker pull {}' or change pull_policy.",
                        image, image
                    );
                }
                false
            }
            PullPolicy::IfNotPresent => {
                // Check if image exists locally
                match self.client.inspect_image(image).await {
                    Ok(_) => {
                        debug!(image, "Image exists locally, skipping pull");
                        false
                    }
                    Err(_) => true,
                }
            }
        };

        if should_pull {
            info!(image, "Pulling Docker image");
            let options = CreateImageOptions {
                from_image: image,
                ..Default::default()
            };

            let mut stream = self.client.create_image(Some(options), None, None);
            let mut last_error = None;

            while let Some(result) = stream.next().await {
                match result {
                    Ok(info) => {
                        if let Some(status) = info.status {
                            debug!(image, status, "Pull progress");
                        }
                        if let Some(error) = info.error {
                            last_error = Some(error);
                        }
                    }
                    Err(e) => {
                        // Parse common Docker pull errors for better messages
                        let err_str = e.to_string();
                        if err_str.contains("manifest unknown") || err_str.contains("not found") {
                            anyhow::bail!(
                                "Image '{}' not found in registry. \
                                 Check the image name and tag are correct.",
                                image
                            );
                        } else if err_str.contains("unauthorized") || err_str.contains("authentication") {
                            anyhow::bail!(
                                "Authentication required to pull '{}'. \
                                 Run 'docker login' first or check your credentials.",
                                image
                            );
                        } else if err_str.contains("timeout") || err_str.contains("connection") {
                            anyhow::bail!(
                                "Network error pulling '{}': {}. \
                                 Check your internet connection and try again.",
                                image, e
                            );
                        } else {
                            anyhow::bail!("Failed to pull image '{}': {}", image, e);
                        }
                    }
                }
            }

            if let Some(error) = last_error {
                anyhow::bail!("Failed to pull image '{}': {}", image, error);
            }

            info!(image, "Image pulled successfully");
        }

        Ok(())
    }

    /// Start a container for a backend
    pub async fn start_container(
        &self,
        config: &BackendConfig,
        hostname: &str,
        admin_url: &str,
    ) -> anyhow::Result<String> {
        let image = config.image.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Docker backend requires 'image' field")
        })?;

        // Pull image if needed
        self.pull_image_if_needed(image, &config.pull_policy).await?;

        // Generate container name
        let container_name = config
            .container_name
            .clone()
            .unwrap_or_else(|| format!("spawngate-{}", hostname.replace('.', "-")));

        // Remove existing container with same name if it exists
        let _ = self.remove_container(&container_name).await;

        // Build environment variables
        let mut env: Vec<String> = config
            .env
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        env.push(format!("PORT={}", config.port));
        env.push(format!(
            "SERVERLESS_PROXY_READY_URL={}/ready/{}",
            admin_url, hostname
        ));

        // Build port bindings
        let port_key = format!("{}/tcp", config.port);
        let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
        port_bindings.insert(
            port_key.clone(),
            Some(vec![PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: Some(config.port.to_string()),
            }]),
        );

        // Build exposed ports
        let mut exposed_ports: HashMap<String, HashMap<(), ()>> = HashMap::new();
        exposed_ports.insert(port_key, HashMap::new());

        // Build host config
        let mut host_config = HostConfig {
            port_bindings: Some(port_bindings),
            network_mode: config.network.clone(),
            ..Default::default()
        };

        // Apply resource limits
        if let Some(ref memory) = config.memory {
            host_config.memory = Some(parse_memory_limit(memory)?);
        }
        if let Some(ref cpus) = config.cpus {
            let cpu_count: f64 = cpus.parse().map_err(|_| {
                anyhow::anyhow!("Invalid CPU limit: {}", cpus)
            })?;
            // NanoCPUs is CPUs * 1e9
            host_config.nano_cpus = Some((cpu_count * 1_000_000_000.0) as i64);
        }

        // Build command arguments if provided
        let cmd = if config.args.is_empty() {
            None
        } else {
            Some(config.args.clone())
        };

        // Create container config
        let container_config = Config {
            image: Some(image.to_string()),
            cmd,
            env: Some(env),
            exposed_ports: Some(exposed_ports),
            host_config: Some(host_config),
            ..Default::default()
        };

        // Create container
        let create_options = CreateContainerOptions {
            name: container_name.clone(),
            platform: None,
        };

        let response = self
            .client
            .create_container(Some(create_options), container_config)
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                if err_str.contains("port is already allocated") || err_str.contains("address already in use") {
                    anyhow::anyhow!(
                        "Port {} is already in use. Another container or process is using this port. \
                         Stop the conflicting service or use a different port.",
                        config.port
                    )
                } else if err_str.contains("Conflict") && err_str.contains("name") {
                    anyhow::anyhow!(
                        "Container name '{}' already exists. \
                         This shouldn't happen as we remove existing containers. \
                         Try: docker rm -f {}",
                        container_name, container_name
                    )
                } else {
                    anyhow::anyhow!(
                        "Failed to create container '{}' from image '{}': {}",
                        container_name, image, e
                    )
                }
            })?;

        let container_id = response.id;
        info!(
            hostname,
            container_id,
            container_name,
            image,
            "Created Docker container"
        );

        // Start container
        self.client
            .start_container(&container_id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                if err_str.contains("port is already allocated") || err_str.contains("address already in use") {
                    anyhow::anyhow!(
                        "Cannot start container: port {} is already in use. \
                         Stop the conflicting service or use a different port.",
                        config.port
                    )
                } else if err_str.contains("OCI runtime") || err_str.contains("executable file not found") {
                    anyhow::anyhow!(
                        "Container failed to start: the image '{}' may have an invalid entrypoint or command. \
                         Error: {}",
                        image, e
                    )
                } else if err_str.contains("no such file") || err_str.contains("not found") {
                    anyhow::anyhow!(
                        "Container failed to start: command or entrypoint not found in image '{}'. \
                         Check that the image is built correctly. Error: {}",
                        image, e
                    )
                } else {
                    anyhow::anyhow!(
                        "Failed to start container '{}' (id: {}): {}",
                        container_name, container_id, e
                    )
                }
            })?;

        info!(hostname, container_id, "Started Docker container");

        Ok(container_id)
    }

    /// Stop a container gracefully
    pub async fn stop_container(
        &self,
        container_id: &str,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let options = StopContainerOptions {
            t: timeout.as_secs() as i64,
        };

        match self.client.stop_container(container_id, Some(options)).await {
            Ok(_) => {
                info!(container_id, "Stopped Docker container");
                Ok(())
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 304, ..
            }) => {
                // Container already stopped
                debug!(container_id, "Container was already stopped");
                Ok(())
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {
                // Container doesn't exist
                debug!(container_id, "Container not found");
                Ok(())
            }
            Err(e) => Err(anyhow::anyhow!("Failed to stop container: {}", e)),
        }
    }

    /// Force kill a container
    pub async fn kill_container(&self, container_id: &str) -> anyhow::Result<()> {
        match self.client.kill_container::<String>(container_id, None).await {
            Ok(_) => {
                info!(container_id, "Killed Docker container");
                Ok(())
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {
                debug!(container_id, "Container not found");
                Ok(())
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 409, ..
            }) => {
                // Container not running
                debug!(container_id, "Container not running");
                Ok(())
            }
            Err(e) => Err(anyhow::anyhow!("Failed to kill container: {}", e)),
        }
    }

    /// Remove a container
    pub async fn remove_container(&self, container_id: &str) -> anyhow::Result<()> {
        let options = RemoveContainerOptions {
            force: true,
            ..Default::default()
        };

        match self.client.remove_container(container_id, Some(options)).await {
            Ok(_) => {
                debug!(container_id, "Removed Docker container");
                Ok(())
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {
                debug!(container_id, "Container not found");
                Ok(())
            }
            Err(e) => {
                warn!(container_id, error = %e, "Failed to remove container");
                Ok(()) // Don't fail on removal errors
            }
        }
    }

    /// Check if a container is running
    pub async fn is_running(&self, container_id: &str) -> bool {
        match self.client.inspect_container(container_id, None).await {
            Ok(info) => info
                .state
                .and_then(|s| s.running)
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    /// Stream container logs and forward them to tracing
    ///
    /// Returns a shutdown sender that can be used to stop log streaming.
    /// The spawned task will exit when the sender is dropped or when
    /// a shutdown signal is received.
    pub fn stream_logs(
        &self,
        container_id: String,
        hostname: String,
    ) -> watch::Sender<bool> {
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let client = self.client.clone();

        tokio::spawn(async move {
            let options = LogsOptions::<String> {
                follow: true,
                stdout: true,
                stderr: true,
                timestamps: false,
                ..Default::default()
            };

            let mut log_stream = client.logs(&container_id, Some(options));

            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        debug!(hostname, container_id, "Log streaming stopped");
                        break;
                    }
                    log_result = log_stream.next() => {
                        match log_result {
                            Some(Ok(output)) => {
                                match output {
                                    LogOutput::StdOut { message } => {
                                        if let Ok(line) = String::from_utf8(message.to_vec()) {
                                            let line = line.trim_end();
                                            if !line.is_empty() {
                                                info!(
                                                    target: "container",
                                                    hostname,
                                                    stream = "stdout",
                                                    "{}",
                                                    line
                                                );
                                            }
                                        }
                                    }
                                    LogOutput::StdErr { message } => {
                                        if let Ok(line) = String::from_utf8(message.to_vec()) {
                                            let line = line.trim_end();
                                            if !line.is_empty() {
                                                warn!(
                                                    target: "container",
                                                    hostname,
                                                    stream = "stderr",
                                                    "{}",
                                                    line
                                                );
                                            }
                                        }
                                    }
                                    LogOutput::Console { message } => {
                                        if let Ok(line) = String::from_utf8(message.to_vec()) {
                                            let line = line.trim_end();
                                            if !line.is_empty() {
                                                info!(
                                                    target: "container",
                                                    hostname,
                                                    stream = "console",
                                                    "{}",
                                                    line
                                                );
                                            }
                                        }
                                    }
                                    LogOutput::StdIn { .. } => {}
                                }
                            }
                            Some(Err(e)) => {
                                warn!(
                                    hostname,
                                    container_id,
                                    error = %e,
                                    "Error reading container logs"
                                );
                                break;
                            }
                            None => {
                                debug!(hostname, container_id, "Container log stream ended");
                                break;
                            }
                        }
                    }
                }
            }
        });

        shutdown_tx
    }
}

/// Parse memory limit string (e.g., "512m", "1g") to bytes
fn parse_memory_limit(limit: &str) -> anyhow::Result<i64> {
    let limit = limit.trim().to_lowercase();
    let (num_str, multiplier) = if limit.ends_with("g") || limit.ends_with("gb") {
        let num = limit.trim_end_matches("gb").trim_end_matches("g");
        (num, 1024 * 1024 * 1024i64)
    } else if limit.ends_with("m") || limit.ends_with("mb") {
        let num = limit.trim_end_matches("mb").trim_end_matches("m");
        (num, 1024 * 1024i64)
    } else if limit.ends_with("k") || limit.ends_with("kb") {
        let num = limit.trim_end_matches("kb").trim_end_matches("k");
        (num, 1024i64)
    } else {
        (limit.as_str(), 1i64)
    };

    let num: f64 = num_str.parse().map_err(|_| {
        anyhow::anyhow!("Invalid memory limit: {}", limit)
    })?;

    Ok((num * multiplier as f64) as i64)
}

/// Wrapper to share DockerManager across tasks
pub type SharedDockerManager = Arc<DockerManager>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_memory_limit() {
        assert_eq!(parse_memory_limit("512m").unwrap(), 512 * 1024 * 1024);
        assert_eq!(parse_memory_limit("1g").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_memory_limit("1G").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_memory_limit("256mb").unwrap(), 256 * 1024 * 1024);
        assert_eq!(parse_memory_limit("1024k").unwrap(), 1024 * 1024);
        assert_eq!(parse_memory_limit("1048576").unwrap(), 1048576);
        assert!(parse_memory_limit("invalid").is_err());
    }
}
