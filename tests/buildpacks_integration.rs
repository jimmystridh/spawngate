//! Integration tests for the buildpacks module
//!
//! These tests create real project structures and verify that the
//! detection and Dockerfile generation work correctly end-to-end.

use spawngate::buildpacks::{AppDetection, Buildpack, Language};
use std::fs;
use tempfile::TempDir;

fn create_temp_dir() -> TempDir {
    tempfile::tempdir().unwrap()
}

// ============================================================================
// Node.js Integration Tests
// ============================================================================

mod nodejs {
    use super::*;

    #[test]
    fn test_express_api() {
        let dir = create_temp_dir();

        // Create a realistic Express.js project
        fs::write(
            dir.path().join("package.json"),
            r#"{
                "name": "express-api",
                "version": "1.0.0",
                "main": "src/index.js",
                "scripts": {
                    "start": "node src/index.js",
                    "dev": "nodemon src/index.js",
                    "test": "jest"
                },
                "dependencies": {
                    "express": "^4.18.2",
                    "cors": "^2.8.5",
                    "helmet": "^7.0.0"
                },
                "devDependencies": {
                    "nodemon": "^3.0.0",
                    "jest": "^29.0.0"
                },
                "engines": {
                    "node": ">=18.0.0"
                }
            }"#,
        )
        .unwrap();

        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/index.js"),
            r#"
const express = require('express');
const app = express();
const PORT = process.env.PORT || 3000;

app.get('/health', (req, res) => res.json({ status: 'ok' }));
app.listen(PORT, () => console.log(`Server running on port ${PORT}`));
            "#,
        )
        .unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::NodeJs);
        assert_eq!(detection.version, Some("18".to_string()));
        assert_eq!(detection.package_manager, Some("npm".to_string()));
        assert_eq!(
            detection.metadata.get("framework"),
            Some(&"express".to_string())
        );
        assert_eq!(detection.entry_point, Some("src/index.js".to_string()));

        // Generate and verify Dockerfile
        let dockerfile = Buildpack::generate_dockerfile(&detection).unwrap();
        assert!(dockerfile.contains("FROM node:18"));
        assert!(dockerfile.contains("npm ci"));
        assert!(dockerfile.contains("EXPOSE 3000"));
    }

    #[test]
    fn test_nextjs_app() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("package.json"),
            r#"{
                "name": "my-nextjs-app",
                "version": "0.1.0",
                "scripts": {
                    "dev": "next dev",
                    "build": "next build",
                    "start": "next start"
                },
                "dependencies": {
                    "next": "14.0.0",
                    "react": "^18.2.0",
                    "react-dom": "^18.2.0"
                }
            }"#,
        )
        .unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::NodeJs);
        assert_eq!(
            detection.metadata.get("framework"),
            Some(&"next".to_string())
        );
        assert!(detection.build_command.is_some());

        let dockerfile = Buildpack::generate_dockerfile(&detection).unwrap();
        assert!(dockerfile.contains("npm run build"));
        assert!(dockerfile.contains(".next"));
    }

    #[test]
    fn test_yarn_project() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("package.json"),
            r#"{
                "name": "yarn-project",
                "version": "1.0.0"
            }"#,
        )
        .unwrap();
        fs::write(dir.path().join("yarn.lock"), "# yarn lockfile").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::NodeJs);
        assert_eq!(detection.package_manager, Some("yarn".to_string()));

        let dockerfile = Buildpack::generate_dockerfile(&detection).unwrap();
        assert!(dockerfile.contains("yarn install"));
    }

    #[test]
    fn test_pnpm_project() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("package.json"),
            r#"{
                "name": "pnpm-project",
                "version": "1.0.0"
            }"#,
        )
        .unwrap();
        fs::write(dir.path().join("pnpm-lock.yaml"), "lockfileVersion: 6.0").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.package_manager, Some("pnpm".to_string()));

        let dockerfile = Buildpack::generate_dockerfile(&detection).unwrap();
        assert!(dockerfile.contains("pnpm install"));
    }

    #[test]
    fn test_fastify_app() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("package.json"),
            r#"{
                "name": "fastify-app",
                "dependencies": {
                    "fastify": "^4.0.0"
                }
            }"#,
        )
        .unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::NodeJs);
        assert_eq!(
            detection.metadata.get("framework"),
            Some(&"fastify".to_string())
        );
    }
}

// ============================================================================
// Python Integration Tests
// ============================================================================

mod python {
    use super::*;

    #[test]
    fn test_flask_api() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("requirements.txt"),
            r#"
flask==3.0.0
gunicorn==21.2.0
python-dotenv==1.0.0
            "#,
        )
        .unwrap();

        fs::write(
            dir.path().join("app.py"),
            r#"
from flask import Flask
app = Flask(__name__)

@app.route('/health')
def health():
    return {'status': 'ok'}

if __name__ == '__main__':
    app.run()
            "#,
        )
        .unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Python);
        assert_eq!(detection.package_manager, Some("pip".to_string()));
        assert_eq!(detection.metadata.get("framework"), Some(&"flask".to_string()));
        assert_eq!(detection.entry_point, Some("app.py".to_string()));

        let dockerfile = Buildpack::generate_dockerfile(&detection).unwrap();
        assert!(dockerfile.contains("FROM python:"));
        assert!(dockerfile.contains("pip install"));
        assert!(dockerfile.contains("gunicorn"));
    }

    #[test]
    fn test_fastapi_app() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("requirements.txt"),
            r#"
fastapi==0.104.0
uvicorn[standard]==0.24.0
            "#,
        )
        .unwrap();

        fs::write(
            dir.path().join("main.py"),
            r#"
from fastapi import FastAPI
app = FastAPI()

@app.get("/health")
def health():
    return {"status": "ok"}
            "#,
        )
        .unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Python);
        assert_eq!(
            detection.metadata.get("framework"),
            Some(&"fastapi".to_string())
        );
        assert!(detection.start_command.as_ref().unwrap().contains("uvicorn"));

        let dockerfile = Buildpack::generate_dockerfile(&detection).unwrap();
        assert!(dockerfile.contains("uvicorn"));
    }

    #[test]
    fn test_django_project() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("requirements.txt"),
            r#"
django==4.2.0
gunicorn==21.2.0
psycopg2-binary==2.9.9
            "#,
        )
        .unwrap();

        fs::write(dir.path().join("manage.py"), "#!/usr/bin/env python").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Python);
        assert_eq!(
            detection.metadata.get("framework"),
            Some(&"django".to_string())
        );
        assert_eq!(detection.entry_point, Some("manage.py".to_string()));
    }

    #[test]
    fn test_poetry_project() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("pyproject.toml"),
            r#"
[tool.poetry]
name = "my-project"
version = "0.1.0"
description = ""

[tool.poetry.dependencies]
python = "^3.11"
fastapi = "^0.104.0"

[build-system]
requires = ["poetry-core"]
build-backend = "poetry.core.masonry.api"
            "#,
        )
        .unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Python);
        assert_eq!(detection.package_manager, Some("poetry".to_string()));

        let dockerfile = Buildpack::generate_dockerfile(&detection).unwrap();
        assert!(dockerfile.contains("poetry"));
    }

    #[test]
    fn test_pipenv_project() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("Pipfile"),
            r#"
[[source]]
url = "https://pypi.org/simple"
verify_ssl = true
name = "pypi"

[packages]
flask = "*"
            "#,
        )
        .unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Python);
        assert_eq!(detection.package_manager, Some("pipenv".to_string()));

        let dockerfile = Buildpack::generate_dockerfile(&detection).unwrap();
        assert!(dockerfile.contains("pipenv"));
    }

    #[test]
    fn test_python_version_from_runtime_txt() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("requirements.txt"), "flask").unwrap();
        fs::write(dir.path().join("runtime.txt"), "python-3.10.12").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.version, Some("3.10".to_string()));

        let dockerfile = Buildpack::generate_dockerfile(&detection).unwrap();
        assert!(dockerfile.contains("python:3.10"));
    }
}

// ============================================================================
// Go Integration Tests
// ============================================================================

mod go {
    use super::*;

    #[test]
    fn test_go_api() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("go.mod"),
            r#"
module github.com/example/myapi

go 1.21

require github.com/gin-gonic/gin v1.9.1
            "#,
        )
        .unwrap();

        fs::write(
            dir.path().join("main.go"),
            r#"
package main

import "github.com/gin-gonic/gin"

func main() {
    r := gin.Default()
    r.GET("/health", func(c *gin.Context) {
        c.JSON(200, gin.H{"status": "ok"})
    })
    r.Run()
}
            "#,
        )
        .unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Go);
        assert_eq!(detection.version, Some("1.21".to_string()));
        assert_eq!(
            detection.metadata.get("module"),
            Some(&"github.com/example/myapi".to_string())
        );
        assert_eq!(detection.entry_point, Some("main.go".to_string()));

        let dockerfile = Buildpack::generate_dockerfile(&detection).unwrap();
        assert!(dockerfile.contains("FROM golang:1.21"));
        assert!(dockerfile.contains("go build"));
        assert!(dockerfile.contains("EXPOSE 8080"));
    }

    #[test]
    fn test_go_cmd_structure() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("go.mod"),
            r#"
module github.com/example/myapp

go 1.22
            "#,
        )
        .unwrap();

        fs::create_dir_all(dir.path().join("cmd/server")).unwrap();
        fs::write(dir.path().join("cmd/server/main.go"), "package main").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Go);
        assert_eq!(detection.version, Some("1.22".to_string()));
        assert!(detection.entry_point.as_ref().unwrap().contains("cmd/server"));
    }
}

// ============================================================================
// Ruby Integration Tests
// ============================================================================

mod ruby {
    use super::*;

    #[test]
    fn test_rails_app() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("Gemfile"),
            r#"
source 'https://rubygems.org'

ruby '3.2.0'

gem 'rails', '~> 7.1'
gem 'puma', '~> 6.0'
gem 'pg', '~> 1.5'
            "#,
        )
        .unwrap();

        fs::write(dir.path().join(".ruby-version"), "3.2.0").unwrap();
        fs::write(dir.path().join("config.ru"), "require_relative 'config/environment'").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Ruby);
        assert_eq!(detection.version, Some("3.2.0".to_string()));
        assert_eq!(detection.metadata.get("framework"), Some(&"rails".to_string()));
        assert!(detection.start_command.as_ref().unwrap().contains("rails server"));

        let dockerfile = Buildpack::generate_dockerfile(&detection).unwrap();
        assert!(dockerfile.contains("FROM ruby:3.2"));
        assert!(dockerfile.contains("bundle"));
        assert!(dockerfile.contains("RAILS_ENV=production"));
    }

    #[test]
    fn test_sinatra_app() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("Gemfile"),
            r#"
source 'https://rubygems.org'
gem 'sinatra'
gem 'puma'
            "#,
        )
        .unwrap();

        fs::write(dir.path().join("config.ru"), "require './app'\nrun Sinatra::Application").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Ruby);
        assert_eq!(
            detection.metadata.get("framework"),
            Some(&"sinatra".to_string())
        );
        assert_eq!(detection.entry_point, Some("config.ru".to_string()));
    }
}

// ============================================================================
// Rust Integration Tests
// ============================================================================

mod rust {
    use super::*;

    #[test]
    fn test_axum_api() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("Cargo.toml"),
            r#"
[package]
name = "my-api"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = "0.7"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
            "#,
        )
        .unwrap();

        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Rust);
        assert_eq!(detection.metadata.get("package"), Some(&"my-api".to_string()));
        assert_eq!(detection.metadata.get("framework"), Some(&"axum".to_string()));

        let dockerfile = Buildpack::generate_dockerfile(&detection).unwrap();
        assert!(dockerfile.contains("cargo build --release"));
        assert!(dockerfile.contains("my-api"));
    }

    #[test]
    fn test_actix_web() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("Cargo.toml"),
            r#"
[package]
name = "actix-server"
version = "0.1.0"

[dependencies]
actix-web = "4"
            "#,
        )
        .unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Rust);
        assert_eq!(
            detection.metadata.get("framework"),
            Some(&"actix-web".to_string())
        );
    }

    #[test]
    fn test_rust_toolchain() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("Cargo.toml"),
            r#"
[package]
name = "my-app"
version = "0.1.0"
            "#,
        )
        .unwrap();

        fs::write(
            dir.path().join("rust-toolchain.toml"),
            r#"
[toolchain]
channel = "1.75.0"
            "#,
        )
        .unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.version, Some("1.75.0".to_string()));
    }
}

// ============================================================================
// Static Site Integration Tests
// ============================================================================

mod static_site {
    use super::*;

    #[test]
    fn test_simple_static_site() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("index.html"),
            r#"
<!DOCTYPE html>
<html>
<head><title>My Site</title></head>
<body><h1>Hello World</h1></body>
</html>
            "#,
        )
        .unwrap();

        fs::write(dir.path().join("style.css"), "body { margin: 0; }").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Static);
        assert_eq!(detection.entry_point, Some("index.html".to_string()));
        assert_eq!(detection.port, Some(80));

        let dockerfile = Buildpack::generate_dockerfile(&detection).unwrap();
        assert!(dockerfile.contains("nginx"));
        assert!(dockerfile.contains("EXPOSE 80"));
    }

    #[test]
    fn test_vite_react_app() {
        let dir = create_temp_dir();

        // Vite React apps have index.html AND package.json
        // They should be detected as Node.js because they need npm build
        fs::write(
            dir.path().join("package.json"),
            r#"{
                "name": "vite-react-app",
                "scripts": {
                    "dev": "vite",
                    "build": "vite build"
                },
                "dependencies": {
                    "react": "^18.2.0"
                },
                "devDependencies": {
                    "vite": "^5.0.0"
                }
            }"#,
        )
        .unwrap();

        fs::write(dir.path().join("index.html"), "<!DOCTYPE html>").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        // Package.json takes precedence, so it's Node.js
        assert_eq!(detection.language, Language::NodeJs);
        assert!(detection.build_command.is_some());
    }
}

// ============================================================================
// Procfile Integration Tests
// ============================================================================

mod procfile {
    use super::*;

    #[test]
    fn test_procfile_with_multiple_processes() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("package.json"),
            r#"{"name": "worker-app"}"#,
        )
        .unwrap();

        fs::write(
            dir.path().join("Procfile"),
            r#"
web: node server.js
worker: node worker.js
clock: node scheduler.js
release: npm run migrate
            "#,
        )
        .unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.processes.len(), 4);

        let web = detection.processes.iter().find(|p| p.name == "web").unwrap();
        assert_eq!(web.command, "node server.js");

        let worker = detection.processes.iter().find(|p| p.name == "worker").unwrap();
        assert_eq!(worker.command, "node worker.js");

        // Web process should be the start command
        assert_eq!(detection.start_command, Some("node server.js".to_string()));
    }

    #[test]
    fn test_procfile_overrides_default_start() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("package.json"),
            r#"{
                "name": "custom-start",
                "scripts": {
                    "start": "node default.js"
                }
            }"#,
        )
        .unwrap();

        fs::write(dir.path().join("Procfile"), "web: node custom.js").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        // Procfile takes precedence
        assert_eq!(detection.start_command, Some("node custom.js".to_string()));
    }

    #[test]
    fn test_procfile_with_python() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("requirements.txt"), "gunicorn").unwrap();
        fs::write(
            dir.path().join("Procfile"),
            "web: gunicorn app:application --bind 0.0.0.0:$PORT",
        )
        .unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Python);
        assert!(detection.start_command.as_ref().unwrap().contains("gunicorn"));
    }
}

// ============================================================================
// Edge Cases and Error Handling
// ============================================================================

mod edge_cases {
    use super::*;

    #[test]
    fn test_empty_directory() {
        let dir = create_temp_dir();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::Unknown);
    }

    #[test]
    fn test_unknown_language_dockerfile_fails() {
        let detection = AppDetection {
            language: Language::Unknown,
            ..Default::default()
        };

        let result = Buildpack::generate_dockerfile(&detection);
        assert!(result.is_err());
    }

    #[test]
    fn test_malformed_package_json() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("package.json"), "not valid json").unwrap();

        let result = Buildpack::detect(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_package_json() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("package.json"), "{}").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::NodeJs);
        // Should have defaults
        assert_eq!(detection.version, Some("20".to_string()));
    }

    #[test]
    fn test_dockerfile_written_to_disk() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("package.json"), r#"{"name": "test"}"#).unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();
        let path = Buildpack::write_dockerfile(dir.path(), &detection).unwrap();

        assert!(path.exists());
        assert_eq!(path.file_name().unwrap(), ".spawngate.Dockerfile");

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("# Generated by spawngate buildpacks"));
        assert!(content.contains("FROM node:"));
    }
}

// ============================================================================
// Multi-language Detection Priority Tests
// ============================================================================

mod priority {
    use super::*;

    #[test]
    fn test_nodejs_over_static() {
        // Node.js should be detected over static when both package.json and index.html exist
        let dir = create_temp_dir();

        fs::write(dir.path().join("package.json"), r#"{"name": "app"}"#).unwrap();
        fs::write(dir.path().join("index.html"), "<html></html>").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();
        assert_eq!(detection.language, Language::NodeJs);
    }

    #[test]
    fn test_python_over_static() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("requirements.txt"), "flask").unwrap();
        fs::write(dir.path().join("index.html"), "<html></html>").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();
        assert_eq!(detection.language, Language::Python);
    }

    #[test]
    fn test_go_over_static() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("go.mod"), "module test\n\ngo 1.21").unwrap();
        fs::write(dir.path().join("index.html"), "<html></html>").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();
        assert_eq!(detection.language, Language::Go);
    }
}

// ============================================================================
// Real-world Project Structure Tests
// ============================================================================

mod real_world {
    use super::*;

    #[test]
    fn test_monorepo_with_nodejs_backend() {
        let dir = create_temp_dir();

        // Simulate a monorepo where the root has package.json
        fs::write(
            dir.path().join("package.json"),
            r#"{
                "name": "monorepo",
                "workspaces": ["packages/*"],
                "scripts": {
                    "start": "node packages/server/index.js"
                }
            }"#,
        )
        .unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();
        assert_eq!(detection.language, Language::NodeJs);
    }

    #[test]
    fn test_typescript_project() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("package.json"),
            r#"{
                "name": "ts-project",
                "main": "dist/index.js",
                "scripts": {
                    "build": "tsc",
                    "start": "node dist/index.js"
                },
                "devDependencies": {
                    "typescript": "^5.0.0"
                }
            }"#,
        )
        .unwrap();

        fs::write(
            dir.path().join("tsconfig.json"),
            r#"{"compilerOptions": {"outDir": "dist"}}"#,
        )
        .unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();

        assert_eq!(detection.language, Language::NodeJs);
        assert!(detection.build_command.is_some());
        assert_eq!(detection.entry_point, Some("dist/index.js".to_string()));
    }

    #[test]
    fn test_python_with_src_layout() {
        let dir = create_temp_dir();

        fs::write(dir.path().join("requirements.txt"), "flask").unwrap();
        fs::write(dir.path().join("pyproject.toml"), r#"
[project]
name = "my-app"
        "#).unwrap();

        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/app.py"), "").unwrap();

        let detection = Buildpack::detect(dir.path()).unwrap();
        assert_eq!(detection.language, Language::Python);
    }
}
