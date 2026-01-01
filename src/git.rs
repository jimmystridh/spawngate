//! Git server for push-to-deploy workflow
//!
//! This module provides a simple git server that receives pushes and triggers builds.

use crate::builder::{BuildConfig, Builder, BuildResult};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Git repository for an application
#[derive(Debug, Clone)]
pub struct GitRepo {
    /// Application name
    pub app_name: String,
    /// Path to bare git repository
    pub repo_path: PathBuf,
    /// Path to working directory (checked out code)
    pub work_path: PathBuf,
    /// Current deployed commit
    pub current_commit: Option<String>,
}

/// Git server configuration
#[derive(Debug, Clone)]
pub struct GitServerConfig {
    /// Base directory for git repositories
    pub repos_dir: PathBuf,
    /// Base directory for working copies
    pub work_dir: PathBuf,
    /// Port for SSH git server (if enabled)
    pub ssh_port: Option<u16>,
    /// Port for HTTP git server
    pub http_port: u16,
    /// Registry URL for built images
    pub registry: Option<String>,
}

impl Default for GitServerConfig {
    fn default() -> Self {
        Self {
            repos_dir: PathBuf::from("./repos"),
            work_dir: PathBuf::from("./work"),
            ssh_port: Some(2222),
            http_port: 3000,
            registry: None,
        }
    }
}

/// Callback for build events
pub type BuildCallback = Box<dyn Fn(&str, &BuildResult) + Send + Sync>;

/// Git server that receives pushes and triggers builds
pub struct GitServer {
    config: GitServerConfig,
    repos: Arc<RwLock<HashMap<String, GitRepo>>>,
    builder: Arc<Builder>,
    on_build_complete: Option<Arc<BuildCallback>>,
}

impl GitServer {
    /// Create a new git server
    pub async fn new(config: GitServerConfig) -> Result<Self> {
        // Create directories
        tokio::fs::create_dir_all(&config.repos_dir).await?;
        tokio::fs::create_dir_all(&config.work_dir).await?;

        let builder = Builder::new(None, config.registry.as_deref()).await?;

        Ok(Self {
            config,
            repos: Arc::new(RwLock::new(HashMap::new())),
            builder: Arc::new(builder),
            on_build_complete: None,
        })
    }

    /// Set callback for build completion
    pub fn on_build_complete<F>(&mut self, callback: F)
    where
        F: Fn(&str, &BuildResult) + Send + Sync + 'static,
    {
        self.on_build_complete = Some(Arc::new(Box::new(callback)));
    }

    /// Create a new app repository
    pub async fn create_app(&self, app_name: &str) -> Result<GitRepo> {
        let repo_path = self.config.repos_dir.join(format!("{}.git", app_name));
        let work_path = self.config.work_dir.join(app_name);

        // Check if already exists
        if repo_path.exists() {
            let repos = self.repos.read().await;
            if let Some(repo) = repos.get(app_name) {
                return Ok(repo.clone());
            }
        }

        info!(app = %app_name, path = %repo_path.display(), "Creating git repository");

        // Initialize bare repository
        let status = Command::new("git")
            .args(["init", "--bare"])
            .arg(&repo_path)
            .status()
            .await
            .context("Failed to initialize git repository")?;

        if !status.success() {
            anyhow::bail!("Failed to initialize git repository for {}", app_name);
        }

        // Create working directory
        tokio::fs::create_dir_all(&work_path).await?;

        // Set up post-receive hook
        let hook_path = repo_path.join("hooks/post-receive");
        let hook_script = format!(
            r#"#!/bin/bash
# Spawngate post-receive hook
APP_NAME="{}"
WORK_DIR="{}"
REPO_DIR="{}"

# Read the pushed refs
while read oldrev newrev refname; do
    if [ "$refname" = "refs/heads/main" ] || [ "$refname" = "refs/heads/master" ]; then
        echo "Deploying $APP_NAME..."

        # Checkout to working directory
        git --work-tree="$WORK_DIR" --git-dir="$REPO_DIR" checkout -f

        # Notify the build service
        if [ -n "$SPAWNGATE_BUILD_URL" ]; then
            curl -s -X POST "$SPAWNGATE_BUILD_URL/build/$APP_NAME" || true
        fi

        echo "Deployed $APP_NAME successfully!"
    fi
done
"#,
            app_name,
            work_path.display(),
            repo_path.display()
        );

        tokio::fs::write(&hook_path, hook_script).await?;

        // Make hook executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = tokio::fs::metadata(&hook_path).await?.permissions();
            perms.set_mode(0o755);
            tokio::fs::set_permissions(&hook_path, perms).await?;
        }

        let repo = GitRepo {
            app_name: app_name.to_string(),
            repo_path,
            work_path,
            current_commit: None,
        };

        // Store in registry
        {
            let mut repos = self.repos.write().await;
            repos.insert(app_name.to_string(), repo.clone());
        }

        Ok(repo)
    }

    /// Delete an app repository
    pub async fn delete_app(&self, app_name: &str) -> Result<()> {
        let repo_path = self.config.repos_dir.join(format!("{}.git", app_name));
        let work_path = self.config.work_dir.join(app_name);

        info!(app = %app_name, "Deleting git repository");

        // Remove from registry
        {
            let mut repos = self.repos.write().await;
            repos.remove(app_name);
        }

        // Delete directories
        if repo_path.exists() {
            tokio::fs::remove_dir_all(&repo_path).await?;
        }
        if work_path.exists() {
            tokio::fs::remove_dir_all(&work_path).await?;
        }

        Ok(())
    }

    /// Get repository info
    pub async fn get_app(&self, app_name: &str) -> Option<GitRepo> {
        let repos = self.repos.read().await;
        repos.get(app_name).cloned()
    }

    /// List all apps
    pub async fn list_apps(&self) -> Vec<GitRepo> {
        let repos = self.repos.read().await;
        repos.values().cloned().collect()
    }

    /// Trigger a build for an app
    pub async fn build_app(&self, app_name: &str) -> Result<BuildResult> {
        let repo = {
            let repos = self.repos.read().await;
            repos.get(app_name).cloned()
        };

        let repo = repo.ok_or_else(|| anyhow::anyhow!("App not found: {}", app_name))?;

        // Get current commit
        let commit = Self::get_head_commit(&repo.repo_path).await?;
        info!(app = %app_name, commit = %commit, "Building app");

        // Build
        let config = BuildConfig {
            app_name: app_name.to_string(),
            source_path: repo.work_path.clone(),
            builder: String::new(),
            buildpacks: vec![],
            build_env: HashMap::new(),
            registry: self.config.registry.clone(),
            clear_cache: false,
        };

        let result = self.builder.build(&config).await?;

        // Update commit
        if result.success {
            let mut repos = self.repos.write().await;
            if let Some(r) = repos.get_mut(app_name) {
                r.current_commit = Some(commit.clone());
            }
        }

        // Callback
        if let Some(callback) = &self.on_build_complete {
            callback(app_name, &result);
        }

        Ok(result)
    }

    /// Get HEAD commit for a repository
    async fn get_head_commit(repo_path: &Path) -> Result<String> {
        let output = Command::new("git")
            .args(["--git-dir", &repo_path.to_string_lossy(), "rev-parse", "HEAD"])
            .output()
            .await
            .context("Failed to get HEAD commit")?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Ok("unknown".to_string())
        }
    }

    /// Get git remote URL for an app
    pub fn get_remote_url(&self, app_name: &str) -> String {
        if let Some(ssh_port) = self.config.ssh_port {
            format!(
                "ssh://git@localhost:{}/{}.git",
                ssh_port, app_name
            )
        } else {
            format!(
                "http://localhost:{}/{}.git",
                self.config.http_port, app_name
            )
        }
    }

    /// Scan for existing repositories
    pub async fn scan_repos(&self) -> Result<()> {
        let mut entries = tokio::fs::read_dir(&self.config.repos_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() && path.extension().map_or(false, |e| e == "git") {
                let app_name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                let work_path = self.config.work_dir.join(&app_name);

                let repo = GitRepo {
                    app_name: app_name.clone(),
                    repo_path: path,
                    work_path,
                    current_commit: None,
                };

                let mut repos = self.repos.write().await;
                repos.insert(app_name.clone(), repo);
                info!(app = %app_name, "Found existing repository");
            }
        }

        Ok(())
    }
}

/// Simple HTTP git server for receiving pushes
pub struct HttpGitServer {
    git_server: Arc<GitServer>,
    port: u16,
}

impl HttpGitServer {
    pub fn new(git_server: Arc<GitServer>, port: u16) -> Self {
        Self { git_server, port }
    }

    /// Start the HTTP server
    pub async fn start(self: Arc<Self>) -> Result<()> {
        let listener = TcpListener::bind(format!("0.0.0.0:{}", self.port)).await?;
        info!(port = %self.port, "HTTP git server listening");

        loop {
            let (stream, addr) = listener.accept().await?;
            let server = Arc::clone(&self);

            tokio::spawn(async move {
                if let Err(e) = server.handle_connection(stream).await {
                    warn!(addr = %addr, error = %e, "Connection error");
                }
            });
        }
    }

    async fn handle_connection(&self, mut stream: TcpStream) -> Result<()> {
        let mut buf_reader = BufReader::new(&mut stream);
        let mut request_line = String::new();
        buf_reader.read_line(&mut request_line).await?;

        let parts: Vec<&str> = request_line.trim().split_whitespace().collect();
        if parts.len() < 2 {
            return Ok(());
        }

        let method = parts[0];
        let path = parts[1];

        debug!(method = %method, path = %path, "HTTP request");

        // Parse path: /{app}.git/...
        let path_parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();

        if path_parts.is_empty() {
            self.send_response(&mut stream, 404, "Not Found").await?;
            return Ok(());
        }

        let app_name = path_parts[0].trim_end_matches(".git");

        match method {
            "GET" if path.contains("/info/refs") => {
                self.handle_info_refs(&mut stream, app_name).await?;
            }
            "POST" if path.contains("/git-receive-pack") => {
                self.handle_receive_pack(&mut stream, app_name).await?;
            }
            "POST" if path.contains("/git-upload-pack") => {
                self.handle_upload_pack(&mut stream, app_name).await?;
            }
            _ => {
                self.send_response(&mut stream, 404, "Not Found").await?;
            }
        }

        Ok(())
    }

    async fn handle_info_refs(&self, stream: &mut TcpStream, app_name: &str) -> Result<()> {
        let repo = self.git_server.get_app(app_name).await;

        if repo.is_none() {
            self.send_response(stream, 404, "Repository not found").await?;
            return Ok(());
        }

        let repo = repo.unwrap();

        // Run git-upload-pack --advertise-refs
        let output = Command::new("git")
            .args(["upload-pack", "--advertise-refs", "."])
            .current_dir(&repo.repo_path)
            .output()
            .await?;

        let mut response = String::new();
        response.push_str("HTTP/1.1 200 OK\r\n");
        response.push_str("Content-Type: application/x-git-upload-pack-advertisement\r\n");
        response.push_str("\r\n");
        response.push_str("001e# service=git-upload-pack\n");
        response.push_str("0000");

        stream.write_all(response.as_bytes()).await?;
        stream.write_all(&output.stdout).await?;

        Ok(())
    }

    async fn handle_receive_pack(&self, stream: &mut TcpStream, app_name: &str) -> Result<()> {
        let repo = self.git_server.get_app(app_name).await;

        if repo.is_none() {
            self.send_response(stream, 404, "Repository not found").await?;
            return Ok(());
        }

        let _repo = repo.unwrap();

        // For simplicity, just acknowledge
        // In production, would need to handle the pack data properly
        let response = "HTTP/1.1 200 OK\r\nContent-Type: application/x-git-receive-pack-result\r\n\r\n0000";
        stream.write_all(response.as_bytes()).await?;

        // Trigger build
        let git_server = Arc::clone(&self.git_server);
        let app = app_name.to_string();
        tokio::spawn(async move {
            if let Err(e) = git_server.build_app(&app).await {
                error!(app = %app, error = %e, "Build failed");
            }
        });

        Ok(())
    }

    async fn handle_upload_pack(&self, stream: &mut TcpStream, _app_name: &str) -> Result<()> {
        self.send_response(stream, 200, "OK").await
    }

    async fn send_response(&self, stream: &mut TcpStream, status: u16, message: &str) -> Result<()> {
        let response = format!(
            "HTTP/1.1 {} {}\r\nContent-Length: 0\r\n\r\n",
            status, message
        );
        stream.write_all(response.as_bytes()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_git_server_config_default() {
        let config = GitServerConfig::default();
        assert_eq!(config.http_port, 3000);
        assert_eq!(config.ssh_port, Some(2222));
    }
}
