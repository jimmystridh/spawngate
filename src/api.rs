//! Platform API server for PaaS management
//!
//! This module provides RESTful API endpoints for managing applications,
//! add-ons, deployments, and logs. The CLI tool communicates with this API.

use crate::addons::{AddonConfig, AddonManager, AddonPlan, AddonType};
use crate::builder::{BuildConfig, BuildMode, Builder};
use crate::dashboard;
use crate::db::{AddonRecord, AppRecord, Database, DeploymentRecord};
use crate::docker::DockerManager;
use crate::dyno::{DynoConfig, DynoManager};
use crate::git::{GitServer, GitServerConfig};
use anyhow::{Context, Result};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::header::{AUTHORIZATION, CONTENT_TYPE};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as AutoBuilder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

/// Platform API configuration
#[derive(Debug, Clone)]
pub struct PlatformApiConfig {
    /// Bind address for the API server
    pub bind_addr: SocketAddr,

    /// Authentication token
    pub auth_token: String,

    /// Base directory for app data
    pub data_dir: PathBuf,

    /// Docker network for apps and add-ons
    pub network_name: String,

    /// Registry URL for built images
    pub registry: Option<String>,
}

impl Default for PlatformApiConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:9999".parse().unwrap(),
            auth_token: "changeme".to_string(),
            data_dir: PathBuf::from("./paas_data"),
            network_name: "spawngate".to_string(),
            registry: None,
        }
    }
}

/// Application state returned by API (includes config from db)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    pub name: String,
    pub status: String,
    pub git_url: Option<String>,
    pub image: Option<String>,
    pub port: u16,
    pub env: HashMap<String, String>,
    pub addons: Vec<String>,
    pub created_at: String,
    pub deployed_at: Option<String>,
    pub commit: Option<String>,
    pub scale: i32,
}

impl App {
    fn from_record(record: AppRecord, env: HashMap<String, String>, addons: Vec<String>) -> Self {
        Self {
            name: record.name,
            status: record.status,
            git_url: record.git_url,
            image: record.image,
            port: record.port as u16,
            env,
            addons,
            created_at: record.created_at,
            deployed_at: record.deployed_at,
            commit: record.commit_hash,
            scale: record.scale,
        }
    }
}

/// Request to scale an app
#[derive(Debug, Deserialize)]
pub struct ScaleRequest {
    pub scale: i32,
}

/// Request to create a new app
#[derive(Debug, Deserialize)]
pub struct CreateAppRequest {
    pub name: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

fn default_port() -> u16 {
    3000
}

/// Request to add an add-on
#[derive(Debug, Deserialize)]
pub struct AddAddonRequest {
    #[serde(rename = "type")]
    pub addon_type: String,
    #[serde(default)]
    pub plan: String,
}

/// Request to deploy an app
#[derive(Debug, Deserialize)]
pub struct DeployRequest {
    pub source: Option<String>,
    pub build_mode: Option<String>,
    #[serde(default)]
    pub clear_cache: bool,
}

/// Request to set config/env vars
#[derive(Debug, Deserialize)]
pub struct SetConfigRequest {
    pub env: HashMap<String, String>,
}

/// API response wrapper
#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}

/// Platform API server
pub struct PlatformApi {
    config: PlatformApiConfig,
    db: Arc<Database>,
    addon_manager: Arc<AddonManager>,
    builder: Arc<Builder>,
    git_server: Arc<GitServer>,
    dyno_manager: Arc<DynoManager>,
    shutdown_rx: watch::Receiver<bool>,
}

impl PlatformApi {
    /// Create a new Platform API server
    pub async fn new(
        config: PlatformApiConfig,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Result<Self> {
        // Create data directory
        tokio::fs::create_dir_all(&config.data_dir).await?;

        // Initialize database
        let db_path = config.data_dir.join("paas.db");
        let db = Database::open(&db_path)?;
        info!("Database initialized at {}", db_path.display());

        // Initialize addon manager
        let addon_manager = AddonManager::new(None, Some(&config.network_name)).await?;

        // Initialize builder
        let builder = Builder::new(None, config.registry.as_deref()).await?;

        // Initialize git server
        let git_config = GitServerConfig {
            repos_dir: config.data_dir.join("repos"),
            work_dir: config.data_dir.join("work"),
            ssh_port: Some(2222),
            http_port: 3000,
            registry: config.registry.clone(),
        };
        let git_server = GitServer::new(git_config).await?;

        // Scan for existing repos
        git_server.scan_repos().await?;

        // Initialize Docker and dyno manager
        let docker = DockerManager::new(None).await?;
        let db_arc = Arc::new(db);
        let dyno_config = DynoConfig {
            network: config.network_name.clone(),
            health_check_url: Some(format!("http://{}", config.bind_addr)),
            memory_limit: Some("512m".to_string()),
            cpu_limit: Some("0.5".to_string()),
        };
        let dyno_manager = DynoManager::new(
            Arc::new(docker),
            Arc::clone(&db_arc),
            dyno_config,
        ).await?;

        Ok(Self {
            config,
            db: db_arc,
            addon_manager: Arc::new(addon_manager),
            builder: Arc::new(builder),
            git_server: Arc::new(git_server),
            dyno_manager: Arc::new(dyno_manager),
            shutdown_rx,
        })
    }

    /// Run the API server
    pub async fn run(self: Arc<Self>) -> Result<()> {
        let listener = TcpListener::bind(self.config.bind_addr).await?;
        info!(addr = %self.config.bind_addr, "Platform API server listening");

        let mut shutdown_rx = self.shutdown_rx.clone();

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            let api = Arc::clone(&self);
                            tokio::spawn(async move {
                                if let Err(e) = api.serve_connection(stream, addr).await {
                                    debug!(addr = %addr, error = %e, "Connection error");
                                }
                            });
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to accept connection");
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Platform API server shutting down");
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    async fn serve_connection<S>(
        self: Arc<Self>,
        stream: S,
        _addr: SocketAddr,
    ) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let io = TokioIo::new(stream);
        let service = service_fn(move |req| {
            let api = Arc::clone(&self);
            async move { api.handle_request(req).await }
        });

        AutoBuilder::new(TokioExecutor::new())
            .serve_connection(io, service)
            .await
            .map_err(|e| anyhow::anyhow!("Connection error: {}", e))?;

        Ok(())
    }

    fn check_auth(&self, req: &Request<hyper::body::Incoming>) -> bool {
        req.headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map(|auth| {
                auth.strip_prefix("Bearer ")
                    .unwrap_or(auth)
                    .eq(&self.config.auth_token)
            })
            .unwrap_or(false)
    }

    async fn handle_request(
        self: Arc<Self>,
        req: Request<hyper::body::Incoming>,
    ) -> Result<Response<Full<Bytes>>, hyper::Error> {
        let path = req.uri().path().to_string();
        let method = req.method().clone();

        debug!(%method, %path, "API request");

        // Health check - no auth required
        if path == "/health" && method == Method::GET {
            return Ok(json_response(StatusCode::OK, r#"{"status":"ok"}"#));
        }

        // Version - no auth required
        if path == "/version" && method == Method::GET {
            let version = serde_json::json!({
                "name": "spawngate-paas",
                "version": env!("CARGO_PKG_VERSION"),
            });
            return Ok(json_response(StatusCode::OK, version.to_string()));
        }

        // Dashboard - no auth required for static assets
        if path == "/" || path == "/dashboard" || path == "/dashboard/" {
            return Ok(dashboard::serve_dashboard());
        }
        if path == "/dashboard/style.css" {
            return Ok(dashboard::serve_css());
        }
        if path == "/dashboard/app.js" {
            return Ok(dashboard::serve_js());
        }

        // Auth required for all other endpoints
        if !self.check_auth(&req) {
            warn!(%path, "Unauthorized API request");
            return Ok(json_response(
                StatusCode::UNAUTHORIZED,
                r#"{"success":false,"error":"unauthorized"}"#,
            ));
        }

        // Route the request
        let response = match (method, path.as_str()) {
            // Apps
            (Method::GET, "/apps") => self.list_apps().await,
            (Method::POST, "/apps") => self.create_app(req).await,
            (Method::GET, path) if path.starts_with("/apps/") && !path.contains("/addons") && !path.contains("/logs") && !path.contains("/config") && !path.contains("/deploy") && !path.contains("/deployments") => {
                let app_name = path.strip_prefix("/apps/").unwrap();
                self.get_app(app_name).await
            }
            (Method::DELETE, path) if path.starts_with("/apps/") && path.matches('/').count() == 2 => {
                let app_name = path.strip_prefix("/apps/").unwrap();
                self.delete_app(app_name).await
            }

            // Add-ons
            (Method::GET, path) if path.ends_with("/addons") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/addons")).unwrap_or("");
                self.list_addons(app_name).await
            }
            (Method::POST, path) if path.ends_with("/addons") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/addons")).unwrap_or("");
                self.add_addon(app_name, req).await
            }
            (Method::DELETE, path) if path.contains("/addons/") => {
                let parts: Vec<&str> = path.split('/').collect();
                if parts.len() >= 5 {
                    let app_name = parts[2];
                    let addon_type = parts[4];
                    self.remove_addon(app_name, addon_type).await
                } else {
                    Ok(json_error(StatusCode::BAD_REQUEST, "Invalid path"))
                }
            }

            // Config/Env
            (Method::GET, path) if path.ends_with("/config") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/config")).unwrap_or("");
                self.get_config(app_name).await
            }
            (Method::PUT, path) if path.ends_with("/config") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/config")).unwrap_or("");
                self.set_config(app_name, req).await
            }

            // Deploy
            (Method::POST, path) if path.ends_with("/deploy") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/deploy")).unwrap_or("");
                self.deploy_app(app_name, req).await
            }

            // Deployments history
            (Method::GET, path) if path.ends_with("/deployments") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/deployments")).unwrap_or("");
                self.list_deployments(app_name).await
            }

            // Scale
            (Method::POST, path) if path.ends_with("/scale") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/scale")).unwrap_or("");
                self.scale_app(app_name, req).await
            }
            (Method::GET, path) if path.ends_with("/processes") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/processes")).unwrap_or("");
                self.list_processes(app_name).await
            }

            // Logs
            (Method::GET, path) if path.ends_with("/logs") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/logs")).unwrap_or("");
                self.get_logs(app_name).await
            }

            // Git info
            (Method::GET, path) if path.ends_with("/git") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/git")).unwrap_or("");
                self.get_git_info(app_name).await
            }

            // Build trigger (for git hooks)
            (Method::POST, path) if path.starts_with("/build/") => {
                let app_name = path.strip_prefix("/build/").unwrap_or("");
                self.trigger_build(app_name).await
            }

            _ => Ok(json_error(StatusCode::NOT_FOUND, "Not found")),
        };

        response.or_else(|e| {
            error!(error = %e, "API error");
            Ok(json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Internal error: {}", e),
            ))
        })
    }

    // ==================== App Management ====================

    async fn list_apps(&self) -> Result<Response<Full<Bytes>>> {
        let records = self.db.list_apps()?;

        let mut apps = Vec::new();
        for record in records {
            let env = self.db.get_all_config(&record.name)?;
            let addon_records = self.db.get_app_addons(&record.name)?;
            let addons: Vec<String> = addon_records.iter()
                .map(|a| format!("{}:{}", a.addon_type, a.plan))
                .collect();
            apps.push(App::from_record(record, env, addons));
        }

        let response = ApiResponse::ok(apps);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    async fn create_app(self: Arc<Self>, req: Request<hyper::body::Incoming>) -> Result<Response<Full<Bytes>>> {
        let body = req.collect().await?.to_bytes();
        let create_req: CreateAppRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => {
                return Ok(json_error(StatusCode::BAD_REQUEST, format!("Invalid JSON: {}", e)));
            }
        };

        // Validate app name
        if create_req.name.is_empty() || create_req.name.len() > 64 {
            return Ok(json_error(StatusCode::BAD_REQUEST, "App name must be 1-64 characters"));
        }

        // Check if already exists
        if self.db.get_app(&create_req.name)?.is_some() {
            return Ok(json_error(StatusCode::CONFLICT, "App already exists"));
        }

        // Create git repository
        let _git_repo = self.git_server.create_app(&create_req.name).await
            .context("Failed to create git repository")?;

        let git_url = self.git_server.get_remote_url(&create_req.name);

        // Create app record
        let record = AppRecord {
            name: create_req.name.clone(),
            status: "idle".to_string(),
            git_url: Some(git_url.clone()),
            image: None,
            port: create_req.port as i32,
            created_at: String::new(), // Will be set by database
            deployed_at: None,
            commit_hash: None,
            scale: 1,
            min_scale: 0,
            max_scale: 10,
        };

        self.db.create_app(&record)?;

        // Store initial env vars
        for (key, value) in &create_req.env {
            self.db.set_config(&create_req.name, key, value, false)?;
        }

        info!(app = %create_req.name, git_url = %git_url, "Created new app");

        // Fetch the created app to return
        let app_record = self.db.get_app(&create_req.name)?.unwrap();
        let app = App::from_record(app_record, create_req.env, vec![]);

        let response = ApiResponse::ok(app);
        Ok(json_response(StatusCode::CREATED, serde_json::to_string(&response)?))
    }

    async fn get_app(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        if let Some(record) = self.db.get_app(app_name)? {
            let env = self.db.get_all_config(app_name)?;

            // Get addon env vars and merge
            let addon_env = self.addon_manager.get_env_vars(app_name).await;
            let mut merged_env = env;
            for (k, v) in addon_env {
                merged_env.entry(k).or_insert(v);
            }

            let addon_records = self.db.get_app_addons(app_name)?;
            let addons: Vec<String> = addon_records.iter()
                .map(|a| format!("{}:{}", a.addon_type, a.plan))
                .collect();

            let app = App::from_record(record, merged_env, addons);
            let response = ApiResponse::ok(app);
            Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
        } else {
            Ok(json_error(StatusCode::NOT_FOUND, "App not found"))
        }
    }

    async fn delete_app(self: Arc<Self>, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        info!(app = %app_name, "Deleting app");

        // Remove add-ons from Docker
        self.addon_manager.deprovision_all(app_name).await?;

        // Delete git repository
        self.git_server.delete_app(app_name).await?;

        // Delete from database (cascades to config, addons, deployments, domains)
        self.db.delete_app(app_name)?;

        let response: ApiResponse<()> = ApiResponse::ok(());
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    // ==================== Add-on Management ====================

    async fn list_addons(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        let addons = self.db.get_app_addons(app_name)?;
        let response = ApiResponse::ok(addons);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    async fn add_addon(
        self: Arc<Self>,
        app_name: &str,
        req: Request<hyper::body::Incoming>,
    ) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        let body = req.collect().await?.to_bytes();
        let add_req: AddAddonRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => {
                return Ok(json_error(StatusCode::BAD_REQUEST, format!("Invalid JSON: {}", e)));
            }
        };

        // Parse addon type
        let addon_type = match add_req.addon_type.to_lowercase().as_str() {
            "postgres" | "postgresql" => AddonType::Postgres,
            "redis" => AddonType::Redis,
            "storage" | "s3" | "minio" => AddonType::Storage,
            _ => {
                return Ok(json_error(StatusCode::BAD_REQUEST, format!("Unknown addon type: {}", add_req.addon_type)));
            }
        };

        // Parse plan
        let plan = match add_req.plan.to_lowercase().as_str() {
            "" | "hobby" => AddonPlan::Hobby,
            "basic" => AddonPlan::Basic,
            "standard" => AddonPlan::Standard,
            "premium" => AddonPlan::Premium,
            _ => {
                return Ok(json_error(StatusCode::BAD_REQUEST, format!("Unknown plan: {}", add_req.plan)));
            }
        };

        let config = AddonConfig {
            addon_type: addon_type.clone(),
            plan: plan.clone(),
            name: None,
            network: Some(self.config.network_name.clone()),
        };

        info!(app = %app_name, addon = %addon_type, "Provisioning addon");

        // Provision the addon via Docker
        let instance = self.addon_manager.provision(app_name, &config).await
            .context("Failed to provision addon")?;

        // Store in database
        let addon_record = AddonRecord {
            id: instance.id.clone(),
            app_name: app_name.to_string(),
            addon_type: addon_type.to_string(),
            plan: format!("{:?}", plan).to_lowercase(),
            container_id: instance.container_id.clone(),
            container_name: Some(instance.container_name.clone()),
            connection_url: Some(instance.connection_url.clone()),
            env_var_name: Some(instance.env_var_name.clone()),
            status: "running".to_string(),
            created_at: String::new(),
        };

        self.db.create_addon(&addon_record)?;

        // Store addon env vars in app config
        self.db.set_config(app_name, &instance.env_var_name, &instance.connection_url, false)?;
        for (key, value) in &instance.env_vars {
            self.db.set_config(app_name, key, value, false)?;
        }

        let response = ApiResponse::ok(addon_record);
        Ok(json_response(StatusCode::CREATED, serde_json::to_string(&response)?))
    }

    async fn remove_addon(self: Arc<Self>, app_name: &str, addon_type_str: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        let addon_type = match addon_type_str.to_lowercase().as_str() {
            "postgres" | "postgresql" => AddonType::Postgres,
            "redis" => AddonType::Redis,
            "storage" | "s3" | "minio" => AddonType::Storage,
            _ => {
                return Ok(json_error(StatusCode::BAD_REQUEST, format!("Unknown addon type: {}", addon_type_str)));
            }
        };

        info!(app = %app_name, addon = %addon_type, "Removing addon");

        // Deprovision from Docker
        self.addon_manager.deprovision(app_name, &addon_type).await
            .context("Failed to remove addon")?;

        // Remove from database
        self.db.delete_addon(app_name, addon_type_str)?;

        let response: ApiResponse<()> = ApiResponse::ok(());
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    // ==================== Config Management ====================

    async fn get_config(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        let mut config = self.db.get_all_config(app_name)?;

        // Merge with addon env vars
        let addon_env = self.addon_manager.get_env_vars(app_name).await;
        for (k, v) in addon_env {
            config.entry(k).or_insert(v);
        }

        let response = ApiResponse::ok(config);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    async fn set_config(
        self: Arc<Self>,
        app_name: &str,
        req: Request<hyper::body::Incoming>,
    ) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        let body = req.collect().await?.to_bytes();
        let config_req: SetConfigRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => {
                return Ok(json_error(StatusCode::BAD_REQUEST, format!("Invalid JSON: {}", e)));
            }
        };

        // Update config in database
        for (key, value) in config_req.env {
            if value.is_empty() {
                self.db.delete_config(app_name, &key)?;
            } else {
                let is_secret = key.contains("SECRET") || key.contains("PASSWORD") || key.contains("KEY");
                self.db.set_config(app_name, &key, &value, is_secret)?;
            }
        }

        let response: ApiResponse<()> = ApiResponse::ok(());
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    // ==================== Deployment ====================

    async fn deploy_app(
        self: Arc<Self>,
        app_name: &str,
        req: Request<hyper::body::Incoming>,
    ) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        let work_path = self.config.data_dir.join("work").join(app_name);

        let body = req.collect().await?.to_bytes();
        let deploy_req: DeployRequest = if body.is_empty() {
            DeployRequest {
                source: None,
                build_mode: None,
                clear_cache: false,
            }
        } else {
            match serde_json::from_slice(&body) {
                Ok(r) => r,
                Err(e) => {
                    return Ok(json_error(StatusCode::BAD_REQUEST, format!("Invalid JSON: {}", e)));
                }
            }
        };

        // Update app status to building
        self.db.update_app_status(app_name, "building")?;

        // Create deployment record
        let deploy_id = uuid::Uuid::new_v4().to_string();
        let deployment = DeploymentRecord {
            id: deploy_id.clone(),
            app_name: app_name.to_string(),
            status: "building".to_string(),
            image: None,
            commit_hash: None,
            build_logs: None,
            duration_secs: None,
            created_at: String::new(),
            finished_at: None,
        };
        self.db.create_deployment(&deployment)?;

        info!(app = %app_name, deploy_id = %deploy_id, "Starting deployment");

        // Determine source path
        let source_path = deploy_req.source
            .map(PathBuf::from)
            .unwrap_or(work_path);

        if !source_path.exists() {
            self.db.update_app_status(app_name, "failed")?;
            self.db.update_deployment(&deploy_id, "failed", None, Some("Source path does not exist"), None)?;
            return Ok(json_error(StatusCode::BAD_REQUEST, "Source path does not exist. Push code with git first."));
        }

        // Determine build mode
        let build_mode = deploy_req.build_mode
            .as_ref()
            .and_then(|m| match m.to_lowercase().as_str() {
                "dockerfile" | "docker" => Some(BuildMode::Dockerfile),
                "buildpack" | "buildpacks" => Some(BuildMode::Buildpack),
                "compose" | "docker-compose" => Some(BuildMode::Compose),
                "image" => Some(BuildMode::Image),
                _ => None,
            })
            .unwrap_or(BuildMode::Auto);

        // Build config
        let config = BuildConfig {
            app_name: app_name.to_string(),
            source_path,
            build_mode,
            dockerfile: None,
            target: None,
            build_args: HashMap::new(),
            builder: String::new(),
            buildpacks: vec![],
            build_env: HashMap::new(),
            registry: self.config.registry.clone(),
            tag: "latest".to_string(),
            clear_cache: deploy_req.clear_cache,
            platform: None,
            image: None,
        };

        // Run build
        let result = self.builder.build(&config).await?;

        // Update deployment record
        let logs = result.logs.join("\n");
        self.db.update_deployment(
            &deploy_id,
            if result.success { "success" } else { "failed" },
            Some(&result.image),
            Some(&logs),
            Some(result.duration_secs),
        )?;

        // Update app state
        if result.success {
            self.db.update_app_status(app_name, "idle")?;
            self.db.update_app_deployment(app_name, &result.image, None)?;
            info!(app = %app_name, image = %result.image, "Deployment successful");
        } else {
            self.db.update_app_status(app_name, "failed")?;
            error!(app = %app_name, error = ?result.error, "Deployment failed");
        }

        let response = ApiResponse::ok(result);
        Ok(json_response(
            if response.data.as_ref().map(|r| r.success).unwrap_or(false) {
                StatusCode::OK
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            },
            serde_json::to_string(&response)?,
        ))
    }

    async fn list_deployments(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        let deployments = self.db.get_deployments(app_name, 20)?;
        let response = ApiResponse::ok(deployments);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    async fn trigger_build(self: Arc<Self>, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Triggered by git post-receive hook
        info!(app = %app_name, "Build triggered by git push");

        let result = self.git_server.build_app(app_name).await?;

        // Create deployment record
        let deploy_id = uuid::Uuid::new_v4().to_string();
        let logs = result.logs.join("\n");

        let deployment = DeploymentRecord {
            id: deploy_id.clone(),
            app_name: app_name.to_string(),
            status: if result.success { "success" } else { "failed" }.to_string(),
            image: Some(result.image.clone()),
            commit_hash: None,
            build_logs: Some(logs),
            duration_secs: Some(result.duration_secs),
            created_at: String::new(),
            finished_at: None,
        };
        self.db.create_deployment(&deployment)?;

        // Update app state
        if result.success {
            self.db.update_app_status(app_name, "idle")?;
            self.db.update_app_deployment(app_name, &result.image, None)?;
        } else {
            self.db.update_app_status(app_name, "failed")?;
        }

        let response = ApiResponse::ok(result);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    // ==================== Logs ====================

    async fn get_logs(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        // Get logs from recent deployment
        let deployments = self.db.get_deployments(app_name, 1)?;
        let logs: Vec<String> = if let Some(deploy) = deployments.first() {
            deploy.build_logs.as_ref()
                .map(|l| l.lines().map(String::from).collect())
                .unwrap_or_default()
        } else {
            vec![]
        };

        let response = ApiResponse::ok(logs);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    // ==================== Git Info ====================

    async fn get_git_info(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        let remote_url = self.git_server.get_remote_url(app_name);
        let repo = self.git_server.get_app(app_name).await;

        let info = serde_json::json!({
            "remote_url": remote_url,
            "current_commit": repo.as_ref().and_then(|r| r.current_commit.clone()),
            "repo_path": repo.as_ref().map(|r| r.repo_path.display().to_string()),
        });

        let response = ApiResponse::ok(info);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    // ==================== Scaling ====================

    async fn scale_app(
        self: Arc<Self>,
        app_name: &str,
        req: Request<hyper::body::Incoming>,
    ) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        let app = match self.db.get_app(app_name)? {
            Some(app) => app,
            None => return Ok(json_error(StatusCode::NOT_FOUND, "App not found")),
        };

        let body = req.collect().await?.to_bytes();
        let scale_req: ScaleRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => {
                return Ok(json_error(StatusCode::BAD_REQUEST, format!("Invalid JSON: {}", e)));
            }
        };

        // Validate scale
        if scale_req.scale < 0 || scale_req.scale > app.max_scale {
            return Ok(json_error(
                StatusCode::BAD_REQUEST,
                format!("Scale must be between 0 and {}", app.max_scale),
            ));
        }

        info!(app = %app_name, scale = scale_req.scale, "Scaling app");

        // Check if app has been deployed (has an image)
        if app.image.is_none() && scale_req.scale > 0 {
            return Ok(json_error(
                StatusCode::BAD_REQUEST,
                "App has not been deployed yet. Deploy the app first with 'git push paas main'",
            ));
        }

        // Use DynoManager to actually spawn/stop containers
        if let Err(e) = self.dyno_manager.scale(app_name, "web", scale_req.scale).await {
            error!(app = %app_name, error = %e, "Failed to scale app");
            return Ok(json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to scale app: {}", e),
            ));
        }

        let result = serde_json::json!({
            "app": app_name,
            "scale": scale_req.scale,
            "previous_scale": app.scale,
        });

        let response = ApiResponse::ok(result);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    async fn list_processes(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        let processes = self.db.get_app_processes(app_name)?;
        let response = ApiResponse::ok(processes);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }
}

// ==================== Helper Functions ====================

fn json_response(status: StatusCode, body: impl Into<Bytes>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(body.into()))
        .expect("valid response")
}

fn json_error(status: StatusCode, message: impl Into<String>) -> Response<Full<Bytes>> {
    let response: ApiResponse<()> = ApiResponse::error(message);
    json_response(status, serde_json::to_string(&response).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = PlatformApiConfig::default();
        assert_eq!(config.bind_addr.port(), 9999);
        assert_eq!(config.network_name, "spawngate");
    }

    #[test]
    fn test_api_response() {
        let response: ApiResponse<String> = ApiResponse::ok("test".to_string());
        assert!(response.success);
        assert_eq!(response.data, Some("test".to_string()));
        assert!(response.error.is_none());

        let error: ApiResponse<String> = ApiResponse::error("failed");
        assert!(!error.success);
        assert!(error.data.is_none());
        assert_eq!(error.error, Some("failed".to_string()));
    }
}
