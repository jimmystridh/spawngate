//! Integration tests for horizontal scaling functionality
//!
//! Tests the InstanceManager, LoadBalancer, HealthChecker, and related API endpoints.

use spawngate::db::Database;
use spawngate::loadbalancer::LoadBalancerManager;
use std::sync::Arc;
use tempfile::TempDir;

// ============================================================================
// Database Scaling Tests
// ============================================================================

mod db_scaling_tests {
    use super::*;
    use spawngate::db::{AppRecord, ProcessRecord};

    fn create_test_db() -> (Database, TempDir) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        (db, tmp)
    }

    fn create_test_app() -> AppRecord {
        AppRecord {
            name: "test-app".to_string(),
            status: "running".to_string(),
            git_url: Some("git@localhost:test-app.git".to_string()),
            image: Some("test-app:latest".to_string()),
            port: 3000,
            created_at: String::new(),
            deployed_at: Some("2024-01-01T00:00:00Z".to_string()),
            commit_hash: Some("abc123".to_string()),
            scale: 1,
            min_scale: 0,
            max_scale: 10,
        }
    }

    #[test]
    fn test_app_scale_persistence() {
        let (db, _tmp) = create_test_db();
        let app = create_test_app();

        db.create_app(&app).unwrap();

        // Get initial scale
        let scale = db.get_app_scale("test-app").unwrap();
        assert_eq!(scale, Some(1));

        // Update scale
        db.update_app_scale("test-app", 5).unwrap();

        // Verify scale persisted
        let scale = db.get_app_scale("test-app").unwrap();
        assert_eq!(scale, Some(5));

        // Get app and verify scale field
        let retrieved = db.get_app("test-app").unwrap().unwrap();
        assert_eq!(retrieved.scale, 5);
    }

    #[test]
    fn test_process_record_lifecycle() {
        let (db, _tmp) = create_test_db();
        let app = create_test_app();
        db.create_app(&app).unwrap();

        // Create process record
        let process = ProcessRecord {
            id: "instance-1".to_string(),
            app_name: "test-app".to_string(),
            process_type: "web".to_string(),
            container_id: Some("container-abc".to_string()),
            container_name: Some("paas-test-app-web-1".to_string()),
            port: Some(10001),
            status: "running".to_string(),
            health_status: Some("healthy".to_string()),
            last_health_check: None,
            started_at: String::new(),
        };
        db.create_process(&process).unwrap();

        // Verify process exists
        let processes = db.get_app_processes("test-app").unwrap();
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].id, "instance-1");
        assert_eq!(processes[0].status, "running");

        // Update process status
        db.update_process_status("instance-1", "stopped").unwrap();

        let processes = db.get_app_processes("test-app").unwrap();
        assert_eq!(processes[0].status, "stopped");
    }

    #[test]
    fn test_multiple_processes_per_app() {
        let (db, _tmp) = create_test_db();
        let app = create_test_app();
        db.create_app(&app).unwrap();

        // Create multiple processes
        for i in 1..=3 {
            let process = ProcessRecord {
                id: format!("instance-{}", i),
                app_name: "test-app".to_string(),
                process_type: "web".to_string(),
                container_id: Some(format!("container-{}", i)),
                container_name: Some(format!("paas-test-app-web-{}", i)),
                port: Some(10000 + i),
                status: "running".to_string(),
                health_status: Some("healthy".to_string()),
                last_health_check: None,
                started_at: String::new(),
            };
            db.create_process(&process).unwrap();
        }

        // Verify all processes exist
        let processes = db.get_app_processes("test-app").unwrap();
        assert_eq!(processes.len(), 3);

        // Verify running count
        let count = db.get_running_process_count("test-app").unwrap();
        assert_eq!(count, 3);

        // Stop one process
        db.update_process_status("instance-2", "stopped").unwrap();

        // Count should reflect only running processes
        let count = db.get_running_process_count("test-app").unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_process_health_update() {
        let (db, _tmp) = create_test_db();
        let app = create_test_app();
        db.create_app(&app).unwrap();

        let process = ProcessRecord {
            id: "instance-1".to_string(),
            app_name: "test-app".to_string(),
            process_type: "web".to_string(),
            container_id: Some("container-abc".to_string()),
            container_name: None,
            port: Some(10001),
            status: "running".to_string(),
            health_status: Some("unknown".to_string()),
            last_health_check: None,
            started_at: String::new(),
        };
        db.create_process(&process).unwrap();

        // Update health status
        db.update_process_health("instance-1", "healthy").unwrap();

        let processes = db.get_app_processes("test-app").unwrap();
        assert_eq!(processes[0].health_status, Some("healthy".to_string()));
        assert!(processes[0].last_health_check.is_some());
    }

    #[test]
    fn test_delete_app_cascades_to_processes() {
        let (db, _tmp) = create_test_db();
        let app = create_test_app();
        db.create_app(&app).unwrap();

        // Create process
        let process = ProcessRecord {
            id: "instance-1".to_string(),
            app_name: "test-app".to_string(),
            process_type: "web".to_string(),
            container_id: None,
            container_name: None,
            port: Some(10001),
            status: "running".to_string(),
            health_status: None,
            last_health_check: None,
            started_at: String::new(),
        };
        db.create_process(&process).unwrap();

        // Delete app
        db.delete_app("test-app").unwrap();

        // Processes should be deleted via CASCADE
        let processes = db.get_app_processes("test-app").unwrap();
        assert!(processes.is_empty());
    }
}

// ============================================================================
// Load Balancer Tests
// ============================================================================

mod loadbalancer_tests {
    use super::*;

    #[tokio::test]
    async fn test_loadbalancer_manager_multiple_apps() {
        let manager = LoadBalancerManager::default();

        // Add backends for multiple apps
        manager.add_backend("app1", "instance-1", 10001).await;
        manager.add_backend("app1", "instance-2", 10002).await;
        manager.add_backend("app2", "instance-3", 10003).await;

        // Verify apps are tracked
        let apps = manager.list_apps().await;
        assert_eq!(apps.len(), 2);

        // Get port from app1 (should round-robin)
        let port1 = manager.get_next_port("app1").await;
        assert!(port1 == Some(10001) || port1 == Some(10002));

        let port2 = manager.get_next_port("app1").await;
        assert!(port2 == Some(10001) || port2 == Some(10002));
        assert_ne!(port1, port2); // Should alternate

        // Get port from app2
        let port3 = manager.get_next_port("app2").await;
        assert_eq!(port3, Some(10003));

        // Unknown app returns None
        let port4 = manager.get_next_port("unknown").await;
        assert_eq!(port4, None);
    }

    #[tokio::test]
    async fn test_loadbalancer_backend_removal() {
        let manager = LoadBalancerManager::default();

        manager.add_backend("app1", "instance-1", 10001).await;
        manager.add_backend("app1", "instance-2", 10002).await;

        // Both backends available
        let lb = manager.get("app1").await.unwrap();
        assert_eq!(lb.total_count().await, 2);

        // Remove one backend
        manager.remove_backend("app1", "instance-1").await;

        assert_eq!(lb.total_count().await, 1);

        // Only remaining backend should be returned
        let port = manager.get_next_port("app1").await;
        assert_eq!(port, Some(10002));
    }

    #[tokio::test]
    async fn test_loadbalancer_health_status() {
        let manager = LoadBalancerManager::default();

        manager.add_backend("app1", "instance-1", 10001).await;
        manager.add_backend("app1", "instance-2", 10002).await;

        let lb = manager.get("app1").await.unwrap();

        // Mark one backend unhealthy
        lb.set_backend_health("instance-1", false).await;

        assert_eq!(lb.healthy_count().await, 1);
        assert_eq!(lb.total_count().await, 2);

        // Only healthy backend should be returned
        for _ in 0..5 {
            let port = manager.get_next_port("app1").await;
            assert_eq!(port, Some(10002));
        }

        // Mark it healthy again
        lb.set_backend_health("instance-1", true).await;

        assert_eq!(lb.healthy_count().await, 2);
    }

    #[tokio::test]
    async fn test_loadbalancer_all_unhealthy() {
        let manager = LoadBalancerManager::default();

        manager.add_backend("app1", "instance-1", 10001).await;

        let lb = manager.get("app1").await.unwrap();
        lb.set_backend_health("instance-1", false).await;

        // No healthy backends should return None
        let port = manager.get_next_port("app1").await;
        assert_eq!(port, None);
    }

    #[tokio::test]
    async fn test_loadbalancer_get_all_ports() {
        let manager = LoadBalancerManager::default();

        manager.add_backend("app1", "instance-1", 10001).await;
        manager.add_backend("app1", "instance-2", 10002).await;
        manager.add_backend("app1", "instance-3", 10003).await;

        let lb = manager.get("app1").await.unwrap();
        let all_ports = lb.get_all_ports().await;

        assert_eq!(all_ports.len(), 3);

        let port_numbers: Vec<u16> = all_ports.iter().map(|(_, p)| *p).collect();
        assert!(port_numbers.contains(&10001));
        assert!(port_numbers.contains(&10002));
        assert!(port_numbers.contains(&10003));
    }
}

// ============================================================================
// Health Check Config Tests
// ============================================================================

mod healthcheck_tests {
    use spawngate::healthcheck::HealthCheckConfig;
    use std::time::Duration;

    #[test]
    fn test_healthcheck_config_defaults() {
        let config = HealthCheckConfig::default();

        assert_eq!(config.interval, Duration::from_secs(30));
        assert_eq!(config.timeout, Duration::from_secs(5));
        assert_eq!(config.failure_threshold, 3);
        assert_eq!(config.success_threshold, 1);
        assert_eq!(config.path, "/");
    }

    #[test]
    fn test_healthcheck_config_custom() {
        let config = HealthCheckConfig {
            interval: Duration::from_secs(10),
            timeout: Duration::from_secs(2),
            failure_threshold: 5,
            success_threshold: 2,
            path: "/health".to_string(),
        };

        assert_eq!(config.interval, Duration::from_secs(10));
        assert_eq!(config.failure_threshold, 5);
    }
}

// ============================================================================
// Instance/Rolling Deploy Result Tests
// ============================================================================

mod instance_tests {
    use spawngate::instance::RollingDeployResult;

    #[test]
    fn test_rolling_deploy_result_success() {
        let result = RollingDeployResult {
            app_name: "test-app".to_string(),
            total_instances: 3,
            successful: 3,
            failed: 0,
        };

        assert!(result.is_success());
    }

    #[test]
    fn test_rolling_deploy_result_partial_failure() {
        let result = RollingDeployResult {
            app_name: "test-app".to_string(),
            total_instances: 3,
            successful: 2,
            failed: 1,
        };

        assert!(!result.is_success());
    }

    #[test]
    fn test_rolling_deploy_result_no_instances() {
        let result = RollingDeployResult {
            app_name: "test-app".to_string(),
            total_instances: 0,
            successful: 0,
            failed: 0,
        };

        assert!(!result.is_success()); // Empty is not success
    }

    #[test]
    fn test_rolling_deploy_result_serialization() {
        let result = RollingDeployResult {
            app_name: "test-app".to_string(),
            total_instances: 2,
            successful: 2,
            failed: 0,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"app_name\":\"test-app\""));
        assert!(json.contains("\"total_instances\":2"));
        assert!(json.contains("\"successful\":2"));
        assert!(json.contains("\"failed\":0"));
    }
}

// ============================================================================
// InstanceConfig Tests
// ============================================================================

mod instance_config_tests {
    use spawngate::instance::InstanceConfig;

    #[test]
    fn test_instance_config_default() {
        let config = InstanceConfig::default();

        assert_eq!(config.network, "spawngate");
        assert!(config.health_check_url.is_none());
        assert_eq!(config.memory_limit, Some("512m".to_string()));
        assert_eq!(config.cpu_limit, Some("0.5".to_string()));
    }

    #[test]
    fn test_instance_config_custom() {
        let config = InstanceConfig {
            network: "custom-network".to_string(),
            health_check_url: Some("http://localhost:9999".to_string()),
            memory_limit: Some("1g".to_string()),
            cpu_limit: Some("1.0".to_string()),
        };

        assert_eq!(config.network, "custom-network");
        assert_eq!(config.health_check_url, Some("http://localhost:9999".to_string()));
        assert_eq!(config.memory_limit, Some("1g".to_string()));
    }
}

// ============================================================================
// Integrated Load Balancer + Database Tests
// ============================================================================

mod integrated_tests {
    use super::*;
    use spawngate::db::{AppRecord, ProcessRecord};

    #[tokio::test]
    async fn test_db_and_loadbalancer_sync() {
        // Simulate what InstanceManager does: create process record and register with LB
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        let lb_manager = Arc::new(LoadBalancerManager::default());

        // Create app
        let app = AppRecord {
            name: "myapp".to_string(),
            status: "running".to_string(),
            git_url: None,
            image: Some("myapp:v1".to_string()),
            port: 3000,
            created_at: String::new(),
            deployed_at: None,
            commit_hash: None,
            scale: 2,
            min_scale: 0,
            max_scale: 10,
        };
        db.create_app(&app).unwrap();

        // Simulate spawning 2 instances
        for i in 1..=2 {
            let instance_id = format!("instance-{}", i);
            let port = 10000 + i as i32;

            // Record in database
            let process = ProcessRecord {
                id: instance_id.clone(),
                app_name: "myapp".to_string(),
                process_type: "web".to_string(),
                container_id: Some(format!("container-{}", i)),
                container_name: None,
                port: Some(port),
                status: "running".to_string(),
                health_status: Some("healthy".to_string()),
                last_health_check: None,
                started_at: String::new(),
            };
            db.create_process(&process).unwrap();

            // Register with load balancer
            lb_manager.add_backend("myapp", &instance_id, port as u16).await;
        }

        // Verify DB state
        let processes = db.get_app_processes("myapp").unwrap();
        assert_eq!(processes.len(), 2);

        // Verify LB state
        let lb = lb_manager.get("myapp").await.unwrap();
        assert_eq!(lb.total_count().await, 2);
        assert_eq!(lb.healthy_count().await, 2);

        // Simulate stopping a instance
        lb_manager.remove_backend("myapp", "instance-1").await;
        db.update_process_status("instance-1", "stopped").unwrap();

        // LB should only route to remaining instance
        let port = lb_manager.get_next_port("myapp").await;
        assert_eq!(port, Some(10002));

        // DB should still have both records (one stopped)
        let running_count = db.get_running_process_count("myapp").unwrap();
        assert_eq!(running_count, 1);
    }
}
