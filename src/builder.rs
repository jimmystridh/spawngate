//! Buildpack builder service for building container images from source code
//!
//! This module provides integration with Cloud Native Buildpacks to automatically
//! build container images from application source code.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, error, info, warn};

/// Default builder image (Paketo buildpacks)
pub const DEFAULT_BUILDER: &str = "paketobuildpacks/builder-jammy-base";

/// Build configuration for an application
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    /// Application name (used as image name)
    pub app_name: String,

    /// Path to source directory
    pub source_path: PathBuf,

    /// Builder image to use (default: Paketo)
    #[serde(default = "default_builder")]
    pub builder: String,

    /// Specific buildpacks to use (optional, auto-detected if empty)
    #[serde(default)]
    pub buildpacks: Vec<String>,

    /// Environment variables to set during build
    #[serde(default)]
    pub build_env: HashMap<String, String>,

    /// Target registry (e.g., "localhost:5000" or empty for local Docker)
    pub registry: Option<String>,

    /// Clear cache before building
    #[serde(default)]
    pub clear_cache: bool,
}

fn default_builder() -> String {
    DEFAULT_BUILDER.to_string()
}

/// Result of a build operation
#[derive(Debug, Clone, Serialize)]
pub struct BuildResult {
    /// Whether the build succeeded
    pub success: bool,

    /// Full image name with tag
    pub image: String,

    /// Build duration in seconds
    pub duration_secs: f64,

    /// Build logs
    pub logs: Vec<String>,

    /// Error message if build failed
    pub error: Option<String>,
}

/// Builder service that uses Cloud Native Buildpacks
pub struct Builder {
    /// Default builder image
    default_builder: String,

    /// Default registry for images
    default_registry: Option<String>,

    /// Path to pack CLI binary
    pack_path: String,
}

impl Builder {
    /// Create a new builder instance
    pub async fn new(
        default_builder: Option<&str>,
        default_registry: Option<&str>,
    ) -> Result<Self> {
        // Find pack CLI
        let pack_path = Self::find_pack_cli().await?;

        Ok(Self {
            default_builder: default_builder.unwrap_or(DEFAULT_BUILDER).to_string(),
            default_registry: default_registry.map(String::from),
            pack_path,
        })
    }

    /// Find the pack CLI binary
    async fn find_pack_cli() -> Result<String> {
        // Check common locations
        let paths = vec![
            "pack",
            "/usr/local/bin/pack",
            "/opt/homebrew/bin/pack",
            "./pack",
        ];

        for path in paths {
            if let Ok(output) = Command::new(path)
                .arg("version")
                .output()
                .await
            {
                if output.status.success() {
                    let version = String::from_utf8_lossy(&output.stdout);
                    info!("Found pack CLI at {}: {}", path, version.trim());
                    return Ok(path.to_string());
                }
            }
        }

        // Try which command
        if let Ok(output) = Command::new("which").arg("pack").output().await {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return Ok(path);
                }
            }
        }

        anyhow::bail!(
            "pack CLI not found. Install it with:\n\
             - macOS: brew install buildpacks/tap/pack\n\
             - Linux: curl -sSL 'https://github.com/buildpacks/pack/releases/download/v0.35.0/pack-v0.35.0-linux.tgz' | tar -xzf - -C /usr/local/bin\n\
             - Windows: scoop install pack"
        )
    }

    /// Build an application from source
    pub async fn build(&self, config: &BuildConfig) -> Result<BuildResult> {
        let start = std::time::Instant::now();
        let mut logs = Vec::new();

        // Validate source path exists
        if !config.source_path.exists() {
            return Ok(BuildResult {
                success: false,
                image: String::new(),
                duration_secs: start.elapsed().as_secs_f64(),
                logs,
                error: Some(format!(
                    "Source path does not exist: {}",
                    config.source_path.display()
                )),
            });
        }

        // Determine image name
        let registry = config.registry.as_ref().or(self.default_registry.as_ref());
        let image_name = if let Some(reg) = registry {
            format!("{}/{}:latest", reg, config.app_name)
        } else {
            format!("{}:latest", config.app_name)
        };

        info!(
            app = %config.app_name,
            source = %config.source_path.display(),
            builder = %config.builder,
            image = %image_name,
            "Starting build"
        );

        // Build the pack command
        let mut cmd = Command::new(&self.pack_path);
        cmd.arg("build")
            .arg(&image_name)
            .arg("--path")
            .arg(&config.source_path)
            .arg("--builder")
            .arg(if config.builder.is_empty() {
                &self.default_builder
            } else {
                &config.builder
            });

        // Add buildpacks if specified
        for bp in &config.buildpacks {
            cmd.arg("--buildpack").arg(bp);
        }

        // Add build environment variables
        for (key, value) in &config.build_env {
            cmd.arg("--env").arg(format!("{}={}", key, value));
        }

        // Clear cache if requested
        if config.clear_cache {
            cmd.arg("--clear-cache");
        }

        // Trust the builder
        cmd.arg("--trust-builder");

        // Set up for streaming output
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        debug!("Running: {:?}", cmd);

        // Spawn the process
        let mut child = cmd.spawn().context("Failed to spawn pack CLI")?;

        // Stream stdout
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        // Collect output
        loop {
            tokio::select! {
                line = stdout_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            info!(target: "builder", "{}", line);
                            logs.push(line);
                        }
                        Ok(None) => break,
                        Err(e) => {
                            warn!("Error reading stdout: {}", e);
                            break;
                        }
                    }
                }
                line = stderr_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            // Pack outputs to stderr too
                            info!(target: "builder", "{}", line);
                            logs.push(line);
                        }
                        Ok(None) => {}
                        Err(e) => {
                            warn!("Error reading stderr: {}", e);
                        }
                    }
                }
            }
        }

        // Wait for process to complete
        let status = child.wait().await.context("Failed to wait for pack CLI")?;

        let duration = start.elapsed().as_secs_f64();

        if status.success() {
            info!(
                app = %config.app_name,
                image = %image_name,
                duration_secs = %duration,
                "Build completed successfully"
            );

            Ok(BuildResult {
                success: true,
                image: image_name,
                duration_secs: duration,
                logs,
                error: None,
            })
        } else {
            let error_msg = format!(
                "Build failed with exit code: {}",
                status.code().unwrap_or(-1)
            );
            error!(
                app = %config.app_name,
                error = %error_msg,
                "Build failed"
            );

            Ok(BuildResult {
                success: false,
                image: image_name,
                duration_secs: duration,
                logs,
                error: Some(error_msg),
            })
        }
    }

    /// Build and push to registry
    pub async fn build_and_push(&self, config: &BuildConfig) -> Result<BuildResult> {
        let mut result = self.build(config).await?;

        if result.success && config.registry.is_some() {
            // Push to registry
            info!(image = %result.image, "Pushing image to registry");

            let mut cmd = Command::new("docker");
            cmd.arg("push").arg(&result.image);

            let output = cmd.output().await.context("Failed to push image")?;

            if !output.status.success() {
                let error = String::from_utf8_lossy(&output.stderr);
                result.success = false;
                result.error = Some(format!("Failed to push image: {}", error));
            } else {
                result.logs.push(format!("Pushed image: {}", result.image));
            }
        }

        Ok(result)
    }

    /// Detect the application type from source
    pub async fn detect_app_type(source_path: &Path) -> Option<AppType> {
        // Check for common files
        if source_path.join("package.json").exists() {
            return Some(AppType::NodeJs);
        }
        if source_path.join("requirements.txt").exists()
            || source_path.join("Pipfile").exists()
            || source_path.join("pyproject.toml").exists()
        {
            return Some(AppType::Python);
        }
        if source_path.join("Gemfile").exists() {
            return Some(AppType::Ruby);
        }
        if source_path.join("go.mod").exists() {
            return Some(AppType::Go);
        }
        if source_path.join("Cargo.toml").exists() {
            return Some(AppType::Rust);
        }
        if source_path.join("pom.xml").exists() || source_path.join("build.gradle").exists() {
            return Some(AppType::Java);
        }
        if source_path.join("*.csproj").exists() || source_path.join("*.fsproj").exists() {
            return Some(AppType::DotNet);
        }
        if source_path.join("mix.exs").exists() {
            return Some(AppType::Elixir);
        }
        if source_path.join("composer.json").exists() {
            return Some(AppType::Php);
        }

        None
    }

    /// Get recommended buildpacks for an app type
    pub fn recommended_buildpacks(app_type: &AppType) -> Vec<&'static str> {
        match app_type {
            AppType::NodeJs => vec!["paketo-buildpacks/nodejs"],
            AppType::Python => vec!["paketo-buildpacks/python"],
            AppType::Ruby => vec!["paketo-buildpacks/ruby"],
            AppType::Go => vec!["paketo-buildpacks/go"],
            AppType::Rust => vec!["paketo-community/rust"],
            AppType::Java => vec!["paketo-buildpacks/java"],
            AppType::DotNet => vec!["paketo-buildpacks/dotnet-core"],
            AppType::Elixir => vec!["paketo-buildpacks/elixir"],
            AppType::Php => vec!["paketo-buildpacks/php"],
            AppType::Static => vec!["paketo-buildpacks/nginx"],
        }
    }
}

/// Detected application type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppType {
    NodeJs,
    Python,
    Ruby,
    Go,
    Rust,
    Java,
    DotNet,
    Elixir,
    Php,
    Static,
}

impl std::fmt::Display for AppType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppType::NodeJs => write!(f, "Node.js"),
            AppType::Python => write!(f, "Python"),
            AppType::Ruby => write!(f, "Ruby"),
            AppType::Go => write!(f, "Go"),
            AppType::Rust => write!(f, "Rust"),
            AppType::Java => write!(f, "Java"),
            AppType::DotNet => write!(f, ".NET"),
            AppType::Elixir => write!(f, "Elixir"),
            AppType::Php => write!(f, "PHP"),
            AppType::Static => write!(f, "Static"),
        }
    }
}

/// Local Docker registry for development
pub struct LocalRegistry {
    /// Container name
    container_name: String,
    /// Registry port
    #[allow(dead_code)]
    port: u16,
    /// Registry URL
    pub url: String,
}

impl LocalRegistry {
    /// Start a local Docker registry
    pub async fn start(port: u16) -> Result<Self> {
        let container_name = "spawngate-registry".to_string();
        let url = format!("localhost:{}", port);

        // Check if already running
        let output = Command::new("docker")
            .args(["ps", "-q", "-f", &format!("name={}", container_name)])
            .output()
            .await?;

        if !output.stdout.is_empty() {
            info!("Local registry already running at {}", url);
            return Ok(Self {
                container_name,
                port,
                url,
            });
        }

        // Remove any stopped container
        let _ = Command::new("docker")
            .args(["rm", "-f", &container_name])
            .output()
            .await;

        // Start registry
        info!("Starting local Docker registry at {}", url);
        let status = Command::new("docker")
            .args([
                "run",
                "-d",
                "--restart=always",
                "-p",
                &format!("{}:5000", port),
                "--name",
                &container_name,
                "registry:2",
            ])
            .status()
            .await
            .context("Failed to start local registry")?;

        if !status.success() {
            anyhow::bail!("Failed to start local Docker registry");
        }

        // Wait for registry to be ready
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        Ok(Self {
            container_name,
            port,
            url,
        })
    }

    /// Stop the local registry
    pub async fn stop(&self) -> Result<()> {
        info!("Stopping local registry");
        Command::new("docker")
            .args(["rm", "-f", &self.container_name])
            .status()
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_builder() {
        assert_eq!(default_builder(), DEFAULT_BUILDER);
    }

    #[tokio::test]
    async fn test_detect_app_type() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();

        // Test Node.js detection
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert_eq!(
            Builder::detect_app_type(dir.path()).await,
            Some(AppType::NodeJs)
        );
        fs::remove_file(dir.path().join("package.json")).unwrap();

        // Test Python detection
        fs::write(dir.path().join("requirements.txt"), "").unwrap();
        assert_eq!(
            Builder::detect_app_type(dir.path()).await,
            Some(AppType::Python)
        );
        fs::remove_file(dir.path().join("requirements.txt")).unwrap();

        // Test Go detection
        fs::write(dir.path().join("go.mod"), "module test").unwrap();
        assert_eq!(
            Builder::detect_app_type(dir.path()).await,
            Some(AppType::Go)
        );
    }

    #[test]
    fn test_recommended_buildpacks() {
        assert_eq!(
            Builder::recommended_buildpacks(&AppType::NodeJs),
            vec!["paketo-buildpacks/nodejs"]
        );
        assert_eq!(
            Builder::recommended_buildpacks(&AppType::Python),
            vec!["paketo-buildpacks/python"]
        );
        assert_eq!(
            Builder::recommended_buildpacks(&AppType::Rust),
            vec!["paketo-community/rust"]
        );
    }
}
