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
const SCHEMA_VERSION: i32 = 1;

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

            // Add future migrations here:
            // if current_version < 2 { self.migrate_v2(&conn)?; }
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
            "SELECT name, status, git_url, image, port, created_at, deployed_at, commit_hash
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
            "SELECT name, status, git_url, image, port, created_at, deployed_at, commit_hash
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

    // ==================== Domain Operations ====================

    /// Add a domain to an app
    pub fn add_domain(&self, domain: &str, app_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO domains (domain, app_name) VALUES (?1, ?2)",
            params![domain, app_name],
        )?;
        Ok(())
    }

    /// Get domains for an app
    pub fn get_app_domains(&self, app_name: &str) -> Result<Vec<DomainRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT domain, app_name, verified, ssl_enabled, created_at
             FROM domains WHERE app_name = ?1"
        )?;

        let domains = stmt.query_map(params![app_name], |row| {
            Ok(DomainRecord {
                domain: row.get(0)?,
                app_name: row.get(1)?,
                verified: row.get(2)?,
                ssl_enabled: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(domains)
    }

    /// Get app for a domain
    pub fn get_domain_app(&self, domain: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT app_name FROM domains WHERE domain = ?1",
            params![domain],
            |row| row.get(0),
        )
        .optional()
        .context("Failed to get domain app")
    }

    /// Update domain verification/SSL status
    pub fn update_domain(&self, domain: &str, verified: bool, ssl_enabled: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE domains SET verified = ?1, ssl_enabled = ?2 WHERE domain = ?3",
            params![verified, ssl_enabled, domain],
        )?;
        Ok(())
    }

    /// Delete a domain
    pub fn delete_domain(&self, domain: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM domains WHERE domain = ?1", params![domain])?;
        Ok(rows > 0)
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

/// Domain record from database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainRecord {
    pub domain: String,
    pub app_name: String,
    pub verified: bool,
    pub ssl_enabled: bool,
    pub created_at: String,
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

        db.add_domain("example.com", "myapp").unwrap();

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
