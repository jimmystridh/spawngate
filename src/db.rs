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
const SCHEMA_VERSION: i32 = 5;

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
