//! Platform API server for PaaS management
//!
//! This module provides RESTful API endpoints for managing applications,
//! add-ons, deployments, and logs. The CLI tool communicates with this API.

use crate::addons::{AddonConfig, AddonManager, AddonPlan, AddonType};
use crate::builder::{BuildConfig, BuildMode, Builder};
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
use tokio::sync::{watch, RwLock};
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

/// Application state stored in the platform
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    /// Application name (unique identifier)
    pub name: String,

    /// Application status
    pub status: AppStatus,

    /// Git repository URL
    pub git_url: Option<String>,

    /// Currently deployed image
    pub image: Option<String>,

    /// Port the application listens on
    pub port: u16,

    /// Environment variables
    pub env: HashMap<String, String>,

    /// Attached add-ons
    pub addons: Vec<String>,

    /// Created timestamp
    pub created_at: String,

    /// Last deployed timestamp
    pub deployed_at: Option<String>,

    /// Current commit hash
    pub commit: Option<String>,
}

/// Application lifecycle status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppStatus {
    /// App is being created
    Creating,
    /// App is idle (not running)
    Idle,
    /// App is building
    Building,
    /// App is running
    Running,
    /// App failed to start/build
    Failed,
    /// App is being deleted
    Deleting,
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
    /// Source path (local) or git URL
    pub source: Option<String>,
    /// Build mode override
    pub build_mode: Option<String>,
    /// Clear build cache
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
    apps: Arc<RwLock<HashMap<String, App>>>,
    addon_manager: Arc<AddonManager>,
    builder: Arc<Builder>,
    git_server: Arc<GitServer>,
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

        // Load existing app state
        let apps = Self::load_apps(&config.data_dir).await.unwrap_or_default();

        Ok(Self {
            config,
            apps: Arc::new(RwLock::new(apps)),
            addon_manager: Arc::new(addon_manager),
            builder: Arc::new(builder),
            git_server: Arc::new(git_server),
            shutdown_rx,
        })
    }

    /// Load app state from disk
    async fn load_apps(data_dir: &PathBuf) -> Result<HashMap<String, App>> {
        let apps_file = data_dir.join("apps.json");
        if apps_file.exists() {
            let content = tokio::fs::read_to_string(&apps_file).await?;
            let apps: HashMap<String, App> = serde_json::from_str(&content)?;
            info!("Loaded {} apps from state file", apps.len());
            Ok(apps)
        } else {
            Ok(HashMap::new())
        }
    }

    /// Save app state to disk
    async fn save_apps(&self) -> Result<()> {
        let apps = self.apps.read().await;
        let apps_file = self.config.data_dir.join("apps.json");
        let content = serde_json::to_string_pretty(&*apps)?;
        tokio::fs::write(&apps_file, content).await?;
        Ok(())
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
            (Method::GET, path) if path.starts_with("/apps/") && !path.contains("/addons") && !path.contains("/logs") && !path.contains("/config") && !path.contains("/deploy") => {
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
        let apps = self.apps.read().await;
        let app_list: Vec<&App> = apps.values().collect();

        let response = ApiResponse::ok(app_list);
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
        {
            let apps = self.apps.read().await;
            if apps.contains_key(&create_req.name) {
                return Ok(json_error(StatusCode::CONFLICT, "App already exists"));
            }
        }

        // Create git repository
        let git_repo = self.git_server.create_app(&create_req.name).await
            .context("Failed to create git repository")?;

        let now = chrono_now();

        let app = App {
            name: create_req.name.clone(),
            status: AppStatus::Idle,
            git_url: Some(self.git_server.get_remote_url(&create_req.name)),
            image: None,
            port: create_req.port,
            env: create_req.env,
            addons: vec![],
            created_at: now,
            deployed_at: None,
            commit: None,
        };

        // Store app
        {
            let mut apps = self.apps.write().await;
            apps.insert(app.name.clone(), app.clone());
        }

        // Persist state
        self.save_apps().await?;

        info!(app = %app.name, git_url = ?git_repo.repo_path, "Created new app");

        let response = ApiResponse::ok(app);
        Ok(json_response(StatusCode::CREATED, serde_json::to_string(&response)?))
    }

    async fn get_app(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        let apps = self.apps.read().await;

        if let Some(app) = apps.get(app_name) {
            let response = ApiResponse::ok(app.clone());
            Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
        } else {
            Ok(json_error(StatusCode::NOT_FOUND, "App not found"))
        }
    }

    async fn delete_app(self: Arc<Self>, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        {
            let apps = self.apps.read().await;
            if !apps.contains_key(app_name) {
                return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
            }
        }

        info!(app = %app_name, "Deleting app");

        // Remove add-ons
        self.addon_manager.deprovision_all(app_name).await?;

        // Delete git repository
        self.git_server.delete_app(app_name).await?;

        // Remove from state
        {
            let mut apps = self.apps.write().await;
            apps.remove(app_name);
        }

        // Persist state
        self.save_apps().await?;

        let response: ApiResponse<()> = ApiResponse::ok(());
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    // ==================== Add-on Management ====================

    async fn list_addons(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        {
            let apps = self.apps.read().await;
            if !apps.contains_key(app_name) {
                return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
            }
        }

        let addons = self.addon_manager.get_app_addons(app_name).await;
        let response = ApiResponse::ok(addons);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    async fn add_addon(
        self: Arc<Self>,
        app_name: &str,
        req: Request<hyper::body::Incoming>,
    ) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        {
            let apps = self.apps.read().await;
            if !apps.contains_key(app_name) {
                return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
            }
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
            plan,
            name: None,
            network: Some(self.config.network_name.clone()),
        };

        info!(app = %app_name, addon = %addon_type, "Provisioning addon");

        let instance = self.addon_manager.provision(app_name, &config).await
            .context("Failed to provision addon")?;

        // Update app's addon list
        {
            let mut apps = self.apps.write().await;
            if let Some(app) = apps.get_mut(app_name) {
                let addon_key = format!("{}:{}", addon_type, instance.id);
                if !app.addons.contains(&addon_key) {
                    app.addons.push(addon_key);
                }
            }
        }

        self.save_apps().await?;

        let response = ApiResponse::ok(instance);
        Ok(json_response(StatusCode::CREATED, serde_json::to_string(&response)?))
    }

    async fn remove_addon(self: Arc<Self>, app_name: &str, addon_type_str: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        {
            let apps = self.apps.read().await;
            if !apps.contains_key(app_name) {
                return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
            }
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

        self.addon_manager.deprovision(app_name, &addon_type).await
            .context("Failed to remove addon")?;

        // Update app's addon list
        {
            let mut apps = self.apps.write().await;
            if let Some(app) = apps.get_mut(app_name) {
                app.addons.retain(|a| !a.starts_with(&format!("{}:", addon_type)));
            }
        }

        self.save_apps().await?;

        let response: ApiResponse<()> = ApiResponse::ok(());
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    // ==================== Config Management ====================

    async fn get_config(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        let apps = self.apps.read().await;

        if let Some(app) = apps.get(app_name) {
            // Merge app env with addon env vars
            let addon_env = self.addon_manager.get_env_vars(app_name).await;
            let mut config = app.env.clone();
            for (k, v) in addon_env {
                config.entry(k).or_insert(v);
            }

            let response = ApiResponse::ok(config);
            Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
        } else {
            Ok(json_error(StatusCode::NOT_FOUND, "App not found"))
        }
    }

    async fn set_config(
        self: Arc<Self>,
        app_name: &str,
        req: Request<hyper::body::Incoming>,
    ) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        {
            let apps = self.apps.read().await;
            if !apps.contains_key(app_name) {
                return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
            }
        }

        let body = req.collect().await?.to_bytes();
        let config_req: SetConfigRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => {
                return Ok(json_error(StatusCode::BAD_REQUEST, format!("Invalid JSON: {}", e)));
            }
        };

        // Update app config
        {
            let mut apps = self.apps.write().await;
            if let Some(app) = apps.get_mut(app_name) {
                for (k, v) in config_req.env {
                    if v.is_empty() {
                        app.env.remove(&k);
                    } else {
                        app.env.insert(k, v);
                    }
                }
            }
        }

        self.save_apps().await?;

        let response: ApiResponse<()> = ApiResponse::ok(());
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    // ==================== Deployment ====================

    async fn deploy_app(
        self: Arc<Self>,
        app_name: &str,
        req: Request<hyper::body::Incoming>,
    ) -> Result<Response<Full<Bytes>>> {
        // Check if app exists and get work path
        let work_path = {
            let apps = self.apps.read().await;
            if !apps.contains_key(app_name) {
                return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
            }
            self.config.data_dir.join("work").join(app_name)
        };

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
        {
            let mut apps = self.apps.write().await;
            if let Some(app) = apps.get_mut(app_name) {
                app.status = AppStatus::Building;
            }
        }

        info!(app = %app_name, "Starting deployment");

        // Determine source path
        let source_path = deploy_req.source
            .map(PathBuf::from)
            .unwrap_or(work_path);

        if !source_path.exists() {
            // Update status to failed
            let mut apps = self.apps.write().await;
            if let Some(app) = apps.get_mut(app_name) {
                app.status = AppStatus::Failed;
            }
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

        // Update app state
        {
            let mut apps = self.apps.write().await;
            if let Some(app) = apps.get_mut(app_name) {
                if result.success {
                    app.status = AppStatus::Idle; // Will be Running when started
                    app.image = Some(result.image.clone());
                    app.deployed_at = Some(chrono_now());
                } else {
                    app.status = AppStatus::Failed;
                }
            }
        }

        self.save_apps().await?;

        if result.success {
            info!(app = %app_name, image = %result.image, "Deployment successful");
        } else {
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

    async fn trigger_build(self: Arc<Self>, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Triggered by git post-receive hook
        info!(app = %app_name, "Build triggered by git push");

        let result = self.git_server.build_app(app_name).await?;

        // Update app state
        {
            let mut apps = self.apps.write().await;
            if let Some(app) = apps.get_mut(app_name) {
                if result.success {
                    app.status = AppStatus::Idle;
                    app.image = Some(result.image.clone());
                    app.deployed_at = Some(chrono_now());
                } else {
                    app.status = AppStatus::Failed;
                }
            }
        }

        self.save_apps().await?;

        let response = ApiResponse::ok(result);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    // ==================== Logs ====================

    async fn get_logs(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        {
            let apps = self.apps.read().await;
            if !apps.contains_key(app_name) {
                return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
            }
        }

        // For now, return empty logs
        // TODO: Integrate with container logs
        let logs: Vec<String> = vec![];
        let response = ApiResponse::ok(logs);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    // ==================== Git Info ====================

    async fn get_git_info(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        {
            let apps = self.apps.read().await;
            if !apps.contains_key(app_name) {
                return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
            }
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

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap();
    format!("{}", duration.as_secs())
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

    #[test]
    fn test_chrono_now() {
        let now = chrono_now();
        let parsed: u64 = now.parse().unwrap();
        assert!(parsed > 1700000000); // After 2023
    }
}
