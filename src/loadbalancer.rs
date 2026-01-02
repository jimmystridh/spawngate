//! Load balancer for distributing requests across multiple instances
//!
//! This module provides load balancing capabilities for the PaaS platform,
//! distributing incoming requests across multiple instance instances.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Load balancing strategy
#[derive(Debug, Clone, Copy, Default)]
pub enum LoadBalanceStrategy {
    /// Round-robin: distribute requests evenly in order
    #[default]
    RoundRobin,
    /// Random: randomly select a backend
    Random,
    /// Least connections: select the backend with fewest active connections
    LeastConnections,
}

/// Backend instance for load balancing
#[derive(Debug, Clone)]
pub struct Backend {
    /// Unique ID (instance ID)
    pub id: String,
    /// Port to connect to
    pub port: u16,
    /// Current number of active connections
    pub active_connections: usize,
    /// Whether the backend is healthy
    pub healthy: bool,
    /// Weight for weighted round-robin (1.0 = normal)
    pub weight: f32,
}

impl Backend {
    pub fn new(id: String, port: u16) -> Self {
        Self {
            id,
            port,
            active_connections: 0,
            healthy: true,
            weight: 1.0,
        }
    }
}

/// Load balancer for an application
#[derive(Debug)]
pub struct AppLoadBalancer {
    /// Application name
    app_name: String,
    /// Available backends (instance instances)
    backends: RwLock<Vec<Backend>>,
    /// Current index for round-robin
    round_robin_index: AtomicUsize,
    /// Load balancing strategy
    strategy: LoadBalanceStrategy,
}

impl AppLoadBalancer {
    pub fn new(app_name: String, strategy: LoadBalanceStrategy) -> Self {
        Self {
            app_name,
            backends: RwLock::new(Vec::new()),
            round_robin_index: AtomicUsize::new(0),
            strategy,
        }
    }

    /// Add a backend to the load balancer
    pub async fn add_backend(&self, id: String, port: u16) {
        let mut backends = self.backends.write().await;
        if !backends.iter().any(|b| b.id == id) {
            backends.push(Backend::new(id.clone(), port));
            info!(
                app = self.app_name,
                instance_id = id,
                port,
                total_backends = backends.len(),
                "Added backend to load balancer"
            );
        }
    }

    /// Remove a backend from the load balancer
    pub async fn remove_backend(&self, id: &str) {
        let mut backends = self.backends.write().await;
        if let Some(pos) = backends.iter().position(|b| b.id == id) {
            backends.remove(pos);
            info!(
                app = self.app_name,
                instance_id = id,
                total_backends = backends.len(),
                "Removed backend from load balancer"
            );
        }
    }

    /// Update backend health status
    pub async fn set_backend_health(&self, id: &str, healthy: bool) {
        let mut backends = self.backends.write().await;
        if let Some(backend) = backends.iter_mut().find(|b| b.id == id) {
            backend.healthy = healthy;
        }
    }

    /// Get the next backend port to use
    pub async fn get_next_port(&self) -> Option<u16> {
        let backends = self.backends.read().await;
        let healthy: Vec<_> = backends.iter().filter(|b| b.healthy).collect();

        if healthy.is_empty() {
            return None;
        }

        let selected = match self.strategy {
            LoadBalanceStrategy::RoundRobin => {
                let idx = self.round_robin_index.fetch_add(1, Ordering::Relaxed) % healthy.len();
                Some(healthy[idx])
            }
            LoadBalanceStrategy::Random => {
                use std::time::SystemTime;
                let seed = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as usize;
                Some(healthy[seed % healthy.len()])
            }
            LoadBalanceStrategy::LeastConnections => {
                healthy.iter().min_by_key(|b| b.active_connections).copied()
            }
        };

        selected.map(|b| {
            debug!(
                app = self.app_name,
                instance_id = b.id,
                port = b.port,
                strategy = ?self.strategy,
                "Selected backend"
            );
            b.port
        })
    }

    /// Get all backend ports (for health checks)
    pub async fn get_all_ports(&self) -> Vec<(String, u16)> {
        let backends = self.backends.read().await;
        backends.iter().map(|b| (b.id.clone(), b.port)).collect()
    }

    /// Get number of healthy backends
    pub async fn healthy_count(&self) -> usize {
        let backends = self.backends.read().await;
        backends.iter().filter(|b| b.healthy).count()
    }

    /// Get total number of backends
    pub async fn total_count(&self) -> usize {
        let backends = self.backends.read().await;
        backends.len()
    }

    /// Increment connection count for a backend
    pub async fn increment_connections(&self, port: u16) {
        let mut backends = self.backends.write().await;
        if let Some(backend) = backends.iter_mut().find(|b| b.port == port) {
            backend.active_connections += 1;
        }
    }

    /// Decrement connection count for a backend
    pub async fn decrement_connections(&self, port: u16) {
        let mut backends = self.backends.write().await;
        if let Some(backend) = backends.iter_mut().find(|b| b.port == port) {
            backend.active_connections = backend.active_connections.saturating_sub(1);
        }
    }
}

/// Global load balancer manager
pub struct LoadBalancerManager {
    /// Load balancers per app
    load_balancers: RwLock<HashMap<String, Arc<AppLoadBalancer>>>,
    /// Default strategy
    default_strategy: LoadBalanceStrategy,
}

impl LoadBalancerManager {
    pub fn new(default_strategy: LoadBalanceStrategy) -> Self {
        Self {
            load_balancers: RwLock::new(HashMap::new()),
            default_strategy,
        }
    }

    /// Get or create a load balancer for an app
    pub async fn get_or_create(&self, app_name: &str) -> Arc<AppLoadBalancer> {
        // Check if exists
        {
            let lbs = self.load_balancers.read().await;
            if let Some(lb) = lbs.get(app_name) {
                return Arc::clone(lb);
            }
        }

        // Create new
        let mut lbs = self.load_balancers.write().await;
        let lb = Arc::new(AppLoadBalancer::new(
            app_name.to_string(),
            self.default_strategy,
        ));
        lbs.insert(app_name.to_string(), Arc::clone(&lb));
        lb
    }

    /// Get a load balancer for an app if it exists
    pub async fn get(&self, app_name: &str) -> Option<Arc<AppLoadBalancer>> {
        let lbs = self.load_balancers.read().await;
        lbs.get(app_name).cloned()
    }

    /// Remove a load balancer for an app
    pub async fn remove(&self, app_name: &str) {
        let mut lbs = self.load_balancers.write().await;
        lbs.remove(app_name);
    }

    /// Get the next port for an app (convenience method)
    pub async fn get_next_port(&self, app_name: &str) -> Option<u16> {
        if let Some(lb) = self.get(app_name).await {
            lb.get_next_port().await
        } else {
            None
        }
    }

    /// Add a backend to an app's load balancer
    pub async fn add_backend(&self, app_name: &str, instance_id: &str, port: u16) {
        let lb = self.get_or_create(app_name).await;
        lb.add_backend(instance_id.to_string(), port).await;
    }

    /// Remove a backend from an app's load balancer
    pub async fn remove_backend(&self, app_name: &str, instance_id: &str) {
        if let Some(lb) = self.get(app_name).await {
            lb.remove_backend(instance_id).await;
        }
    }

    /// Get all apps with load balancers
    pub async fn list_apps(&self) -> Vec<String> {
        let lbs = self.load_balancers.read().await;
        lbs.keys().cloned().collect()
    }
}

impl Default for LoadBalancerManager {
    fn default() -> Self {
        Self::new(LoadBalanceStrategy::RoundRobin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_round_robin() {
        let lb = AppLoadBalancer::new("test".to_string(), LoadBalanceStrategy::RoundRobin);
        lb.add_backend("instance-1".to_string(), 10001).await;
        lb.add_backend("instance-2".to_string(), 10002).await;
        lb.add_backend("instance-3".to_string(), 10003).await;

        // Should cycle through backends
        assert_eq!(lb.get_next_port().await, Some(10001));
        assert_eq!(lb.get_next_port().await, Some(10002));
        assert_eq!(lb.get_next_port().await, Some(10003));
        assert_eq!(lb.get_next_port().await, Some(10001)); // wraps around
    }

    #[tokio::test]
    async fn test_unhealthy_backend_skipped() {
        let lb = AppLoadBalancer::new("test".to_string(), LoadBalanceStrategy::RoundRobin);
        lb.add_backend("instance-1".to_string(), 10001).await;
        lb.add_backend("instance-2".to_string(), 10002).await;

        // Mark one as unhealthy
        lb.set_backend_health("instance-1", false).await;

        // Should only return healthy backend
        assert_eq!(lb.get_next_port().await, Some(10002));
        assert_eq!(lb.get_next_port().await, Some(10002));
    }

    #[tokio::test]
    async fn test_no_healthy_backends() {
        let lb = AppLoadBalancer::new("test".to_string(), LoadBalanceStrategy::RoundRobin);
        lb.add_backend("instance-1".to_string(), 10001).await;
        lb.set_backend_health("instance-1", false).await;

        assert_eq!(lb.get_next_port().await, None);
    }

    #[tokio::test]
    async fn test_remove_backend() {
        let lb = AppLoadBalancer::new("test".to_string(), LoadBalanceStrategy::RoundRobin);
        lb.add_backend("instance-1".to_string(), 10001).await;
        lb.add_backend("instance-2".to_string(), 10002).await;

        assert_eq!(lb.total_count().await, 2);

        lb.remove_backend("instance-1").await;
        assert_eq!(lb.total_count().await, 1);
        assert_eq!(lb.get_next_port().await, Some(10002));
    }

    #[tokio::test]
    async fn test_load_balancer_manager() {
        let manager = LoadBalancerManager::default();

        manager.add_backend("app1", "instance-1", 10001).await;
        manager.add_backend("app1", "instance-2", 10002).await;
        manager.add_backend("app2", "instance-3", 10003).await;

        assert_eq!(manager.list_apps().await.len(), 2);

        // Round-robin for app1
        assert_eq!(manager.get_next_port("app1").await, Some(10001));
        assert_eq!(manager.get_next_port("app1").await, Some(10002));

        // Single backend for app2
        assert_eq!(manager.get_next_port("app2").await, Some(10003));

        // Unknown app
        assert_eq!(manager.get_next_port("unknown").await, None);
    }
}
