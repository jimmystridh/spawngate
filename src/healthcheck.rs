//! Health check system for instance instances
//!
//! Periodically checks the health of running instances and updates their status.

use crate::db::Database;
use crate::loadbalancer::LoadBalancerManager;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

/// Health check configuration
#[derive(Debug, Clone)]
pub struct HealthCheckConfig {
    /// Interval between health checks
    pub interval: Duration,
    /// Timeout for each health check request
    pub timeout: Duration,
    /// Number of consecutive failures before marking unhealthy
    pub failure_threshold: u32,
    /// Number of consecutive successes before marking healthy
    pub success_threshold: u32,
    /// Path for HTTP health check (if using HTTP)
    pub path: String,
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
            timeout: Duration::from_secs(5),
            failure_threshold: 3,
            success_threshold: 1,
            path: "/".to_string(),
        }
    }
}

/// Tracks consecutive health check results for an instance
struct InstanceHealthState {
    consecutive_failures: u32,
    consecutive_successes: u32,
    is_healthy: bool,
}

impl Default for InstanceHealthState {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            consecutive_successes: 0,
            is_healthy: true, // Assume healthy initially
        }
    }
}

/// Health checker that monitors instances
pub struct HealthChecker {
    db: Arc<Database>,
    load_balancer: Arc<LoadBalancerManager>,
    config: HealthCheckConfig,
    shutdown_rx: watch::Receiver<bool>,
}

impl HealthChecker {
    pub fn new(
        db: Arc<Database>,
        load_balancer: Arc<LoadBalancerManager>,
        config: HealthCheckConfig,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Self {
        Self {
            db,
            load_balancer,
            config,
            shutdown_rx,
        }
    }

    /// Run the health checker
    pub async fn run(mut self) {
        info!(
            interval_secs = self.config.interval.as_secs(),
            "Health checker started"
        );

        let mut health_states: std::collections::HashMap<String, InstanceHealthState> =
            std::collections::HashMap::new();

        loop {
            tokio::select! {
                _ = tokio::time::sleep(self.config.interval) => {
                    self.check_all_instances(&mut health_states).await;
                }
                _ = self.shutdown_rx.changed() => {
                    if *self.shutdown_rx.borrow() {
                        info!("Health checker shutting down");
                        break;
                    }
                }
            }
        }
    }

    /// Check health of all running instances
    async fn check_all_instances(
        &self,
        health_states: &mut std::collections::HashMap<String, InstanceHealthState>,
    ) {
        // Get all apps with load balancers
        let apps = self.load_balancer.list_apps().await;

        for app_name in apps {
            if let Some(lb) = self.load_balancer.get(&app_name).await {
                let backends = lb.get_all_ports().await;

                for (instance_id, port) in backends {
                    let is_healthy = self.check_instance_health(port).await;
                    let state = health_states.entry(instance_id.clone()).or_default();

                    if is_healthy {
                        state.consecutive_successes += 1;
                        state.consecutive_failures = 0;

                        // Check if should transition to healthy
                        if !state.is_healthy
                            && state.consecutive_successes >= self.config.success_threshold
                        {
                            state.is_healthy = true;
                            info!(
                                app = app_name,
                                instance_id,
                                port,
                                "Instance is now healthy"
                            );
                            lb.set_backend_health(&instance_id, true).await;
                            if let Err(e) = self.db.update_process_health(&instance_id, "healthy") {
                                error!(error = %e, "Failed to update process health in database");
                            }
                        }
                    } else {
                        state.consecutive_failures += 1;
                        state.consecutive_successes = 0;

                        // Check if should transition to unhealthy
                        if state.is_healthy
                            && state.consecutive_failures >= self.config.failure_threshold
                        {
                            state.is_healthy = false;
                            warn!(
                                app = app_name,
                                instance_id,
                                port,
                                failures = state.consecutive_failures,
                                "Instance is now unhealthy"
                            );
                            lb.set_backend_health(&instance_id, false).await;
                            if let Err(e) = self.db.update_process_health(&instance_id, "unhealthy") {
                                error!(error = %e, "Failed to update process health in database");
                            }
                        }
                    }
                }
            }
        }

        // Clean up health states for instances that no longer exist
        let all_instance_ids: std::collections::HashSet<String> = {
            let mut ids = std::collections::HashSet::new();
            for app_name in self.load_balancer.list_apps().await {
                if let Some(lb) = self.load_balancer.get(&app_name).await {
                    for (instance_id, _) in lb.get_all_ports().await {
                        ids.insert(instance_id);
                    }
                }
            }
            ids
        };

        health_states.retain(|instance_id, _| all_instance_ids.contains(instance_id));
    }

    /// Check if a instance is healthy by testing TCP connection
    async fn check_instance_health(&self, port: u16) -> bool {
        let addr = format!("127.0.0.1:{}", port);

        // Try to establish a TCP connection
        match tokio::time::timeout(self.config.timeout, TcpStream::connect(&addr)).await {
            Ok(Ok(_stream)) => {
                debug!(port, "Health check passed (TCP connect)");
                true
            }
            Ok(Err(e)) => {
                debug!(port, error = %e, "Health check failed (connection error)");
                false
            }
            Err(_) => {
                debug!(port, "Health check failed (timeout)");
                false
            }
        }
    }
}

/// Builder for configuring and running the health checker
pub struct HealthCheckerBuilder {
    db: Option<Arc<Database>>,
    load_balancer: Option<Arc<LoadBalancerManager>>,
    config: HealthCheckConfig,
    shutdown_rx: Option<watch::Receiver<bool>>,
}

impl HealthCheckerBuilder {
    pub fn new() -> Self {
        Self {
            db: None,
            load_balancer: None,
            config: HealthCheckConfig::default(),
            shutdown_rx: None,
        }
    }

    pub fn database(mut self, db: Arc<Database>) -> Self {
        self.db = Some(db);
        self
    }

    pub fn load_balancer(mut self, lb: Arc<LoadBalancerManager>) -> Self {
        self.load_balancer = Some(lb);
        self
    }

    pub fn config(mut self, config: HealthCheckConfig) -> Self {
        self.config = config;
        self
    }

    pub fn shutdown_receiver(mut self, rx: watch::Receiver<bool>) -> Self {
        self.shutdown_rx = Some(rx);
        self
    }

    pub fn build(self) -> Option<HealthChecker> {
        Some(HealthChecker::new(
            self.db?,
            self.load_balancer?,
            self.config,
            self.shutdown_rx?,
        ))
    }
}

impl Default for HealthCheckerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HealthCheckConfig::default();
        assert_eq!(config.interval, Duration::from_secs(30));
        assert_eq!(config.timeout, Duration::from_secs(5));
        assert_eq!(config.failure_threshold, 3);
        assert_eq!(config.success_threshold, 1);
    }

    #[test]
    fn test_health_state_transitions() {
        let mut state = InstanceHealthState::default();
        assert!(state.is_healthy);

        // Simulate failures
        for _ in 0..3 {
            state.consecutive_failures += 1;
        }
        assert_eq!(state.consecutive_failures, 3);

        // Mark as unhealthy
        state.is_healthy = false;

        // Simulate recovery
        state.consecutive_failures = 0;
        state.consecutive_successes = 1;
        state.is_healthy = true;
        assert!(state.is_healthy);
    }
}
