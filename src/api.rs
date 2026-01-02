//! Platform API server for PaaS management
//!
//! This module provides RESTful API endpoints for managing applications,
//! add-ons, deployments, and logs. The CLI tool communicates with this API.

use crate::addons::{AddonConfig, AddonManager, AddonPlan, AddonType};
use crate::auth::{AuthConfig, AuthManager};
use crate::builder::{BuildConfig, BuildMode, Builder};
use crate::dashboard;
use crate::db::{AddonRecord, AppRecord, Database, DeploymentRecord, WebhookRecord, WebhookEventRecord};
use crate::domains::{DnsVerifier, DomainManager, SslManager};
use crate::docker::DockerManager;
use crate::instance::{InstanceConfig, InstanceManager};
use crate::git::{GitServer, GitServerConfig};
use crate::healthcheck::{HealthCheckConfig, HealthChecker};
use crate::loadbalancer::LoadBalancerManager;
use crate::secrets::SecretsManager;
use crate::webhooks::{
    DeployStatus, WebhookConfig, WebhookHandler, WebhookProvider,
    generate_badge_svg, generate_webhook_secret, StatusNotifier,
};
use anyhow::{Context, Result};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, SET_COOKIE};
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

/// Request to set a secret
#[derive(Debug, Deserialize)]
pub struct SetSecretRequest {
    pub key: String,
    pub value: String,
}

/// Request to set multiple secrets
#[derive(Debug, Deserialize)]
pub struct SetSecretsRequest {
    pub secrets: HashMap<String, String>,
}

/// Add a custom domain to an app
#[derive(Debug, Deserialize)]
pub struct AddDomainRequest {
    pub domain: String,
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
    instance_manager: Arc<InstanceManager>,
    load_balancer: Arc<LoadBalancerManager>,
    secrets_manager: Arc<tokio::sync::RwLock<SecretsManager>>,
    webhook_handler: Arc<tokio::sync::RwLock<WebhookHandler>>,
    status_notifier: Arc<StatusNotifier>,
    domain_manager: Arc<DomainManager>,
    dns_verifier: Arc<DnsVerifier>,
    ssl_manager: Arc<SslManager>,
    auth_manager: Arc<AuthManager>,
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

        // Initialize Docker, load balancer, and instance manager
        let docker = DockerManager::new(None).await?;
        let db_arc = Arc::new(db);
        let load_balancer = Arc::new(LoadBalancerManager::default());
        let instance_config = InstanceConfig {
            network: config.network_name.clone(),
            health_check_url: Some(format!("http://{}", config.bind_addr)),
            memory_limit: Some("512m".to_string()),
            cpu_limit: Some("0.5".to_string()),
        };
        let instance_manager = InstanceManager::new(
            Arc::new(docker),
            Arc::clone(&db_arc),
            instance_config,
            Arc::clone(&load_balancer),
        ).await?;

        // Initialize secrets manager
        let secrets_key_path = config.data_dir.join("secrets.key");
        let secrets_manager = if secrets_key_path.exists() {
            SecretsManager::load_from_file(&secrets_key_path)
                .context("Failed to load secrets key")?
        } else {
            let sm = SecretsManager::new();
            sm.save_to_file(&secrets_key_path)
                .context("Failed to save secrets key")?;
            info!("Generated new secrets encryption key");
            sm
        };

        // Initialize webhook handler with existing configurations
        let webhook_handler = WebhookHandler::new();

        // Load existing webhook configs from database
        // (Configs are loaded on-demand from the database)

        // Initialize status notifier for CI updates
        let status_notifier = StatusNotifier::new();

        // Initialize domain management
        let domain_manager = DomainManager::new();
        let dns_verifier = DnsVerifier::new();
        let ssl_manager = SslManager::new(config.data_dir.join("certs"));

        // Load existing domains from database
        if let Ok(domains) = db_arc.get_all_domains() {
            let domain_count = domains.len();
            for domain_record in domains {
                domain_manager.add_domain_from_db(
                    &domain_record.domain,
                    &domain_record.app_name,
                    domain_record.verified,
                    domain_record.ssl_enabled,
                    domain_record.verification_token,
                    domain_record.cert_expires_at,
                ).await;
            }
            debug!("Loaded {} custom domains from database", domain_count);
        }

        // Initialize JWT auth manager
        let auth_config = AuthConfig {
            secret: config.auth_token.clone(),
            token_expiry_hours: 24,
            cookie_name: "spawngate_session".to_string(),
            cookie_secure: !config.bind_addr.ip().is_loopback(),
            cookie_http_only: true,
            cookie_same_site: "Strict".to_string(),
        };
        let auth_manager = AuthManager::new(auth_config);
        debug!("JWT authentication initialized");

        Ok(Self {
            config,
            db: db_arc,
            addon_manager: Arc::new(addon_manager),
            builder: Arc::new(builder),
            git_server: Arc::new(git_server),
            instance_manager: Arc::new(instance_manager),
            load_balancer,
            secrets_manager: Arc::new(tokio::sync::RwLock::new(secrets_manager)),
            webhook_handler: Arc::new(tokio::sync::RwLock::new(webhook_handler)),
            status_notifier: Arc::new(status_notifier),
            domain_manager: Arc::new(domain_manager),
            dns_verifier: Arc::new(dns_verifier),
            ssl_manager: Arc::new(ssl_manager),
            auth_manager: Arc::new(auth_manager),
            shutdown_rx,
        })
    }

    /// Run the API server
    pub async fn run(self: Arc<Self>) -> Result<()> {
        let listener = TcpListener::bind(self.config.bind_addr).await?;
        info!(addr = %self.config.bind_addr, "Platform API server listening");

        // Start health checker in background
        let health_checker = HealthChecker::new(
            Arc::clone(&self.db),
            Arc::clone(&self.load_balancer),
            HealthCheckConfig::default(),
            self.shutdown_rx.clone(),
        );
        tokio::spawn(async move {
            health_checker.run().await;
        });

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
        // Try JWT from cookie first (dashboard sessions)
        if let Some(cookie) = req.headers().get(COOKIE).and_then(|v| v.to_str().ok()) {
            if let Some(token) = self.auth_manager.extract_token_from_cookie(cookie) {
                if self.auth_manager.verify_token(&token).is_ok() {
                    return true;
                }
            }
        }

        // Try JWT from Authorization header
        if let Some(auth) = req.headers().get(AUTHORIZATION).and_then(|v| v.to_str().ok()) {
            if let Some(token) = self.auth_manager.extract_token_from_header(auth) {
                if self.auth_manager.verify_token(&token).is_ok() {
                    return true;
                }
            }

            // Fall back to legacy static token auth (for CLI compatibility)
            let token = auth.strip_prefix("Bearer ").unwrap_or(auth);
            if token == self.config.auth_token {
                return true;
            }
        }

        false
    }

    fn check_dashboard_auth(&self, req: &Request<hyper::body::Incoming>) -> bool {
        // Only check cookie-based JWT for dashboard
        if let Some(cookie) = req.headers().get(COOKIE).and_then(|v| v.to_str().ok()) {
            if let Some(token) = self.auth_manager.extract_token_from_cookie(cookie) {
                return self.auth_manager.verify_token(&token).is_ok();
            }
        }
        false
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

        // Dashboard static assets - no auth required
        if path == "/dashboard/style.css" {
            return Ok(dashboard::serve_css());
        }
        if path == "/dashboard/app.js" {
            return Ok(dashboard::serve_js());
        }

        // Dashboard login page - no auth required
        if path == "/dashboard/login" && method == Method::GET {
            return Ok(dashboard::serve_login());
        }

        // Dashboard login handler - authenticate and set cookie
        if path == "/dashboard/auth" && method == Method::POST {
            return Ok(self.handle_dashboard_login(req).await);
        }

        // Dashboard logout - clear session cookie
        if path == "/dashboard/logout" && method == Method::POST {
            return Ok(self.handle_dashboard_logout());
        }

        // Main dashboard - requires auth, redirect to login if not authenticated
        if path == "/" || path == "/dashboard" || path == "/dashboard/" {
            if self.check_dashboard_auth(&req) {
                return Ok(dashboard::serve_dashboard());
            } else {
                return Ok(redirect_response("/dashboard/login"));
            }
        }

        // Dashboard API endpoints - require auth
        if path.starts_with("/dashboard/") {
            if !self.check_dashboard_auth(&req) {
                return Ok(redirect_response("/dashboard/login"));
            }
            // Handle dashboard-specific API endpoints here (like /dashboard/apps for HTMX)
            return Ok(self.handle_dashboard_api(&path, &method, req).await);
        }

        // API endpoints - require auth
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

            // Individual instance restart
            (Method::POST, path) if path.contains("/instances/") && path.ends_with("/restart") => {
                let path_trimmed = path.strip_suffix("/restart").unwrap_or("");
                let parts: Vec<&str> = path_trimmed.split('/').collect();
                if parts.len() >= 5 {
                    let app_name = parts[2];
                    let instance_id = parts[4];
                    self.restart_instance(app_name, instance_id).await
                } else {
                    Ok(json_error(StatusCode::BAD_REQUEST, "Invalid path"))
                }
            }

            // Individual instance stop
            (Method::POST, path) if path.contains("/instances/") && path.ends_with("/stop") => {
                let path_trimmed = path.strip_suffix("/stop").unwrap_or("");
                let parts: Vec<&str> = path_trimmed.split('/').collect();
                if parts.len() >= 5 {
                    let app_name = parts[2];
                    let instance_id = parts[4];
                    self.stop_instance(app_name, instance_id).await
                } else {
                    Ok(json_error(StatusCode::BAD_REQUEST, "Invalid path"))
                }
            }

            // Restart (rolling deploy)
            (Method::POST, path) if path.ends_with("/restart") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/restart")).unwrap_or("");
                self.restart_app(app_name).await
            }

            // Logs
            (Method::GET, path) if path.ends_with("/logs") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/logs")).unwrap_or("");
                self.get_logs(app_name).await
            }

            // Metrics
            (Method::GET, path) if path.ends_with("/metrics") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/metrics")).unwrap_or("");
                self.get_app_metrics(app_name).await
            }

            // Git info
            (Method::GET, path) if path.ends_with("/git") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/git")).unwrap_or("");
                self.get_git_info(app_name).await
            }

            // Secrets management
            (Method::GET, path) if path.ends_with("/secrets") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/secrets")).unwrap_or("");
                self.list_secrets(app_name).await
            }
            (Method::POST, path) if path.ends_with("/secrets") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/secrets")).unwrap_or("");
                self.set_secrets(app_name, req).await
            }
            (Method::DELETE, path) if path.contains("/secrets/") => {
                let parts: Vec<&str> = path.split('/').collect();
                if parts.len() >= 5 {
                    let app_name = parts[2];
                    let secret_key = parts[4];
                    self.delete_secret(app_name, secret_key).await
                } else {
                    Ok(json_error(StatusCode::BAD_REQUEST, "Invalid path"))
                }
            }
            (Method::GET, path) if path.ends_with("/secrets/audit") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/secrets/audit")).unwrap_or("");
                self.get_secrets_audit_log(app_name).await
            }

            // Key rotation
            (Method::POST, "/secrets/rotate") => self.rotate_encryption_key().await,

            // Webhook management
            (Method::GET, path) if path.ends_with("/webhook") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/webhook")).unwrap_or("");
                self.get_webhook_config(app_name).await
            }
            (Method::POST, path) if path.ends_with("/webhook") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/webhook")).unwrap_or("");
                self.create_webhook(app_name, req).await
            }
            (Method::DELETE, path) if path.ends_with("/webhook") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/webhook")).unwrap_or("");
                self.delete_webhook(app_name).await
            }
            (Method::GET, path) if path.ends_with("/webhook/events") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/webhook/events")).unwrap_or("");
                self.get_webhook_events(app_name).await
            }

            // Incoming webhooks (GitHub/GitLab)
            (Method::POST, path) if path.starts_with("/webhooks/") => {
                let app_name = path.strip_prefix("/webhooks/").unwrap_or("");
                self.handle_incoming_webhook(app_name, req).await
            }

            // Build status badge
            (Method::GET, path) if path.ends_with("/badge.svg") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/badge.svg")).unwrap_or("");
                self.get_build_badge(app_name).await
            }

            // Custom domains
            (Method::GET, path) if path.ends_with("/domains") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/domains")).unwrap_or("");
                self.list_domains(app_name).await
            }
            (Method::POST, path) if path.ends_with("/domains") => {
                let app_name = path.strip_prefix("/apps/").and_then(|p| p.strip_suffix("/domains")).unwrap_or("");
                self.add_domain(app_name, req).await
            }
            (Method::DELETE, path) if path.contains("/domains/") => {
                let parts: Vec<&str> = path.split('/').collect();
                if parts.len() >= 5 {
                    let app_name = parts[2];
                    let domain = parts[4];
                    self.remove_domain(app_name, domain).await
                } else {
                    Ok(json_error(StatusCode::BAD_REQUEST, "Invalid path"))
                }
            }
            (Method::POST, path) if path.contains("/domains/") && path.ends_with("/verify") => {
                let path_trimmed = path.strip_suffix("/verify").unwrap_or("");
                let parts: Vec<&str> = path_trimmed.split('/').collect();
                if parts.len() >= 5 {
                    let app_name = parts[2];
                    let domain = parts[4];
                    self.verify_domain(app_name, domain).await
                } else {
                    Ok(json_error(StatusCode::BAD_REQUEST, "Invalid path"))
                }
            }
            (Method::POST, path) if path.contains("/domains/") && path.ends_with("/ssl") => {
                let path_trimmed = path.strip_suffix("/ssl").unwrap_or("");
                let parts: Vec<&str> = path_trimmed.split('/').collect();
                if parts.len() >= 5 {
                    let app_name = parts[2];
                    let domain = parts[4];
                    self.enable_domain_ssl(app_name, domain).await
                } else {
                    Ok(json_error(StatusCode::BAD_REQUEST, "Invalid path"))
                }
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

    // ==================== Metrics ====================

    async fn get_app_metrics(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        // Get running processes for this app
        let processes = self.db.get_app_processes(app_name)?;
        let running_containers: Vec<_> = processes
            .iter()
            .filter(|p| p.status == "running" && p.container_id.is_some())
            .collect();

        let mut total_cpu: f64 = 0.0;
        let mut total_memory_used: u64 = 0;
        let mut total_memory_limit: u64 = 0;
        let mut container_count = 0;

        for proc in &running_containers {
            if let Some(ref container_id) = proc.container_id {
                if let Ok(stats) = self.instance_manager.get_container_stats(container_id).await {
                    total_cpu += stats.cpu_percent;
                    total_memory_used += stats.memory_used;
                    total_memory_limit += stats.memory_limit;
                    container_count += 1;
                }
            }
        }

        let metrics = if container_count > 0 {
            let memory_percent = if total_memory_limit > 0 {
                (total_memory_used as f64 / total_memory_limit as f64) * 100.0
            } else {
                0.0
            };

            serde_json::json!({
                "cpu_percent": total_cpu,
                "memory_percent": memory_percent,
                "memory_used": total_memory_used,
                "memory_limit": total_memory_limit,
                "cpu_cores": container_count,
                "instance_count": running_containers.len(),
            })
        } else {
            serde_json::json!({
                "cpu_percent": 0,
                "memory_percent": 0,
                "memory_used": 0,
                "memory_limit": 0,
                "cpu_cores": 0,
                "instance_count": 0,
            })
        };

        let response = ApiResponse::ok(metrics);
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

        // Use InstanceManager to actually spawn/stop containers
        if let Err(e) = self.instance_manager.scale(app_name, "web", scale_req.scale).await {
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

    async fn restart_app(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        info!(app = %app_name, "Triggering rolling restart");

        // Use InstanceManager for rolling restart
        match self.instance_manager.rolling_restart(app_name, None).await {
            Ok(result) => {
                let response = ApiResponse::ok(result);
                Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
            }
            Err(e) => {
                error!(app = %app_name, error = %e, "Rolling restart failed");
                Ok(json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Rolling restart failed: {}", e),
                ))
            }
        }
    }

    async fn restart_instance(&self, app_name: &str, instance_id: &str) -> Result<Response<Full<Bytes>>> {
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        info!(app = %app_name, instance = %instance_id, "Restarting individual instance");

        match self.instance_manager.restart_instance(app_name, instance_id).await {
            Ok(_) => {
                let response = ApiResponse::ok(serde_json::json!({
                    "app": app_name,
                    "instance": instance_id,
                    "status": "restarting"
                }));
                Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
            }
            Err(e) => {
                error!(app = %app_name, instance = %instance_id, error = %e, "Instance restart failed");
                Ok(json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Instance restart failed: {}", e),
                ))
            }
        }
    }

    async fn stop_instance(&self, app_name: &str, instance_id: &str) -> Result<Response<Full<Bytes>>> {
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        info!(app = %app_name, instance = %instance_id, "Stopping individual instance");

        match self.instance_manager.stop_instance(instance_id).await {
            Ok(_) => {
                let response = ApiResponse::ok(serde_json::json!({
                    "app": app_name,
                    "instance": instance_id,
                    "status": "stopped"
                }));
                Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
            }
            Err(e) => {
                error!(app = %app_name, instance = %instance_id, error = %e, "Instance stop failed");
                Ok(json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Instance stop failed: {}", e),
                ))
            }
        }
    }

    // ==================== Secrets Management ====================

    async fn list_secrets(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        // Get list of secret keys (not values)
        let secret_keys = self.db.get_secret_keys(app_name)?;

        let result = serde_json::json!({
            "app": app_name,
            "secrets": secret_keys,
            "count": secret_keys.len(),
        });

        let response = ApiResponse::ok(result);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    async fn set_secrets(
        self: Arc<Self>,
        app_name: &str,
        req: Request<hyper::body::Incoming>,
    ) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        let body = req.collect().await?.to_bytes();
        let secrets_req: SetSecretsRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => {
                return Ok(json_error(StatusCode::BAD_REQUEST, format!("Invalid JSON: {}", e)));
            }
        };

        let secrets_manager = self.secrets_manager.read().await;
        let mut set_count = 0;

        for (key, value) in secrets_req.secrets {
            // Encrypt the secret value
            let encrypted = match secrets_manager.encrypt(&value) {
                Ok(enc) => enc,
                Err(e) => {
                    error!(key = %key, error = %e, "Failed to encrypt secret");
                    return Ok(json_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to encrypt secret: {}", e),
                    ));
                }
            };

            // Store encrypted value with is_secret=true
            self.db.set_config(app_name, &key, &encrypted, true)?;

            // Log the access
            self.db.log_secret_access(app_name, &key, "set", None, None)?;

            info!(app = %app_name, key = %key, "Secret set");
            set_count += 1;
        }

        let result = serde_json::json!({
            "app": app_name,
            "secrets_set": set_count,
        });

        let response = ApiResponse::ok(result);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    async fn delete_secret(&self, app_name: &str, secret_key: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        // Check if secret exists and is marked as secret
        if !self.db.is_secret_key(app_name, secret_key)? {
            return Ok(json_error(StatusCode::NOT_FOUND, "Secret not found"));
        }

        // Delete the secret
        self.db.delete_config(app_name, secret_key)?;

        // Log the deletion
        self.db.log_secret_access(app_name, secret_key, "delete", None, None)?;

        info!(app = %app_name, key = %secret_key, "Secret deleted");

        let response: ApiResponse<()> = ApiResponse::ok(());
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    async fn get_secrets_audit_log(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        let audit_log = self.db.get_secret_audit_log(app_name, 100)?;

        let response = ApiResponse::ok(audit_log);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    async fn rotate_encryption_key(self: Arc<Self>) -> Result<Response<Full<Bytes>>> {
        info!("Rotating encryption key");

        let mut secrets_manager = self.secrets_manager.write().await;

        // Get old key ID for logging
        let old_key_id = secrets_manager.current_key_id().to_string();

        // Rotate to new key
        let new_key = secrets_manager.rotate_key();
        let new_key_id = new_key.id().to_string();

        // Save the key file
        let key_path = self.config.data_dir.join("secrets.key");
        secrets_manager.save_to_file(&key_path)?;

        // Re-encrypt all secrets with new key
        let apps = self.db.list_apps()?;
        let mut re_encrypted_count = 0;

        for app in apps {
            let secret_keys = self.db.get_secret_keys(&app.name)?;

            for key in secret_keys {
                if let Some(encrypted_value) = self.db.get_config(&app.name, &key)? {
                    if SecretsManager::is_encrypted(&encrypted_value) {
                        match secrets_manager.re_encrypt(&encrypted_value) {
                            Ok(new_encrypted) => {
                                self.db.set_config(&app.name, &key, &new_encrypted, true)?;
                                self.db.log_secret_access(&app.name, &key, "re-encrypt", None, None)?;
                                re_encrypted_count += 1;
                            }
                            Err(e) => {
                                warn!(app = %app.name, key = %key, error = %e, "Failed to re-encrypt secret");
                            }
                        }
                    }
                }
            }
        }

        info!(
            old_key_id = %old_key_id,
            new_key_id = %new_key_id,
            re_encrypted = re_encrypted_count,
            "Key rotation complete"
        );

        let result = serde_json::json!({
            "old_key_id": old_key_id,
            "new_key_id": new_key_id,
            "secrets_re_encrypted": re_encrypted_count,
        });

        let response = ApiResponse::ok(result);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    /// Get decrypted config for an app (for internal use when starting containers)
    pub async fn get_decrypted_config(&self, app_name: &str) -> Result<HashMap<String, String>> {
        let config = self.db.get_all_config(app_name)?;
        let secrets_manager = self.secrets_manager.read().await;

        let mut decrypted = HashMap::new();
        for (key, value) in config {
            let decrypted_value = if SecretsManager::is_encrypted(&value) {
                // Log the access
                self.db.log_secret_access(app_name, &key, "read", None, None)?;
                secrets_manager.decrypt(&value)?
            } else {
                value
            };
            decrypted.insert(key, decrypted_value);
        }

        Ok(decrypted)
    }

    // ==================== Webhook Management ====================

    async fn get_webhook_config(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        match self.db.get_webhook(app_name)? {
            Some(webhook) => {
                // Don't expose the secret in the response, just show metadata
                let result = serde_json::json!({
                    "app": app_name,
                    "enabled": true,
                    "provider": webhook.provider,
                    "deploy_branch": webhook.deploy_branch,
                    "auto_deploy": webhook.auto_deploy,
                    "repo_name": webhook.repo_name,
                    "webhook_url": format!("{}/webhooks/{}", self.config.bind_addr, app_name),
                    "has_status_token": webhook.status_token.is_some(),
                    "created_at": webhook.created_at,
                    "updated_at": webhook.updated_at,
                });
                let response = ApiResponse::ok(result);
                Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
            }
            None => {
                let result = serde_json::json!({
                    "app": app_name,
                    "enabled": false,
                });
                let response = ApiResponse::ok(result);
                Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
            }
        }
    }

    async fn create_webhook(
        self: Arc<Self>,
        app_name: &str,
        req: Request<hyper::body::Incoming>,
    ) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        #[derive(Deserialize)]
        struct CreateWebhookRequest {
            #[serde(default = "default_provider")]
            provider: String,
            #[serde(default = "default_branch")]
            deploy_branch: String,
            #[serde(default = "default_true")]
            auto_deploy: bool,
            status_token: Option<String>,
            repo_name: Option<String>,
        }

        fn default_provider() -> String { "github".to_string() }
        fn default_branch() -> String { "main".to_string() }
        fn default_true() -> bool { true }

        let body = req.collect().await?.to_bytes();
        let webhook_req: CreateWebhookRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => {
                return Ok(json_error(StatusCode::BAD_REQUEST, format!("Invalid JSON: {}", e)));
            }
        };

        // Generate a secret if one doesn't exist
        let secret = generate_webhook_secret();

        let webhook = WebhookRecord {
            app_name: app_name.to_string(),
            secret: secret.clone(),
            provider: webhook_req.provider.clone(),
            deploy_branch: webhook_req.deploy_branch.clone(),
            auto_deploy: webhook_req.auto_deploy,
            status_token: webhook_req.status_token,
            repo_name: webhook_req.repo_name,
            created_at: String::new(), // Will be set by DB
            updated_at: String::new(),
        };

        self.db.save_webhook(&webhook)?;

        // Register with webhook handler
        let config = WebhookConfig {
            secret: secret.clone(),
            provider: webhook_req.provider.parse().unwrap_or(WebhookProvider::GitHub),
            deploy_branch: webhook_req.deploy_branch,
            auto_deploy: webhook_req.auto_deploy,
            status_token: webhook.status_token.clone(),
            repo_name: webhook.repo_name.clone(),
        };

        let mut handler = self.webhook_handler.write().await;
        handler.register(app_name, config);

        info!(app = %app_name, provider = %webhook.provider, "Webhook created");

        let result = serde_json::json!({
            "app": app_name,
            "webhook_url": format!("{}/webhooks/{}", self.config.bind_addr, app_name),
            "secret": secret,  // Only show secret on creation
            "provider": webhook.provider,
            "deploy_branch": webhook.deploy_branch,
            "auto_deploy": webhook.auto_deploy,
        });

        let response = ApiResponse::ok(result);
        Ok(json_response(StatusCode::CREATED, serde_json::to_string(&response)?))
    }

    async fn delete_webhook(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        let deleted = self.db.delete_webhook(app_name)?;

        if deleted {
            // Unregister from handler
            let mut handler = self.webhook_handler.write().await;
            handler.unregister(app_name);

            info!(app = %app_name, "Webhook deleted");
            let response: ApiResponse<()> = ApiResponse::ok(());
            Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
        } else {
            Ok(json_error(StatusCode::NOT_FOUND, "Webhook not found"))
        }
    }

    async fn get_webhook_events(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        let events = self.db.get_webhook_events(app_name, 50)?;
        let response = ApiResponse::ok(events);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    async fn handle_incoming_webhook(
        self: Arc<Self>,
        app_name: &str,
        req: Request<hyper::body::Incoming>,
    ) -> Result<Response<Full<Bytes>>> {
        // Get webhook config from database
        let webhook_config = match self.db.get_webhook(app_name)? {
            Some(w) => w,
            None => {
                warn!(app = %app_name, "Webhook received for app without webhook config");
                return Ok(json_error(StatusCode::NOT_FOUND, "Webhook not configured for this app"));
            }
        };

        // Extract headers for signature verification
        let signature = req.headers()
            .get("X-Hub-Signature-256")
            .or_else(|| req.headers().get("X-Hub-Signature"))
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        let gitlab_token = req.headers()
            .get("X-Gitlab-Token")
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        let event_type = req.headers()
            .get("X-GitHub-Event")
            .or_else(|| req.headers().get("X-Gitlab-Event"))
            .and_then(|v| v.to_str().ok())
            .unwrap_or("push")
            .to_string();

        // Read body
        let body = req.collect().await?.to_bytes();
        let payload = body.to_vec();

        // Register config with handler for verification
        let config = WebhookConfig {
            secret: webhook_config.secret.clone(),
            provider: webhook_config.provider.parse().unwrap_or(WebhookProvider::GitHub),
            deploy_branch: webhook_config.deploy_branch.clone(),
            auto_deploy: webhook_config.auto_deploy,
            status_token: webhook_config.status_token.clone(),
            repo_name: webhook_config.repo_name.clone(),
        };

        let mut handler = self.webhook_handler.write().await;
        handler.register(app_name, config.clone());

        // Verify signature
        let provider: WebhookProvider = webhook_config.provider.parse().unwrap_or(WebhookProvider::GitHub);

        let verified = match provider {
            WebhookProvider::GitHub => {
                if let Some(sig) = &signature {
                    handler.verify_github_signature(app_name, &payload, sig)
                } else {
                    warn!(app = %app_name, "GitHub webhook missing signature");
                    false
                }
            }
            WebhookProvider::GitLab => {
                if let Some(token) = &gitlab_token {
                    handler.verify_gitlab_token(app_name, token)
                } else {
                    warn!(app = %app_name, "GitLab webhook missing token");
                    false
                }
            }
            _ => true, // Generic webhooks don't verify
        };

        if !verified {
            return Ok(json_error(StatusCode::UNAUTHORIZED, "Invalid webhook signature"));
        }

        // Parse the event
        let event = match provider {
            WebhookProvider::GitHub => handler.parse_github_push(app_name, &payload),
            WebhookProvider::GitLab => handler.parse_gitlab_push(app_name, &payload),
            _ => handler.parse_generic_push(app_name, &payload),
        };

        drop(handler); // Release the lock

        let event = match event {
            Ok(e) => e,
            Err(e) => {
                warn!(app = %app_name, error = %e, "Failed to parse webhook payload");
                return Ok(json_error(StatusCode::BAD_REQUEST, format!("Invalid payload: {}", e)));
            }
        };

        info!(
            app = %app_name,
            branch = %event.branch,
            commit = %event.commit_sha,
            should_deploy = event.should_deploy,
            "Webhook received"
        );

        // Log the event
        let event_record = WebhookEventRecord {
            id: None,
            app_name: app_name.to_string(),
            event_type: event_type.clone(),
            provider: provider.to_string(),
            branch: Some(event.branch.clone()),
            commit_sha: Some(event.commit_sha.clone()),
            commit_message: event.commit_message.clone(),
            author: event.author.clone(),
            payload: Some(String::from_utf8_lossy(&payload).to_string()),
            triggered_deploy: event.should_deploy,
            deployment_id: None,
            created_at: None,
        };
        self.db.log_webhook_event(&event_record)?;

        // Trigger deploy if configured
        if event.should_deploy {
            // Update build status to pending
            self.db.update_build_status(app_name, "pending", Some(&event.commit_sha))?;

            // Send pending status to GitHub/GitLab
            if let Some(token) = &config.status_token {
                if let Some(repo) = &config.repo_name {
                    let _ = self.status_notifier.notify_github(
                        token,
                        repo,
                        &event.commit_sha,
                        DeployStatus::Pending,
                        "Deployment queued",
                        None,
                    ).await;
                }
            }

            info!(app = %app_name, commit = %event.commit_sha, "Triggering deploy from webhook");

            // Trigger the build asynchronously
            let self_clone = Arc::clone(&self);
            let app_name_owned = app_name.to_string();
            let commit_sha = event.commit_sha.clone();

            tokio::spawn(async move {
                // Update status to building
                let _ = self_clone.db.update_build_status(&app_name_owned, "building", Some(&commit_sha));

                // Trigger build
                match self_clone.trigger_build_internal(&app_name_owned).await {
                    Ok(_) => {
                        let _ = self_clone.db.update_build_status(&app_name_owned, "success", Some(&commit_sha));
                        info!(app = %app_name_owned, "Webhook deploy succeeded");
                    }
                    Err(e) => {
                        let _ = self_clone.db.update_build_status(&app_name_owned, "failure", Some(&commit_sha));
                        error!(app = %app_name_owned, error = %e, "Webhook deploy failed");
                    }
                }
            });

            let result = serde_json::json!({
                "status": "accepted",
                "message": "Deployment triggered",
                "branch": event.branch,
                "commit": event.commit_sha,
            });
            let response = ApiResponse::ok(result);
            Ok(json_response(StatusCode::ACCEPTED, serde_json::to_string(&response)?))
        } else {
            let result = serde_json::json!({
                "status": "ignored",
                "message": format!("Branch '{}' is not the deploy branch ('{}')", event.branch, config.deploy_branch),
                "branch": event.branch,
                "commit": event.commit_sha,
            });
            let response = ApiResponse::ok(result);
            Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
        }
    }

    async fn trigger_build_internal(&self, app_name: &str) -> Result<()> {
        // Similar to trigger_build but returns Result instead of Response
        let _app = self.db.get_app(app_name)?
            .ok_or_else(|| anyhow::anyhow!("App not found"))?;

        let repo = self.git_server.get_app(app_name).await
            .ok_or_else(|| anyhow::anyhow!("Git repo not found"))?;

        let build_config = BuildConfig {
            app_name: app_name.to_string(),
            source_path: repo.repo_path.clone(),
            build_mode: BuildMode::Auto,
            dockerfile: None,
            target: None,
            build_args: HashMap::new(),
            builder: "paketobuildpacks/builder:base".to_string(),
            buildpacks: vec![],
            build_env: self.db.get_all_config(app_name)?,
            registry: None,
            tag: "latest".to_string(),
            clear_cache: false,
            platform: None,
            image: None,
        };

        let result = self.builder.build(&build_config).await?;

        if result.success {
            // Update app with new image
            self.db.update_app_deployment(
                app_name,
                &result.image,
                repo.current_commit.as_deref(),
            )?;

            // Record deployment
            let deploy_id = uuid::Uuid::new_v4().to_string();
            let deployment = DeploymentRecord {
                id: deploy_id,
                app_name: app_name.to_string(),
                status: "success".to_string(),
                image: Some(result.image.clone()),
                commit_hash: repo.current_commit.clone(),
                build_logs: Some(result.logs.join("\n")),
                duration_secs: Some(result.duration_secs),
                created_at: String::new(), // Will be set by DB
                finished_at: None,
            };
            self.db.create_deployment(&deployment)?;

            // Restart the app if it's running
            let _ = self.instance_manager.rolling_restart(app_name, Some(&result.image)).await;
        } else {
            anyhow::bail!("Build failed: {}", result.error.unwrap_or_default());
        }

        Ok(())
    }

    async fn get_build_badge(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        let status = self.db.get_build_status(app_name)?
            .map(|s| match s.status.as_str() {
                "success" => DeployStatus::Success,
                "failure" | "failed" => DeployStatus::Failure,
                "pending" => DeployStatus::Pending,
                "building" => DeployStatus::Building,
                _ => DeployStatus::Error,
            })
            .unwrap_or(DeployStatus::Error);

        let svg = generate_badge_svg(status, app_name);

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "image/svg+xml")
            .header("Cache-Control", "no-cache")
            .body(Full::new(svg.into()))
            .expect("valid response"))
    }

    // ==================== Custom Domains ====================

    async fn list_domains(&self, app_name: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        let domains = self.db.get_app_domains(app_name)?;
        let response = ApiResponse::ok(domains);
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    async fn add_domain(self: Arc<Self>, app_name: &str, req: Request<hyper::body::Incoming>) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        let body = req.collect().await?.to_bytes();
        let add_req: AddDomainRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => {
                return Ok(json_error(StatusCode::BAD_REQUEST, format!("Invalid JSON: {}", e)));
            }
        };

        // Validate domain
        let domain = add_req.domain.trim().to_lowercase();
        if domain.is_empty() || !domain.contains('.') {
            return Ok(json_error(StatusCode::BAD_REQUEST, "Invalid domain format"));
        }

        // Check if domain already exists
        if self.db.get_domain(&domain)?.is_some() {
            return Ok(json_error(StatusCode::CONFLICT, "Domain already exists"));
        }

        // Generate verification token
        let verification_token = format!("spawngate-verify-{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("xxxx"));

        // Add to database
        self.db.add_domain(&domain, app_name, &verification_token)?;

        // Add to in-memory manager
        self.domain_manager.add_domain(&domain, app_name, &verification_token).await;

        info!(domain = %domain, app = %app_name, "Added custom domain");

        // Return instructions for DNS verification
        let response_data = serde_json::json!({
            "domain": domain,
            "app_name": app_name,
            "verified": false,
            "ssl_enabled": false,
            "verification_token": verification_token,
            "dns_instructions": format!(
                "Add a TXT record to your DNS:\n\n  Name: _spawngate.{}\n  Value: {}\n\nThen run: paas domains verify {} --app {}",
                domain, verification_token, domain, app_name
            )
        });

        let response = ApiResponse::ok(response_data);
        Ok(json_response(StatusCode::CREATED, serde_json::to_string(&response)?))
    }

    async fn remove_domain(&self, app_name: &str, domain: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        // Check if domain exists and belongs to this app
        match self.db.get_domain(domain)? {
            Some(d) if d.app_name == app_name => {}
            Some(_) => {
                return Ok(json_error(StatusCode::FORBIDDEN, "Domain belongs to another app"));
            }
            None => {
                return Ok(json_error(StatusCode::NOT_FOUND, "Domain not found"));
            }
        }

        // Remove from database
        self.db.delete_domain(domain)?;

        // Remove from in-memory manager
        let _ = self.domain_manager.remove_domain(domain).await;

        info!(domain = %domain, app = %app_name, "Removed custom domain");

        let response: ApiResponse<()> = ApiResponse::ok(());
        Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
    }

    async fn verify_domain(&self, app_name: &str, domain: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        // Get domain record
        let domain_record = match self.db.get_domain(domain)? {
            Some(d) if d.app_name == app_name => d,
            Some(_) => {
                return Ok(json_error(StatusCode::FORBIDDEN, "Domain belongs to another app"));
            }
            None => {
                return Ok(json_error(StatusCode::NOT_FOUND, "Domain not found"));
            }
        };

        if domain_record.verified {
            return Ok(json_error(StatusCode::BAD_REQUEST, "Domain is already verified"));
        }

        // Verify DNS
        let expected_token = domain_record.verification_token.unwrap_or_default();
        match self.dns_verifier.verify(domain, &expected_token).await {
            Ok(true) => {
                // Update database
                self.db.update_domain_verification(domain, true)?;

                // Update in-memory manager
                let _ = self.domain_manager.set_verified(domain, true).await;

                info!(domain = %domain, "Domain verified successfully");

                let response = ApiResponse::ok(serde_json::json!({
                    "domain": domain,
                    "verified": true,
                    "message": "Domain verified! You can now enable SSL."
                }));
                Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
            }
            Ok(false) => {
                warn!(domain = %domain, "DNS verification failed");
                Ok(json_error(StatusCode::BAD_REQUEST, format!(
                    "DNS verification failed. Please add a TXT record:\n  Name: _spawngate.{}\n  Value: {}",
                    domain, expected_token
                )))
            }
            Err(e) => {
                error!(domain = %domain, error = %e, "DNS lookup error");
                Ok(json_error(StatusCode::INTERNAL_SERVER_ERROR, format!("DNS lookup error: {}", e)))
            }
        }
    }

    async fn enable_domain_ssl(&self, app_name: &str, domain: &str) -> Result<Response<Full<Bytes>>> {
        // Check if app exists
        if self.db.get_app(app_name)?.is_none() {
            return Ok(json_error(StatusCode::NOT_FOUND, "App not found"));
        }

        // Get domain record
        let domain_record = match self.db.get_domain(domain)? {
            Some(d) if d.app_name == app_name => d,
            Some(_) => {
                return Ok(json_error(StatusCode::FORBIDDEN, "Domain belongs to another app"));
            }
            None => {
                return Ok(json_error(StatusCode::NOT_FOUND, "Domain not found"));
            }
        };

        if !domain_record.verified {
            return Ok(json_error(StatusCode::BAD_REQUEST, "Domain must be verified before enabling SSL"));
        }

        if domain_record.ssl_enabled {
            return Ok(json_error(StatusCode::BAD_REQUEST, "SSL is already enabled for this domain"));
        }

        // Provision SSL certificate
        match self.ssl_manager.provision_certificate(domain).await {
            Ok(cert_info) => {
                // Update database
                self.db.update_domain_ssl(
                    domain,
                    true,
                    Some(&cert_info.cert_path),
                    Some(&cert_info.key_path),
                    Some(&cert_info.expires_at),
                )?;

                // Update in-memory manager
                let _ = self.domain_manager.set_ssl_enabled(domain, true, Some(cert_info.expires_at.clone())).await;

                info!(domain = %domain, "SSL enabled successfully");

                let response = ApiResponse::ok(serde_json::json!({
                    "domain": domain,
                    "ssl_enabled": true,
                    "cert_expires_at": cert_info.expires_at,
                    "message": "SSL enabled! Your domain is now accessible via HTTPS."
                }));
                Ok(json_response(StatusCode::OK, serde_json::to_string(&response)?))
            }
            Err(e) => {
                error!(domain = %domain, error = %e, "Failed to provision SSL certificate");
                Ok(json_error(StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to provision SSL: {}", e)))
            }
        }
    }

    // ==================== Dashboard Authentication ====================

    async fn handle_dashboard_login(&self, req: Request<hyper::body::Incoming>) -> Response<Full<Bytes>> {
        let body = match req.collect().await {
            Ok(b) => b.to_bytes(),
            Err(_) => return json_error(StatusCode::BAD_REQUEST, "Failed to read request body"),
        };

        // Parse form data (token=xxx)
        let body_str = String::from_utf8_lossy(&body);
        let mut token_value = None;

        for pair in body_str.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                if key == "token" {
                    token_value = Some(urlencoding::decode(value).unwrap_or_default().to_string());
                }
            }
        }

        let provided_token = match token_value {
            Some(t) => t,
            None => return self.login_error_response("Token is required"),
        };

        // Verify the provided token matches the configured auth token
        if provided_token != self.config.auth_token {
            warn!("Dashboard login failed: invalid token");
            return self.login_error_response("Invalid token");
        }

        // Create JWT session token
        let jwt = match self.auth_manager.create_token("dashboard", "admin") {
            Ok(t) => t,
            Err(e) => {
                error!(error = %e, "Failed to create JWT token");
                return self.login_error_response("Authentication failed");
            }
        };

        // Create session cookie
        let cookie = self.auth_manager.create_session_cookie(&jwt);

        info!("Dashboard login successful");

        // Redirect to dashboard with session cookie
        Response::builder()
            .status(StatusCode::SEE_OTHER)
            .header("Location", "/dashboard")
            .header(SET_COOKIE, cookie)
            .body(Full::new(Bytes::new()))
            .expect("valid response")
    }

    fn login_error_response(&self, message: &str) -> Response<Full<Bytes>> {
        let html = format!(
            r##"<!DOCTYPE html>
<html><head><meta charset="UTF-8"><title>Login Error</title>
<link rel="stylesheet" href="/dashboard/style.css">
</head><body class="login-page dark">
<div class="login-container"><div class="login-card">
<div class="login-header"><h1>Login Failed</h1><p class="error">{}</p></div>
<a href="/dashboard/login" class="btn btn-primary btn-block">Try Again</a>
</div></div></body></html>"##,
            message
        );
        html_response(StatusCode::UNAUTHORIZED, html)
    }

    fn handle_dashboard_logout(&self) -> Response<Full<Bytes>> {
        let cookie = self.auth_manager.create_logout_cookie();
        info!("Dashboard logout");

        Response::builder()
            .status(StatusCode::SEE_OTHER)
            .header("Location", "/dashboard/login")
            .header(SET_COOKIE, cookie)
            .body(Full::new(Bytes::new()))
            .expect("valid response")
    }

    async fn handle_dashboard_api(
        &self,
        path: &str,
        method: &Method,
        req: Request<hyper::body::Incoming>,
    ) -> Response<Full<Bytes>> {
        // Dashboard HTMX API endpoints
        match (method, path) {
            // Get apps list (HTMX partial)
            (&Method::GET, "/dashboard/apps") => {
                match self.db.list_apps() {
                    Ok(apps) => {
                        let apps_json: Vec<serde_json::Value> = apps.iter().map(|app| {
                            serde_json::json!({
                                "name": app.name,
                                "status": app.status,
                                "port": app.port,
                            })
                        }).collect();
                        let html = dashboard::render_apps_list(&apps_json);
                        html_response(StatusCode::OK, html)
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to list apps for dashboard");
                        html_response(StatusCode::INTERNAL_SERVER_ERROR,
                            r##"<div class="error">Failed to load apps</div>"##)
                    }
                }
            }

            // Get single app detail (HTMX partial)
            (&Method::GET, p) if p.starts_with("/dashboard/apps/") && !p.contains("/config") && !p.contains("/domains") && !p.contains("/addons") && !p.contains("/deployments") => {
                let app_name = p.strip_prefix("/dashboard/apps/").unwrap_or("");
                match self.db.get_app(app_name) {
                    Ok(Some(app)) => {
                        let config = self.db.get_all_config(app_name).unwrap_or_default();
                        let addons = self.db.get_app_addons(app_name).unwrap_or_default();
                        let app_json = serde_json::json!({
                            "name": app.name,
                            "status": app.status,
                            "port": app.port,
                            "git_url": app.git_url,
                            "image": app.image,
                            "env": config,
                            "addons": addons.iter().map(|a| a.addon_type.clone()).collect::<Vec<_>>(),
                        });
                        let html = dashboard::render_app_detail(&app_json);
                        html_response(StatusCode::OK, html)
                    }
                    Ok(None) => {
                        html_response(StatusCode::NOT_FOUND,
                            r##"<div class="error">App not found</div>"##)
                    }
                    Err(e) => {
                        error!(error = %e, app = %app_name, "Failed to get app for dashboard");
                        html_response(StatusCode::INTERNAL_SERVER_ERROR,
                            r##"<div class="error">Failed to load app</div>"##)
                    }
                }
            }

            // Get config vars (HTMX partial)
            (&Method::GET, p) if p.ends_with("/config") => {
                let app_name = p.strip_prefix("/dashboard/apps/")
                    .and_then(|p| p.strip_suffix("/config"))
                    .unwrap_or("");
                match self.db.get_all_config(app_name) {
                    Ok(config) => {
                        let config_json = serde_json::to_value(&config).unwrap_or_default();
                        let html = dashboard::render_config_vars(&config_json);
                        html_response(StatusCode::OK, html)
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to get config");
                        html_response(StatusCode::INTERNAL_SERVER_ERROR,
                            r##"<div class="error">Failed to load config</div>"##)
                    }
                }
            }

            // Get domains (HTMX partial)
            (&Method::GET, p) if p.ends_with("/domains") => {
                let app_name = p.strip_prefix("/dashboard/apps/")
                    .and_then(|p| p.strip_suffix("/domains"))
                    .unwrap_or("");
                match self.db.get_app_domains(app_name) {
                    Ok(domains) => {
                        let domains_json: Vec<serde_json::Value> = domains.iter().map(|d| {
                            serde_json::json!({
                                "hostname": d.domain,
                                "dns_verified": d.verified,
                                "ssl_status": if d.ssl_enabled { "active" } else { "pending" },
                            })
                        }).collect();
                        let html = dashboard::render_domains_list(&domains_json);
                        html_response(StatusCode::OK, html)
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to get domains");
                        html_response(StatusCode::INTERNAL_SERVER_ERROR,
                            r##"<div class="error">Failed to load domains</div>"##)
                    }
                }
            }

            // Get addons (HTMX partial)
            (&Method::GET, p) if p.ends_with("/addons") => {
                let app_name = p.strip_prefix("/dashboard/apps/")
                    .and_then(|p| p.strip_suffix("/addons"))
                    .unwrap_or("");
                match self.db.get_app_addons(app_name) {
                    Ok(addons) => {
                        let addons_json: Vec<serde_json::Value> = addons.iter().map(|a| {
                            serde_json::json!({
                                "addon_type": a.addon_type,
                                "plan": a.plan,
                                "status": a.status,
                            })
                        }).collect();
                        let html = dashboard::render_addons_list(&addons_json);
                        html_response(StatusCode::OK, html)
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to get addons");
                        html_response(StatusCode::INTERNAL_SERVER_ERROR,
                            r##"<div class="error">Failed to load addons</div>"##)
                    }
                }
            }

            // Get deployments (HTMX partial)
            (&Method::GET, p) if p.ends_with("/deployments") => {
                let app_name = p.strip_prefix("/dashboard/apps/")
                    .and_then(|p| p.strip_suffix("/deployments"))
                    .unwrap_or("");
                match self.db.get_deployments(app_name, 10) {
                    Ok(deploys) => {
                        let deploys_json: Vec<serde_json::Value> = deploys.iter().map(|d| {
                            serde_json::json!({
                                "id": d.id,
                                "status": d.status,
                                "image": d.image,
                                "commit_hash": d.commit_hash,
                                "duration_secs": d.duration_secs,
                                "created_at": d.created_at,
                            })
                        }).collect();
                        let html = dashboard::render_deployments_list(&deploys_json);
                        html_response(StatusCode::OK, html)
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to get deployments");
                        html_response(StatusCode::INTERNAL_SERVER_ERROR,
                            r##"<div class="error">Failed to load deployments</div>"##)
                    }
                }
            }

            // Get instances (HTMX partial)
            (&Method::GET, p) if p.ends_with("/instances") => {
                let app_name = p.strip_prefix("/dashboard/apps/")
                    .and_then(|p| p.strip_suffix("/instances"))
                    .unwrap_or("");

                // Get app info for scale settings
                let app = self.db.get_app(app_name).ok().flatten();
                let (scale, min_scale, max_scale) = app.as_ref()
                    .map(|a| (a.scale as i64, a.min_scale as i64, a.max_scale as i64))
                    .unwrap_or((1, 0, 10));

                match self.db.get_app_processes(app_name) {
                    Ok(processes) => {
                        let instances_json: Vec<serde_json::Value> = processes.iter()
                            .filter(|p| p.status != "stopped")
                            .map(|p| {
                                serde_json::json!({
                                    "id": p.id,
                                    "process_type": p.process_type,
                                    "status": p.status,
                                    "health_status": p.health_status,
                                    "port": p.port,
                                    "started_at": p.started_at,
                                })
                            }).collect();
                        let html = dashboard::render_instances_list(app_name, &instances_json, scale, min_scale, max_scale);
                        html_response(StatusCode::OK, html)
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to get instances");
                        html_response(StatusCode::INTERNAL_SERVER_ERROR,
                            r##"<div class="error">Failed to load instances</div>"##)
                    }
                }
            }

            _ => {
                html_response(StatusCode::NOT_FOUND,
                    r##"<div class="error">Not found</div>"##)
            }
        }
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

fn redirect_response(location: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header("Location", location)
        .body(Full::new(Bytes::new()))
        .expect("valid response")
}

fn html_response(status: StatusCode, body: impl Into<Bytes>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Full::new(body.into()))
        .expect("valid response")
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
