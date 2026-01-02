//! SQLite database for persistent app state
//!
//! This module provides durable storage for apps, addons, deployments,
//! and configuration variables that survives restarts.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{debug, info};

/// Current schema version for migrations
const SCHEMA_VERSION: i32 = 10;

/// Database connection wrapper with thread-safe access
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// Open or create a database at the given path
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)
            .context("Failed to open database")?;

        // Enable WAL mode for better concurrency
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        db.run_migrations()?;

        info!("Database opened at {}", path.display());
        Ok(db)
    }

    /// Open an in-memory database (for testing)
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .context("Failed to open in-memory database")?;

        conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        db.run_migrations()?;
        Ok(db)
    }

    /// Run database migrations
    fn run_migrations(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Create migrations table if it doesn't exist
        conn.execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            )",
            [],
        )?;

        // Get current version
        let current_version: i32 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if current_version < SCHEMA_VERSION {
            info!("Running migrations from v{} to v{}", current_version, SCHEMA_VERSION);

            if current_version < 1 {
                self.migrate_v1(&conn)?;
            }

            if current_version < 2 {
                self.migrate_v2(&conn)?;
            }

            if current_version < 3 {
                self.migrate_v3(&conn)?;
            }

            if current_version < 4 {
                self.migrate_v4(&conn)?;
            }

            if current_version < 5 {
                self.migrate_v5(&conn)?;
            }

            if current_version < 6 {
                self.migrate_v6(&conn)?;
            }

            if current_version < 7 {
                self.migrate_v7(&conn)?;
            }

            if current_version < 8 {
                self.migrate_v8(&conn)?;
            }

            if current_version < 9 {
                self.migrate_v9(&conn)?;
            }

            if current_version < 10 {
                self.migrate_v10(&conn)?;
            }
        }

        Ok(())
    }

    /// Migration v1: Initial schema
    fn migrate_v1(&self, conn: &Connection) -> Result<()> {
        debug!("Applying migration v1: initial schema");

        conn.execute_batch(r#"
            -- Apps table
            CREATE TABLE IF NOT EXISTS apps (
                name TEXT PRIMARY KEY,
                status TEXT NOT NULL DEFAULT 'idle',
                git_url TEXT,
                image TEXT,
                port INTEGER NOT NULL DEFAULT 3000,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                deployed_at TEXT,
                commit_hash TEXT
            );

            -- App environment variables
            CREATE TABLE IF NOT EXISTS app_config (
                app_name TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                is_secret INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (app_name, key),
                FOREIGN KEY (app_name) REFERENCES apps(name) ON DELETE CASCADE
            );

            -- App addons
            CREATE TABLE IF NOT EXISTS app_addons (
                id TEXT PRIMARY KEY,
                app_name TEXT NOT NULL,
                addon_type TEXT NOT NULL,
                plan TEXT NOT NULL DEFAULT 'hobby',
                container_id TEXT,
                container_name TEXT,
                connection_url TEXT,
                env_var_name TEXT,
                status TEXT NOT NULL DEFAULT 'provisioning',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (app_name) REFERENCES apps(name) ON DELETE CASCADE,
                UNIQUE (app_name, addon_type)
            );

            -- Deployments history
            CREATE TABLE IF NOT EXISTS deployments (
                id TEXT PRIMARY KEY,
                app_name TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                image TEXT,
                commit_hash TEXT,
                build_logs TEXT,
                duration_secs REAL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                finished_at TEXT,
                FOREIGN KEY (app_name) REFERENCES apps(name) ON DELETE CASCADE
            );

            -- Domains
            CREATE TABLE IF NOT EXISTS domains (
                domain TEXT PRIMARY KEY,
                app_name TEXT NOT NULL,
                verified INTEGER NOT NULL DEFAULT 0,
                ssl_enabled INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (app_name) REFERENCES apps(name) ON DELETE CASCADE
            );

            -- Create indexes
            CREATE INDEX IF NOT EXISTS idx_app_config_app ON app_config(app_name);
            CREATE INDEX IF NOT EXISTS idx_app_addons_app ON app_addons(app_name);
            CREATE INDEX IF NOT EXISTS idx_deployments_app ON deployments(app_name);
            CREATE INDEX IF NOT EXISTS idx_deployments_created ON deployments(created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_domains_app ON domains(app_name);

            -- Record migration
            INSERT INTO schema_migrations (version) VALUES (1);
        "#)?;

        Ok(())
    }

    /// Migration v2: Add scaling support
    fn migrate_v2(&self, conn: &Connection) -> Result<()> {
        debug!("Applying migration v2: scaling support");

        conn.execute_batch(r#"
            -- Add scaling columns to apps
            ALTER TABLE apps ADD COLUMN scale INTEGER NOT NULL DEFAULT 1;
            ALTER TABLE apps ADD COLUMN min_scale INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE apps ADD COLUMN max_scale INTEGER NOT NULL DEFAULT 10;

            -- App processes (running instances)
            CREATE TABLE IF NOT EXISTS app_processes (
                id TEXT PRIMARY KEY,
                app_name TEXT NOT NULL,
                process_type TEXT NOT NULL DEFAULT 'web',
                container_id TEXT,
                container_name TEXT,
                port INTEGER,
                status TEXT NOT NULL DEFAULT 'starting',
                health_status TEXT DEFAULT 'unknown',
                last_health_check TEXT,
                started_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (app_name) REFERENCES apps(name) ON DELETE CASCADE
            );

            -- Create indexes
            CREATE INDEX IF NOT EXISTS idx_app_processes_app ON app_processes(app_name);
            CREATE INDEX IF NOT EXISTS idx_app_processes_status ON app_processes(status);

            -- Record migration
            INSERT INTO schema_migrations (version) VALUES (2);
        "#)?;

        Ok(())
    }

    /// Migration v3: Secrets audit log
    fn migrate_v3(&self, conn: &Connection) -> Result<()> {
        debug!("Applying migration v3: secrets audit log");

        conn.execute_batch(r#"
            -- Secrets audit log
            CREATE TABLE IF NOT EXISTS secrets_audit_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                app_name TEXT NOT NULL,
                secret_key TEXT NOT NULL,
                action TEXT NOT NULL,
                actor TEXT,
                ip_address TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (app_name) REFERENCES apps(name) ON DELETE CASCADE
            );

            -- Create indexes for audit log queries
            CREATE INDEX IF NOT EXISTS idx_secrets_audit_app ON secrets_audit_log(app_name);
            CREATE INDEX IF NOT EXISTS idx_secrets_audit_time ON secrets_audit_log(created_at);

            -- Encryption keys table
            CREATE TABLE IF NOT EXISTS encryption_keys (
                id TEXT PRIMARY KEY,
                key_data TEXT NOT NULL,
                is_current INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                rotated_at TEXT
            );

            -- Record migration
            INSERT INTO schema_migrations (version) VALUES (3);
        "#)?;

        Ok(())
    }

    /// Migration v4: Webhooks and CI integration
    fn migrate_v4(&self, conn: &Connection) -> Result<()> {
        debug!("Applying migration v4: webhooks");

        conn.execute_batch(r#"
            -- Webhook configurations
            CREATE TABLE IF NOT EXISTS webhooks (
                app_name TEXT PRIMARY KEY,
                secret TEXT NOT NULL,
                provider TEXT NOT NULL DEFAULT 'github',
                deploy_branch TEXT NOT NULL DEFAULT 'main',
                auto_deploy INTEGER NOT NULL DEFAULT 1,
                status_token TEXT,
                repo_name TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (app_name) REFERENCES apps(name) ON DELETE CASCADE
            );

            -- Webhook events log
            CREATE TABLE IF NOT EXISTS webhook_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                app_name TEXT NOT NULL,
                event_type TEXT NOT NULL,
                provider TEXT NOT NULL,
                branch TEXT,
                commit_sha TEXT,
                commit_message TEXT,
                author TEXT,
                payload TEXT,
                triggered_deploy INTEGER NOT NULL DEFAULT 0,
                deployment_id TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (app_name) REFERENCES apps(name) ON DELETE CASCADE
            );

            -- Build status for badges
            CREATE TABLE IF NOT EXISTS build_status (
                app_name TEXT PRIMARY KEY,
                status TEXT NOT NULL DEFAULT 'unknown',
                commit_sha TEXT,
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (app_name) REFERENCES apps(name) ON DELETE CASCADE
            );

            -- Create indexes
            CREATE INDEX IF NOT EXISTS idx_webhook_events_app ON webhook_events(app_name);
            CREATE INDEX IF NOT EXISTS idx_webhook_events_time ON webhook_events(created_at);

            -- Record migration
            INSERT INTO schema_migrations (version) VALUES (4);
        "#)?;

        Ok(())
    }

    /// Migration v5: Custom domains
    fn migrate_v5(&self, conn: &Connection) -> Result<()> {
        debug!("Applying migration v5: custom domains");

        conn.execute_batch(r#"
            -- Custom domains table
            CREATE TABLE IF NOT EXISTS custom_domains (
                domain TEXT PRIMARY KEY,
                app_name TEXT NOT NULL,
                verified INTEGER NOT NULL DEFAULT 0,
                ssl_enabled INTEGER NOT NULL DEFAULT 0,
                verification_token TEXT,
                cert_path TEXT,
                key_path TEXT,
                cert_expires_at TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (app_name) REFERENCES apps(name) ON DELETE CASCADE
            );

            -- Index for looking up domains by app
            CREATE INDEX IF NOT EXISTS idx_domains_app ON custom_domains(app_name);

            -- Record migration
            INSERT INTO schema_migrations (version) VALUES (5);
        "#)?;

        Ok(())
    }

    /// Migration v6: API tokens
    fn migrate_v6(&self, conn: &Connection) -> Result<()> {
        debug!("Applying migration v6: API tokens");

        conn.execute_batch(r#"
            -- API tokens for programmatic access
            CREATE TABLE IF NOT EXISTS api_tokens (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                token_hash TEXT NOT NULL,
                token_prefix TEXT NOT NULL,
                scopes TEXT NOT NULL DEFAULT 'read',
                last_used_at TEXT,
                expires_at TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            -- API token usage log
            CREATE TABLE IF NOT EXISTS api_token_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                token_id TEXT NOT NULL,
                action TEXT NOT NULL,
                resource TEXT,
                ip_address TEXT,
                user_agent TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (token_id) REFERENCES api_tokens(id) ON DELETE CASCADE
            );

            -- Index for token lookup
            CREATE INDEX IF NOT EXISTS idx_api_tokens_prefix ON api_tokens(token_prefix);
            CREATE INDEX IF NOT EXISTS idx_api_token_log_token ON api_token_log(token_id);
            CREATE INDEX IF NOT EXISTS idx_api_token_log_time ON api_token_log(created_at);

            -- Record migration
            INSERT INTO schema_migrations (version) VALUES (6);
        "#)?;

        Ok(())
    }

    /// Migration v7: Metrics history
    fn migrate_v7(&self, conn: &Connection) -> Result<()> {
        debug!("Applying migration v7: metrics history");

        conn.execute_batch(r#"
            -- Request metrics (aggregated per minute)
            CREATE TABLE IF NOT EXISTS request_metrics (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                app_name TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                request_count INTEGER NOT NULL DEFAULT 0,
                error_count INTEGER NOT NULL DEFAULT 0,
                avg_response_time_ms REAL NOT NULL DEFAULT 0,
                p50_response_time_ms REAL,
                p95_response_time_ms REAL,
                p99_response_time_ms REAL,
                FOREIGN KEY (app_name) REFERENCES apps(name) ON DELETE CASCADE
            );

            -- Resource metrics (CPU, memory per instance)
            CREATE TABLE IF NOT EXISTS resource_metrics (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                app_name TEXT NOT NULL,
                instance_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                cpu_percent REAL NOT NULL DEFAULT 0,
                memory_used INTEGER NOT NULL DEFAULT 0,
                memory_limit INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY (app_name) REFERENCES apps(name) ON DELETE CASCADE
            );

            -- Indices for efficient time-series queries
            CREATE INDEX IF NOT EXISTS idx_request_metrics_app_time ON request_metrics(app_name, timestamp);
            CREATE INDEX IF NOT EXISTS idx_resource_metrics_app_time ON resource_metrics(app_name, timestamp);
            CREATE INDEX IF NOT EXISTS idx_resource_metrics_instance ON resource_metrics(instance_id, timestamp);

            -- Record migration
            INSERT INTO schema_migrations (version) VALUES (7);
        "#)?;

        Ok(())
    }

    /// Migration v8: Activity events
    fn migrate_v8(&self, conn: &Connection) -> Result<()> {
        debug!("Applying migration v8: activity events");

        conn.execute_batch(r#"
            -- Activity events for tracking all platform actions
            CREATE TABLE IF NOT EXISTS activity_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_type TEXT NOT NULL,
                action TEXT NOT NULL,
                app_name TEXT,
                resource_type TEXT,
                resource_id TEXT,
                actor TEXT,
                actor_type TEXT DEFAULT 'user',
                details TEXT,
                ip_address TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            -- Indices for efficient activity queries
            CREATE INDEX IF NOT EXISTS idx_activity_events_time ON activity_events(created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_activity_events_app ON activity_events(app_name, created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_activity_events_type ON activity_events(event_type, created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_activity_events_actor ON activity_events(actor, created_at DESC);

            -- Record migration
            INSERT INTO schema_migrations (version) VALUES (8);
        "#)?;

        Ok(())
    }

    /// Migration v9: Alerting rules
    fn migrate_v9(&self, conn: &Connection) -> Result<()> {
        debug!("Applying migration v9: alerting rules");

        conn.execute_batch(r#"
            -- Alert rules for monitoring conditions
            CREATE TABLE IF NOT EXISTS alert_rules (
                id TEXT PRIMARY KEY,
                app_name TEXT,
                name TEXT NOT NULL,
                description TEXT,
                metric_type TEXT NOT NULL,
                condition TEXT NOT NULL,
                threshold REAL NOT NULL,
                duration_secs INTEGER NOT NULL DEFAULT 60,
                severity TEXT NOT NULL DEFAULT 'warning',
                enabled INTEGER NOT NULL DEFAULT 1,
                notification_channels TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (app_name) REFERENCES apps(name) ON DELETE CASCADE
            );

            -- Alert events (triggered alerts)
            CREATE TABLE IF NOT EXISTS alert_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                rule_id TEXT NOT NULL,
                app_name TEXT,
                status TEXT NOT NULL DEFAULT 'firing',
                metric_value REAL NOT NULL,
                threshold REAL NOT NULL,
                message TEXT,
                started_at TEXT NOT NULL DEFAULT (datetime('now')),
                resolved_at TEXT,
                acknowledged_at TEXT,
                acknowledged_by TEXT,
                FOREIGN KEY (rule_id) REFERENCES alert_rules(id) ON DELETE CASCADE
            );

            -- Alert notification log
            CREATE TABLE IF NOT EXISTS alert_notifications (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                alert_event_id INTEGER NOT NULL,
                channel TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                sent_at TEXT,
                error_message TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (alert_event_id) REFERENCES alert_events(id) ON DELETE CASCADE
            );

            -- Indices for efficient queries
            CREATE INDEX IF NOT EXISTS idx_alert_rules_app ON alert_rules(app_name);
            CREATE INDEX IF NOT EXISTS idx_alert_rules_enabled ON alert_rules(enabled);
            CREATE INDEX IF NOT EXISTS idx_alert_events_rule ON alert_events(rule_id);
            CREATE INDEX IF NOT EXISTS idx_alert_events_status ON alert_events(status, started_at DESC);
            CREATE INDEX IF NOT EXISTS idx_alert_events_app ON alert_events(app_name, started_at DESC);
            CREATE INDEX IF NOT EXISTS idx_alert_notifications_event ON alert_notifications(alert_event_id);

            -- Record migration
            INSERT INTO schema_migrations (version) VALUES (9);
        "#)?;

        Ok(())
    }

    /// Migration v10: App formations (process types with individual scaling)
    fn migrate_v10(&self, conn: &Connection) -> Result<()> {
        debug!("Applying migration v10: app formations");

        conn.execute_batch(r#"
            -- App formations (process types and their scaling config)
            CREATE TABLE IF NOT EXISTS app_formations (
                app_name TEXT NOT NULL,
                process_type TEXT NOT NULL,
                quantity INTEGER NOT NULL DEFAULT 1,
                size TEXT NOT NULL DEFAULT 'standard',
                command TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (app_name, process_type),
                FOREIGN KEY (app_name) REFERENCES apps(name) ON DELETE CASCADE
            );

            -- Index for efficient lookups
            CREATE INDEX IF NOT EXISTS idx_app_formations_app ON app_formations(app_name);

            -- Record migration
            INSERT INTO schema_migrations (version) VALUES (10);
        "#)?;

        Ok(())
    }

    // ==================== App Operations ====================

    /// Create a new app
    pub fn create_app(&self, app: &AppRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO apps (name, status, git_url, port) VALUES (?1, ?2, ?3, ?4)",
            params![app.name, app.status, app.git_url, app.port],
        )?;
        Ok(())
    }

    /// Get an app by name
    pub fn get_app(&self, name: &str) -> Result<Option<AppRecord>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT name, status, git_url, image, port, created_at, deployed_at, commit_hash,
                    COALESCE(scale, 1), COALESCE(min_scale, 0), COALESCE(max_scale, 10)
             FROM apps WHERE name = ?1",
            params![name],
            |row| {
                Ok(AppRecord {
                    name: row.get(0)?,
                    status: row.get(1)?,
                    git_url: row.get(2)?,
                    image: row.get(3)?,
                    port: row.get(4)?,
                    created_at: row.get(5)?,
                    deployed_at: row.get(6)?,
                    commit_hash: row.get(7)?,
                    scale: row.get(8)?,
                    min_scale: row.get(9)?,
                    max_scale: row.get(10)?,
                })
            },
        )
        .optional()
        .context("Failed to get app")
    }

    /// List all apps
    pub fn list_apps(&self) -> Result<Vec<AppRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT name, status, git_url, image, port, created_at, deployed_at, commit_hash,
                    COALESCE(scale, 1), COALESCE(min_scale, 0), COALESCE(max_scale, 10)
             FROM apps ORDER BY created_at DESC"
        )?;

        let apps = stmt.query_map([], |row| {
            Ok(AppRecord {
                name: row.get(0)?,
                status: row.get(1)?,
                git_url: row.get(2)?,
                image: row.get(3)?,
                port: row.get(4)?,
                created_at: row.get(5)?,
                deployed_at: row.get(6)?,
                commit_hash: row.get(7)?,
                scale: row.get(8)?,
                min_scale: row.get(9)?,
                max_scale: row.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(apps)
    }

    /// Update app status
    pub fn update_app_status(&self, name: &str, status: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE apps SET status = ?1 WHERE name = ?2",
            params![status, name],
        )?;
        Ok(())
    }

    /// Update app after deployment
    pub fn update_app_deployment(&self, name: &str, image: &str, commit: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE apps SET image = ?1, commit_hash = ?2, deployed_at = datetime('now') WHERE name = ?3",
            params![image, commit, name],
        )?;
        Ok(())
    }

    /// Delete an app
    pub fn delete_app(&self, name: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM apps WHERE name = ?1", params![name])?;
        Ok(rows > 0)
    }

    // ==================== Config Operations ====================

    /// Set a config value
    pub fn set_config(&self, app_name: &str, key: &str, value: &str, is_secret: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO app_config (app_name, key, value, is_secret, updated_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))
             ON CONFLICT(app_name, key) DO UPDATE SET
                value = excluded.value,
                is_secret = excluded.is_secret,
                updated_at = datetime('now')",
            params![app_name, key, value, is_secret],
        )?;
        Ok(())
    }

    /// Get a config value
    pub fn get_config(&self, app_name: &str, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM app_config WHERE app_name = ?1 AND key = ?2",
            params![app_name, key],
            |row| row.get(0),
        )
        .optional()
        .context("Failed to get config")
    }

    /// Get all config for an app
    pub fn get_all_config(&self, app_name: &str) -> Result<HashMap<String, String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT key, value FROM app_config WHERE app_name = ?1"
        )?;

        let mut config = HashMap::new();
        let rows = stmt.query_map(params![app_name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        for row in rows {
            let (key, value) = row?;
            config.insert(key, value);
        }

        Ok(config)
    }

    /// Delete a config value
    pub fn delete_config(&self, app_name: &str, key: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM app_config WHERE app_name = ?1 AND key = ?2",
            params![app_name, key],
        )?;
        Ok(rows > 0)
    }

    // ==================== Addon Operations ====================

    /// Create an addon
    pub fn create_addon(&self, addon: &AddonRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO app_addons (id, app_name, addon_type, plan, container_id, container_name, connection_url, env_var_name, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                addon.id, addon.app_name, addon.addon_type, addon.plan,
                addon.container_id, addon.container_name, addon.connection_url,
                addon.env_var_name, addon.status
            ],
        )?;
        Ok(())
    }

    /// Get addons for an app
    pub fn get_app_addons(&self, app_name: &str) -> Result<Vec<AddonRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, app_name, addon_type, plan, container_id, container_name, connection_url, env_var_name, status, created_at
             FROM app_addons WHERE app_name = ?1"
        )?;

        let addons = stmt.query_map(params![app_name], |row| {
            Ok(AddonRecord {
                id: row.get(0)?,
                app_name: row.get(1)?,
                addon_type: row.get(2)?,
                plan: row.get(3)?,
                container_id: row.get(4)?,
                container_name: row.get(5)?,
                connection_url: row.get(6)?,
                env_var_name: row.get(7)?,
                status: row.get(8)?,
                created_at: row.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(addons)
    }

    /// Get addon by app and type
    pub fn get_addon(&self, app_name: &str, addon_type: &str) -> Result<Option<AddonRecord>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, app_name, addon_type, plan, container_id, container_name, connection_url, env_var_name, status, created_at
             FROM app_addons WHERE app_name = ?1 AND addon_type = ?2",
            params![app_name, addon_type],
            |row| {
                Ok(AddonRecord {
                    id: row.get(0)?,
                    app_name: row.get(1)?,
                    addon_type: row.get(2)?,
                    plan: row.get(3)?,
                    container_id: row.get(4)?,
                    container_name: row.get(5)?,
                    connection_url: row.get(6)?,
                    env_var_name: row.get(7)?,
                    status: row.get(8)?,
                    created_at: row.get(9)?,
                })
            },
        )
        .optional()
        .context("Failed to get addon")
    }

    /// Update addon status
    pub fn update_addon(&self, id: &str, container_id: &str, connection_url: &str, status: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE app_addons SET container_id = ?1, connection_url = ?2, status = ?3 WHERE id = ?4",
            params![container_id, connection_url, status, id],
        )?;
        Ok(())
    }

    /// Delete addon
    pub fn delete_addon(&self, app_name: &str, addon_type: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM app_addons WHERE app_name = ?1 AND addon_type = ?2",
            params![app_name, addon_type],
        )?;
        Ok(rows > 0)
    }

    // ==================== Deployment Operations ====================

    /// Create a deployment record
    pub fn create_deployment(&self, deployment: &DeploymentRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO deployments (id, app_name, status, commit_hash)
             VALUES (?1, ?2, ?3, ?4)",
            params![deployment.id, deployment.app_name, deployment.status, deployment.commit_hash],
        )?;
        Ok(())
    }

    /// Update deployment after build
    pub fn update_deployment(
        &self,
        id: &str,
        status: &str,
        image: Option<&str>,
        logs: Option<&str>,
        duration: Option<f64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE deployments SET
                status = ?1,
                image = ?2,
                build_logs = ?3,
                duration_secs = ?4,
                finished_at = datetime('now')
             WHERE id = ?5",
            params![status, image, logs, duration, id],
        )?;
        Ok(())
    }

    /// Get recent deployments for an app
    pub fn get_deployments(&self, app_name: &str, limit: usize) -> Result<Vec<DeploymentRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, app_name, status, image, commit_hash, build_logs, duration_secs, created_at, finished_at
             FROM deployments WHERE app_name = ?1 ORDER BY created_at DESC LIMIT ?2"
        )?;

        let deployments = stmt.query_map(params![app_name, limit as i64], |row| {
            Ok(DeploymentRecord {
                id: row.get(0)?,
                app_name: row.get(1)?,
                status: row.get(2)?,
                image: row.get(3)?,
                commit_hash: row.get(4)?,
                build_logs: row.get(5)?,
                duration_secs: row.get(6)?,
                created_at: row.get(7)?,
                finished_at: row.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(deployments)
    }

    // ==================== Scaling Operations ====================

    /// Update app scale
    pub fn update_app_scale(&self, name: &str, scale: i32) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE apps SET scale = ?1 WHERE name = ?2",
            params![scale, name],
        )?;
        Ok(())
    }

    /// Get app scale
    pub fn get_app_scale(&self, name: &str) -> Result<Option<i32>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(scale, 1) FROM apps WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )
        .optional()
        .context("Failed to get app scale")
    }

    // ==================== Process Operations ====================

    /// Create a process record
    pub fn create_process(&self, process: &ProcessRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO app_processes (id, app_name, process_type, container_id, container_name, port, status, health_status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                process.id, process.app_name, process.process_type,
                process.container_id, process.container_name, process.port,
                process.status, process.health_status
            ],
        )?;
        Ok(())
    }

    /// Get processes for an app
    pub fn get_app_processes(&self, app_name: &str) -> Result<Vec<ProcessRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, app_name, process_type, container_id, container_name, port, status, health_status, last_health_check, started_at
             FROM app_processes WHERE app_name = ?1 ORDER BY started_at DESC"
        )?;

        let processes = stmt.query_map(params![app_name], |row| {
            Ok(ProcessRecord {
                id: row.get(0)?,
                app_name: row.get(1)?,
                process_type: row.get(2)?,
                container_id: row.get(3)?,
                container_name: row.get(4)?,
                port: row.get(5)?,
                status: row.get(6)?,
                health_status: row.get(7)?,
                last_health_check: row.get(8)?,
                started_at: row.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(processes)
    }

    /// Get running process count for an app
    pub fn get_running_process_count(&self, app_name: &str) -> Result<i32> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM app_processes WHERE app_name = ?1 AND status = 'running'",
            params![app_name],
            |row| row.get(0),
        )
        .context("Failed to count processes")
    }

    /// Update process status
    pub fn update_process_status(&self, id: &str, status: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE app_processes SET status = ?1 WHERE id = ?2",
            params![status, id],
        )?;
        Ok(())
    }

    /// Update process health status
    pub fn update_process_health(&self, id: &str, health_status: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE app_processes SET health_status = ?1, last_health_check = datetime('now') WHERE id = ?2",
            params![health_status, id],
        )?;
        Ok(())
    }

    /// Delete a process record
    pub fn delete_process(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM app_processes WHERE id = ?1", params![id])?;
        Ok(rows > 0)
    }

    /// Delete all processes for an app
    pub fn delete_app_processes(&self, app_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM app_processes WHERE app_name = ?1", params![app_name])?;
        Ok(())
    }

    // ==================== Secrets Audit Log Operations ====================

    /// Log a secret access event
    pub fn log_secret_access(
        &self,
        app_name: &str,
        secret_key: &str,
        action: &str,
        actor: Option<&str>,
        ip_address: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO secrets_audit_log (app_name, secret_key, action, actor, ip_address)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![app_name, secret_key, action, actor, ip_address],
        )?;
        debug!(
            app = app_name,
            key = secret_key,
            action = action,
            "Secret access logged"
        );
        Ok(())
    }

    /// Get audit log for an app
    pub fn get_secret_audit_log(&self, app_name: &str, limit: usize) -> Result<Vec<SecretAuditRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, app_name, secret_key, action, actor, ip_address, created_at
             FROM secrets_audit_log WHERE app_name = ?1 ORDER BY created_at DESC LIMIT ?2"
        )?;

        let records = stmt.query_map(params![app_name, limit as i64], |row| {
            Ok(SecretAuditRecord {
                id: row.get(0)?,
                app_name: row.get(1)?,
                secret_key: row.get(2)?,
                action: row.get(3)?,
                actor: row.get(4)?,
                ip_address: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Get all audit log entries (for admin)
    pub fn get_all_secret_audit_log(&self, limit: usize) -> Result<Vec<SecretAuditRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, app_name, secret_key, action, actor, ip_address, created_at
             FROM secrets_audit_log ORDER BY created_at DESC LIMIT ?1"
        )?;

        let records = stmt.query_map(params![limit as i64], |row| {
            Ok(SecretAuditRecord {
                id: row.get(0)?,
                app_name: row.get(1)?,
                secret_key: row.get(2)?,
                action: row.get(3)?,
                actor: row.get(4)?,
                ip_address: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    // ==================== Encryption Key Operations ====================

    /// Save an encryption key
    pub fn save_encryption_key(&self, id: &str, key_data: &str, is_current: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // If this is the current key, unset any existing current key
        if is_current {
            conn.execute(
                "UPDATE encryption_keys SET is_current = 0, rotated_at = datetime('now') WHERE is_current = 1",
                [],
            )?;
        }

        conn.execute(
            "INSERT INTO encryption_keys (id, key_data, is_current)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET is_current = excluded.is_current",
            params![id, key_data, is_current],
        )?;

        info!(key_id = id, is_current = is_current, "Encryption key saved");
        Ok(())
    }

    /// Get the current encryption key
    pub fn get_current_encryption_key(&self) -> Result<Option<EncryptionKeyRecord>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, key_data, is_current, created_at, rotated_at
             FROM encryption_keys WHERE is_current = 1",
            [],
            |row| {
                Ok(EncryptionKeyRecord {
                    id: row.get(0)?,
                    key_data: row.get(1)?,
                    is_current: row.get(2)?,
                    created_at: row.get(3)?,
                    rotated_at: row.get(4)?,
                })
            },
        )
        .optional()
        .context("Failed to get current encryption key")
    }

    /// Get all encryption keys (for rotation/decryption)
    pub fn get_all_encryption_keys(&self) -> Result<Vec<EncryptionKeyRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, key_data, is_current, created_at, rotated_at
             FROM encryption_keys ORDER BY created_at DESC"
        )?;

        let keys = stmt.query_map([], |row| {
            Ok(EncryptionKeyRecord {
                id: row.get(0)?,
                key_data: row.get(1)?,
                is_current: row.get(2)?,
                created_at: row.get(3)?,
                rotated_at: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(keys)
    }

    /// Delete old encryption keys (keep N most recent)
    pub fn cleanup_old_encryption_keys(&self, keep_count: usize) -> Result<usize> {
        let conn = self.conn.lock().unwrap();

        // Get IDs to keep
        let mut stmt = conn.prepare(
            "SELECT id FROM encryption_keys ORDER BY created_at DESC LIMIT ?1"
        )?;
        let keep_ids: Vec<String> = stmt.query_map(params![keep_count as i64], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        if keep_ids.is_empty() {
            return Ok(0);
        }

        // Delete others
        let placeholders: Vec<String> = keep_ids.iter().enumerate().map(|(i, _)| format!("?{}", i + 1)).collect();
        let sql = format!(
            "DELETE FROM encryption_keys WHERE id NOT IN ({})",
            placeholders.join(",")
        );

        let params: Vec<&dyn rusqlite::ToSql> = keep_ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let deleted = conn.execute(&sql, params.as_slice())?;

        if deleted > 0 {
            info!(deleted = deleted, kept = keep_count, "Cleaned up old encryption keys");
        }

        Ok(deleted)
    }

    // ==================== Secret Config Operations ====================

    /// Get config entries marked as secrets
    pub fn get_secret_keys(&self, app_name: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT key FROM app_config WHERE app_name = ?1 AND is_secret = 1"
        )?;

        let keys = stmt.query_map(params![app_name], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(keys)
    }

    /// Check if a config key is marked as secret
    pub fn is_secret_key(&self, app_name: &str, key: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT is_secret FROM app_config WHERE app_name = ?1 AND key = ?2",
            params![app_name, key],
            |row| row.get::<_, bool>(0),
        )
        .optional()
        .map(|opt| opt.unwrap_or(false))
        .context("Failed to check secret key")
    }

    // ==================== Webhook Operations ====================

    /// Create or update webhook configuration
    pub fn save_webhook(&self, webhook: &WebhookRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO webhooks (app_name, secret, provider, deploy_branch, auto_deploy, status_token, repo_name, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))
             ON CONFLICT(app_name) DO UPDATE SET
                secret = excluded.secret,
                provider = excluded.provider,
                deploy_branch = excluded.deploy_branch,
                auto_deploy = excluded.auto_deploy,
                status_token = excluded.status_token,
                repo_name = excluded.repo_name,
                updated_at = datetime('now')",
            params![
                webhook.app_name, webhook.secret, webhook.provider,
                webhook.deploy_branch, webhook.auto_deploy,
                webhook.status_token, webhook.repo_name
            ],
        )?;
        info!(app = %webhook.app_name, "Webhook configuration saved");
        Ok(())
    }

    /// Get webhook configuration for an app
    pub fn get_webhook(&self, app_name: &str) -> Result<Option<WebhookRecord>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT app_name, secret, provider, deploy_branch, auto_deploy, status_token, repo_name, created_at, updated_at
             FROM webhooks WHERE app_name = ?1",
            params![app_name],
            |row| {
                Ok(WebhookRecord {
                    app_name: row.get(0)?,
                    secret: row.get(1)?,
                    provider: row.get(2)?,
                    deploy_branch: row.get(3)?,
                    auto_deploy: row.get(4)?,
                    status_token: row.get(5)?,
                    repo_name: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            },
        )
        .optional()
        .context("Failed to get webhook")
    }

    /// Delete webhook configuration
    pub fn delete_webhook(&self, app_name: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM webhooks WHERE app_name = ?1", params![app_name])?;
        Ok(rows > 0)
    }

    /// Log a webhook event
    pub fn log_webhook_event(&self, event: &WebhookEventRecord) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO webhook_events (app_name, event_type, provider, branch, commit_sha, commit_message, author, payload, triggered_deploy, deployment_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                event.app_name, event.event_type, event.provider, event.branch,
                event.commit_sha, event.commit_message, event.author, event.payload,
                event.triggered_deploy, event.deployment_id
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Get recent webhook events for an app
    pub fn get_webhook_events(&self, app_name: &str, limit: usize) -> Result<Vec<WebhookEventRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, app_name, event_type, provider, branch, commit_sha, commit_message, author, payload, triggered_deploy, deployment_id, created_at
             FROM webhook_events WHERE app_name = ?1 ORDER BY created_at DESC LIMIT ?2"
        )?;

        let events = stmt.query_map(params![app_name, limit as i64], |row| {
            Ok(WebhookEventRecord {
                id: Some(row.get(0)?),
                app_name: row.get(1)?,
                event_type: row.get(2)?,
                provider: row.get(3)?,
                branch: row.get(4)?,
                commit_sha: row.get(5)?,
                commit_message: row.get(6)?,
                author: row.get(7)?,
                payload: row.get(8)?,
                triggered_deploy: row.get(9)?,
                deployment_id: row.get(10)?,
                created_at: row.get(11)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(events)
    }

    /// Update build status for an app
    pub fn update_build_status(&self, app_name: &str, status: &str, commit_sha: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO build_status (app_name, status, commit_sha, updated_at)
             VALUES (?1, ?2, ?3, datetime('now'))
             ON CONFLICT(app_name) DO UPDATE SET
                status = excluded.status,
                commit_sha = excluded.commit_sha,
                updated_at = datetime('now')",
            params![app_name, status, commit_sha],
        )?;
        Ok(())
    }

    /// Get build status for an app
    pub fn get_build_status(&self, app_name: &str) -> Result<Option<BuildStatusRecord>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT app_name, status, commit_sha, updated_at FROM build_status WHERE app_name = ?1",
            params![app_name],
            |row| {
                Ok(BuildStatusRecord {
                    app_name: row.get(0)?,
                    status: row.get(1)?,
                    commit_sha: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            },
        )
        .optional()
        .context("Failed to get build status")
    }

    // ==================== Custom Domains ====================

    /// Add a custom domain to an app
    pub fn add_domain(&self, domain: &str, app_name: &str, verification_token: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO custom_domains (domain, app_name, verification_token)
             VALUES (?1, ?2, ?3)",
            params![domain, app_name, verification_token],
        )?;
        Ok(())
    }

    /// Get a domain by name
    pub fn get_domain(&self, domain: &str) -> Result<Option<CustomDomainRecord>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT domain, app_name, verified, ssl_enabled, verification_token, cert_path, key_path, cert_expires_at, created_at
             FROM custom_domains WHERE domain = ?1",
            params![domain],
            |row| {
                Ok(CustomDomainRecord {
                    domain: row.get(0)?,
                    app_name: row.get(1)?,
                    verified: row.get(2)?,
                    ssl_enabled: row.get(3)?,
                    verification_token: row.get(4)?,
                    cert_path: row.get(5)?,
                    key_path: row.get(6)?,
                    cert_expires_at: row.get(7)?,
                    created_at: row.get(8)?,
                })
            },
        )
        .optional()
        .context("Failed to get domain")
    }

    /// Get all domains for an app
    pub fn get_app_domains(&self, app_name: &str) -> Result<Vec<CustomDomainRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT domain, app_name, verified, ssl_enabled, verification_token, cert_path, key_path, cert_expires_at, created_at
             FROM custom_domains WHERE app_name = ?1 ORDER BY created_at"
        )?;

        let domains = stmt.query_map(params![app_name], |row| {
            Ok(CustomDomainRecord {
                domain: row.get(0)?,
                app_name: row.get(1)?,
                verified: row.get(2)?,
                ssl_enabled: row.get(3)?,
                verification_token: row.get(4)?,
                cert_path: row.get(5)?,
                key_path: row.get(6)?,
                cert_expires_at: row.get(7)?,
                created_at: row.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(domains)
    }

    /// Get all custom domains
    pub fn get_all_domains(&self) -> Result<Vec<CustomDomainRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT domain, app_name, verified, ssl_enabled, verification_token, cert_path, key_path, cert_expires_at, created_at
             FROM custom_domains ORDER BY app_name, created_at"
        )?;

        let domains = stmt.query_map([], |row| {
            Ok(CustomDomainRecord {
                domain: row.get(0)?,
                app_name: row.get(1)?,
                verified: row.get(2)?,
                ssl_enabled: row.get(3)?,
                verification_token: row.get(4)?,
                cert_path: row.get(5)?,
                key_path: row.get(6)?,
                cert_expires_at: row.get(7)?,
                created_at: row.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(domains)
    }

    /// Delete a domain
    pub fn delete_domain(&self, domain: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute(
            "DELETE FROM custom_domains WHERE domain = ?1",
            params![domain],
        )?;
        Ok(affected > 0)
    }

    /// Update domain verification status
    pub fn update_domain_verification(&self, domain: &str, verified: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE custom_domains SET verified = ?1, updated_at = datetime('now') WHERE domain = ?2",
            params![verified, domain],
        )?;
        Ok(())
    }

    /// Update domain SSL configuration
    pub fn update_domain_ssl(&self, domain: &str, ssl_enabled: bool, cert_path: Option<&str>, key_path: Option<&str>, expires_at: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE custom_domains SET ssl_enabled = ?1, cert_path = ?2, key_path = ?3, cert_expires_at = ?4, updated_at = datetime('now') WHERE domain = ?5",
            params![ssl_enabled, cert_path, key_path, expires_at, domain],
        )?;
        Ok(())
    }

    /// Find app by domain (for routing)
    pub fn find_app_by_domain(&self, domain: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();

        // First try exact match
        if let Some(app_name) = conn.query_row(
            "SELECT app_name FROM custom_domains WHERE domain = ?1 AND verified = 1",
            params![domain],
            |row| row.get::<_, String>(0),
        ).optional()? {
            return Ok(Some(app_name));
        }

        // Try wildcard match (e.g., *.example.com matches foo.example.com)
        let parts: Vec<&str> = domain.splitn(2, '.').collect();
        if parts.len() == 2 {
            let wildcard = format!("*.{}", parts[1]);
            if let Some(app_name) = conn.query_row(
                "SELECT app_name FROM custom_domains WHERE domain = ?1 AND verified = 1",
                params![wildcard],
                |row| row.get::<_, String>(0),
            ).optional()? {
                return Ok(Some(app_name));
            }
        }

        Ok(None)
    }

    // ==================== API Token Operations ====================

    /// Create a new API token
    pub fn create_api_token(&self, token: &ApiTokenRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO api_tokens (id, name, token_hash, token_prefix, scopes, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                token.id, token.name, token.token_hash, token.token_prefix,
                token.scopes, token.expires_at
            ],
        )?;
        info!(token_id = %token.id, name = %token.name, "API token created");
        Ok(())
    }

    /// Get all API tokens (without hash)
    pub fn list_api_tokens(&self) -> Result<Vec<ApiTokenRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, token_hash, token_prefix, scopes, last_used_at, expires_at, created_at
             FROM api_tokens ORDER BY created_at DESC"
        )?;

        let tokens = stmt.query_map([], |row| {
            Ok(ApiTokenRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                token_hash: row.get(2)?,
                token_prefix: row.get(3)?,
                scopes: row.get(4)?,
                last_used_at: row.get(5)?,
                expires_at: row.get(6)?,
                created_at: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(tokens)
    }

    /// Get API token by ID
    pub fn get_api_token(&self, id: &str) -> Result<Option<ApiTokenRecord>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, name, token_hash, token_prefix, scopes, last_used_at, expires_at, created_at
             FROM api_tokens WHERE id = ?1",
            params![id],
            |row| {
                Ok(ApiTokenRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    token_hash: row.get(2)?,
                    token_prefix: row.get(3)?,
                    scopes: row.get(4)?,
                    last_used_at: row.get(5)?,
                    expires_at: row.get(6)?,
                    created_at: row.get(7)?,
                })
            },
        )
        .optional()
        .context("Failed to get API token")
    }

    /// Find API token by prefix (for quick lookup)
    pub fn find_api_token_by_prefix(&self, prefix: &str) -> Result<Option<ApiTokenRecord>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, name, token_hash, token_prefix, scopes, last_used_at, expires_at, created_at
             FROM api_tokens WHERE token_prefix = ?1",
            params![prefix],
            |row| {
                Ok(ApiTokenRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    token_hash: row.get(2)?,
                    token_prefix: row.get(3)?,
                    scopes: row.get(4)?,
                    last_used_at: row.get(5)?,
                    expires_at: row.get(6)?,
                    created_at: row.get(7)?,
                })
            },
        )
        .optional()
        .context("Failed to find API token")
    }

    /// Update token last used timestamp
    pub fn update_api_token_last_used(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE api_tokens SET last_used_at = datetime('now') WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// Delete an API token
    pub fn delete_api_token(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM api_tokens WHERE id = ?1", params![id])?;
        if rows > 0 {
            info!(token_id = %id, "API token deleted");
        }
        Ok(rows > 0)
    }

    /// Log API token usage
    pub fn log_api_token_usage(
        &self,
        token_id: &str,
        action: &str,
        resource: Option<&str>,
        ip_address: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO api_token_log (token_id, action, resource, ip_address, user_agent)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![token_id, action, resource, ip_address, user_agent],
        )?;
        Ok(())
    }

    /// Get API token usage log
    pub fn get_api_token_log(&self, token_id: &str, limit: usize) -> Result<Vec<ApiTokenLogRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, token_id, action, resource, ip_address, user_agent, created_at
             FROM api_token_log WHERE token_id = ?1 ORDER BY created_at DESC LIMIT ?2"
        )?;

        let logs = stmt.query_map(params![token_id, limit as i64], |row| {
            Ok(ApiTokenLogRecord {
                id: row.get(0)?,
                token_id: row.get(1)?,
                action: row.get(2)?,
                resource: row.get(3)?,
                ip_address: row.get(4)?,
                user_agent: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(logs)
    }

    // ==================== Metrics Operations ====================

    /// Record request metrics
    pub fn record_request_metrics(&self, metrics: &RequestMetricsRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO request_metrics (app_name, timestamp, request_count, error_count, avg_response_time_ms, p50_response_time_ms, p95_response_time_ms, p99_response_time_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                metrics.app_name, metrics.timestamp, metrics.request_count,
                metrics.error_count, metrics.avg_response_time_ms,
                metrics.p50_response_time_ms, metrics.p95_response_time_ms, metrics.p99_response_time_ms
            ],
        )?;
        Ok(())
    }

    /// Record resource metrics
    pub fn record_resource_metrics(&self, metrics: &ResourceMetricsRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO resource_metrics (app_name, instance_id, timestamp, cpu_percent, memory_used, memory_limit)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                metrics.app_name, metrics.instance_id, metrics.timestamp,
                metrics.cpu_percent, metrics.memory_used, metrics.memory_limit
            ],
        )?;
        Ok(())
    }

    /// Get request metrics for time range
    pub fn get_request_metrics(&self, app_name: &str, since: &str, limit: usize) -> Result<Vec<RequestMetricsRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, app_name, timestamp, request_count, error_count, avg_response_time_ms, p50_response_time_ms, p95_response_time_ms, p99_response_time_ms
             FROM request_metrics WHERE app_name = ?1 AND timestamp >= ?2 ORDER BY timestamp DESC LIMIT ?3"
        )?;

        let records = stmt.query_map(params![app_name, since, limit as i64], |row| {
            Ok(RequestMetricsRecord {
                id: row.get(0)?,
                app_name: row.get(1)?,
                timestamp: row.get(2)?,
                request_count: row.get(3)?,
                error_count: row.get(4)?,
                avg_response_time_ms: row.get(5)?,
                p50_response_time_ms: row.get(6)?,
                p95_response_time_ms: row.get(7)?,
                p99_response_time_ms: row.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Get resource metrics for time range
    pub fn get_resource_metrics(&self, app_name: &str, since: &str, limit: usize) -> Result<Vec<ResourceMetricsRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, app_name, instance_id, timestamp, cpu_percent, memory_used, memory_limit
             FROM resource_metrics WHERE app_name = ?1 AND timestamp >= ?2 ORDER BY timestamp DESC LIMIT ?3"
        )?;

        let records = stmt.query_map(params![app_name, since, limit as i64], |row| {
            Ok(ResourceMetricsRecord {
                id: row.get(0)?,
                app_name: row.get(1)?,
                instance_id: row.get(2)?,
                timestamp: row.get(3)?,
                cpu_percent: row.get(4)?,
                memory_used: row.get(5)?,
                memory_limit: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Get resource metrics for a specific instance
    pub fn get_instance_metrics(&self, instance_id: &str, since: &str, limit: usize) -> Result<Vec<ResourceMetricsRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, app_name, instance_id, timestamp, cpu_percent, memory_used, memory_limit
             FROM resource_metrics WHERE instance_id = ?1 AND timestamp >= ?2 ORDER BY timestamp DESC LIMIT ?3"
        )?;

        let records = stmt.query_map(params![instance_id, since, limit as i64], |row| {
            Ok(ResourceMetricsRecord {
                id: row.get(0)?,
                app_name: row.get(1)?,
                instance_id: row.get(2)?,
                timestamp: row.get(3)?,
                cpu_percent: row.get(4)?,
                memory_used: row.get(5)?,
                memory_limit: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Cleanup old metrics (keep last N days)
    pub fn cleanup_old_metrics(&self, days: i64) -> Result<(usize, usize)> {
        let conn = self.conn.lock().unwrap();
        let cutoff = format!("datetime('now', '-{} days')", days);

        let request_deleted = conn.execute(
            &format!("DELETE FROM request_metrics WHERE timestamp < {}", cutoff),
            [],
        )?;

        let resource_deleted = conn.execute(
            &format!("DELETE FROM resource_metrics WHERE timestamp < {}", cutoff),
            [],
        )?;

        Ok((request_deleted, resource_deleted))
    }

    // ==================== Activity Operations ====================

    /// Log an activity event
    pub fn log_activity(
        &self,
        event_type: &str,
        action: &str,
        app_name: Option<&str>,
        resource_type: Option<&str>,
        resource_id: Option<&str>,
        actor: Option<&str>,
        actor_type: &str,
        details: Option<&str>,
        ip_address: Option<&str>,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO activity_events (event_type, action, app_name, resource_type, resource_id, actor, actor_type, details, ip_address)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![event_type, action, app_name, resource_type, resource_id, actor, actor_type, details, ip_address],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Get activity events for an app
    pub fn get_app_activity(&self, app_name: &str, limit: usize) -> Result<Vec<ActivityEventRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, event_type, action, app_name, resource_type, resource_id, actor, actor_type, details, ip_address, created_at
             FROM activity_events WHERE app_name = ?1 ORDER BY created_at DESC LIMIT ?2"
        )?;

        let records = stmt.query_map(params![app_name, limit as i64], |row| {
            Ok(ActivityEventRecord {
                id: row.get(0)?,
                event_type: row.get(1)?,
                action: row.get(2)?,
                app_name: row.get(3)?,
                resource_type: row.get(4)?,
                resource_id: row.get(5)?,
                actor: row.get(6)?,
                actor_type: row.get(7)?,
                details: row.get(8)?,
                ip_address: row.get(9)?,
                created_at: row.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Get all activity events (platform-wide)
    pub fn get_all_activity(&self, limit: usize) -> Result<Vec<ActivityEventRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, event_type, action, app_name, resource_type, resource_id, actor, actor_type, details, ip_address, created_at
             FROM activity_events ORDER BY created_at DESC LIMIT ?1"
        )?;

        let records = stmt.query_map(params![limit as i64], |row| {
            Ok(ActivityEventRecord {
                id: row.get(0)?,
                event_type: row.get(1)?,
                action: row.get(2)?,
                app_name: row.get(3)?,
                resource_type: row.get(4)?,
                resource_id: row.get(5)?,
                actor: row.get(6)?,
                actor_type: row.get(7)?,
                details: row.get(8)?,
                ip_address: row.get(9)?,
                created_at: row.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Get filtered activity events
    pub fn get_filtered_activity(
        &self,
        app_name: Option<&str>,
        event_type: Option<&str>,
        actor: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ActivityEventRecord>> {
        let conn = self.conn.lock().unwrap();

        let mut sql = String::from(
            "SELECT id, event_type, action, app_name, resource_type, resource_id, actor, actor_type, details, ip_address, created_at
             FROM activity_events WHERE 1=1"
        );

        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let mut param_idx = 1;

        if let Some(app) = app_name {
            sql.push_str(&format!(" AND app_name = ?{}", param_idx));
            params_vec.push(Box::new(app.to_string()));
            param_idx += 1;
        }

        if let Some(et) = event_type {
            sql.push_str(&format!(" AND event_type = ?{}", param_idx));
            params_vec.push(Box::new(et.to_string()));
            param_idx += 1;
        }

        if let Some(a) = actor {
            sql.push_str(&format!(" AND actor = ?{}", param_idx));
            params_vec.push(Box::new(a.to_string()));
            param_idx += 1;
        }

        sql.push_str(&format!(" ORDER BY created_at DESC LIMIT ?{}", param_idx));
        params_vec.push(Box::new(limit as i64));

        let params: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();

        let mut stmt = conn.prepare(&sql)?;
        let records = stmt.query_map(params.as_slice(), |row| {
            Ok(ActivityEventRecord {
                id: row.get(0)?,
                event_type: row.get(1)?,
                action: row.get(2)?,
                app_name: row.get(3)?,
                resource_type: row.get(4)?,
                resource_id: row.get(5)?,
                actor: row.get(6)?,
                actor_type: row.get(7)?,
                details: row.get(8)?,
                ip_address: row.get(9)?,
                created_at: row.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Cleanup old activity events (keep last N days)
    pub fn cleanup_old_activity(&self, days: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let cutoff = format!("datetime('now', '-{} days')", days);
        let deleted = conn.execute(
            &format!("DELETE FROM activity_events WHERE created_at < {}", cutoff),
            [],
        )?;
        Ok(deleted)
    }

    // ==================== Alert Rule Operations ====================

    pub fn create_alert_rule(&self, rule: &AlertRuleRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO alert_rules (id, app_name, name, description, metric_type, condition, threshold, duration_secs, severity, enabled, notification_channels)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                rule.id, rule.app_name, rule.name, rule.description, rule.metric_type,
                rule.condition, rule.threshold, rule.duration_secs, rule.severity,
                rule.enabled, rule.notification_channels
            ],
        )?;
        Ok(())
    }

    pub fn get_alert_rule(&self, id: &str) -> Result<Option<AlertRuleRecord>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, app_name, name, description, metric_type, condition, threshold, duration_secs, severity, enabled, notification_channels, created_at, updated_at
             FROM alert_rules WHERE id = ?1",
            params![id],
            |row| Ok(AlertRuleRecord {
                id: row.get(0)?,
                app_name: row.get(1)?,
                name: row.get(2)?,
                description: row.get(3)?,
                metric_type: row.get(4)?,
                condition: row.get(5)?,
                threshold: row.get(6)?,
                duration_secs: row.get(7)?,
                severity: row.get(8)?,
                enabled: row.get(9)?,
                notification_channels: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            }),
        ).optional().context("Failed to get alert rule")
    }

    pub fn list_alert_rules(&self, app_name: Option<&str>) -> Result<Vec<AlertRuleRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut rules = Vec::new();

        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<AlertRuleRecord> {
            Ok(AlertRuleRecord {
                id: row.get(0)?,
                app_name: row.get(1)?,
                name: row.get(2)?,
                description: row.get(3)?,
                metric_type: row.get(4)?,
                condition: row.get(5)?,
                threshold: row.get(6)?,
                duration_secs: row.get(7)?,
                severity: row.get(8)?,
                enabled: row.get(9)?,
                notification_channels: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            })
        };

        if let Some(name) = app_name {
            let mut stmt = conn.prepare(
                "SELECT id, app_name, name, description, metric_type, condition, threshold, duration_secs, severity, enabled, notification_channels, created_at, updated_at
                 FROM alert_rules WHERE app_name = ?1 ORDER BY created_at DESC"
            )?;
            let rows = stmt.query_map(params![name], map_row)?;
            for row in rows {
                rules.push(row?);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, app_name, name, description, metric_type, condition, threshold, duration_secs, severity, enabled, notification_channels, created_at, updated_at
                 FROM alert_rules ORDER BY created_at DESC"
            )?;
            let rows = stmt.query_map([], map_row)?;
            for row in rows {
                rules.push(row?);
            }
        }

        Ok(rules)
    }

    pub fn list_enabled_alert_rules(&self) -> Result<Vec<AlertRuleRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut rules = Vec::new();

        let mut stmt = conn.prepare(
            "SELECT id, app_name, name, description, metric_type, condition, threshold, duration_secs, severity, enabled, notification_channels, created_at, updated_at
             FROM alert_rules WHERE enabled = 1 ORDER BY app_name, metric_type"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(AlertRuleRecord {
                id: row.get(0)?,
                app_name: row.get(1)?,
                name: row.get(2)?,
                description: row.get(3)?,
                metric_type: row.get(4)?,
                condition: row.get(5)?,
                threshold: row.get(6)?,
                duration_secs: row.get(7)?,
                severity: row.get(8)?,
                enabled: row.get(9)?,
                notification_channels: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            })
        })?;

        for row in rows {
            rules.push(row?);
        }
        Ok(rules)
    }

    pub fn update_alert_rule(&self, rule: &AlertRuleRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE alert_rules SET name = ?2, description = ?3, metric_type = ?4, condition = ?5, threshold = ?6, duration_secs = ?7, severity = ?8, enabled = ?9, notification_channels = ?10, updated_at = datetime('now')
             WHERE id = ?1",
            params![
                rule.id, rule.name, rule.description, rule.metric_type, rule.condition,
                rule.threshold, rule.duration_secs, rule.severity, rule.enabled, rule.notification_channels
            ],
        )?;
        Ok(())
    }

    pub fn toggle_alert_rule(&self, id: &str, enabled: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE alert_rules SET enabled = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, enabled],
        )?;
        Ok(())
    }

    pub fn delete_alert_rule(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM alert_rules WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ==================== Alert Event Operations ====================

    pub fn create_alert_event(&self, rule_id: &str, app_name: Option<&str>, metric_value: f64, threshold: f64, message: Option<&str>) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO alert_events (rule_id, app_name, status, metric_value, threshold, message)
             VALUES (?1, ?2, 'firing', ?3, ?4, ?5)",
            params![rule_id, app_name, metric_value, threshold, message],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_alert_event(&self, id: i64) -> Result<Option<AlertEventRecord>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, rule_id, app_name, status, metric_value, threshold, message, started_at, resolved_at, acknowledged_at, acknowledged_by
             FROM alert_events WHERE id = ?1",
            params![id],
            |row| Ok(AlertEventRecord {
                id: row.get(0)?,
                rule_id: row.get(1)?,
                app_name: row.get(2)?,
                status: row.get(3)?,
                metric_value: row.get(4)?,
                threshold: row.get(5)?,
                message: row.get(6)?,
                started_at: row.get(7)?,
                resolved_at: row.get(8)?,
                acknowledged_at: row.get(9)?,
                acknowledged_by: row.get(10)?,
            }),
        ).optional().context("Failed to get alert event")
    }

    pub fn list_alert_events(&self, app_name: Option<&str>, status: Option<&str>, limit: usize) -> Result<Vec<AlertEventRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut events = Vec::new();

        let query = match (app_name, status) {
            (Some(_), Some(_)) => "SELECT id, rule_id, app_name, status, metric_value, threshold, message, started_at, resolved_at, acknowledged_at, acknowledged_by
                                   FROM alert_events WHERE app_name = ?1 AND status = ?2 ORDER BY started_at DESC LIMIT ?3",
            (Some(_), None) => "SELECT id, rule_id, app_name, status, metric_value, threshold, message, started_at, resolved_at, acknowledged_at, acknowledged_by
                                FROM alert_events WHERE app_name = ?1 ORDER BY started_at DESC LIMIT ?2",
            (None, Some(_)) => "SELECT id, rule_id, app_name, status, metric_value, threshold, message, started_at, resolved_at, acknowledged_at, acknowledged_by
                                FROM alert_events WHERE status = ?1 ORDER BY started_at DESC LIMIT ?2",
            (None, None) => "SELECT id, rule_id, app_name, status, metric_value, threshold, message, started_at, resolved_at, acknowledged_at, acknowledged_by
                             FROM alert_events ORDER BY started_at DESC LIMIT ?1",
        };

        let mut stmt = conn.prepare(query)?;
        let rows: Box<dyn Iterator<Item = rusqlite::Result<AlertEventRecord>>> = match (app_name, status) {
            (Some(app), Some(st)) => Box::new(stmt.query_map(params![app, st, limit], |row| {
                Ok(AlertEventRecord {
                    id: row.get(0)?,
                    rule_id: row.get(1)?,
                    app_name: row.get(2)?,
                    status: row.get(3)?,
                    metric_value: row.get(4)?,
                    threshold: row.get(5)?,
                    message: row.get(6)?,
                    started_at: row.get(7)?,
                    resolved_at: row.get(8)?,
                    acknowledged_at: row.get(9)?,
                    acknowledged_by: row.get(10)?,
                })
            })?),
            (Some(app), None) => Box::new(stmt.query_map(params![app, limit], |row| {
                Ok(AlertEventRecord {
                    id: row.get(0)?,
                    rule_id: row.get(1)?,
                    app_name: row.get(2)?,
                    status: row.get(3)?,
                    metric_value: row.get(4)?,
                    threshold: row.get(5)?,
                    message: row.get(6)?,
                    started_at: row.get(7)?,
                    resolved_at: row.get(8)?,
                    acknowledged_at: row.get(9)?,
                    acknowledged_by: row.get(10)?,
                })
            })?),
            (None, Some(st)) => Box::new(stmt.query_map(params![st, limit], |row| {
                Ok(AlertEventRecord {
                    id: row.get(0)?,
                    rule_id: row.get(1)?,
                    app_name: row.get(2)?,
                    status: row.get(3)?,
                    metric_value: row.get(4)?,
                    threshold: row.get(5)?,
                    message: row.get(6)?,
                    started_at: row.get(7)?,
                    resolved_at: row.get(8)?,
                    acknowledged_at: row.get(9)?,
                    acknowledged_by: row.get(10)?,
                })
            })?),
            (None, None) => Box::new(stmt.query_map(params![limit], |row| {
                Ok(AlertEventRecord {
                    id: row.get(0)?,
                    rule_id: row.get(1)?,
                    app_name: row.get(2)?,
                    status: row.get(3)?,
                    metric_value: row.get(4)?,
                    threshold: row.get(5)?,
                    message: row.get(6)?,
                    started_at: row.get(7)?,
                    resolved_at: row.get(8)?,
                    acknowledged_at: row.get(9)?,
                    acknowledged_by: row.get(10)?,
                })
            })?),
        };

        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    pub fn get_firing_alerts(&self) -> Result<Vec<AlertEventRecord>> {
        self.list_alert_events(None, Some("firing"), 100)
    }

    pub fn resolve_alert_event(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE alert_events SET status = 'resolved', resolved_at = datetime('now') WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn acknowledge_alert_event(&self, id: i64, acknowledged_by: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE alert_events SET acknowledged_at = datetime('now'), acknowledged_by = ?2 WHERE id = ?1",
            params![id, acknowledged_by],
        )?;
        Ok(())
    }

    pub fn get_active_alert_for_rule(&self, rule_id: &str) -> Result<Option<AlertEventRecord>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, rule_id, app_name, status, metric_value, threshold, message, started_at, resolved_at, acknowledged_at, acknowledged_by
             FROM alert_events WHERE rule_id = ?1 AND status = 'firing' ORDER BY started_at DESC LIMIT 1",
            params![rule_id],
            |row| Ok(AlertEventRecord {
                id: row.get(0)?,
                rule_id: row.get(1)?,
                app_name: row.get(2)?,
                status: row.get(3)?,
                metric_value: row.get(4)?,
                threshold: row.get(5)?,
                message: row.get(6)?,
                started_at: row.get(7)?,
                resolved_at: row.get(8)?,
                acknowledged_at: row.get(9)?,
                acknowledged_by: row.get(10)?,
            }),
        ).optional().context("Failed to get active alert")
    }

    // ==================== Alert Notification Operations ====================

    pub fn create_alert_notification(&self, alert_event_id: i64, channel: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO alert_notifications (alert_event_id, channel, status) VALUES (?1, ?2, 'pending')",
            params![alert_event_id, channel],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_notification_status(&self, id: i64, status: &str, error_message: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        if status == "sent" {
            conn.execute(
                "UPDATE alert_notifications SET status = ?2, sent_at = datetime('now') WHERE id = ?1",
                params![id, status],
            )?;
        } else {
            conn.execute(
                "UPDATE alert_notifications SET status = ?2, error_message = ?3 WHERE id = ?1",
                params![id, status, error_message],
            )?;
        }
        Ok(())
    }

    pub fn get_pending_notifications(&self) -> Result<Vec<AlertNotificationRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut notifications = Vec::new();

        let mut stmt = conn.prepare(
            "SELECT id, alert_event_id, channel, status, sent_at, error_message, created_at
             FROM alert_notifications WHERE status = 'pending' ORDER BY created_at"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(AlertNotificationRecord {
                id: row.get(0)?,
                alert_event_id: row.get(1)?,
                channel: row.get(2)?,
                status: row.get(3)?,
                sent_at: row.get(4)?,
                error_message: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;

        for row in rows {
            notifications.push(row?);
        }
        Ok(notifications)
    }

    pub fn cleanup_old_alerts(&self, days: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let cutoff = format!("datetime('now', '-{} days')", days);
        let deleted = conn.execute(
            &format!("DELETE FROM alert_events WHERE status = 'resolved' AND resolved_at < {}", cutoff),
            [],
        )?;
        Ok(deleted)
    }

    // ==================== Formation Operations ====================

    pub fn set_formation(&self, formation: &FormationRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO app_formations (app_name, process_type, quantity, size, command)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(app_name, process_type) DO UPDATE SET
                quantity = excluded.quantity,
                size = excluded.size,
                command = excluded.command,
                updated_at = datetime('now')",
            params![
                formation.app_name, formation.process_type, formation.quantity,
                formation.size, formation.command
            ],
        )?;
        Ok(())
    }

    pub fn get_formation(&self, app_name: &str, process_type: &str) -> Result<Option<FormationRecord>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT app_name, process_type, quantity, size, command, created_at, updated_at
             FROM app_formations WHERE app_name = ?1 AND process_type = ?2",
            params![app_name, process_type],
            |row| Ok(FormationRecord {
                app_name: row.get(0)?,
                process_type: row.get(1)?,
                quantity: row.get(2)?,
                size: row.get(3)?,
                command: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            }),
        ).optional().context("Failed to get formation")
    }

    pub fn get_app_formations(&self, app_name: &str) -> Result<Vec<FormationRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut formations = Vec::new();

        let mut stmt = conn.prepare(
            "SELECT app_name, process_type, quantity, size, command, created_at, updated_at
             FROM app_formations WHERE app_name = ?1 ORDER BY process_type"
        )?;

        let rows = stmt.query_map(params![app_name], |row| {
            Ok(FormationRecord {
                app_name: row.get(0)?,
                process_type: row.get(1)?,
                quantity: row.get(2)?,
                size: row.get(3)?,
                command: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;

        for row in rows {
            formations.push(row?);
        }
        Ok(formations)
    }

    pub fn update_formation_quantity(&self, app_name: &str, process_type: &str, quantity: i32) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE app_formations SET quantity = ?3, updated_at = datetime('now')
             WHERE app_name = ?1 AND process_type = ?2",
            params![app_name, process_type, quantity],
        )?;
        Ok(())
    }

    pub fn update_formation_size(&self, app_name: &str, process_type: &str, size: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE app_formations SET size = ?3, updated_at = datetime('now')
             WHERE app_name = ?1 AND process_type = ?2",
            params![app_name, process_type, size],
        )?;
        Ok(())
    }

    pub fn delete_formation(&self, app_name: &str, process_type: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM app_formations WHERE app_name = ?1 AND process_type = ?2",
            params![app_name, process_type],
        )?;
        Ok(())
    }

    pub fn batch_update_formations(&self, app_name: &str, formations: &[FormationRecord]) -> Result<()> {
        for formation in formations {
            self.set_formation(formation)?;
        }
        Ok(())
    }

    pub fn ensure_default_formation(&self, app_name: &str) -> Result<()> {
        // Create a default 'web' formation if none exist
        let existing = self.get_app_formations(app_name)?;
        if existing.is_empty() {
            self.set_formation(&FormationRecord {
                app_name: app_name.to_string(),
                process_type: "web".to_string(),
                quantity: 1,
                size: "standard".to_string(),
                command: None,
                created_at: String::new(),
                updated_at: String::new(),
            })?;
        }
        Ok(())
    }
}

// ==================== Record Types ====================

/// App record from database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppRecord {
    pub name: String,
    pub status: String,
    pub git_url: Option<String>,
    pub image: Option<String>,
    pub port: i32,
    pub created_at: String,
    pub deployed_at: Option<String>,
    pub commit_hash: Option<String>,
    pub scale: i32,
    pub min_scale: i32,
    pub max_scale: i32,
}

/// Process (running instance) record from database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessRecord {
    pub id: String,
    pub app_name: String,
    pub process_type: String,
    pub container_id: Option<String>,
    pub container_name: Option<String>,
    pub port: Option<i32>,
    pub status: String,
    pub health_status: Option<String>,
    pub last_health_check: Option<String>,
    pub started_at: String,
}

/// Addon record from database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddonRecord {
    pub id: String,
    pub app_name: String,
    pub addon_type: String,
    pub plan: String,
    pub container_id: Option<String>,
    pub container_name: Option<String>,
    pub connection_url: Option<String>,
    pub env_var_name: Option<String>,
    pub status: String,
    pub created_at: String,
}

/// Deployment record from database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentRecord {
    pub id: String,
    pub app_name: String,
    pub status: String,
    pub image: Option<String>,
    pub commit_hash: Option<String>,
    pub build_logs: Option<String>,
    pub duration_secs: Option<f64>,
    pub created_at: String,
    pub finished_at: Option<String>,
}

/// Domain record from database (basic)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainRecord {
    pub domain: String,
    pub app_name: String,
    pub verified: bool,
    pub ssl_enabled: bool,
    pub created_at: String,
}

/// Custom domain record with full details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomDomainRecord {
    pub domain: String,
    pub app_name: String,
    pub verified: bool,
    pub ssl_enabled: bool,
    pub verification_token: Option<String>,
    pub cert_path: Option<String>,
    pub key_path: Option<String>,
    pub cert_expires_at: Option<String>,
    pub created_at: String,
}

/// Secret audit log record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretAuditRecord {
    pub id: i64,
    pub app_name: String,
    pub secret_key: String,
    pub action: String,
    pub actor: Option<String>,
    pub ip_address: Option<String>,
    pub created_at: String,
}

/// Encryption key record from database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptionKeyRecord {
    pub id: String,
    pub key_data: String,
    pub is_current: bool,
    pub created_at: String,
    pub rotated_at: Option<String>,
}

/// Webhook configuration record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookRecord {
    pub app_name: String,
    pub secret: String,
    pub provider: String,
    pub deploy_branch: String,
    pub auto_deploy: bool,
    pub status_token: Option<String>,
    pub repo_name: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Webhook event record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEventRecord {
    pub id: Option<i64>,
    pub app_name: String,
    pub event_type: String,
    pub provider: String,
    pub branch: Option<String>,
    pub commit_sha: Option<String>,
    pub commit_message: Option<String>,
    pub author: Option<String>,
    pub payload: Option<String>,
    pub triggered_deploy: bool,
    pub deployment_id: Option<String>,
    pub created_at: Option<String>,
}

/// Build status record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildStatusRecord {
    pub app_name: String,
    pub status: String,
    pub commit_sha: Option<String>,
    pub updated_at: String,
}

/// API token record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiTokenRecord {
    pub id: String,
    pub name: String,
    pub token_hash: String,
    pub token_prefix: String,
    pub scopes: String,
    pub last_used_at: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
}

/// API token usage log record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiTokenLogRecord {
    pub id: i64,
    pub token_id: String,
    pub action: String,
    pub resource: Option<String>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: String,
}

/// Request metrics record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestMetricsRecord {
    pub id: i64,
    pub app_name: String,
    pub timestamp: String,
    pub request_count: i64,
    pub error_count: i64,
    pub avg_response_time_ms: f64,
    pub p50_response_time_ms: Option<f64>,
    pub p95_response_time_ms: Option<f64>,
    pub p99_response_time_ms: Option<f64>,
}

/// Resource metrics record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceMetricsRecord {
    pub id: i64,
    pub app_name: String,
    pub instance_id: String,
    pub timestamp: String,
    pub cpu_percent: f64,
    pub memory_used: i64,
    pub memory_limit: i64,
}

/// Activity event record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEventRecord {
    pub id: i64,
    pub event_type: String,
    pub action: String,
    pub app_name: Option<String>,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub actor: Option<String>,
    pub actor_type: String,
    pub details: Option<String>,
    pub ip_address: Option<String>,
    pub created_at: String,
}

/// Alert rule record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRuleRecord {
    pub id: String,
    pub app_name: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub metric_type: String,
    pub condition: String,
    pub threshold: f64,
    pub duration_secs: i64,
    pub severity: String,
    pub enabled: bool,
    pub notification_channels: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Alert event record (triggered alert)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertEventRecord {
    pub id: i64,
    pub rule_id: String,
    pub app_name: Option<String>,
    pub status: String,
    pub metric_value: f64,
    pub threshold: f64,
    pub message: Option<String>,
    pub started_at: String,
    pub resolved_at: Option<String>,
    pub acknowledged_at: Option<String>,
    pub acknowledged_by: Option<String>,
}

/// Alert notification record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertNotificationRecord {
    pub id: i64,
    pub alert_event_id: i64,
    pub channel: String,
    pub status: String,
    pub sent_at: Option<String>,
    pub error_message: Option<String>,
    pub created_at: String,
}

/// Formation record (process type scaling configuration)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormationRecord {
    pub app_name: String,
    pub process_type: String,
    pub quantity: i32,
    pub size: String,
    pub command: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_get_app() {
        let db = Database::open_in_memory().unwrap();

        let app = AppRecord {
            name: "myapp".to_string(),
            status: "idle".to_string(),
            git_url: Some("git@localhost:myapp.git".to_string()),
            image: None,
            port: 3000,
            created_at: String::new(),
            deployed_at: None,
            commit_hash: None,
            scale: 1,
            min_scale: 0,
            max_scale: 10,
        };

        db.create_app(&app).unwrap();

        let retrieved = db.get_app("myapp").unwrap().unwrap();
        assert_eq!(retrieved.name, "myapp");
        assert_eq!(retrieved.status, "idle");
        assert_eq!(retrieved.port, 3000);
    }

    #[test]
    fn test_list_apps() {
        let db = Database::open_in_memory().unwrap();

        for i in 1..=3 {
            let app = AppRecord {
                name: format!("app{}", i),
                status: "idle".to_string(),
                git_url: None,
                image: None,
                port: 3000,
                created_at: String::new(),
                deployed_at: None,
                commit_hash: None,
                scale: 1,
                min_scale: 0,
                max_scale: 10,
            };
            db.create_app(&app).unwrap();
        }

        let apps = db.list_apps().unwrap();
        assert_eq!(apps.len(), 3);
    }

    #[test]
    fn test_config_operations() {
        let db = Database::open_in_memory().unwrap();

        let app = AppRecord {
            name: "myapp".to_string(),
            status: "idle".to_string(),
            git_url: None,
            image: None,
            port: 3000,
            created_at: String::new(),
            deployed_at: None,
            commit_hash: None,
            scale: 1,
            min_scale: 0,
            max_scale: 10,
        };
        db.create_app(&app).unwrap();

        db.set_config("myapp", "DATABASE_URL", "postgres://...", false).unwrap();
        db.set_config("myapp", "API_KEY", "secret123", true).unwrap();

        let value = db.get_config("myapp", "DATABASE_URL").unwrap();
        assert_eq!(value, Some("postgres://...".to_string()));

        let all_config = db.get_all_config("myapp").unwrap();
        assert_eq!(all_config.len(), 2);

        db.delete_config("myapp", "API_KEY").unwrap();
        let all_config = db.get_all_config("myapp").unwrap();
        assert_eq!(all_config.len(), 1);
    }

    #[test]
    fn test_addon_operations() {
        let db = Database::open_in_memory().unwrap();

        let app = AppRecord {
            name: "myapp".to_string(),
            status: "idle".to_string(),
            git_url: None,
            image: None,
            port: 3000,
            created_at: String::new(),
            deployed_at: None,
            commit_hash: None,
            scale: 1,
            min_scale: 0,
            max_scale: 10,
        };
        db.create_app(&app).unwrap();

        let addon = AddonRecord {
            id: "addon-123".to_string(),
            app_name: "myapp".to_string(),
            addon_type: "postgres".to_string(),
            plan: "hobby".to_string(),
            container_id: None,
            container_name: Some("pg-myapp".to_string()),
            connection_url: None,
            env_var_name: Some("DATABASE_URL".to_string()),
            status: "provisioning".to_string(),
            created_at: String::new(),
        };
        db.create_addon(&addon).unwrap();

        let addons = db.get_app_addons("myapp").unwrap();
        assert_eq!(addons.len(), 1);
        assert_eq!(addons[0].addon_type, "postgres");

        db.update_addon("addon-123", "container-abc", "postgres://...", "running").unwrap();

        let addon = db.get_addon("myapp", "postgres").unwrap().unwrap();
        assert_eq!(addon.status, "running");
        assert_eq!(addon.connection_url, Some("postgres://...".to_string()));
    }

    #[test]
    fn test_delete_app_cascades() {
        let db = Database::open_in_memory().unwrap();

        let app = AppRecord {
            name: "myapp".to_string(),
            status: "idle".to_string(),
            git_url: None,
            image: None,
            port: 3000,
            created_at: String::new(),
            deployed_at: None,
            commit_hash: None,
            scale: 1,
            min_scale: 0,
            max_scale: 10,
        };
        db.create_app(&app).unwrap();

        db.set_config("myapp", "KEY", "value", false).unwrap();

        let addon = AddonRecord {
            id: "addon-123".to_string(),
            app_name: "myapp".to_string(),
            addon_type: "postgres".to_string(),
            plan: "hobby".to_string(),
            container_id: None,
            container_name: None,
            connection_url: None,
            env_var_name: None,
            status: "running".to_string(),
            created_at: String::new(),
        };
        db.create_addon(&addon).unwrap();

        db.add_domain("example.com", "myapp", "verify-token-123").unwrap();

        // Delete app - should cascade to config, addons, domains
        db.delete_app("myapp").unwrap();

        assert!(db.get_app("myapp").unwrap().is_none());
        assert!(db.get_all_config("myapp").unwrap().is_empty());
        assert!(db.get_app_addons("myapp").unwrap().is_empty());
        assert!(db.get_app_domains("myapp").unwrap().is_empty());
    }

    #[test]
    fn test_deployment_history() {
        let db = Database::open_in_memory().unwrap();

        let app = AppRecord {
            name: "myapp".to_string(),
            status: "idle".to_string(),
            git_url: None,
            image: None,
            port: 3000,
            created_at: String::new(),
            deployed_at: None,
            commit_hash: None,
            scale: 1,
            min_scale: 0,
            max_scale: 10,
        };
        db.create_app(&app).unwrap();

        let deploy = DeploymentRecord {
            id: "deploy-1".to_string(),
            app_name: "myapp".to_string(),
            status: "pending".to_string(),
            image: None,
            commit_hash: Some("abc123".to_string()),
            build_logs: None,
            duration_secs: None,
            created_at: String::new(),
            finished_at: None,
        };
        db.create_deployment(&deploy).unwrap();

        db.update_deployment(
            "deploy-1",
            "success",
            Some("myapp:latest"),
            Some("Build succeeded"),
            Some(45.5),
        ).unwrap();

        let deploys = db.get_deployments("myapp", 10).unwrap();
        assert_eq!(deploys.len(), 1);
        assert_eq!(deploys[0].status, "success");
        assert_eq!(deploys[0].duration_secs, Some(45.5));
    }
}
