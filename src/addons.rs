//! Add-on provisioning system for managed services (PostgreSQL, Redis, MinIO/S3)
//!
//! This module provides automatic provisioning and lifecycle management for
//! common backing services that applications need.

use crate::config::PullPolicy;
use crate::docker::DockerManager;
use anyhow::{Context, Result};
use bollard::container::{Config as ContainerConfig, CreateContainerOptions};
use bollard::models::{HostConfig, PortBinding};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Add-on types supported by the platform
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AddonType {
    Postgres,
    Redis,
    Storage, // S3-compatible (MinIO)
}

impl std::fmt::Display for AddonType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddonType::Postgres => write!(f, "postgres"),
            AddonType::Redis => write!(f, "redis"),
            AddonType::Storage => write!(f, "storage"),
        }
    }
}

/// Plan tier for add-ons (affects resource limits)
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AddonPlan {
    /// Minimal resources, good for development
    #[default]
    Hobby,
    /// Small production workloads
    Basic,
    /// Standard production workloads
    Standard,
    /// High-performance workloads
    Premium,
}

impl AddonPlan {
    /// Memory limit for this plan
    pub fn memory_limit(&self) -> &'static str {
        match self {
            AddonPlan::Hobby => "256m",
            AddonPlan::Basic => "512m",
            AddonPlan::Standard => "1g",
            AddonPlan::Premium => "2g",
        }
    }

    /// CPU limit for this plan
    pub fn cpu_limit(&self) -> f64 {
        match self {
            AddonPlan::Hobby => 0.25,
            AddonPlan::Basic => 0.5,
            AddonPlan::Standard => 1.0,
            AddonPlan::Premium => 2.0,
        }
    }
}

/// Configuration for a single add-on instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddonConfig {
    /// Add-on type
    #[serde(rename = "type")]
    pub addon_type: AddonType,

    /// Plan tier
    #[serde(default)]
    pub plan: AddonPlan,

    /// Custom name/bucket (for storage)
    pub name: Option<String>,

    /// Docker network to connect to
    pub network: Option<String>,
}

/// Runtime state of a provisioned add-on
#[derive(Debug, Clone, Serialize)]
pub struct AddonInstance {
    /// Unique identifier for this add-on instance
    pub id: String,

    /// Add-on type
    pub addon_type: AddonType,

    /// Plan tier
    pub plan: AddonPlan,

    /// App this add-on belongs to
    pub app_name: String,

    /// Docker container ID
    pub container_id: Option<String>,

    /// Container name
    pub container_name: String,

    /// Connection URL (to be injected as env var)
    pub connection_url: String,

    /// Environment variable name for the connection URL
    pub env_var_name: String,

    /// Additional environment variables
    pub env_vars: HashMap<String, String>,

    /// Current status
    pub status: AddonStatus,
}

/// Add-on lifecycle status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AddonStatus {
    /// Add-on is being provisioned
    Provisioning,
    /// Add-on is running and available
    Running,
    /// Add-on is stopping
    Stopping,
    /// Add-on is stopped
    Stopped,
    /// Add-on failed to provision
    Failed,
}

/// Manages add-on lifecycle
pub struct AddonManager {
    /// Docker manager for container operations
    docker: Arc<DockerManager>,

    /// Active add-on instances keyed by "{app_name}:{addon_type}"
    instances: Arc<RwLock<HashMap<String, AddonInstance>>>,

    /// Network name for add-ons (all add-ons and apps share this network)
    network_name: String,

    /// Port allocator starting port
    next_port: Arc<RwLock<u16>>,
}

impl AddonManager {
    /// Create a new add-on manager
    pub async fn new(docker_host: Option<&str>, network_name: Option<&str>) -> Result<Self> {
        let docker = DockerManager::new(docker_host).await?;

        let network = network_name.unwrap_or("spawngate").to_string();

        // Ensure the network exists
        Self::ensure_network(&docker, &network).await?;

        Ok(Self {
            docker: Arc::new(docker),
            instances: Arc::new(RwLock::new(HashMap::new())),
            network_name: network,
            next_port: Arc::new(RwLock::new(15432)), // Start from 15432
        })
    }

    /// Ensure the Docker network exists
    async fn ensure_network(docker: &DockerManager, network_name: &str) -> Result<()> {
        use bollard::network::CreateNetworkOptions;

        let client = docker.client();

        // Check if network exists
        match client.inspect_network::<String>(network_name, None).await {
            Ok(_) => {
                debug!("Network '{}' already exists", network_name);
                return Ok(());
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {
                // Network doesn't exist, create it
            }
            Err(e) => return Err(e.into()),
        }

        info!("Creating Docker network '{}'", network_name);
        client
            .create_network(CreateNetworkOptions {
                name: network_name,
                driver: "bridge",
                ..Default::default()
            })
            .await
            .context("Failed to create Docker network")?;

        Ok(())
    }

    /// Allocate the next available port
    async fn allocate_port(&self) -> u16 {
        let mut port = self.next_port.write().await;
        let allocated = *port;
        *port += 1;
        allocated
    }

    /// Get instance key
    fn instance_key(app_name: &str, addon_type: &AddonType) -> String {
        format!("{}:{}", app_name, addon_type)
    }

    /// Provision an add-on for an app
    pub async fn provision(
        &self,
        app_name: &str,
        config: &AddonConfig,
    ) -> Result<AddonInstance> {
        let key = Self::instance_key(app_name, &config.addon_type);

        // Check if already provisioned
        {
            let instances = self.instances.read().await;
            if let Some(existing) = instances.get(&key) {
                if existing.status == AddonStatus::Running {
                    info!(
                        "Add-on {} already provisioned for app {}",
                        config.addon_type, app_name
                    );
                    return Ok(existing.clone());
                }
            }
        }

        info!(
            "Provisioning {} ({:?}) for app {}",
            config.addon_type, config.plan, app_name
        );

        let instance = match config.addon_type {
            AddonType::Postgres => self.provision_postgres(app_name, config).await?,
            AddonType::Redis => self.provision_redis(app_name, config).await?,
            AddonType::Storage => self.provision_storage(app_name, config).await?,
        };

        // Store the instance
        {
            let mut instances = self.instances.write().await;
            instances.insert(key, instance.clone());
        }

        Ok(instance)
    }

    /// Provision a PostgreSQL database
    async fn provision_postgres(&self, app_name: &str, config: &AddonConfig) -> Result<AddonInstance> {
        let container_name = format!("spawngate-postgres-{}", app_name);
        let db_name = app_name.replace('-', "_").replace('.', "_");
        let db_user = format!("{}_user", db_name);
        let db_password = generate_password();
        let port = self.allocate_port().await;

        // Create container config
        let env = vec![
            format!("POSTGRES_DB={}", db_name),
            format!("POSTGRES_USER={}", db_user),
            format!("POSTGRES_PASSWORD={}", db_password),
        ];

        let image = "postgres:16-alpine";
        let memory = parse_memory_limit(config.plan.memory_limit());
        let nano_cpus = (config.plan.cpu_limit() * 1_000_000_000.0) as i64;

        let mut port_bindings = HashMap::new();
        port_bindings.insert(
            "5432/tcp".to_string(),
            Some(vec![PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: Some(port.to_string()),
            }]),
        );

        let host_config = HostConfig {
            memory: Some(memory),
            nano_cpus: Some(nano_cpus),
            port_bindings: Some(port_bindings),
            network_mode: Some(self.network_name.clone()),
            restart_policy: Some(bollard::models::RestartPolicy {
                name: Some(bollard::models::RestartPolicyNameEnum::UNLESS_STOPPED),
                maximum_retry_count: None,
            }),
            ..Default::default()
        };

        let container_config = ContainerConfig {
            image: Some(image.to_string()),
            env: Some(env.clone()),
            host_config: Some(host_config),
            ..Default::default()
        };

        // Pull image if needed
        self.docker.pull_image_if_needed(image, &PullPolicy::IfNotPresent).await?;

        // Create and start container
        let client = self.docker.client();
        let create_opts = CreateContainerOptions {
            name: &container_name,
            platform: None,
        };

        let response = client
            .create_container(Some(create_opts), container_config)
            .await
            .context("Failed to create PostgreSQL container")?;

        client
            .start_container::<String>(&response.id, None)
            .await
            .context("Failed to start PostgreSQL container")?;

        info!("PostgreSQL container started: {}", container_name);

        // Connection URL using container name (for internal network access)
        let internal_url = format!(
            "postgres://{}:{}@{}:5432/{}",
            db_user, db_password, container_name, db_name
        );

        // Also provide external URL for tools running outside Docker
        let external_url = format!(
            "postgres://{}:{}@127.0.0.1:{}/{}",
            db_user, db_password, port, db_name
        );

        let mut env_vars = HashMap::new();
        env_vars.insert("PGHOST".to_string(), container_name.clone());
        env_vars.insert("PGPORT".to_string(), "5432".to_string());
        env_vars.insert("PGDATABASE".to_string(), db_name.clone());
        env_vars.insert("PGUSER".to_string(), db_user.clone());
        env_vars.insert("PGPASSWORD".to_string(), db_password.clone());
        env_vars.insert("DATABASE_URL_EXTERNAL".to_string(), external_url);

        Ok(AddonInstance {
            id: response.id.clone(),
            addon_type: AddonType::Postgres,
            plan: config.plan.clone(),
            app_name: app_name.to_string(),
            container_id: Some(response.id),
            container_name,
            connection_url: internal_url,
            env_var_name: "DATABASE_URL".to_string(),
            env_vars,
            status: AddonStatus::Running,
        })
    }

    /// Provision a Redis instance
    async fn provision_redis(&self, app_name: &str, config: &AddonConfig) -> Result<AddonInstance> {
        let container_name = format!("spawngate-redis-{}", app_name);
        let password = generate_password();
        let port = self.allocate_port().await;

        let image = "redis:7-alpine";
        let memory = parse_memory_limit(config.plan.memory_limit());
        let nano_cpus = (config.plan.cpu_limit() * 1_000_000_000.0) as i64;

        let mut port_bindings = HashMap::new();
        port_bindings.insert(
            "6379/tcp".to_string(),
            Some(vec![PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: Some(port.to_string()),
            }]),
        );

        let host_config = HostConfig {
            memory: Some(memory),
            nano_cpus: Some(nano_cpus),
            port_bindings: Some(port_bindings),
            network_mode: Some(self.network_name.clone()),
            restart_policy: Some(bollard::models::RestartPolicy {
                name: Some(bollard::models::RestartPolicyNameEnum::UNLESS_STOPPED),
                maximum_retry_count: None,
            }),
            ..Default::default()
        };

        let cmd = vec!["redis-server", "--requirepass", &password];

        let container_config = ContainerConfig {
            image: Some(image.to_string()),
            cmd: Some(cmd.iter().map(|s| s.to_string()).collect()),
            host_config: Some(host_config),
            ..Default::default()
        };

        // Pull image if needed
        self.docker.pull_image_if_needed(image, &PullPolicy::IfNotPresent).await?;

        // Create and start container
        let client = self.docker.client();
        let create_opts = CreateContainerOptions {
            name: &container_name,
            platform: None,
        };

        let response = client
            .create_container(Some(create_opts), container_config)
            .await
            .context("Failed to create Redis container")?;

        client
            .start_container::<String>(&response.id, None)
            .await
            .context("Failed to start Redis container")?;

        info!("Redis container started: {}", container_name);

        let internal_url = format!("redis://:{}@{}:6379", password, container_name);
        let external_url = format!("redis://:{}@127.0.0.1:{}", password, port);

        let mut env_vars = HashMap::new();
        env_vars.insert("REDIS_HOST".to_string(), container_name.clone());
        env_vars.insert("REDIS_PORT".to_string(), "6379".to_string());
        env_vars.insert("REDIS_PASSWORD".to_string(), password);
        env_vars.insert("REDIS_URL_EXTERNAL".to_string(), external_url);

        Ok(AddonInstance {
            id: response.id.clone(),
            addon_type: AddonType::Redis,
            plan: config.plan.clone(),
            app_name: app_name.to_string(),
            container_id: Some(response.id),
            container_name,
            connection_url: internal_url,
            env_var_name: "REDIS_URL".to_string(),
            env_vars,
            status: AddonStatus::Running,
        })
    }

    /// Provision MinIO (S3-compatible storage)
    async fn provision_storage(&self, app_name: &str, config: &AddonConfig) -> Result<AddonInstance> {
        let container_name = format!("spawngate-minio-{}", app_name);
        let bucket_name = config
            .name
            .clone()
            .unwrap_or_else(|| format!("{}-uploads", app_name.replace('.', "-")));
        let access_key = generate_access_key();
        let secret_key = generate_password();
        let api_port = self.allocate_port().await;
        let console_port = self.allocate_port().await;

        let image = "minio/minio:latest";
        let memory = parse_memory_limit(config.plan.memory_limit());
        let nano_cpus = (config.plan.cpu_limit() * 1_000_000_000.0) as i64;

        let mut port_bindings = HashMap::new();
        port_bindings.insert(
            "9000/tcp".to_string(),
            Some(vec![PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: Some(api_port.to_string()),
            }]),
        );
        port_bindings.insert(
            "9001/tcp".to_string(),
            Some(vec![PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: Some(console_port.to_string()),
            }]),
        );

        let env = vec![
            format!("MINIO_ROOT_USER={}", access_key),
            format!("MINIO_ROOT_PASSWORD={}", secret_key),
        ];

        let host_config = HostConfig {
            memory: Some(memory),
            nano_cpus: Some(nano_cpus),
            port_bindings: Some(port_bindings),
            network_mode: Some(self.network_name.clone()),
            restart_policy: Some(bollard::models::RestartPolicy {
                name: Some(bollard::models::RestartPolicyNameEnum::UNLESS_STOPPED),
                maximum_retry_count: None,
            }),
            ..Default::default()
        };

        let cmd = vec!["server", "/data", "--console-address", ":9001"];

        let container_config = ContainerConfig {
            image: Some(image.to_string()),
            cmd: Some(cmd.iter().map(|s| s.to_string()).collect()),
            env: Some(env.clone()),
            host_config: Some(host_config),
            ..Default::default()
        };

        // Pull image if needed
        self.docker.pull_image_if_needed(image, &PullPolicy::IfNotPresent).await?;

        // Create and start container
        let client = self.docker.client();
        let create_opts = CreateContainerOptions {
            name: &container_name,
            platform: None,
        };

        let response = client
            .create_container(Some(create_opts), container_config)
            .await
            .context("Failed to create MinIO container")?;

        client
            .start_container::<String>(&response.id, None)
            .await
            .context("Failed to start MinIO container")?;

        info!("MinIO container started: {}", container_name);

        // Create bucket after container starts
        // (In production, would wait for health check and use mc client)
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let endpoint_internal = format!("http://{}:9000", container_name);
        let endpoint_external = format!("http://127.0.0.1:{}", api_port);

        let mut env_vars = HashMap::new();
        env_vars.insert("S3_ENDPOINT".to_string(), endpoint_internal.clone());
        env_vars.insert("S3_ENDPOINT_EXTERNAL".to_string(), endpoint_external);
        env_vars.insert("S3_ACCESS_KEY".to_string(), access_key.clone());
        env_vars.insert("S3_SECRET_KEY".to_string(), secret_key.clone());
        env_vars.insert("S3_BUCKET".to_string(), bucket_name.clone());
        env_vars.insert("S3_REGION".to_string(), "us-east-1".to_string());
        env_vars.insert("MINIO_CONSOLE_PORT".to_string(), console_port.to_string());

        // AWS SDK compatible vars
        env_vars.insert("AWS_ACCESS_KEY_ID".to_string(), access_key);
        env_vars.insert("AWS_SECRET_ACCESS_KEY".to_string(), secret_key);
        env_vars.insert("AWS_ENDPOINT_URL".to_string(), endpoint_internal.clone());
        env_vars.insert("AWS_DEFAULT_REGION".to_string(), "us-east-1".to_string());

        Ok(AddonInstance {
            id: response.id.clone(),
            addon_type: AddonType::Storage,
            plan: config.plan.clone(),
            app_name: app_name.to_string(),
            container_id: Some(response.id),
            container_name,
            connection_url: endpoint_internal,
            env_var_name: "S3_ENDPOINT".to_string(),
            env_vars,
            status: AddonStatus::Running,
        })
    }

    /// Deprovision an add-on
    pub async fn deprovision(&self, app_name: &str, addon_type: &AddonType) -> Result<()> {
        let key = Self::instance_key(app_name, addon_type);

        let instance = {
            let mut instances = self.instances.write().await;
            instances.remove(&key)
        };

        if let Some(instance) = instance {
            if let Some(container_id) = &instance.container_id {
                info!("Stopping add-on container: {}", instance.container_name);

                let client = self.docker.client();

                // Stop the container
                if let Err(e) = client
                    .stop_container(container_id, None)
                    .await
                {
                    warn!("Failed to stop container {}: {}", container_id, e);
                }

                // Remove the container
                if let Err(e) = client
                    .remove_container(
                        container_id,
                        Some(bollard::container::RemoveContainerOptions {
                            force: true,
                            ..Default::default()
                        }),
                    )
                    .await
                {
                    warn!("Failed to remove container {}: {}", container_id, e);
                }

                info!("Add-on {} deprovisioned for app {}", addon_type, app_name);
            }
        }

        Ok(())
    }

    /// Deprovision all add-ons for an app
    pub async fn deprovision_all(&self, app_name: &str) -> Result<()> {
        let addon_types = vec![AddonType::Postgres, AddonType::Redis, AddonType::Storage];

        for addon_type in addon_types {
            self.deprovision(app_name, &addon_type).await?;
        }

        Ok(())
    }

    /// Get environment variables for an app's add-ons
    pub async fn get_env_vars(&self, app_name: &str) -> HashMap<String, String> {
        let instances = self.instances.read().await;
        let mut env_vars = HashMap::new();

        for addon_type in [AddonType::Postgres, AddonType::Redis, AddonType::Storage] {
            let key = Self::instance_key(app_name, &addon_type);
            if let Some(instance) = instances.get(&key) {
                // Add the main connection URL
                env_vars.insert(instance.env_var_name.clone(), instance.connection_url.clone());

                // Add additional env vars
                for (k, v) in &instance.env_vars {
                    env_vars.insert(k.clone(), v.clone());
                }
            }
        }

        env_vars
    }

    /// Get status of all add-ons for an app
    pub async fn get_app_addons(&self, app_name: &str) -> Vec<AddonInstance> {
        let instances = self.instances.read().await;
        instances
            .values()
            .filter(|i| i.app_name == app_name)
            .cloned()
            .collect()
    }

    /// Get status of all add-ons
    pub async fn list_all(&self) -> Vec<AddonInstance> {
        let instances = self.instances.read().await;
        instances.values().cloned().collect()
    }
}

/// Parse memory limit string (e.g., "512m", "1g") to bytes
fn parse_memory_limit(s: &str) -> i64 {
    let s = s.to_lowercase();
    if let Some(num) = s.strip_suffix('g') {
        num.parse::<i64>().unwrap_or(512) * 1024 * 1024 * 1024
    } else if let Some(num) = s.strip_suffix('m') {
        num.parse::<i64>().unwrap_or(512) * 1024 * 1024
    } else if let Some(num) = s.strip_suffix('k') {
        num.parse::<i64>().unwrap_or(512) * 1024
    } else {
        s.parse::<i64>().unwrap_or(512 * 1024 * 1024)
    }
}

/// Generate a random password
fn generate_password() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{:x}", timestamp)
}

/// Generate a random access key (for MinIO)
fn generate_access_key() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("AKIA{:X}", timestamp % 0xFFFFFFFFFFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_memory_limit() {
        assert_eq!(parse_memory_limit("512m"), 512 * 1024 * 1024);
        assert_eq!(parse_memory_limit("1g"), 1024 * 1024 * 1024);
        assert_eq!(parse_memory_limit("256M"), 256 * 1024 * 1024);
        assert_eq!(parse_memory_limit("2G"), 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_addon_plan_resources() {
        assert_eq!(AddonPlan::Hobby.memory_limit(), "256m");
        assert_eq!(AddonPlan::Basic.memory_limit(), "512m");
        assert_eq!(AddonPlan::Standard.memory_limit(), "1g");
        assert_eq!(AddonPlan::Premium.memory_limit(), "2g");

        assert_eq!(AddonPlan::Hobby.cpu_limit(), 0.25);
        assert_eq!(AddonPlan::Basic.cpu_limit(), 0.5);
        assert_eq!(AddonPlan::Standard.cpu_limit(), 1.0);
        assert_eq!(AddonPlan::Premium.cpu_limit(), 2.0);
    }

    #[test]
    fn test_instance_key() {
        assert_eq!(
            AddonManager::instance_key("myapp", &AddonType::Postgres),
            "myapp:postgres"
        );
        assert_eq!(
            AddonManager::instance_key("api.example.com", &AddonType::Redis),
            "api.example.com:redis"
        );
    }
}
