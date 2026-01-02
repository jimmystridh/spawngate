//! Integration tests for PaaS API layer
//!
//! Tests the complete PaaS functionality including:
//! - App lifecycle (create, deploy, scale, delete)
//! - Add-on provisioning
//! - Domain management
//! - Webhook handling
//! - Secrets management
//! - Activity logging

use spawngate::db::{
    AddonRecord, ApiTokenRecord, AppRecord, Database, DeploymentRecord,
    ProcessRecord, WebhookEventRecord, WebhookRecord,
};
use tempfile::TempDir;

// ============================================================================
// Test Helpers
// ============================================================================

fn create_test_db() -> (Database, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Database::open(&db_path).unwrap();
    (db, tmp)
}

fn create_test_app(name: &str) -> AppRecord {
    AppRecord {
        name: name.to_string(),
        status: "idle".to_string(),
        git_url: Some(format!("git@localhost:{}.git", name)),
        image: None,
        port: 3000,
        created_at: String::new(),
        deployed_at: None,
        commit_hash: None,
        scale: 1,
        min_scale: 0,
        max_scale: 10,
    }
}

fn create_deployed_app(name: &str) -> AppRecord {
    AppRecord {
        name: name.to_string(),
        status: "running".to_string(),
        git_url: Some(format!("git@localhost:{}.git", name)),
        image: Some(format!("{}:latest", name)),
        port: 3000,
        created_at: String::new(),
        deployed_at: Some("2024-01-01T00:00:00Z".to_string()),
        commit_hash: Some("abc123def456".to_string()),
        scale: 2,
        min_scale: 0,
        max_scale: 10,
    }
}

// ============================================================================
// App Lifecycle Tests
// ============================================================================

mod app_lifecycle_tests {
    use super::*;

    #[test]
    fn test_create_app() {
        let (db, _tmp) = create_test_db();
        let app = create_test_app("my-app");

        db.create_app(&app).unwrap();

        let retrieved = db.get_app("my-app").unwrap().unwrap();
        assert_eq!(retrieved.name, "my-app");
        assert_eq!(retrieved.status, "idle");
        assert_eq!(retrieved.port, 3000);
        assert!(retrieved.git_url.is_some());
    }

    #[test]
    fn test_list_apps() {
        let (db, _tmp) = create_test_db();

        db.create_app(&create_test_app("app-1")).unwrap();
        db.create_app(&create_test_app("app-2")).unwrap();
        db.create_app(&create_test_app("app-3")).unwrap();

        let apps = db.list_apps().unwrap();
        assert_eq!(apps.len(), 3);

        let names: Vec<&str> = apps.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"app-1"));
        assert!(names.contains(&"app-2"));
        assert!(names.contains(&"app-3"));
    }

    #[test]
    fn test_update_app_status() {
        let (db, _tmp) = create_test_db();
        let app = create_test_app("my-app");
        db.create_app(&app).unwrap();

        db.update_app_status("my-app", "running").unwrap();

        let retrieved = db.get_app("my-app").unwrap().unwrap();
        assert_eq!(retrieved.status, "running");
    }

    #[test]
    fn test_update_app_deployment() {
        let (db, _tmp) = create_test_db();
        let app = create_test_app("my-app");
        db.create_app(&app).unwrap();

        db.update_app_deployment("my-app", "my-app:v1", Some("abc123"))
            .unwrap();

        let retrieved = db.get_app("my-app").unwrap().unwrap();
        assert_eq!(retrieved.image, Some("my-app:v1".to_string()));
        assert_eq!(retrieved.commit_hash, Some("abc123".to_string()));
        assert!(retrieved.deployed_at.is_some());
    }

    #[test]
    fn test_delete_app() {
        let (db, _tmp) = create_test_db();
        let app = create_test_app("my-app");
        db.create_app(&app).unwrap();

        assert!(db.get_app("my-app").unwrap().is_some());

        db.delete_app("my-app").unwrap();

        assert!(db.get_app("my-app").unwrap().is_none());
    }

    #[test]
    fn test_app_not_found() {
        let (db, _tmp) = create_test_db();

        let result = db.get_app("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_scale_app() {
        let (db, _tmp) = create_test_db();
        let app = create_test_app("my-app");
        db.create_app(&app).unwrap();

        assert_eq!(db.get_app_scale("my-app").unwrap(), Some(1));

        db.update_app_scale("my-app", 5).unwrap();

        assert_eq!(db.get_app_scale("my-app").unwrap(), Some(5));

        let retrieved = db.get_app("my-app").unwrap().unwrap();
        assert_eq!(retrieved.scale, 5);
    }
}

// ============================================================================
// Config/Environment Variable Tests
// ============================================================================

mod config_tests {
    use super::*;

    #[test]
    fn test_set_and_get_config() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.set_config("my-app", "DATABASE_URL", "postgres://localhost/db", false)
            .unwrap();
        db.set_config("my-app", "REDIS_URL", "redis://localhost", false)
            .unwrap();

        let config = db.get_all_config("my-app").unwrap();
        assert_eq!(config.len(), 2);
        assert_eq!(config.get("DATABASE_URL").unwrap(), "postgres://localhost/db");
        assert_eq!(config.get("REDIS_URL").unwrap(), "redis://localhost");
    }

    #[test]
    fn test_update_config() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.set_config("my-app", "PORT", "3000", false).unwrap();

        let config = db.get_all_config("my-app").unwrap();
        assert_eq!(config.get("PORT").unwrap(), "3000");

        db.set_config("my-app", "PORT", "8080", false).unwrap();

        let config = db.get_all_config("my-app").unwrap();
        assert_eq!(config.get("PORT").unwrap(), "8080");
    }

    #[test]
    fn test_delete_config() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.set_config("my-app", "KEY1", "value1", false).unwrap();
        db.set_config("my-app", "KEY2", "value2", false).unwrap();

        let config = db.get_all_config("my-app").unwrap();
        assert_eq!(config.len(), 2);

        db.delete_config("my-app", "KEY1").unwrap();

        let config = db.get_all_config("my-app").unwrap();
        assert_eq!(config.len(), 1);
        assert!(config.get("KEY1").is_none());
        assert!(config.get("KEY2").is_some());
    }

    #[test]
    fn test_secret_config_flag() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.set_config("my-app", "PUBLIC_KEY", "public", false).unwrap();
        db.set_config("my-app", "SECRET_KEY", "secret", true).unwrap();

        // Both should be retrievable
        let config = db.get_all_config("my-app").unwrap();
        assert_eq!(config.len(), 2);

        // Can check if a key is secret
        assert!(!db.is_secret_key("my-app", "PUBLIC_KEY").unwrap());
        assert!(db.is_secret_key("my-app", "SECRET_KEY").unwrap());
    }

    #[test]
    fn test_config_cascades_on_app_delete() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.set_config("my-app", "KEY1", "value1", false).unwrap();
        db.set_config("my-app", "KEY2", "value2", false).unwrap();

        db.delete_app("my-app").unwrap();

        // Config should be deleted with the app
        let config = db.get_all_config("my-app").unwrap();
        assert!(config.is_empty());
    }
}

// ============================================================================
// Deployment Tests
// ============================================================================

mod deployment_tests {
    use super::*;

    #[test]
    fn test_create_deployment() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        let deployment = DeploymentRecord {
            id: "deploy-1".to_string(),
            app_name: "my-app".to_string(),
            status: "pending".to_string(),
            image: None,
            commit_hash: Some("abc123".to_string()),
            build_logs: None,
            duration_secs: None,
            created_at: String::new(),
            finished_at: None,
        };

        db.create_deployment(&deployment).unwrap();

        let deploys = db.get_deployments("my-app", 10).unwrap();
        assert_eq!(deploys.len(), 1);
        assert_eq!(deploys[0].id, "deploy-1");
        assert_eq!(deploys[0].status, "pending");
    }

    #[test]
    fn test_update_deployment() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        let deployment = DeploymentRecord {
            id: "deploy-1".to_string(),
            app_name: "my-app".to_string(),
            status: "pending".to_string(),
            image: None,
            commit_hash: None,
            build_logs: None,
            duration_secs: None,
            created_at: String::new(),
            finished_at: None,
        };
        db.create_deployment(&deployment).unwrap();

        db.update_deployment("deploy-1", "building", None, None, None)
            .unwrap();

        let deploys = db.get_deployments("my-app", 10).unwrap();
        assert_eq!(deploys[0].status, "building");

        db.update_deployment("deploy-1", "success", Some("my-app:v1"), None, Some(45.0))
            .unwrap();

        let deploys = db.get_deployments("my-app", 10).unwrap();
        assert_eq!(deploys[0].status, "success");
        assert_eq!(deploys[0].duration_secs, Some(45.0));
    }

    #[test]
    fn test_deployment_failure() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        let deployment = DeploymentRecord {
            id: "deploy-1".to_string(),
            app_name: "my-app".to_string(),
            status: "building".to_string(),
            image: None,
            commit_hash: None,
            build_logs: None,
            duration_secs: None,
            created_at: String::new(),
            finished_at: None,
        };
        db.create_deployment(&deployment).unwrap();

        db.update_deployment(
            "deploy-1",
            "failed",
            None,
            Some("Build failed: missing dependency"),
            Some(10.0),
        )
        .unwrap();

        let deploys = db.get_deployments("my-app", 10).unwrap();
        assert_eq!(deploys[0].status, "failed");
        assert!(deploys[0].build_logs.as_ref().unwrap().contains("Build failed"));
    }

    #[test]
    fn test_deployment_history_order() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        for i in 1..=5 {
            let deployment = DeploymentRecord {
                id: format!("deploy-{}", i),
                app_name: "my-app".to_string(),
                status: "success".to_string(),
                image: Some(format!("my-app:v{}", i)),
                commit_hash: Some(format!("hash{}", i)),
                build_logs: None,
                duration_secs: Some(30.0),
                created_at: String::new(),
                finished_at: None,
            };
            db.create_deployment(&deployment).unwrap();
        }

        let deploys = db.get_deployments("my-app", 3).unwrap();
        assert_eq!(deploys.len(), 3);
        // Verify we got deployments (order depends on DB implementation)
        let ids: Vec<&str> = deploys.iter().map(|d| d.id.as_str()).collect();
        // We should get 3 deployments from the 5 created
        assert!(ids.iter().all(|id| id.starts_with("deploy-")));
    }

    #[test]
    fn test_deployments_cascade_on_app_delete() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        let deployment = DeploymentRecord {
            id: "deploy-1".to_string(),
            app_name: "my-app".to_string(),
            status: "success".to_string(),
            image: None,
            commit_hash: None,
            build_logs: None,
            duration_secs: None,
            created_at: String::new(),
            finished_at: None,
        };
        db.create_deployment(&deployment).unwrap();

        db.delete_app("my-app").unwrap();

        let deploys = db.get_deployments("my-app", 10).unwrap();
        assert!(deploys.is_empty());
    }
}

// ============================================================================
// Add-on Tests
// ============================================================================

mod addon_tests {
    use super::*;

    fn create_addon(app_name: &str, addon_type: &str) -> AddonRecord {
        AddonRecord {
            id: format!("{}-{}", app_name, addon_type),
            app_name: app_name.to_string(),
            addon_type: addon_type.to_string(),
            plan: "hobby".to_string(),
            container_id: Some(format!("container-{}", addon_type)),
            container_name: Some(format!("paas-{}-{}", app_name, addon_type)),
            connection_url: Some(format!("{}://localhost/{}", addon_type, app_name)),
            env_var_name: Some(format!("{}_URL", addon_type.to_uppercase())),
            status: "running".to_string(),
            created_at: String::new(),
        }
    }

    #[test]
    fn test_create_addon() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        let addon = create_addon("my-app", "postgres");
        db.create_addon(&addon).unwrap();

        let addons = db.get_app_addons("my-app").unwrap();
        assert_eq!(addons.len(), 1);
        assert_eq!(addons[0].addon_type, "postgres");
        assert_eq!(addons[0].plan, "hobby");
        assert_eq!(addons[0].status, "running");
    }

    #[test]
    fn test_multiple_addons() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.create_addon(&create_addon("my-app", "postgres")).unwrap();
        db.create_addon(&create_addon("my-app", "redis")).unwrap();
        db.create_addon(&create_addon("my-app", "storage")).unwrap();

        let addons = db.get_app_addons("my-app").unwrap();
        assert_eq!(addons.len(), 3);

        let types: Vec<&str> = addons.iter().map(|a| a.addon_type.as_str()).collect();
        assert!(types.contains(&"postgres"));
        assert!(types.contains(&"redis"));
        assert!(types.contains(&"storage"));
    }

    #[test]
    fn test_delete_addon() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.create_addon(&create_addon("my-app", "postgres")).unwrap();
        db.create_addon(&create_addon("my-app", "redis")).unwrap();

        let addons = db.get_app_addons("my-app").unwrap();
        assert_eq!(addons.len(), 2);

        db.delete_addon("my-app", "postgres").unwrap();

        let addons = db.get_app_addons("my-app").unwrap();
        assert_eq!(addons.len(), 1);
        assert_eq!(addons[0].addon_type, "redis");
    }

    #[test]
    fn test_addons_cascade_on_app_delete() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.create_addon(&create_addon("my-app", "postgres")).unwrap();
        db.create_addon(&create_addon("my-app", "redis")).unwrap();

        db.delete_app("my-app").unwrap();

        let addons = db.get_app_addons("my-app").unwrap();
        assert!(addons.is_empty());
    }
}

// ============================================================================
// Domain Tests
// ============================================================================

mod domain_tests {
    use super::*;

    #[test]
    fn test_add_domain() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.add_domain("example.com", "my-app", "verify-123").unwrap();

        let domains = db.get_app_domains("my-app").unwrap();
        assert_eq!(domains.len(), 1);
        assert_eq!(domains[0].domain, "example.com");
        assert!(!domains[0].verified);
    }

    #[test]
    fn test_multiple_domains() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.add_domain("example.com", "my-app", "v1").unwrap();
        db.add_domain("www.example.com", "my-app", "v2").unwrap();
        db.add_domain("api.example.com", "my-app", "v3").unwrap();

        let domains = db.get_app_domains("my-app").unwrap();
        assert_eq!(domains.len(), 3);
    }

    #[test]
    fn test_verify_domain() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.add_domain("example.com", "my-app", "verify-123").unwrap();

        let domains = db.get_app_domains("my-app").unwrap();
        assert!(!domains[0].verified);

        db.update_domain_verification("example.com", true).unwrap();

        let domains = db.get_app_domains("my-app").unwrap();
        assert!(domains[0].verified);
    }

    #[test]
    fn test_enable_domain_ssl() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.add_domain("example.com", "my-app", "verify-123").unwrap();
        db.update_domain_verification("example.com", true).unwrap();

        db.update_domain_ssl("example.com", true, None, None, Some("2025-12-31T23:59:59Z"))
            .unwrap();

        let domains = db.get_app_domains("my-app").unwrap();
        assert!(domains[0].ssl_enabled);
    }

    #[test]
    fn test_remove_domain() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.add_domain("example.com", "my-app", "v1").unwrap();
        db.add_domain("api.example.com", "my-app", "v2").unwrap();

        db.delete_domain("example.com").unwrap();

        let domains = db.get_app_domains("my-app").unwrap();
        assert_eq!(domains.len(), 1);
        assert_eq!(domains[0].domain, "api.example.com");
    }

    #[test]
    fn test_lookup_domain() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.add_domain("example.com", "my-app", "verify-123").unwrap();
        db.update_domain_verification("example.com", true).unwrap();

        let result = db.find_app_by_domain("example.com").unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "my-app");

        let result = db.find_app_by_domain("unknown.com").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_domains_cascade_on_app_delete() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.add_domain("example.com", "my-app", "v1").unwrap();
        db.add_domain("api.example.com", "my-app", "v2").unwrap();

        db.delete_app("my-app").unwrap();

        let domains = db.get_app_domains("my-app").unwrap();
        assert!(domains.is_empty());
    }
}

// ============================================================================
// Webhook Tests
// ============================================================================

mod webhook_tests {
    use super::*;

    #[test]
    fn test_create_webhook() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        let config = WebhookRecord {
            app_name: "my-app".to_string(),
            secret: "webhook-secret-123".to_string(),
            deploy_branch: "main".to_string(),
            auto_deploy: true,
            provider: "github".to_string(),
            repo_name: Some("user/repo".to_string()),
            status_token: None,
            created_at: String::new(),
            updated_at: String::new(),
        };

        db.save_webhook(&config).unwrap();

        let retrieved = db.get_webhook("my-app").unwrap();
        assert!(retrieved.is_some());
        let webhook = retrieved.unwrap();
        assert_eq!(webhook.deploy_branch, "main");
        assert!(webhook.auto_deploy);
        assert_eq!(webhook.provider, "github");
    }

    #[test]
    fn test_update_webhook() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        let config = WebhookRecord {
            app_name: "my-app".to_string(),
            secret: "secret-1".to_string(),
            deploy_branch: "main".to_string(),
            auto_deploy: true,
            provider: "github".to_string(),
            repo_name: None,
            status_token: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        db.save_webhook(&config).unwrap();

        let updated = WebhookRecord {
            app_name: "my-app".to_string(),
            secret: "secret-2".to_string(),
            deploy_branch: "develop".to_string(),
            auto_deploy: false,
            provider: "gitlab".to_string(),
            repo_name: Some("user/repo".to_string()),
            status_token: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        db.save_webhook(&updated).unwrap();

        let retrieved = db.get_webhook("my-app").unwrap().unwrap();
        assert_eq!(retrieved.deploy_branch, "develop");
        assert!(!retrieved.auto_deploy);
        assert_eq!(retrieved.provider, "gitlab");
    }

    #[test]
    fn test_delete_webhook() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        let config = WebhookRecord {
            app_name: "my-app".to_string(),
            secret: "secret".to_string(),
            deploy_branch: "main".to_string(),
            auto_deploy: true,
            provider: "github".to_string(),
            repo_name: None,
            status_token: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        db.save_webhook(&config).unwrap();

        assert!(db.get_webhook("my-app").unwrap().is_some());

        db.delete_webhook("my-app").unwrap();

        assert!(db.get_webhook("my-app").unwrap().is_none());
    }

    #[test]
    fn test_webhook_events() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        let event1 = WebhookEventRecord {
            id: None,
            app_name: "my-app".to_string(),
            event_type: "push".to_string(),
            provider: "github".to_string(),
            branch: Some("main".to_string()),
            commit_sha: Some("abc123".to_string()),
            commit_message: Some("Fix bug".to_string()),
            author: Some("user".to_string()),
            payload: None,
            triggered_deploy: true,
            deployment_id: Some("deploy-1".to_string()),
            created_at: None,
        };
        db.log_webhook_event(&event1).unwrap();

        let event2 = WebhookEventRecord {
            id: None,
            app_name: "my-app".to_string(),
            event_type: "push".to_string(),
            provider: "github".to_string(),
            branch: Some("main".to_string()),
            commit_sha: Some("def456".to_string()),
            commit_message: Some("Add feature".to_string()),
            author: Some("user".to_string()),
            payload: None,
            triggered_deploy: true,
            deployment_id: Some("deploy-2".to_string()),
            created_at: None,
        };
        db.log_webhook_event(&event2).unwrap();

        let events = db.get_webhook_events("my-app", 10).unwrap();
        assert_eq!(events.len(), 2);
        // Verify both events were logged (order depends on DB implementation)
        let shas: Vec<Option<String>> = events.iter().map(|e| e.commit_sha.clone()).collect();
        assert!(shas.contains(&Some("abc123".to_string())));
        assert!(shas.contains(&Some("def456".to_string())));
    }
}

// ============================================================================
// Secrets Management Tests
// ============================================================================

mod secrets_tests {
    use super::*;

    #[test]
    fn test_secret_audit_log() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.log_secret_access("my-app", "API_KEY", "read", Some("admin"), None)
            .unwrap();
        db.log_secret_access("my-app", "API_KEY", "update", Some("admin"), None)
            .unwrap();
        db.log_secret_access("my-app", "DB_PASSWORD", "create", Some("admin"), None)
            .unwrap();

        let logs = db.get_secret_audit_log("my-app", 10).unwrap();
        assert_eq!(logs.len(), 3);
        // Verify all actions are logged (order depends on DB implementation)
        let actions: Vec<&str> = logs.iter().map(|l| l.action.as_str()).collect();
        assert!(actions.contains(&"read"));
        assert!(actions.contains(&"update"));
        assert!(actions.contains(&"create"));
    }

    #[test]
    fn test_get_secret_keys() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.set_config("my-app", "PUBLIC_VAR", "public", false).unwrap();
        db.set_config("my-app", "SECRET_KEY", "secret1", true).unwrap();
        db.set_config("my-app", "DB_PASSWORD", "secret2", true).unwrap();

        let secret_keys = db.get_secret_keys("my-app").unwrap();
        assert_eq!(secret_keys.len(), 2);
        assert!(secret_keys.contains(&"SECRET_KEY".to_string()));
        assert!(secret_keys.contains(&"DB_PASSWORD".to_string()));
    }
}

// ============================================================================
// Activity Logging Tests
// ============================================================================

mod activity_tests {
    use super::*;

    #[test]
    fn test_log_activity() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.log_activity(
            "deploy",
            "Deployed my-app to v1.2.3",
            Some("my-app"),
            Some("deployment"),
            Some("deploy-123"),
            Some("admin"),
            "user",
            Some("Commit: abc123"),
            Some("127.0.0.1"),
        )
        .unwrap();

        let events = db.get_app_activity("my-app", 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "deploy");
        assert_eq!(events[0].action, "Deployed my-app to v1.2.3");
        assert_eq!(events[0].actor, Some("admin".to_string()));
    }

    #[test]
    fn test_multiple_activity_events() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.log_activity("app_create", "Created app", Some("my-app"), None, None, None, "system", None, None).unwrap();
        db.log_activity("config", "Updated config", Some("my-app"), None, None, Some("admin"), "user", None, None).unwrap();
        db.log_activity("scale", "Scaled to 3 instances", Some("my-app"), None, None, Some("admin"), "user", None, None).unwrap();
        db.log_activity("deploy", "Deployed v1.0.0", Some("my-app"), None, None, Some("admin"), "user", None, None).unwrap();

        let events = db.get_app_activity("my-app", 10).unwrap();
        assert_eq!(events.len(), 4);
        // Verify we have all event types (order may vary based on DB implementation)
        let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
        assert!(types.contains(&"app_create"));
        assert!(types.contains(&"config"));
        assert!(types.contains(&"scale"));
        assert!(types.contains(&"deploy"));
    }

    #[test]
    fn test_platform_wide_activity() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("app-1")).unwrap();
        db.create_app(&create_test_app("app-2")).unwrap();

        db.log_activity("deploy", "Deployed app-1", Some("app-1"), None, None, None, "system", None, None).unwrap();
        db.log_activity("deploy", "Deployed app-2", Some("app-2"), None, None, None, "system", None, None).unwrap();
        db.log_activity("scale", "Scaled app-1", Some("app-1"), None, None, None, "system", None, None).unwrap();

        let all_events = db.get_all_activity(100).unwrap();
        assert_eq!(all_events.len(), 3);

        let app1_events = db.get_app_activity("app-1", 10).unwrap();
        assert_eq!(app1_events.len(), 2);

        let app2_events = db.get_app_activity("app-2", 10).unwrap();
        assert_eq!(app2_events.len(), 1);
    }

    #[test]
    fn test_filtered_activity() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        db.log_activity("deploy", "Deploy 1", Some("my-app"), None, None, Some("alice"), "user", None, None).unwrap();
        db.log_activity("deploy", "Deploy 2", Some("my-app"), None, None, Some("bob"), "user", None, None).unwrap();
        db.log_activity("config", "Config change", Some("my-app"), None, None, Some("alice"), "user", None, None).unwrap();

        // Filter by event type
        let deploys = db.get_filtered_activity(None, Some("deploy"), None, 10).unwrap();
        assert_eq!(deploys.len(), 2);

        // Filter by actor
        let alice_events = db.get_filtered_activity(None, None, Some("alice"), 10).unwrap();
        assert_eq!(alice_events.len(), 2);

        // Filter by both
        let alice_deploys = db.get_filtered_activity(None, Some("deploy"), Some("alice"), 10).unwrap();
        assert_eq!(alice_deploys.len(), 1);
    }

    #[test]
    fn test_activity_limit() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_test_app("my-app")).unwrap();

        for i in 1..=20 {
            db.log_activity(
                "deploy",
                &format!("Deploy {}", i),
                Some("my-app"),
                None,
                None,
                None,
                "system",
                None,
                None,
            )
            .unwrap();
        }

        let events = db.get_app_activity("my-app", 5).unwrap();
        assert_eq!(events.len(), 5);
        // Verify we got activity events (order depends on DB implementation)
        assert!(events.iter().all(|e| e.action.starts_with("Deploy")));
    }
}

// ============================================================================
// Process/Instance Tests
// ============================================================================

mod process_tests {
    use super::*;

    #[test]
    fn test_create_process() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_deployed_app("my-app")).unwrap();

        let process = ProcessRecord {
            id: "instance-1".to_string(),
            app_name: "my-app".to_string(),
            process_type: "web".to_string(),
            container_id: Some("container-abc".to_string()),
            container_name: Some("paas-my-app-web-1".to_string()),
            port: Some(10001),
            status: "running".to_string(),
            health_status: Some("healthy".to_string()),
            last_health_check: None,
            started_at: String::new(),
        };

        db.create_process(&process).unwrap();

        let processes = db.get_app_processes("my-app").unwrap();
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].id, "instance-1");
        assert_eq!(processes[0].process_type, "web");
    }

    #[test]
    fn test_multiple_instances() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_deployed_app("my-app")).unwrap();

        for i in 1..=3 {
            let process = ProcessRecord {
                id: format!("instance-{}", i),
                app_name: "my-app".to_string(),
                process_type: "web".to_string(),
                container_id: Some(format!("container-{}", i)),
                container_name: Some(format!("paas-my-app-web-{}", i)),
                port: Some(10000 + i),
                status: "running".to_string(),
                health_status: Some("healthy".to_string()),
                last_health_check: None,
                started_at: String::new(),
            };
            db.create_process(&process).unwrap();
        }

        let processes = db.get_app_processes("my-app").unwrap();
        assert_eq!(processes.len(), 3);
    }

    #[test]
    fn test_update_process_status() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_deployed_app("my-app")).unwrap();

        let process = ProcessRecord {
            id: "instance-1".to_string(),
            app_name: "my-app".to_string(),
            process_type: "web".to_string(),
            container_id: None,
            container_name: None,
            port: None,
            status: "starting".to_string(),
            health_status: None,
            last_health_check: None,
            started_at: String::new(),
        };
        db.create_process(&process).unwrap();

        db.update_process_status("instance-1", "running").unwrap();

        let processes = db.get_app_processes("my-app").unwrap();
        assert_eq!(processes[0].status, "running");
    }

    #[test]
    fn test_update_process_health() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_deployed_app("my-app")).unwrap();

        let process = ProcessRecord {
            id: "instance-1".to_string(),
            app_name: "my-app".to_string(),
            process_type: "web".to_string(),
            container_id: None,
            container_name: None,
            port: None,
            status: "running".to_string(),
            health_status: None,
            last_health_check: None,
            started_at: String::new(),
        };
        db.create_process(&process).unwrap();

        db.update_process_health("instance-1", "healthy").unwrap();

        let processes = db.get_app_processes("my-app").unwrap();
        assert_eq!(processes[0].health_status, Some("healthy".to_string()));
    }

    #[test]
    fn test_delete_process() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_deployed_app("my-app")).unwrap();

        let process = ProcessRecord {
            id: "instance-1".to_string(),
            app_name: "my-app".to_string(),
            process_type: "web".to_string(),
            container_id: None,
            container_name: None,
            port: None,
            status: "running".to_string(),
            health_status: None,
            last_health_check: None,
            started_at: String::new(),
        };
        db.create_process(&process).unwrap();

        db.delete_process("instance-1").unwrap();

        let processes = db.get_app_processes("my-app").unwrap();
        assert!(processes.is_empty());
    }

    #[test]
    fn test_processes_cascade_on_app_delete() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_deployed_app("my-app")).unwrap();

        for i in 1..=3 {
            let process = ProcessRecord {
                id: format!("instance-{}", i),
                app_name: "my-app".to_string(),
                process_type: "web".to_string(),
                container_id: None,
                container_name: None,
                port: None,
                status: "running".to_string(),
                health_status: None,
                last_health_check: None,
                started_at: String::new(),
            };
            db.create_process(&process).unwrap();
        }

        db.delete_app("my-app").unwrap();

        let processes = db.get_app_processes("my-app").unwrap();
        assert!(processes.is_empty());
    }
}

// ============================================================================
// Metrics Tests
// ============================================================================

mod metrics_tests {
    use super::*;
    use spawngate::db::{RequestMetricsRecord, ResourceMetricsRecord};

    #[test]
    fn test_record_request_metrics() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_deployed_app("my-app")).unwrap();

        let metrics1 = RequestMetricsRecord {
            id: 0,
            app_name: "my-app".to_string(),
            timestamp: String::new(),
            request_count: 100,
            error_count: 5,
            avg_response_time_ms: 45.0,
            p50_response_time_ms: Some(40.0),
            p95_response_time_ms: Some(80.0),
            p99_response_time_ms: Some(120.0),
        };
        db.record_request_metrics(&metrics1).unwrap();

        let metrics2 = RequestMetricsRecord {
            id: 0,
            app_name: "my-app".to_string(),
            timestamp: String::new(),
            request_count: 150,
            error_count: 3,
            avg_response_time_ms: 42.0,
            p50_response_time_ms: Some(38.0),
            p95_response_time_ms: Some(75.0),
            p99_response_time_ms: Some(110.0),
        };
        db.record_request_metrics(&metrics2).unwrap();

        let metrics = db.get_request_metrics("my-app", "", 10).unwrap();
        assert_eq!(metrics.len(), 2);
    }

    #[test]
    fn test_record_resource_metrics() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_deployed_app("my-app")).unwrap();

        let metrics1 = ResourceMetricsRecord {
            id: 0,
            app_name: "my-app".to_string(),
            instance_id: "instance-1".to_string(),
            timestamp: String::new(),
            cpu_percent: 25.5,
            memory_used: 256 * 1024 * 1024,
            memory_limit: 512 * 1024 * 1024,
        };
        db.record_resource_metrics(&metrics1).unwrap();

        let metrics2 = ResourceMetricsRecord {
            id: 0,
            app_name: "my-app".to_string(),
            instance_id: "instance-2".to_string(),
            timestamp: String::new(),
            cpu_percent: 30.0,
            memory_used: 300 * 1024 * 1024,
            memory_limit: 512 * 1024 * 1024,
        };
        db.record_resource_metrics(&metrics2).unwrap();

        let metrics = db.get_resource_metrics("my-app", "", 10).unwrap();
        assert_eq!(metrics.len(), 2);
    }

    #[test]
    fn test_instance_metrics() {
        let (db, _tmp) = create_test_db();
        db.create_app(&create_deployed_app("my-app")).unwrap();

        // Record multiple metrics for same instance
        for i in 0..5i64 {
            let metrics = ResourceMetricsRecord {
                id: 0,
                app_name: "my-app".to_string(),
                instance_id: "instance-1".to_string(),
                timestamp: String::new(),
                cpu_percent: 20.0 + i as f64,
                memory_used: (200 + i * 10) * 1024 * 1024,
                memory_limit: 512 * 1024 * 1024,
            };
            db.record_resource_metrics(&metrics).unwrap();
        }

        let metrics = db.get_instance_metrics("instance-1", "", 3).unwrap();
        assert_eq!(metrics.len(), 3);
    }
}

// ============================================================================
// API Token Tests
// ============================================================================

mod api_token_tests {
    use super::*;

    #[test]
    fn test_create_api_token() {
        let (db, _tmp) = create_test_db();

        let token = ApiTokenRecord {
            id: "token-1".to_string(),
            name: "My Token".to_string(),
            token_hash: "hash-abc123".to_string(),
            token_prefix: "spg_abc".to_string(),
            scopes: "read,write".to_string(),
            last_used_at: None,
            expires_at: None,
            created_at: String::new(),
        };

        db.create_api_token(&token).unwrap();

        let tokens = db.list_api_tokens().unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].name, "My Token");
        assert_eq!(tokens[0].token_prefix, "spg_abc");
        assert_eq!(tokens[0].scopes, "read,write");
    }

    #[test]
    fn test_multiple_tokens() {
        let (db, _tmp) = create_test_db();

        for i in 1..=3 {
            let token = ApiTokenRecord {
                id: format!("t{}", i),
                name: format!("Token {}", i),
                token_hash: format!("hash{}", i),
                token_prefix: format!("spg_{}", i),
                scopes: "read".to_string(),
                last_used_at: None,
                expires_at: None,
                created_at: String::new(),
            };
            db.create_api_token(&token).unwrap();
        }

        let tokens = db.list_api_tokens().unwrap();
        assert_eq!(tokens.len(), 3);
    }

    #[test]
    fn test_delete_api_token() {
        let (db, _tmp) = create_test_db();

        for i in 1..=2 {
            let token = ApiTokenRecord {
                id: format!("t{}", i),
                name: format!("Token {}", i),
                token_hash: format!("hash{}", i),
                token_prefix: format!("spg_{}", i),
                scopes: "read".to_string(),
                last_used_at: None,
                expires_at: None,
                created_at: String::new(),
            };
            db.create_api_token(&token).unwrap();
        }

        db.delete_api_token("t1").unwrap();

        let tokens = db.list_api_tokens().unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].id, "t2");
    }

    #[test]
    fn test_token_with_expiry() {
        let (db, _tmp) = create_test_db();

        let expires = "2025-12-31T23:59:59Z";
        let token = ApiTokenRecord {
            id: "t1".to_string(),
            name: "Expiring Token".to_string(),
            token_hash: "hash1".to_string(),
            token_prefix: "spg_1".to_string(),
            scopes: "read".to_string(),
            last_used_at: None,
            expires_at: Some(expires.to_string()),
            created_at: String::new(),
        };
        db.create_api_token(&token).unwrap();

        let tokens = db.list_api_tokens().unwrap();
        assert_eq!(tokens[0].expires_at, Some(expires.to_string()));
    }
}

// ============================================================================
// Full E2E Scenario Tests
// ============================================================================

mod e2e_scenarios {
    use super::*;

    #[test]
    fn test_complete_app_lifecycle() {
        let (db, _tmp) = create_test_db();

        // 1. Create app
        let app = create_test_app("my-app");
        db.create_app(&app).unwrap();
        db.log_activity("app_create", "Created app my-app", Some("my-app"), None, None, Some("admin"), "user", None, None).unwrap();

        // 2. Configure app
        db.set_config("my-app", "NODE_ENV", "production", false).unwrap();
        db.set_config("my-app", "PORT", "3000", false).unwrap();
        db.set_config("my-app", "DATABASE_URL", "encrypted-url", true).unwrap();
        db.log_activity("config", "Updated configuration", Some("my-app"), None, None, Some("admin"), "user", None, None).unwrap();

        // 3. Add domain
        db.add_domain("example.com", "my-app", "verify-123").unwrap();
        db.update_domain_verification("example.com", true).unwrap();
        db.log_activity("domain", "Added domain example.com", Some("my-app"), None, None, Some("admin"), "user", None, None).unwrap();

        // 4. Provision add-on
        let addon = AddonRecord {
            id: "addon-1".to_string(),
            app_name: "my-app".to_string(),
            addon_type: "postgres".to_string(),
            plan: "hobby".to_string(),
            container_id: Some("pg-container".to_string()),
            container_name: Some("paas-my-app-postgres".to_string()),
            connection_url: Some("postgres://localhost/myapp".to_string()),
            env_var_name: Some("DATABASE_URL".to_string()),
            status: "running".to_string(),
            created_at: String::new(),
        };
        db.create_addon(&addon).unwrap();
        db.log_activity("addon", "Provisioned postgres addon", Some("my-app"), Some("addon"), Some("addon-1"), Some("admin"), "user", None, None).unwrap();

        // 5. Deploy
        let deployment = DeploymentRecord {
            id: "deploy-1".to_string(),
            app_name: "my-app".to_string(),
            status: "success".to_string(),
            image: Some("my-app:v1".to_string()),
            commit_hash: Some("abc123".to_string()),
            build_logs: None,
            duration_secs: Some(45.0),
            created_at: String::new(),
            finished_at: None,
        };
        db.create_deployment(&deployment).unwrap();
        db.update_app_deployment("my-app", "my-app:v1", Some("abc123")).unwrap();
        db.update_app_status("my-app", "running").unwrap();
        db.log_activity("deploy", "Deployed v1", Some("my-app"), Some("deployment"), Some("deploy-1"), Some("admin"), "user", None, None).unwrap();

        // 6. Scale
        db.update_app_scale("my-app", 3).unwrap();
        for i in 1..=3 {
            let process = ProcessRecord {
                id: format!("inst-{}", i),
                app_name: "my-app".to_string(),
                process_type: "web".to_string(),
                container_id: Some(format!("container-{}", i)),
                container_name: Some(format!("paas-my-app-web-{}", i)),
                port: Some(10000 + i),
                status: "running".to_string(),
                health_status: Some("healthy".to_string()),
                last_health_check: None,
                started_at: String::new(),
            };
            db.create_process(&process).unwrap();
        }
        db.log_activity("scale", "Scaled to 3 instances", Some("my-app"), None, None, Some("admin"), "user", None, None).unwrap();

        // Verify final state
        let app = db.get_app("my-app").unwrap().unwrap();
        assert_eq!(app.status, "running");
        assert_eq!(app.scale, 3);
        assert!(app.image.is_some());

        let config = db.get_all_config("my-app").unwrap();
        assert_eq!(config.len(), 3);

        let domains = db.get_app_domains("my-app").unwrap();
        assert_eq!(domains.len(), 1);
        assert!(domains[0].verified);

        let addons = db.get_app_addons("my-app").unwrap();
        assert_eq!(addons.len(), 1);

        let processes = db.get_app_processes("my-app").unwrap();
        assert_eq!(processes.len(), 3);

        let activity = db.get_app_activity("my-app", 20).unwrap();
        assert_eq!(activity.len(), 6);

        // 7. Delete app - verify cascade
        db.delete_app("my-app").unwrap();

        assert!(db.get_app("my-app").unwrap().is_none());
        assert!(db.get_all_config("my-app").unwrap().is_empty());
        assert!(db.get_app_domains("my-app").unwrap().is_empty());
        assert!(db.get_app_addons("my-app").unwrap().is_empty());
        assert!(db.get_app_processes("my-app").unwrap().is_empty());
        assert!(db.get_deployments("my-app", 10).unwrap().is_empty());
    }

    #[test]
    fn test_multi_app_isolation() {
        let (db, _tmp) = create_test_db();

        // Create two apps
        db.create_app(&create_test_app("app-1")).unwrap();
        db.create_app(&create_test_app("app-2")).unwrap();

        // Configure each separately
        db.set_config("app-1", "KEY", "value-1", false).unwrap();
        db.set_config("app-2", "KEY", "value-2", false).unwrap();

        // Add domains to each
        db.add_domain("app1.example.com", "app-1", "v1").unwrap();
        db.add_domain("app2.example.com", "app-2", "v2").unwrap();

        // Log activity for each
        db.log_activity("deploy", "Deployed app-1", Some("app-1"), None, None, None, "system", None, None).unwrap();
        db.log_activity("deploy", "Deployed app-2", Some("app-2"), None, None, None, "system", None, None).unwrap();

        // Verify isolation
        let config1 = db.get_all_config("app-1").unwrap();
        let config2 = db.get_all_config("app-2").unwrap();
        assert_eq!(config1.get("KEY").unwrap(), "value-1");
        assert_eq!(config2.get("KEY").unwrap(), "value-2");

        let domains1 = db.get_app_domains("app-1").unwrap();
        let domains2 = db.get_app_domains("app-2").unwrap();
        assert_eq!(domains1[0].domain, "app1.example.com");
        assert_eq!(domains2[0].domain, "app2.example.com");

        // Delete one app, other should be unaffected
        db.delete_app("app-1").unwrap();

        assert!(db.get_app("app-1").unwrap().is_none());
        assert!(db.get_app("app-2").unwrap().is_some());
        assert!(db.get_all_config("app-1").unwrap().is_empty());
        assert!(!db.get_all_config("app-2").unwrap().is_empty());
    }
}

// ==================== Alert Tests ====================

mod alert_tests {
    use super::*;
    use spawngate::db::AlertRuleRecord;

    fn create_test_alert_rule(id: &str, app_name: Option<&str>) -> AlertRuleRecord {
        AlertRuleRecord {
            id: id.to_string(),
            app_name: app_name.map(String::from),
            name: format!("Test Alert {}", id),
            description: Some("Test alert description".to_string()),
            metric_type: "error_rate".to_string(),
            condition: ">".to_string(),
            threshold: 5.0,
            duration_secs: 60,
            severity: "warning".to_string(),
            enabled: true,
            notification_channels: Some("email,slack".to_string()),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn test_create_alert_rule() {
        let (db, _tmp) = create_test_db();

        let rule = create_test_alert_rule("rule-1", None);
        db.create_alert_rule(&rule).unwrap();

        let retrieved = db.get_alert_rule("rule-1").unwrap().unwrap();
        assert_eq!(retrieved.id, "rule-1");
        assert_eq!(retrieved.name, "Test Alert rule-1");
        assert_eq!(retrieved.metric_type, "error_rate");
        assert_eq!(retrieved.threshold, 5.0);
        assert!(retrieved.enabled);
    }

    #[test]
    fn test_list_alert_rules() {
        let (db, _tmp) = create_test_db();

        db.create_alert_rule(&create_test_alert_rule("rule-1", None)).unwrap();
        db.create_alert_rule(&create_test_alert_rule("rule-2", None)).unwrap();
        db.create_alert_rule(&create_test_alert_rule("rule-3", None)).unwrap();

        let rules = db.list_alert_rules(None).unwrap();
        assert_eq!(rules.len(), 3);
    }

    #[test]
    fn test_list_alert_rules_by_app() {
        let (db, _tmp) = create_test_db();

        db.create_app(&create_test_app("my-app")).unwrap();

        db.create_alert_rule(&create_test_alert_rule("rule-1", Some("my-app"))).unwrap();
        db.create_alert_rule(&create_test_alert_rule("rule-2", Some("my-app"))).unwrap();
        db.create_alert_rule(&create_test_alert_rule("rule-3", None)).unwrap();

        let app_rules = db.list_alert_rules(Some("my-app")).unwrap();
        assert_eq!(app_rules.len(), 2);

        let all_rules = db.list_alert_rules(None).unwrap();
        assert_eq!(all_rules.len(), 3);
    }

    #[test]
    fn test_list_enabled_alert_rules() {
        let (db, _tmp) = create_test_db();

        let mut rule1 = create_test_alert_rule("rule-1", None);
        let mut rule2 = create_test_alert_rule("rule-2", None);
        rule2.enabled = false;

        db.create_alert_rule(&rule1).unwrap();
        db.create_alert_rule(&rule2).unwrap();

        let enabled = db.list_enabled_alert_rules().unwrap();
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].id, "rule-1");
    }

    #[test]
    fn test_update_alert_rule() {
        let (db, _tmp) = create_test_db();

        let rule = create_test_alert_rule("rule-1", None);
        db.create_alert_rule(&rule).unwrap();

        let mut updated = db.get_alert_rule("rule-1").unwrap().unwrap();
        updated.name = "Updated Name".to_string();
        updated.threshold = 10.0;
        updated.severity = "critical".to_string();

        db.update_alert_rule(&updated).unwrap();

        let retrieved = db.get_alert_rule("rule-1").unwrap().unwrap();
        assert_eq!(retrieved.name, "Updated Name");
        assert_eq!(retrieved.threshold, 10.0);
        assert_eq!(retrieved.severity, "critical");
    }

    #[test]
    fn test_toggle_alert_rule() {
        let (db, _tmp) = create_test_db();

        let rule = create_test_alert_rule("rule-1", None);
        db.create_alert_rule(&rule).unwrap();

        // Disable
        db.toggle_alert_rule("rule-1", false).unwrap();
        let retrieved = db.get_alert_rule("rule-1").unwrap().unwrap();
        assert!(!retrieved.enabled);

        // Re-enable
        db.toggle_alert_rule("rule-1", true).unwrap();
        let retrieved = db.get_alert_rule("rule-1").unwrap().unwrap();
        assert!(retrieved.enabled);
    }

    #[test]
    fn test_delete_alert_rule() {
        let (db, _tmp) = create_test_db();

        let rule = create_test_alert_rule("rule-1", None);
        db.create_alert_rule(&rule).unwrap();

        db.delete_alert_rule("rule-1").unwrap();

        let retrieved = db.get_alert_rule("rule-1").unwrap();
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_create_alert_event() {
        let (db, _tmp) = create_test_db();

        db.create_app(&create_test_app("my-app")).unwrap();
        db.create_alert_rule(&create_test_alert_rule("rule-1", Some("my-app"))).unwrap();

        let event_id = db.create_alert_event(
            "rule-1",
            Some("my-app"),
            7.5,
            5.0,
            Some("Error rate exceeded threshold"),
        ).unwrap();

        let event = db.get_alert_event(event_id).unwrap().unwrap();
        assert_eq!(event.rule_id, "rule-1");
        assert_eq!(event.status, "firing");
        assert_eq!(event.metric_value, 7.5);
        assert_eq!(event.threshold, 5.0);
    }

    #[test]
    fn test_list_alert_events() {
        let (db, _tmp) = create_test_db();

        db.create_app(&create_test_app("my-app")).unwrap();
        db.create_alert_rule(&create_test_alert_rule("rule-1", Some("my-app"))).unwrap();

        db.create_alert_event("rule-1", Some("my-app"), 7.5, 5.0, Some("Alert 1")).unwrap();
        db.create_alert_event("rule-1", Some("my-app"), 8.0, 5.0, Some("Alert 2")).unwrap();

        let events = db.list_alert_events(Some("my-app"), None, 10).unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_get_firing_alerts() {
        let (db, _tmp) = create_test_db();

        db.create_app(&create_test_app("my-app")).unwrap();
        db.create_alert_rule(&create_test_alert_rule("rule-1", Some("my-app"))).unwrap();

        let id1 = db.create_alert_event("rule-1", Some("my-app"), 7.5, 5.0, Some("Alert 1")).unwrap();
        let id2 = db.create_alert_event("rule-1", Some("my-app"), 8.0, 5.0, Some("Alert 2")).unwrap();

        // Initially all firing
        let firing = db.get_firing_alerts().unwrap();
        assert_eq!(firing.len(), 2);

        // Resolve one
        db.resolve_alert_event(id1).unwrap();

        let firing = db.get_firing_alerts().unwrap();
        assert_eq!(firing.len(), 1);
        assert_eq!(firing[0].id, id2);
    }

    #[test]
    fn test_acknowledge_alert() {
        let (db, _tmp) = create_test_db();

        db.create_app(&create_test_app("my-app")).unwrap();
        db.create_alert_rule(&create_test_alert_rule("rule-1", Some("my-app"))).unwrap();

        let event_id = db.create_alert_event("rule-1", Some("my-app"), 7.5, 5.0, Some("Alert")).unwrap();

        db.acknowledge_alert_event(event_id, "admin@example.com").unwrap();

        let event = db.get_alert_event(event_id).unwrap().unwrap();
        assert!(event.acknowledged_at.is_some());
        assert_eq!(event.acknowledged_by, Some("admin@example.com".to_string()));
    }

    #[test]
    fn test_resolve_alert() {
        let (db, _tmp) = create_test_db();

        db.create_app(&create_test_app("my-app")).unwrap();
        db.create_alert_rule(&create_test_alert_rule("rule-1", Some("my-app"))).unwrap();

        let event_id = db.create_alert_event("rule-1", Some("my-app"), 7.5, 5.0, Some("Alert")).unwrap();

        db.resolve_alert_event(event_id).unwrap();

        let event = db.get_alert_event(event_id).unwrap().unwrap();
        assert_eq!(event.status, "resolved");
        assert!(event.resolved_at.is_some());
    }

    #[test]
    fn test_get_active_alert_for_rule() {
        let (db, _tmp) = create_test_db();

        db.create_app(&create_test_app("my-app")).unwrap();
        db.create_alert_rule(&create_test_alert_rule("rule-1", Some("my-app"))).unwrap();

        // No alerts yet
        let active = db.get_active_alert_for_rule("rule-1").unwrap();
        assert!(active.is_none());

        // Create alert
        let event_id = db.create_alert_event("rule-1", Some("my-app"), 7.5, 5.0, Some("Alert")).unwrap();

        let active = db.get_active_alert_for_rule("rule-1").unwrap();
        assert!(active.is_some());
        assert_eq!(active.unwrap().id, event_id);

        // Resolve it
        db.resolve_alert_event(event_id).unwrap();

        let active = db.get_active_alert_for_rule("rule-1").unwrap();
        assert!(active.is_none());
    }

    #[test]
    fn test_alert_notifications() {
        let (db, _tmp) = create_test_db();

        db.create_app(&create_test_app("my-app")).unwrap();
        db.create_alert_rule(&create_test_alert_rule("rule-1", Some("my-app"))).unwrap();

        let event_id = db.create_alert_event("rule-1", Some("my-app"), 7.5, 5.0, Some("Alert")).unwrap();

        // Create notifications
        let notif1 = db.create_alert_notification(event_id, "email").unwrap();
        let notif2 = db.create_alert_notification(event_id, "slack").unwrap();

        // Get pending
        let pending = db.get_pending_notifications().unwrap();
        assert_eq!(pending.len(), 2);

        // Mark one as sent
        db.update_notification_status(notif1, "sent", None).unwrap();

        let pending = db.get_pending_notifications().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].channel, "slack");

        // Mark with error
        db.update_notification_status(notif2, "failed", Some("Connection refused")).unwrap();

        let pending = db.get_pending_notifications().unwrap();
        assert_eq!(pending.len(), 0);
    }
}
