//! Buildpacks - Auto-detect language and generate Dockerfiles
//!
//! This module provides Heroku-style buildpack functionality without requiring
//! external tools like `pack`. It detects the application language from project
//! files and generates appropriate Dockerfiles.
//!
//! Supported languages:
//! - Node.js (package.json)
//! - Python (requirements.txt, Pipfile, pyproject.toml)
//! - Go (go.mod)
//! - Ruby (Gemfile)
//! - Rust (Cargo.toml)
//! - Static sites (index.html)

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, info};

/// Detected language/runtime for an application
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    NodeJs,
    Python,
    Go,
    Ruby,
    Rust,
    Static,
    Unknown,
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Language::NodeJs => write!(f, "Node.js"),
            Language::Python => write!(f, "Python"),
            Language::Go => write!(f, "Go"),
            Language::Ruby => write!(f, "Ruby"),
            Language::Rust => write!(f, "Rust"),
            Language::Static => write!(f, "Static"),
            Language::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Process type from Procfile (web, worker, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessType {
    pub name: String,
    pub command: String,
}

/// Detected application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppDetection {
    /// Detected language
    pub language: Language,
    /// Language version (if detected)
    pub version: Option<String>,
    /// Process types from Procfile
    pub processes: Vec<ProcessType>,
    /// Main entry point (e.g., "server.js", "app.py")
    pub entry_point: Option<String>,
    /// Package manager (npm, yarn, pip, etc.)
    pub package_manager: Option<String>,
    /// Build command (if detected)
    pub build_command: Option<String>,
    /// Start command (if detected)
    pub start_command: Option<String>,
    /// Detected port
    pub port: Option<u16>,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

impl Default for AppDetection {
    fn default() -> Self {
        Self {
            language: Language::Unknown,
            version: None,
            processes: Vec::new(),
            entry_point: None,
            package_manager: None,
            build_command: None,
            start_command: None,
            port: None,
            metadata: HashMap::new(),
        }
    }
}

/// Buildpack detector and Dockerfile generator
pub struct Buildpack;

impl Buildpack {
    /// Detect the application type from source directory
    pub fn detect(source_path: &Path) -> Result<AppDetection> {
        info!(path = %source_path.display(), "Detecting application type");

        let mut detection = AppDetection::default();

        // Parse Procfile first (highest priority for process types)
        if let Ok(processes) = Self::parse_procfile(source_path) {
            detection.processes = processes;
        }

        // Detect language based on files present
        if source_path.join("package.json").exists() {
            detection.language = Language::NodeJs;
            Self::detect_nodejs(source_path, &mut detection)?;
        } else if source_path.join("requirements.txt").exists()
            || source_path.join("Pipfile").exists()
            || source_path.join("pyproject.toml").exists()
            || source_path.join("setup.py").exists()
        {
            detection.language = Language::Python;
            Self::detect_python(source_path, &mut detection)?;
        } else if source_path.join("go.mod").exists() {
            detection.language = Language::Go;
            Self::detect_go(source_path, &mut detection)?;
        } else if source_path.join("Gemfile").exists() {
            detection.language = Language::Ruby;
            Self::detect_ruby(source_path, &mut detection)?;
        } else if source_path.join("Cargo.toml").exists() {
            detection.language = Language::Rust;
            Self::detect_rust(source_path, &mut detection)?;
        } else if source_path.join("index.html").exists() {
            detection.language = Language::Static;
            Self::detect_static(source_path, &mut detection)?;
        }

        // Procfile web process ALWAYS overrides detected start command
        if let Some(web) = detection.processes.iter().find(|p| p.name == "web") {
            detection.start_command = Some(web.command.clone());
        }

        info!(
            language = %detection.language,
            version = ?detection.version,
            entry_point = ?detection.entry_point,
            "Application detected"
        );

        Ok(detection)
    }

    /// Parse a Procfile and return process types
    pub fn parse_procfile(source_path: &Path) -> Result<Vec<ProcessType>> {
        let procfile_path = source_path.join("Procfile");
        if !procfile_path.exists() {
            return Ok(Vec::new());
        }

        let content = std::fs::read_to_string(&procfile_path)
            .context("Failed to read Procfile")?;

        let mut processes = Vec::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if let Some((name, command)) = line.split_once(':') {
                processes.push(ProcessType {
                    name: name.trim().to_string(),
                    command: command.trim().to_string(),
                });
            }
        }

        debug!(count = processes.len(), "Parsed Procfile");
        Ok(processes)
    }

    /// Detect Node.js application details
    fn detect_nodejs(source_path: &Path, detection: &mut AppDetection) -> Result<()> {
        let package_json_path = source_path.join("package.json");
        let content = std::fs::read_to_string(&package_json_path)
            .context("Failed to read package.json")?;

        let pkg: serde_json::Value = serde_json::from_str(&content)
            .context("Failed to parse package.json")?;

        // Detect package manager
        if source_path.join("yarn.lock").exists() {
            detection.package_manager = Some("yarn".to_string());
        } else if source_path.join("pnpm-lock.yaml").exists() {
            detection.package_manager = Some("pnpm".to_string());
        } else {
            detection.package_manager = Some("npm".to_string());
        }

        // Get Node.js version from engines
        if let Some(engines) = pkg.get("engines") {
            if let Some(node_version) = engines.get("node").and_then(|v| v.as_str()) {
                // Extract major version (e.g., ">=18.0.0" -> "18", "20.x" -> "20")
                let version = node_version
                    .trim_start_matches(|c: char| !c.is_ascii_digit())
                    .split(|c: char| !c.is_ascii_digit())
                    .next()
                    .unwrap_or("20");
                detection.version = Some(version.to_string());
            }
        }

        // Default to Node 20 LTS
        if detection.version.is_none() {
            detection.version = Some("20".to_string());
        }

        // Detect entry point
        if let Some(main) = pkg.get("main").and_then(|v| v.as_str()) {
            detection.entry_point = Some(main.to_string());
        }

        // Detect scripts
        if let Some(scripts) = pkg.get("scripts").and_then(|v| v.as_object()) {
            // Build command
            if scripts.contains_key("build") {
                let pm = detection.package_manager.as_deref().unwrap_or("npm");
                detection.build_command = Some(format!("{} run build", pm));
            }

            // Start command
            if let Some(start) = scripts.get("start").and_then(|v| v.as_str()) {
                detection.start_command = Some(start.to_string());
            } else if detection.entry_point.is_some() {
                detection.start_command = Some(format!(
                    "node {}",
                    detection.entry_point.as_ref().unwrap()
                ));
            }
        }

        // Check for common frameworks
        if let Some(deps) = pkg.get("dependencies").and_then(|v| v.as_object()) {
            if deps.contains_key("next") {
                detection.metadata.insert("framework".to_string(), "next".to_string());
                if detection.start_command.is_none() {
                    detection.start_command = Some("npm start".to_string());
                }
            } else if deps.contains_key("express") {
                detection.metadata.insert("framework".to_string(), "express".to_string());
            } else if deps.contains_key("fastify") {
                detection.metadata.insert("framework".to_string(), "fastify".to_string());
            } else if deps.contains_key("koa") {
                detection.metadata.insert("framework".to_string(), "koa".to_string());
            }
        }

        // Default start command
        if detection.start_command.is_none() {
            if let Some(entry) = &detection.entry_point {
                detection.start_command = Some(format!("node {}", entry));
            } else if source_path.join("index.js").exists() {
                detection.entry_point = Some("index.js".to_string());
                detection.start_command = Some("node index.js".to_string());
            } else if source_path.join("server.js").exists() {
                detection.entry_point = Some("server.js".to_string());
                detection.start_command = Some("node server.js".to_string());
            } else if source_path.join("app.js").exists() {
                detection.entry_point = Some("app.js".to_string());
                detection.start_command = Some("node app.js".to_string());
            }
        }

        Ok(())
    }

    /// Detect Python application details
    fn detect_python(source_path: &Path, detection: &mut AppDetection) -> Result<()> {
        // Detect package manager and dependencies file
        if source_path.join("Pipfile").exists() {
            detection.package_manager = Some("pipenv".to_string());
        } else if source_path.join("pyproject.toml").exists() {
            // Check if it's Poetry or just pyproject.toml
            let content = std::fs::read_to_string(source_path.join("pyproject.toml"))?;
            if content.contains("[tool.poetry]") {
                detection.package_manager = Some("poetry".to_string());
            } else {
                detection.package_manager = Some("pip".to_string());
            }
        } else {
            detection.package_manager = Some("pip".to_string());
        }

        // Try to detect Python version from various sources
        if source_path.join("runtime.txt").exists() {
            let content = std::fs::read_to_string(source_path.join("runtime.txt"))?;
            // Format: python-3.11.4
            if let Some(version) = content.trim().strip_prefix("python-") {
                let major_minor = version.split('.').take(2).collect::<Vec<_>>().join(".");
                detection.version = Some(major_minor);
            }
        } else if source_path.join(".python-version").exists() {
            let content = std::fs::read_to_string(source_path.join(".python-version"))?;
            let version = content.trim();
            let major_minor = version.split('.').take(2).collect::<Vec<_>>().join(".");
            detection.version = Some(major_minor);
        }

        // Default to Python 3.11
        if detection.version.is_none() {
            detection.version = Some("3.11".to_string());
        }

        // Detect common frameworks and entry points
        let common_entry_points = [
            "app.py",
            "main.py",
            "server.py",
            "application.py",
            "wsgi.py",
            "asgi.py",
        ];

        for entry in common_entry_points {
            if source_path.join(entry).exists() {
                detection.entry_point = Some(entry.to_string());
                break;
            }
        }

        // Check for WSGI/ASGI frameworks
        if source_path.join("requirements.txt").exists() {
            let content = std::fs::read_to_string(source_path.join("requirements.txt"))?;
            let content_lower = content.to_lowercase();

            if content_lower.contains("django") {
                detection.metadata.insert("framework".to_string(), "django".to_string());
                // Look for manage.py to find the project
                if source_path.join("manage.py").exists() {
                    detection.entry_point = Some("manage.py".to_string());
                    detection.start_command = Some(
                        "gunicorn --bind 0.0.0.0:$PORT $(find . -name wsgi.py -printf '%h' | head -1 | sed 's|./||;s|/|.|g').wsgi:application".to_string()
                    );
                }
            } else if content_lower.contains("flask") {
                detection.metadata.insert("framework".to_string(), "flask".to_string());
                if detection.entry_point.is_some() {
                    let entry = detection.entry_point.as_ref().unwrap().replace(".py", "");
                    detection.start_command = Some(format!(
                        "gunicorn --bind 0.0.0.0:$PORT {}:app",
                        entry
                    ));
                }
            } else if content_lower.contains("fastapi") {
                detection.metadata.insert("framework".to_string(), "fastapi".to_string());
                if detection.entry_point.is_some() {
                    let entry = detection.entry_point.as_ref().unwrap().replace(".py", "");
                    detection.start_command = Some(format!(
                        "uvicorn {}:app --host 0.0.0.0 --port $PORT",
                        entry
                    ));
                }
            }
        }

        // Default start command
        if detection.start_command.is_none() {
            if let Some(entry) = &detection.entry_point {
                detection.start_command = Some(format!("python {}", entry));
            }
        }

        Ok(())
    }

    /// Detect Go application details
    fn detect_go(source_path: &Path, detection: &mut AppDetection) -> Result<()> {
        let go_mod_path = source_path.join("go.mod");
        let content = std::fs::read_to_string(&go_mod_path)
            .context("Failed to read go.mod")?;

        // Parse Go version from go.mod
        for line in content.lines() {
            if line.starts_with("go ") {
                let version = line.strip_prefix("go ").unwrap_or("1.21").trim();
                detection.version = Some(version.to_string());
                break;
            }
        }

        // Default to Go 1.21
        if detection.version.is_none() {
            detection.version = Some("1.21".to_string());
        }

        // Parse module name
        for line in content.lines() {
            if line.starts_with("module ") {
                let module = line.strip_prefix("module ").unwrap_or("").trim();
                detection.metadata.insert("module".to_string(), module.to_string());
                break;
            }
        }

        // Check for main.go
        if source_path.join("main.go").exists() {
            detection.entry_point = Some("main.go".to_string());
        } else if source_path.join("cmd").exists() {
            // Check for cmd/appname/main.go pattern
            if let Ok(entries) = std::fs::read_dir(source_path.join("cmd")) {
                for entry in entries.flatten() {
                    if entry.path().join("main.go").exists() {
                        detection.entry_point = Some(format!("cmd/{}/main.go", entry.file_name().to_string_lossy()));
                        break;
                    }
                }
            }
        }

        // Default start command - runs the compiled binary
        detection.start_command = Some("./app".to_string());
        detection.build_command = Some("go build -o app .".to_string());

        Ok(())
    }

    /// Detect Ruby application details
    fn detect_ruby(source_path: &Path, detection: &mut AppDetection) -> Result<()> {
        detection.package_manager = Some("bundler".to_string());

        // Check for Ruby version
        if source_path.join(".ruby-version").exists() {
            let content = std::fs::read_to_string(source_path.join(".ruby-version"))?;
            detection.version = Some(content.trim().to_string());
        }

        // Default to Ruby 3.2
        if detection.version.is_none() {
            detection.version = Some("3.2".to_string());
        }

        // Detect Rails
        let gemfile = source_path.join("Gemfile");
        if gemfile.exists() {
            let content = std::fs::read_to_string(&gemfile)?;
            if content.contains("rails") {
                detection.metadata.insert("framework".to_string(), "rails".to_string());
                detection.start_command = Some("bundle exec rails server -b 0.0.0.0 -p $PORT".to_string());
            } else if content.contains("sinatra") {
                detection.metadata.insert("framework".to_string(), "sinatra".to_string());
            }
        }

        // Check for config.ru (Rack apps)
        if source_path.join("config.ru").exists() {
            detection.entry_point = Some("config.ru".to_string());
            if detection.start_command.is_none() {
                detection.start_command = Some("bundle exec rackup -p $PORT -o 0.0.0.0".to_string());
            }
        }

        Ok(())
    }

    /// Detect Rust application details
    fn detect_rust(source_path: &Path, detection: &mut AppDetection) -> Result<()> {
        let cargo_toml_path = source_path.join("Cargo.toml");
        let content = std::fs::read_to_string(&cargo_toml_path)
            .context("Failed to read Cargo.toml")?;

        // Parse package name
        for line in content.lines() {
            if line.starts_with("name") {
                if let Some((_, value)) = line.split_once('=') {
                    let name = value.trim().trim_matches('"');
                    detection.metadata.insert("package".to_string(), name.to_string());
                    break;
                }
            }
        }

        // Check for common web frameworks
        if content.contains("actix-web") {
            detection.metadata.insert("framework".to_string(), "actix-web".to_string());
        } else if content.contains("axum") {
            detection.metadata.insert("framework".to_string(), "axum".to_string());
        } else if content.contains("rocket") {
            detection.metadata.insert("framework".to_string(), "rocket".to_string());
        } else if content.contains("warp") {
            detection.metadata.insert("framework".to_string(), "warp".to_string());
        }

        // Use rust-toolchain.toml if present
        if source_path.join("rust-toolchain.toml").exists() {
            let toolchain = std::fs::read_to_string(source_path.join("rust-toolchain.toml"))?;
            for line in toolchain.lines() {
                if line.starts_with("channel") {
                    if let Some((_, value)) = line.split_once('=') {
                        detection.version = Some(value.trim().trim_matches('"').to_string());
                        break;
                    }
                }
            }
        }

        // Default to stable
        if detection.version.is_none() {
            detection.version = Some("stable".to_string());
        }

        detection.build_command = Some("cargo build --release".to_string());

        // Binary name from package
        if let Some(pkg) = detection.metadata.get("package") {
            detection.start_command = Some(format!("./target/release/{}", pkg));
        } else {
            detection.start_command = Some("./target/release/app".to_string());
        }

        Ok(())
    }

    /// Detect static site details
    fn detect_static(source_path: &Path, detection: &mut AppDetection) -> Result<()> {
        detection.entry_point = Some("index.html".to_string());

        // Check for build tools
        if source_path.join("package.json").exists() {
            // It's a static site with a build step (React, Vue, etc.)
            let content = std::fs::read_to_string(source_path.join("package.json"))?;
            let pkg: serde_json::Value = serde_json::from_str(&content)?;

            if let Some(scripts) = pkg.get("scripts").and_then(|v| v.as_object()) {
                if scripts.contains_key("build") {
                    detection.build_command = Some("npm run build".to_string());

                    // Detect common static site generators
                    if let Some(deps) = pkg.get("dependencies").and_then(|v| v.as_object()) {
                        if deps.contains_key("react") || deps.contains_key("react-dom") {
                            detection.metadata.insert("framework".to_string(), "react".to_string());
                            detection.metadata.insert("build_dir".to_string(), "build".to_string());
                        } else if deps.contains_key("vue") {
                            detection.metadata.insert("framework".to_string(), "vue".to_string());
                            detection.metadata.insert("build_dir".to_string(), "dist".to_string());
                        }
                    }
                    if let Some(dev_deps) = pkg.get("devDependencies").and_then(|v| v.as_object()) {
                        if dev_deps.contains_key("vite") {
                            detection.metadata.insert("build_tool".to_string(), "vite".to_string());
                            detection.metadata.insert("build_dir".to_string(), "dist".to_string());
                        }
                    }
                }
            }
        }

        // Use nginx to serve static files
        detection.start_command = Some("nginx -g 'daemon off;'".to_string());
        detection.port = Some(80);

        Ok(())
    }

    /// Generate a Dockerfile for the detected application
    pub fn generate_dockerfile(detection: &AppDetection) -> Result<String> {
        match detection.language {
            Language::NodeJs => Self::generate_nodejs_dockerfile(detection),
            Language::Python => Self::generate_python_dockerfile(detection),
            Language::Go => Self::generate_go_dockerfile(detection),
            Language::Ruby => Self::generate_ruby_dockerfile(detection),
            Language::Rust => Self::generate_rust_dockerfile(detection),
            Language::Static => Self::generate_static_dockerfile(detection),
            Language::Unknown => {
                anyhow::bail!(
                    "Unable to detect application type. Please add a Dockerfile or use one of the supported languages:\n\
                     - Node.js (package.json)\n\
                     - Python (requirements.txt, Pipfile, pyproject.toml)\n\
                     - Go (go.mod)\n\
                     - Ruby (Gemfile)\n\
                     - Rust (Cargo.toml)\n\
                     - Static (index.html)"
                )
            }
        }
    }

    /// Generate Dockerfile for Node.js
    fn generate_nodejs_dockerfile(detection: &AppDetection) -> Result<String> {
        let version = detection.version.as_deref().unwrap_or("20");
        let pm = detection.package_manager.as_deref().unwrap_or("npm");

        let install_cmd = match pm {
            "yarn" => "yarn install --frozen-lockfile",
            "pnpm" => "pnpm install --frozen-lockfile",
            _ => "npm ci",
        };

        let build_cmd = detection.build_command.as_deref();
        let start_cmd = detection.start_command.as_deref().unwrap_or("npm start");

        let build_step = if let Some(cmd) = build_cmd {
            format!("RUN {}\n", cmd)
        } else {
            String::new()
        };

        // Check if it's Next.js for special handling
        let is_nextjs = detection.metadata.get("framework").map(|f| f == "next").unwrap_or(false);

        let dockerfile = if is_nextjs {
            format!(r#"# Generated by spawngate buildpacks
FROM node:{version}-alpine AS builder
WORKDIR /app

# Install dependencies
COPY package*.json yarn.lock* pnpm-lock.yaml* ./
RUN {install_cmd}

# Copy source and build
COPY . .
RUN npm run build

# Production image
FROM node:{version}-alpine
WORKDIR /app

ENV NODE_ENV=production
ENV PORT=3000

COPY --from=builder /app/package*.json ./
COPY --from=builder /app/.next ./.next
COPY --from=builder /app/public ./public
COPY --from=builder /app/node_modules ./node_modules

EXPOSE 3000
CMD ["npm", "start"]
"#)
        } else {
            format!(r#"# Generated by spawngate buildpacks
FROM node:{version}-alpine AS builder
WORKDIR /app

# Install dependencies
COPY package*.json yarn.lock* pnpm-lock.yaml* ./
RUN {install_cmd}

# Copy source
COPY . .

# Build if needed
{build_step}
# Production image
FROM node:{version}-alpine
WORKDIR /app

ENV NODE_ENV=production
ENV PORT=3000

COPY --from=builder /app ./

EXPOSE 3000
CMD {start_cmd_json}
"#,
                start_cmd_json = serde_json::to_string(&start_cmd.split_whitespace().collect::<Vec<_>>())?
            )
        };

        Ok(dockerfile)
    }

    /// Generate Dockerfile for Python
    fn generate_python_dockerfile(detection: &AppDetection) -> Result<String> {
        let version = detection.version.as_deref().unwrap_or("3.11");
        let pm = detection.package_manager.as_deref().unwrap_or("pip");

        let (install_deps, copy_deps) = match pm {
            "poetry" => (
                "RUN pip install poetry && poetry config virtualenvs.create false",
                "COPY pyproject.toml poetry.lock* ./\nRUN poetry install --no-dev --no-interaction --no-ansi",
            ),
            "pipenv" => (
                "RUN pip install pipenv",
                "COPY Pipfile Pipfile.lock* ./\nRUN pipenv install --system --deploy",
            ),
            _ => (
                "",
                "COPY requirements.txt ./\nRUN pip install --no-cache-dir -r requirements.txt",
            ),
        };

        let start_cmd = detection.start_command.as_deref()
            .unwrap_or("python app.py");

        // Check for gunicorn/uvicorn
        let needs_wsgi = detection.metadata.get("framework")
            .map(|f| f == "django" || f == "flask")
            .unwrap_or(false);

        let needs_asgi = detection.metadata.get("framework")
            .map(|f| f == "fastapi")
            .unwrap_or(false);

        let extra_deps = if needs_wsgi {
            "gunicorn"
        } else if needs_asgi {
            "uvicorn[standard]"
        } else {
            ""
        };

        let extra_install = if !extra_deps.is_empty() {
            format!("RUN pip install {}\n", extra_deps)
        } else {
            String::new()
        };

        let dockerfile = format!(r#"# Generated by spawngate buildpacks
FROM python:{version}-slim

WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    gcc \
    && rm -rf /var/lib/apt/lists/*

{install_deps}

# Install Python dependencies
{copy_deps}

{extra_install}
# Copy application code
COPY . .

ENV PORT=8000
ENV PYTHONUNBUFFERED=1

EXPOSE 8000

CMD {start_cmd_json}
"#,
            install_deps = install_deps,
            copy_deps = copy_deps,
            extra_install = extra_install,
            start_cmd_json = serde_json::to_string(
                &shell_words::split(start_cmd).unwrap_or_else(|_| vec![start_cmd.to_string()])
            )?
        );

        Ok(dockerfile)
    }

    /// Generate Dockerfile for Go
    fn generate_go_dockerfile(detection: &AppDetection) -> Result<String> {
        let version = detection.version.as_deref().unwrap_or("1.21");
        let _module = detection.metadata.get("module").map(|s| s.as_str()).unwrap_or("app");

        let dockerfile = format!(r#"# Generated by spawngate buildpacks
FROM golang:{version}-alpine AS builder

WORKDIR /app

# Install git for private modules
RUN apk add --no-cache git

# Download dependencies
COPY go.mod go.sum* ./
RUN go mod download

# Build application
COPY . .
RUN CGO_ENABLED=0 GOOS=linux go build -ldflags="-s -w" -o /app/server .

# Production image
FROM alpine:3.19

RUN apk add --no-cache ca-certificates tzdata

WORKDIR /app

COPY --from=builder /app/server .

ENV PORT=8080

EXPOSE 8080

CMD ["./server"]
"#);

        Ok(dockerfile)
    }

    /// Generate Dockerfile for Ruby
    fn generate_ruby_dockerfile(detection: &AppDetection) -> Result<String> {
        let version = detection.version.as_deref().unwrap_or("3.2");
        let is_rails = detection.metadata.get("framework")
            .map(|f| f == "rails")
            .unwrap_or(false);

        let start_cmd = detection.start_command.as_deref()
            .unwrap_or("bundle exec ruby app.rb");

        let rails_extras = if is_rails {
            r#"
# Precompile assets for Rails
RUN bundle exec rake assets:precompile

ENV RAILS_ENV=production
ENV RAILS_LOG_TO_STDOUT=true
"#
        } else {
            ""
        };

        let dockerfile = format!(r#"# Generated by spawngate buildpacks
FROM ruby:{version}-slim

WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    libpq-dev \
    nodejs \
    && rm -rf /var/lib/apt/lists/*

# Install bundler
RUN gem install bundler

# Install gems
COPY Gemfile Gemfile.lock* ./
RUN bundle config set --local without 'development test' && \
    bundle install --jobs 4 --retry 3

# Copy application code
COPY . .
{rails_extras}
ENV PORT=3000

EXPOSE 3000

CMD {start_cmd_json}
"#,
            rails_extras = rails_extras,
            start_cmd_json = serde_json::to_string(
                &shell_words::split(start_cmd).unwrap_or_else(|_| vec![start_cmd.to_string()])
            )?
        );

        Ok(dockerfile)
    }

    /// Generate Dockerfile for Rust
    fn generate_rust_dockerfile(detection: &AppDetection) -> Result<String> {
        let version = detection.version.as_deref().unwrap_or("stable");
        let pkg_name = detection.metadata.get("package").map(|s| s.as_str()).unwrap_or("app");

        let dockerfile = format!(r#"# Generated by spawngate buildpacks
FROM rust:{version}-slim AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Cache dependencies
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo "fn main() {{}}" > src/main.rs && \
    cargo build --release && \
    rm -rf src

# Build application
COPY . .
RUN touch src/main.rs && cargo build --release

# Production image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/{pkg_name} ./app

ENV PORT=8080

EXPOSE 8080

CMD ["./app"]
"#);

        Ok(dockerfile)
    }

    /// Generate Dockerfile for static sites
    fn generate_static_dockerfile(detection: &AppDetection) -> Result<String> {
        let has_build = detection.build_command.is_some();
        let build_dir = detection.metadata.get("build_dir")
            .map(|s| s.as_str())
            .unwrap_or(".");

        let dockerfile = if has_build {
            format!(r#"# Generated by spawngate buildpacks
FROM node:20-alpine AS builder

WORKDIR /app

# Install dependencies
COPY package*.json yarn.lock* pnpm-lock.yaml* ./
RUN npm ci

# Build
COPY . .
RUN npm run build

# Production image with nginx
FROM nginx:alpine

# Copy nginx config
RUN echo 'server {{ \
    listen 80; \
    location / {{ \
        root /usr/share/nginx/html; \
        index index.html; \
        try_files $uri $uri/ /index.html; \
    }} \
}}' > /etc/nginx/conf.d/default.conf

# Copy built assets
COPY --from=builder /app/{build_dir} /usr/share/nginx/html

EXPOSE 80

CMD ["nginx", "-g", "daemon off;"]
"#)
        } else {
            format!(r#"# Generated by spawngate buildpacks
FROM nginx:alpine

# Copy nginx config
RUN echo 'server {{ \
    listen 80; \
    location / {{ \
        root /usr/share/nginx/html; \
        index index.html; \
        try_files $uri $uri/ /index.html; \
    }} \
}}' > /etc/nginx/conf.d/default.conf

# Copy static files
COPY . /usr/share/nginx/html

EXPOSE 80

CMD ["nginx", "-g", "daemon off;"]
"#)
        };

        Ok(dockerfile)
    }

    /// Write generated Dockerfile to the source directory
    pub fn write_dockerfile(source_path: &Path, detection: &AppDetection) -> Result<std::path::PathBuf> {
        let dockerfile_content = Self::generate_dockerfile(detection)?;
        let dockerfile_path = source_path.join(".spawngate.Dockerfile");

        std::fs::write(&dockerfile_path, &dockerfile_content)
            .context("Failed to write generated Dockerfile")?;

        info!(
            path = %dockerfile_path.display(),
            language = %detection.language,
            "Generated Dockerfile"
        );

        Ok(dockerfile_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_temp_dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    // ==================== Procfile Tests ====================

    #[test]
    fn test_parse_procfile() {
        let dir = create_temp_dir();
        let procfile = dir.path().join("Procfile");

        fs::write(&procfile, r#"
web: node server.js
worker: node worker.js
# This is a comment
release: npm run migrate
        "#).unwrap();

        let processes = Buildpack::parse_procfile(dir.path()).unwrap();

        assert_eq!(processes.len(), 3);
        assert_eq!(processes[0].name, "web");
        assert_eq!(processes[0].command, "node server.js");
        assert_eq!(processes[1].name, "worker");
        assert_eq!(processes[1].command, "node worker.js");
        assert_eq!(processes[2].name, "release");
        assert_eq!(processes[2].command, "npm run migrate");
    }

    #[test]
    fn test_parse_procfile_missing() {
        let dir = create_temp_dir();
        let processes = Buildpack::parse_procfile(dir.path()).unwrap();
        assert!(processes.is_empty());
    }

    // ==================== Node.js Detection Tests ====================

    #[test]
    fn test_detect_nodejs_basic() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("package.json"), r#"{
            "name": "my-app",
            "main": "server.js",
            "scripts": {
                "start": "node server.js"
            }
        }"#).unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::NodeJs);
        assert_eq!(detection.entry_point, Some("server.js".to_string()));
        assert_eq!(detection.start_command, Some("node server.js".to_string()));
        assert_eq!(detection.package_manager, Some("npm".to_string()));
    }

    #[test]
    fn test_detect_nodejs_with_yarn() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("package.json"), r#"{
            "name": "my-app"
        }"#).unwrap();
        fs::write(dir.path().join("yarn.lock"), "").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::NodeJs);
        assert_eq!(detection.package_manager, Some("yarn".to_string()));
    }

    #[test]
    fn test_detect_nodejs_with_pnpm() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("package.json"), r#"{
            "name": "my-app"
        }"#).unwrap();
        fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::NodeJs);
        assert_eq!(detection.package_manager, Some("pnpm".to_string()));
    }

    #[test]
    fn test_detect_nodejs_with_engines() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("package.json"), r#"{
            "name": "my-app",
            "engines": {
                "node": ">=18.0.0"
            }
        }"#).unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::NodeJs);
        assert_eq!(detection.version, Some("18".to_string()));
    }

    #[test]
    fn test_detect_nodejs_express() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("package.json"), r#"{
            "name": "my-app",
            "dependencies": {
                "express": "^4.18.0"
            }
        }"#).unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::NodeJs);
        assert_eq!(detection.metadata.get("framework"), Some(&"express".to_string()));
    }

    #[test]
    fn test_detect_nodejs_nextjs() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("package.json"), r#"{
            "name": "my-nextjs-app",
            "dependencies": {
                "next": "^14.0.0",
                "react": "^18.0.0"
            },
            "scripts": {
                "build": "next build",
                "start": "next start"
            }
        }"#).unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::NodeJs);
        assert_eq!(detection.metadata.get("framework"), Some(&"next".to_string()));
        assert!(detection.build_command.is_some());
    }

    // ==================== Python Detection Tests ====================

    #[test]
    fn test_detect_python_pip() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("requirements.txt"), "flask==2.0.0\ngunicorn").unwrap();
        fs::write(dir.path().join("app.py"), "").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Python);
        assert_eq!(detection.package_manager, Some("pip".to_string()));
        assert_eq!(detection.entry_point, Some("app.py".to_string()));
        assert_eq!(detection.metadata.get("framework"), Some(&"flask".to_string()));
    }

    #[test]
    fn test_detect_python_poetry() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("pyproject.toml"), r#"
[tool.poetry]
name = "my-app"
version = "0.1.0"
        "#).unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Python);
        assert_eq!(detection.package_manager, Some("poetry".to_string()));
    }

    #[test]
    fn test_detect_python_pipenv() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("Pipfile"), "").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Python);
        assert_eq!(detection.package_manager, Some("pipenv".to_string()));
    }

    #[test]
    fn test_detect_python_version() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("requirements.txt"), "flask").unwrap();
        fs::write(dir.path().join("runtime.txt"), "python-3.10.1").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Python);
        assert_eq!(detection.version, Some("3.10".to_string()));
    }

    #[test]
    fn test_detect_python_fastapi() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("requirements.txt"), "fastapi\nuvicorn").unwrap();
        fs::write(dir.path().join("main.py"), "").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Python);
        assert_eq!(detection.metadata.get("framework"), Some(&"fastapi".to_string()));
        assert!(detection.start_command.as_ref().unwrap().contains("uvicorn"));
    }

    // ==================== Go Detection Tests ====================

    #[test]
    fn test_detect_go() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("go.mod"), r#"
module github.com/example/myapp

go 1.21
        "#).unwrap();
        fs::write(dir.path().join("main.go"), "").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Go);
        assert_eq!(detection.version, Some("1.21".to_string()));
        assert_eq!(detection.entry_point, Some("main.go".to_string()));
        assert_eq!(detection.metadata.get("module"), Some(&"github.com/example/myapp".to_string()));
    }

    // ==================== Ruby Detection Tests ====================

    #[test]
    fn test_detect_ruby() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("Gemfile"), r#"
source 'https://rubygems.org'
gem 'sinatra'
        "#).unwrap();
        fs::write(dir.path().join(".ruby-version"), "3.3.0").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Ruby);
        assert_eq!(detection.version, Some("3.3.0".to_string()));
        assert_eq!(detection.metadata.get("framework"), Some(&"sinatra".to_string()));
    }

    #[test]
    fn test_detect_ruby_rails() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("Gemfile"), r#"
source 'https://rubygems.org'
gem 'rails', '~> 7.0'
        "#).unwrap();
        fs::write(dir.path().join("config.ru"), "").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Ruby);
        assert_eq!(detection.metadata.get("framework"), Some(&"rails".to_string()));
        assert!(detection.start_command.as_ref().unwrap().contains("rails server"));
    }

    // ==================== Rust Detection Tests ====================

    #[test]
    fn test_detect_rust() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("Cargo.toml"), r#"
[package]
name = "my-api"
version = "0.1.0"

[dependencies]
axum = "0.7"
        "#).unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Rust);
        assert_eq!(detection.metadata.get("package"), Some(&"my-api".to_string()));
        assert_eq!(detection.metadata.get("framework"), Some(&"axum".to_string()));
    }

    // ==================== Static Site Detection Tests ====================

    #[test]
    fn test_detect_static_simple() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("index.html"), "<html></html>").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Static);
        assert_eq!(detection.entry_point, Some("index.html".to_string()));
    }

    #[test]
    fn test_detect_nodejs_react() {
        // React apps with package.json are detected as Node.js (not Static)
        // because they need npm to build
        let dir = create_temp_dir();

        fs::write(dir.path().join("index.html"), "<html></html>").unwrap();
        fs::write(dir.path().join("package.json"), r#"{
            "dependencies": {
                "react": "^18.0.0",
                "react-dom": "^18.0.0"
            },
            "scripts": {
                "build": "react-scripts build"
            }
        }"#).unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        // Should be Node.js because package.json takes precedence
        assert_eq!(detection.language, Language::NodeJs);
        assert!(detection.build_command.is_some());
    }

    // ==================== Dockerfile Generation Tests ====================

    #[test]
    fn test_generate_nodejs_dockerfile() {
        let detection = AppDetection {
            language: Language::NodeJs,
            version: Some("20".to_string()),
            package_manager: Some("npm".to_string()),
            start_command: Some("node server.js".to_string()),
            ..Default::default()
        };

        let dockerfile = Buildpack::generate_dockerfile(&detection).unwrap();

        assert!(dockerfile.contains("FROM node:20"));
        assert!(dockerfile.contains("npm ci"));
        assert!(dockerfile.contains("EXPOSE 3000"));
    }

    #[test]
    fn test_generate_python_dockerfile() {
        let detection = AppDetection {
            language: Language::Python,
            version: Some("3.11".to_string()),
            package_manager: Some("pip".to_string()),
            start_command: Some("python app.py".to_string()),
            ..Default::default()
        };

        let dockerfile = Buildpack::generate_dockerfile(&detection).unwrap();

        assert!(dockerfile.contains("FROM python:3.11"));
        assert!(dockerfile.contains("pip install"));
        assert!(dockerfile.contains("EXPOSE 8000"));
    }

    #[test]
    fn test_generate_go_dockerfile() {
        let mut detection = AppDetection {
            language: Language::Go,
            version: Some("1.21".to_string()),
            ..Default::default()
        };
        detection.metadata.insert("module".to_string(), "github.com/example/app".to_string());

        let dockerfile = Buildpack::generate_dockerfile(&detection).unwrap();

        assert!(dockerfile.contains("FROM golang:1.21"));
        assert!(dockerfile.contains("go build"));
        assert!(dockerfile.contains("EXPOSE 8080"));
    }

    #[test]
    fn test_generate_static_dockerfile() {
        let detection = AppDetection {
            language: Language::Static,
            entry_point: Some("index.html".to_string()),
            ..Default::default()
        };

        let dockerfile = Buildpack::generate_dockerfile(&detection).unwrap();

        assert!(dockerfile.contains("FROM nginx"));
        assert!(dockerfile.contains("EXPOSE 80"));
    }

    #[test]
    fn test_generate_dockerfile_unknown() {
        let detection = AppDetection {
            language: Language::Unknown,
            ..Default::default()
        };

        let result = Buildpack::generate_dockerfile(&detection);
        assert!(result.is_err());
    }

    // ==================== Procfile Override Tests ====================

    #[test]
    fn test_procfile_overrides_start_command() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("package.json"), r#"{
            "name": "my-app",
            "scripts": {
                "start": "node default.js"
            }
        }"#).unwrap();
        fs::write(dir.path().join("Procfile"), "web: node custom-start.js").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::NodeJs);
        assert_eq!(detection.start_command, Some("node custom-start.js".to_string()));
    }

    // ==================== Integration Tests ====================

    #[test]
    fn test_write_dockerfile() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("package.json"), r#"{
            "name": "test-app"
        }"#).unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();
        let dockerfile_path = Buildpack::write_dockerfile(dir.path(), &detection).unwrap();

        assert!(dockerfile_path.exists());
        assert_eq!(dockerfile_path.file_name().unwrap(), ".spawngate.Dockerfile");

        let content = fs::read_to_string(&dockerfile_path).unwrap();
        assert!(content.contains("FROM node:"));
    }
}
