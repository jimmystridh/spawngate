//! Instance manager for horizontal scaling
//!
//! Manages multiple container instances per app for horizontal scaling.

use crate::db::{Database, ProcessRecord};
use crate::docker::DockerManager;
use crate::loadbalancer::LoadBalancerManager;
use anyhow::{Context, Result};
use bollard::container::{Config, CreateContainerOptions, StartContainerOptions};
use bollard::models::{HostConfig, PortBinding};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Port range for dynamically assigned ports
const PORT_RANGE_START: u16 = 10000;
const PORT_RANGE_END: u16 = 20000;

/// Instance manager configuration
#[derive(Debug, Clone)]
pub struct InstanceConfig {
    /// Docker network to connect containers to
    pub network: String,
    /// Base URL for health check callbacks
    pub health_check_url: Option<String>,
    /// Default memory limit per instance
    pub memory_limit: Option<String>,
    /// Default CPU limit per instance
    pub cpu_limit: Option<String>,
}

impl Default for InstanceConfig {
    fn default() -> Self {
        Self {
            network: "spawngate".to_string(),
            health_check_url: None,
            memory_limit: Some("512m".to_string()),
            cpu_limit: Some("0.5".to_string()),
        }
    }
}

/// Running instance
#[derive(Debug, Clone)]
pub struct Instance {
    pub id: String,
    pub app_name: String,
    pub process_type: String,
    pub container_id: String,
    pub container_name: String,
    pub port: u16,
    pub status: InstanceStatus,
    pub started_at: String,
}

/// Result of a rolling deploy operation
#[derive(Debug, Clone, serde::Serialize)]
pub struct RollingDeployResult {
    pub app_name: String,
    pub total_instances: usize,
    pub successful: usize,
    pub failed: usize,
}

impl RollingDeployResult {
    pub fn is_success(&self) -> bool {
        self.failed == 0 && self.total_instances > 0
    }
}

/// Instance status
#[derive(Debug, Clone, PartialEq)]
pub enum InstanceStatus {
    Starting,
    Running,
    Unhealthy,
    Stopping,
    Stopped,
    Crashed,
}

impl std::fmt::Display for InstanceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstanceStatus::Starting => write!(f, "starting"),
            InstanceStatus::Running => write!(f, "running"),
            InstanceStatus::Unhealthy => write!(f, "unhealthy"),
            InstanceStatus::Stopping => write!(f, "stopping"),
            InstanceStatus::Stopped => write!(f, "stopped"),
            InstanceStatus::Crashed => write!(f, "crashed"),
        }
    }
}

/// Manages instances (processes) for applications
pub struct InstanceManager {
    docker: Arc<DockerManager>,
    db: Arc<Database>,
    config: InstanceConfig,
    /// Currently assigned ports
    assigned_ports: RwLock<HashMap<String, u16>>,
    /// Next port to try assigning
    next_port: AtomicU16,
    /// Load balancer manager for distributing requests
    load_balancer: Arc<LoadBalancerManager>,
}

impl InstanceManager {
    /// Create a new instance manager
    pub async fn new(
        docker: Arc<DockerManager>,
        db: Arc<Database>,
        config: InstanceConfig,
        load_balancer: Arc<LoadBalancerManager>,
    ) -> Result<Self> {
        Ok(Self {
            docker,
            db,
            config,
            assigned_ports: RwLock::new(HashMap::new()),
            next_port: AtomicU16::new(PORT_RANGE_START),
            load_balancer,
        })
    }

    /// Get the load balancer manager
    pub fn load_balancer(&self) -> &Arc<LoadBalancerManager> {
        &self.load_balancer
    }

    /// Get container stats for a specific container
    pub async fn get_container_stats(&self, container_id: &str) -> Result<crate::docker::ContainerStats> {
        self.docker.get_container_stats(container_id).await
    }

    /// Scale an app to the specified number of instances
    pub async fn scale(&self, app_name: &str, process_type: &str, target_count: i32) -> Result<()> {
        let app = self.db.get_app(app_name)?
            .ok_or_else(|| anyhow::anyhow!("App not found: {}", app_name))?;

        let image = app.image
            .ok_or_else(|| anyhow::anyhow!("App has no image. Deploy the app first."))?;

        // Get current processes
        let current_processes = self.db.get_app_processes(app_name)?;
        let current_count = current_processes
            .iter()
            .filter(|p| p.process_type == process_type && p.status != "stopped" && p.status != "crashed")
            .count() as i32;

        info!(
            app = app_name,
            process_type,
            current = current_count,
            target = target_count,
            "Scaling app"
        );

        if target_count > current_count {
            // Scale up - spawn new instances
            let to_spawn = target_count - current_count;
            for i in 0..to_spawn {
                info!(
                    app = app_name,
                    process_type,
                    instance = current_count + i + 1,
                    "Spawning new instance"
                );
                if let Err(e) = self.spawn_instance(app_name, &image, process_type, app.port as u16).await {
                    error!(app = app_name, error = %e, "Failed to spawn instance");
                }
            }
        } else if target_count < current_count {
            // Scale down - stop instances (newest first)
            let to_stop = current_count - target_count;
            let mut running: Vec<_> = current_processes
                .iter()
                .filter(|p| p.process_type == process_type && p.status == "running")
                .collect();

            // Sort by started_at descending (newest first)
            running.sort_by(|a, b| b.started_at.cmp(&a.started_at));

            for proc in running.iter().take(to_stop as usize) {
                info!(
                    app = app_name,
                    process_type,
                    instance_id = proc.id,
                    "Stopping instance"
                );
                if let Err(e) = self.stop_instance(&proc.id).await {
                    error!(instance_id = proc.id, error = %e, "Failed to stop instance");
                }
            }
        }

        // Update app scale in database
        self.db.update_app_scale(app_name, target_count)?;

        Ok(())
    }

    /// Spawn a single instance
    pub async fn spawn_instance(
        &self,
        app_name: &str,
        image: &str,
        process_type: &str,
        app_port: u16,
    ) -> Result<Instance> {
        let instance_id = Uuid::new_v4().to_string();
        let container_name = format!("paas-{}-{}-{}", app_name, process_type, &instance_id[..8]);

        // Allocate a port
        let host_port = self.allocate_port(&instance_id).await?;

        // Get app config (env vars)
        let config_vars = self.db.get_all_config(app_name)?;

        // Build environment variables
        let mut env: Vec<String> = config_vars
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        env.push(format!("PORT={}", app_port));
        env.push(format!("INSTANCE={}.{}", process_type, &instance_id[..8]));
        env.push(format!("INSTANCE_ID={}", instance_id));
        if let Some(ref health_url) = self.config.health_check_url {
            env.push(format!("HEALTH_CHECK_URL={}/health/{}", health_url, instance_id));
        }

        // Build port bindings - map host_port to container's app_port
        let port_key = format!("{}/tcp", app_port);
        let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
        port_bindings.insert(
            port_key.clone(),
            Some(vec![PortBinding {
                host_ip: Some("0.0.0.0".to_string()),
                host_port: Some(host_port.to_string()),
            }]),
        );

        // Build exposed ports
        let mut exposed_ports: HashMap<String, HashMap<(), ()>> = HashMap::new();
        exposed_ports.insert(port_key, HashMap::new());

        // Build host config
        let mut host_config = HostConfig {
            port_bindings: Some(port_bindings),
            network_mode: Some(self.config.network.clone()),
            ..Default::default()
        };

        // Apply resource limits
        if let Some(ref memory) = self.config.memory_limit {
            host_config.memory = Some(parse_memory_limit(memory)?);
        }
        if let Some(ref cpus) = self.config.cpu_limit {
            let cpu_count: f64 = cpus.parse()
                .map_err(|_| anyhow::anyhow!("Invalid CPU limit: {}", cpus))?;
            host_config.nano_cpus = Some((cpu_count * 1_000_000_000.0) as i64);
        }

        // Create container config
        let container_config = Config {
            image: Some(image.to_string()),
            env: Some(env),
            exposed_ports: Some(exposed_ports),
            host_config: Some(host_config),
            labels: Some(HashMap::from([
                ("paas.app".to_string(), app_name.to_string()),
                ("paas.process_type".to_string(), process_type.to_string()),
                ("paas.instance_id".to_string(), instance_id.clone()),
            ])),
            ..Default::default()
        };

        // Create container
        let create_options = CreateContainerOptions {
            name: container_name.clone(),
            platform: None,
        };

        let response = self.docker.client()
            .create_container(Some(create_options), container_config)
            .await
            .context("Failed to create container")?;

        let container_id = response.id;

        debug!(
            app = app_name,
            instance_id,
            container_id,
            container_name,
            host_port,
            "Created instance container"
        );

        // Start container
        self.docker.client()
            .start_container(&container_id, None::<StartContainerOptions<String>>)
            .await
            .context("Failed to start container")?;

        info!(
            app = app_name,
            instance_id,
            container_id,
            host_port,
            "Started instance"
        );

        // Record in database (started_at is set by database default)
        let process_record = ProcessRecord {
            id: instance_id.clone(),
            app_name: app_name.to_string(),
            process_type: process_type.to_string(),
            container_id: Some(container_id.clone()),
            container_name: Some(container_name.clone()),
            port: Some(host_port as i32),
            status: "running".to_string(),
            health_status: Some("unknown".to_string()),
            last_health_check: None,
            started_at: String::new(), // Will be set by database default
        };
        self.db.create_process(&process_record)?;

        // Register with load balancer
        self.load_balancer.add_backend(app_name, &instance_id, host_port).await;

        Ok(Instance {
            id: instance_id,
            app_name: app_name.to_string(),
            process_type: process_type.to_string(),
            container_id,
            container_name,
            port: host_port,
            status: InstanceStatus::Running,
            started_at: process_record.started_at,
        })
    }

    /// Stop an instance
    pub async fn stop_instance(&self, instance_id: &str) -> Result<()> {
        // Get process record
        let processes = self.db.get_app_processes("")?;
        let proc = processes.iter()
            .find(|p| p.id == instance_id)
            .ok_or_else(|| anyhow::anyhow!("Instance not found: {}", instance_id))?;

        let app_name = proc.app_name.clone();

        // Unregister from load balancer first (so new requests don't go to this instance)
        self.load_balancer.remove_backend(&app_name, instance_id).await;

        if let Some(ref container_id) = proc.container_id {
            // Stop the container
            self.docker.stop_container(container_id, std::time::Duration::from_secs(30)).await?;

            // Remove the container
            self.docker.remove_container(container_id).await?;
        }

        // Release port
        self.release_port(instance_id).await;

        // Update database
        self.db.update_process_status(instance_id, "stopped")?;

        info!(instance_id, "Stopped instance");

        Ok(())
    }

    /// Stop all instances for an app
    pub async fn stop_all(&self, app_name: &str) -> Result<()> {
        let processes = self.db.get_app_processes(app_name)?;

        for proc in processes {
            if proc.status != "stopped" && proc.status != "crashed" {
                if let Err(e) = self.stop_instance(&proc.id).await {
                    warn!(instance_id = proc.id, error = %e, "Failed to stop instance");
                }
            }
        }

        Ok(())
    }

    /// Restart a single instance (stop then start with same image)
    pub async fn restart_instance(&self, app_name: &str, instance_id: &str) -> Result<()> {
        let app = self.db.get_app(app_name)?
            .ok_or_else(|| anyhow::anyhow!("App not found: {}", app_name))?;

        let image = app.image.as_ref()
            .ok_or_else(|| anyhow::anyhow!("App has no image"))?;

        let processes = self.db.get_app_processes(app_name)?;
        let proc = processes.iter()
            .find(|p| p.id == instance_id)
            .ok_or_else(|| anyhow::anyhow!("Instance not found: {}", instance_id))?;

        let process_type = proc.process_type.clone();
        let app_port = app.port as u16;

        info!(app = %app_name, instance = %instance_id, "Restarting instance");

        // Stop the old instance
        self.stop_instance(instance_id).await?;

        // Spawn a new instance to replace it
        let new_instance = self.spawn_instance(app_name, image, &process_type, app_port).await?;

        info!(
            app = %app_name,
            old_instance = %instance_id,
            new_instance = %new_instance.id,
            "Instance restarted successfully"
        );

        Ok(())
    }

    /// Restart all instances for an app (for rolling deploys)
    pub async fn restart_all(&self, app_name: &str) -> Result<()> {
        self.rolling_restart(app_name, None).await?;
        Ok(())
    }

    /// Perform a rolling restart with optional new image
    ///
    /// This implements a zero-downtime rolling deploy:
    /// 1. For each old instance: spawn a new one, wait for it to be ready, then stop the old one
    /// 2. Maintains at least one running instance at all times
    pub async fn rolling_restart(&self, app_name: &str, new_image: Option<&str>) -> Result<RollingDeployResult> {
        let app = self.db.get_app(app_name)?
            .ok_or_else(|| anyhow::anyhow!("App not found: {}", app_name))?;

        let image = new_image.map(String::from)
            .or(app.image)
            .ok_or_else(|| anyhow::anyhow!("App has no image"))?;

        let processes = self.db.get_app_processes(app_name)?;
        let running: Vec<_> = processes
            .iter()
            .filter(|p| p.status == "running")
            .cloned()
            .collect();

        let total = running.len();

        if total == 0 {
            info!(app = app_name, "No running instances to restart");
            return Ok(RollingDeployResult {
                app_name: app_name.to_string(),
                total_instances: 0,
                successful: 0,
                failed: 0,
            });
        }

        info!(
            app = app_name,
            count = total,
            image = image,
            "Starting rolling deploy"
        );

        let mut successful = 0;
        let mut failed = 0;

        for (idx, proc) in running.iter().enumerate() {
            info!(
                app = app_name,
                instance = idx + 1,
                total,
                "Rolling deploy: replacing instance"
            );

            // Spawn new instance with potentially new image
            match self.spawn_instance(app_name, &image, &proc.process_type, app.port as u16).await {
                Ok(new_instance) => {
                    debug!(
                        app = app_name,
                        old_instance = proc.id,
                        new_instance = new_instance.id,
                        "New instance spawned, waiting for ready..."
                    );

                    // Wait for new instance to be ready (check if port is accepting connections)
                    let ready = self.wait_for_instance_ready(new_instance.port, 30).await;

                    if ready {
                        info!(
                            app = app_name,
                            new_instance = new_instance.id,
                            port = new_instance.port,
                            "New instance is ready"
                        );

                        // Stop old instance
                        if let Err(e) = self.stop_instance(&proc.id).await {
                            warn!(instance_id = proc.id, error = %e, "Failed to stop old instance");
                        }

                        successful += 1;
                    } else {
                        warn!(
                            app = app_name,
                            new_instance = new_instance.id,
                            "New instance failed to become ready, keeping old instance"
                        );
                        // Stop the unhealthy new instance
                        let _ = self.stop_instance(&new_instance.id).await;
                        failed += 1;
                    }
                }
                Err(e) => {
                    error!(
                        app = app_name,
                        instance = idx + 1,
                        error = %e,
                        "Failed to spawn replacement instance"
                    );
                    failed += 1;
                }
            }
        }

        info!(
            app = app_name,
            successful,
            failed,
            "Rolling deploy complete"
        );

        Ok(RollingDeployResult {
            app_name: app_name.to_string(),
            total_instances: total,
            successful,
            failed,
        })
    }

    /// Wait for an instance to become ready (accepts TCP connections)
    async fn wait_for_instance_ready(&self, port: u16, timeout_secs: u64) -> bool {
        let addr = format!("127.0.0.1:{}", port);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

        while std::time::Instant::now() < deadline {
            if tokio::net::TcpStream::connect(&addr).await.is_ok() {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }

        false
    }

    /// Get all running instances for an app
    pub async fn list_instances(&self, app_name: &str) -> Result<Vec<ProcessRecord>> {
        Ok(self.db.get_app_processes(app_name)?)
    }

    /// Get instance ports for load balancing
    pub async fn get_instance_ports(&self, app_name: &str) -> Vec<u16> {
        match self.db.get_app_processes(app_name) {
            Ok(processes) => processes
                .iter()
                .filter(|p| p.status == "running")
                .filter_map(|p| p.port.map(|port| port as u16))
                .collect(),
            Err(_) => vec![],
        }
    }

    /// Allocate a port for a new instance
    async fn allocate_port(&self, instance_id: &str) -> Result<u16> {
        let mut assigned = self.assigned_ports.write().await;

        // Find an available port
        let mut attempts = 0;
        loop {
            let port = self.next_port.fetch_add(1, Ordering::SeqCst);

            // Wrap around if we exceed the range
            if port >= PORT_RANGE_END {
                self.next_port.store(PORT_RANGE_START, Ordering::SeqCst);
            }

            // Check if port is already assigned
            if !assigned.values().any(|&p| p == port) {
                // Check if port is actually available (not used by other processes)
                if is_port_available(port) {
                    assigned.insert(instance_id.to_string(), port);
                    return Ok(port);
                }
            }

            attempts += 1;
            if attempts > (PORT_RANGE_END - PORT_RANGE_START) as usize {
                return Err(anyhow::anyhow!("No available ports in range"));
            }
        }
    }

    /// Release a port when an instance stops
    async fn release_port(&self, instance_id: &str) {
        let mut assigned = self.assigned_ports.write().await;
        assigned.remove(instance_id);
    }
}

/// Check if a port is available
fn is_port_available(port: u16) -> bool {
    std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
}

/// Parse memory limit string (e.g., "512m", "1g") to bytes
fn parse_memory_limit(limit: &str) -> Result<i64> {
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

    let num: f64 = num_str.parse()
        .map_err(|_| anyhow::anyhow!("Invalid memory limit: {}", limit))?;

    Ok((num * multiplier as f64) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instance_status_display() {
        assert_eq!(InstanceStatus::Running.to_string(), "running");
        assert_eq!(InstanceStatus::Starting.to_string(), "starting");
        assert_eq!(InstanceStatus::Crashed.to_string(), "crashed");
    }

    #[test]
    fn test_parse_memory_limit() {
        assert_eq!(parse_memory_limit("512m").unwrap(), 512 * 1024 * 1024);
        assert_eq!(parse_memory_limit("1g").unwrap(), 1024 * 1024 * 1024);
        assert!(parse_memory_limit("invalid").is_err());
    }

    #[test]
    fn test_is_port_available() {
        // High ports are usually available
        assert!(is_port_available(59999));
    }
}
