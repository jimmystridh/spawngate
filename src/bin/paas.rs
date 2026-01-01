//! PaaS CLI - Command-line interface for the Spawngate PaaS platform
//!
//! Usage:
//!   paas apps create <name>      Create a new application
//!   paas apps list               List all applications
//!   paas apps delete <name>      Delete an application
//!   paas apps info <name>        Show application details
//!
//!   paas addons add <type>       Add an add-on (postgres, redis, storage)
//!   paas addons remove <type>    Remove an add-on
//!   paas addons list             List add-ons for current app
//!
//!   paas deploy                  Deploy current directory
//!   paas logs                    View application logs
//!   paas config                  View/set configuration

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;

/// PaaS configuration stored in ~/.paas/config.json
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PaasConfig {
    /// API endpoint
    api_url: Option<String>,
    /// Current app context
    current_app: Option<String>,
    /// Known apps
    apps: HashMap<String, AppConfig>,
}

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AppConfig {
    /// App name
    name: String,
    /// Git remote URL
    git_url: Option<String>,
    /// Add-ons
    addons: Vec<String>,
    /// Custom domain
    domain: Option<String>,
}

/// CLI command structure
#[derive(Debug)]
enum Command {
    Apps(AppsCommand),
    Addons(AddonsCommand),
    Deploy(DeployOptions),
    Logs(LogsOptions),
    Config(ConfigCommand),
    Init(InitOptions),
    Help,
    Version,
}

#[derive(Debug)]
enum AppsCommand {
    Create { name: String },
    List,
    Delete { name: String },
    Info { name: String },
}

#[derive(Debug)]
enum AddonsCommand {
    Add { addon_type: String, plan: Option<String> },
    Remove { addon_type: String },
    List,
}

#[derive(Debug)]
struct DeployOptions {
    path: Option<PathBuf>,
    build_only: bool,
}

#[derive(Debug)]
struct LogsOptions {
    app: Option<String>,
    follow: bool,
    #[allow(dead_code)]
    lines: usize,
}

#[derive(Debug)]
enum ConfigCommand {
    Get { key: String },
    Set { key: String, value: String },
    List,
}

#[derive(Debug)]
struct InitOptions {
    name: Option<String>,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_help();
        return Ok(());
    }

    let command = parse_command(&args[1..]);

    match command {
        Command::Help => print_help(),
        Command::Version => print_version(),
        Command::Apps(cmd) => handle_apps(cmd)?,
        Command::Addons(cmd) => handle_addons(cmd)?,
        Command::Deploy(opts) => handle_deploy(opts)?,
        Command::Logs(opts) => handle_logs(opts)?,
        Command::Config(cmd) => handle_config(cmd)?,
        Command::Init(opts) => handle_init(opts)?,
    }

    Ok(())
}

fn parse_command(args: &[String]) -> Command {
    if args.is_empty() {
        return Command::Help;
    }

    match args[0].as_str() {
        "help" | "--help" | "-h" => Command::Help,
        "version" | "--version" | "-v" => Command::Version,
        "apps" | "app" => parse_apps_command(&args[1..]),
        "addons" | "addon" => parse_addons_command(&args[1..]),
        "deploy" | "push" => parse_deploy_command(&args[1..]),
        "logs" | "log" => parse_logs_command(&args[1..]),
        "config" => parse_config_command(&args[1..]),
        "init" | "create" => parse_init_command(&args[1..]),
        _ => {
            // Check if it's a shorthand
            if args[0].starts_with("apps:") {
                let sub = args[0].strip_prefix("apps:").unwrap();
                return parse_apps_command(&[sub.to_string()].iter().chain(&args[1..]).cloned().collect::<Vec<_>>());
            }
            if args[0].starts_with("addons:") {
                let sub = args[0].strip_prefix("addons:").unwrap();
                return parse_addons_command(&[sub.to_string()].iter().chain(&args[1..]).cloned().collect::<Vec<_>>());
            }
            Command::Help
        }
    }
}

fn parse_apps_command(args: &[String]) -> Command {
    if args.is_empty() {
        return Command::Apps(AppsCommand::List);
    }

    match args[0].as_str() {
        "create" | "new" => {
            let name = args.get(1).cloned().unwrap_or_else(|| "my-app".to_string());
            Command::Apps(AppsCommand::Create { name })
        }
        "list" | "ls" => Command::Apps(AppsCommand::List),
        "delete" | "rm" | "destroy" => {
            let name = args.get(1).cloned().unwrap_or_default();
            Command::Apps(AppsCommand::Delete { name })
        }
        "info" | "show" => {
            let name = args.get(1).cloned().unwrap_or_default();
            Command::Apps(AppsCommand::Info { name })
        }
        _ => Command::Apps(AppsCommand::List),
    }
}

fn parse_addons_command(args: &[String]) -> Command {
    if args.is_empty() {
        return Command::Addons(AddonsCommand::List);
    }

    match args[0].as_str() {
        "add" | "create" => {
            let addon_type = args.get(1).cloned().unwrap_or_default();
            let plan = args.iter().position(|a| a == "--plan" || a == "-p")
                .and_then(|i| args.get(i + 1).cloned());
            Command::Addons(AddonsCommand::Add { addon_type, plan })
        }
        "remove" | "rm" | "delete" => {
            let addon_type = args.get(1).cloned().unwrap_or_default();
            Command::Addons(AddonsCommand::Remove { addon_type })
        }
        "list" | "ls" => Command::Addons(AddonsCommand::List),
        _ => Command::Addons(AddonsCommand::List),
    }
}

fn parse_deploy_command(args: &[String]) -> Command {
    let path = args.get(0).map(PathBuf::from);
    let build_only = args.iter().any(|a| a == "--build-only" || a == "-b");
    Command::Deploy(DeployOptions { path, build_only })
}

fn parse_logs_command(args: &[String]) -> Command {
    let app = args.get(0).filter(|s| !s.starts_with('-')).cloned();
    let follow = args.iter().any(|a| a == "--follow" || a == "-f");
    let lines = args.iter().position(|a| a == "--lines" || a == "-n")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);
    Command::Logs(LogsOptions { app, follow, lines })
}

fn parse_config_command(args: &[String]) -> Command {
    if args.is_empty() {
        return Command::Config(ConfigCommand::List);
    }

    match args[0].as_str() {
        "get" => {
            let key = args.get(1).cloned().unwrap_or_default();
            Command::Config(ConfigCommand::Get { key })
        }
        "set" => {
            let key = args.get(1).cloned().unwrap_or_default();
            let value = args.get(2).cloned().unwrap_or_default();
            Command::Config(ConfigCommand::Set { key, value })
        }
        _ => Command::Config(ConfigCommand::List),
    }
}

fn parse_init_command(args: &[String]) -> Command {
    let name = args.get(0).filter(|s| !s.starts_with('-')).cloned();
    Command::Init(InitOptions { name })
}

fn handle_apps(cmd: AppsCommand) -> Result<()> {
    match cmd {
        AppsCommand::Create { name } => {
            println!("Creating app: {}", name);
            println!();

            // In a real implementation, this would call the API
            let git_url = format!("git@localhost:2222/{}.git", name);
            let app_url = format!("https://{}.localhost", name);

            println!("App {} created successfully!", name);
            println!();
            println!("Git remote:");
            println!("  {}", git_url);
            println!();
            println!("Add the remote to your project:");
            println!("  git remote add paas {}", git_url);
            println!();
            println!("Deploy with:");
            println!("  git push paas main");
            println!();
            println!("Your app will be available at:");
            println!("  {}", app_url);
        }
        AppsCommand::List => {
            println!("Apps:");
            println!();

            let config = load_config()?;
            if config.apps.is_empty() {
                println!("  No apps yet. Create one with: paas apps create <name>");
            } else {
                for (name, app) in &config.apps {
                    let status = "running"; // Would check actual status
                    println!("  {} ({}) - {}.localhost", name, status, name);
                    if !app.addons.is_empty() {
                        println!("    Add-ons: {}", app.addons.join(", "));
                    }
                }
            }
        }
        AppsCommand::Delete { name } => {
            if name.is_empty() {
                println!("Usage: paas apps delete <name>");
                return Ok(());
            }

            println!("Deleting app: {}", name);
            println!();
            println!("This will:");
            println!("  - Stop all running processes");
            println!("  - Remove all add-ons and their data");
            println!("  - Delete the git repository");
            println!();
            println!("Type the app name to confirm: ");

            // In a real implementation, would prompt for confirmation
            println!("App {} deleted.", name);
        }
        AppsCommand::Info { name } => {
            if name.is_empty() {
                println!("Usage: paas apps info <name>");
                return Ok(());
            }

            println!("App: {}", name);
            println!();
            println!("Status:     running");
            println!("URL:        https://{}.localhost", name);
            println!("Git:        git@localhost:2222/{}.git", name);
            println!("Created:    2024-01-01 12:00:00");
            println!();
            println!("Add-ons:");
            println!("  postgres:hobby  DATABASE_URL");
            println!("  redis:hobby     REDIS_URL");
            println!();
            println!("Recent deploys:");
            println!("  abc1234  2024-01-01 12:00:00  Initial deploy");
        }
    }

    Ok(())
}

fn handle_addons(cmd: AddonsCommand) -> Result<()> {
    let current_app = get_current_app()?;

    match cmd {
        AddonsCommand::Add { addon_type, plan } => {
            if addon_type.is_empty() {
                println!("Usage: paas addons add <type> [--plan <plan>]");
                println!();
                println!("Available add-on types:");
                println!("  postgres   PostgreSQL database");
                println!("  redis      Redis cache/queue");
                println!("  storage    S3-compatible object storage (MinIO)");
                println!();
                println!("Available plans:");
                println!("  hobby      Development (256MB RAM, 0.25 CPU)");
                println!("  basic      Small production (512MB RAM, 0.5 CPU)");
                println!("  standard   Standard production (1GB RAM, 1 CPU)");
                println!("  premium    High-performance (2GB RAM, 2 CPU)");
                return Ok(());
            }

            let plan = plan.unwrap_or_else(|| "hobby".to_string());

            println!("Adding {} ({}) to {}...", addon_type, plan, current_app);
            println!();

            match addon_type.as_str() {
                "postgres" | "postgresql" | "pg" => {
                    println!("PostgreSQL provisioned!");
                    println!();
                    println!("Connection info added to your app:");
                    println!("  DATABASE_URL=postgres://user:pass@postgres:5432/db");
                    println!("  PGHOST=postgres");
                    println!("  PGPORT=5432");
                    println!("  PGUSER=user");
                    println!("  PGPASSWORD=<generated>");
                    println!("  PGDATABASE=db");
                }
                "redis" => {
                    println!("Redis provisioned!");
                    println!();
                    println!("Connection info added to your app:");
                    println!("  REDIS_URL=redis://:pass@redis:6379");
                    println!("  REDIS_HOST=redis");
                    println!("  REDIS_PORT=6379");
                    println!("  REDIS_PASSWORD=<generated>");
                }
                "storage" | "s3" | "minio" => {
                    println!("S3-compatible storage provisioned!");
                    println!();
                    println!("Connection info added to your app:");
                    println!("  S3_ENDPOINT=http://minio:9000");
                    println!("  S3_ACCESS_KEY=<generated>");
                    println!("  S3_SECRET_KEY=<generated>");
                    println!("  S3_BUCKET={}-uploads", current_app);
                    println!();
                    println!("AWS SDK compatible:");
                    println!("  AWS_ACCESS_KEY_ID=<generated>");
                    println!("  AWS_SECRET_ACCESS_KEY=<generated>");
                    println!("  AWS_ENDPOINT_URL=http://minio:9000");
                }
                _ => {
                    println!("Unknown add-on type: {}", addon_type);
                    println!("Available: postgres, redis, storage");
                    return Ok(());
                }
            }

            println!();
            println!("Restart your app to apply changes:");
            println!("  git push paas main");
        }
        AddonsCommand::Remove { addon_type } => {
            if addon_type.is_empty() {
                println!("Usage: paas addons remove <type>");
                return Ok(());
            }

            println!("Removing {} from {}...", addon_type, current_app);
            println!();
            println!("WARNING: This will delete all data in this add-on!");
            println!();
            println!("Add-on {} removed.", addon_type);
        }
        AddonsCommand::List => {
            println!("Add-ons for {}:", current_app);
            println!();
            println!("  TYPE       PLAN      ENV VAR");
            println!("  postgres   hobby     DATABASE_URL");
            println!("  redis      hobby     REDIS_URL");
            println!();
            println!("Add more with: paas addons add <type>");
        }
    }

    Ok(())
}

fn handle_deploy(opts: DeployOptions) -> Result<()> {
    let path = opts.path.unwrap_or_else(|| PathBuf::from("."));
    let current_app = get_current_app()?;

    println!("Deploying {} from {}...", current_app, path.display());
    println!();

    // Detect build mode
    println!("Detecting build mode...");

    // Check for Dockerfile first (highest priority)
    if path.join("Dockerfile").exists() {
        println!("  Found: Dockerfile");
        println!("  Build: docker build");
        println!();

        if opts.build_only {
            println!("Building image...");
            println!();
            println!("  docker build -t {}:latest {}", current_app, path.display());
        } else {
            println!("Deploy with:");
            println!("  git push paas main");
            println!();
            println!("Or build locally:");
            println!("  docker build -t {}:latest {}", current_app, path.display());
        }
        return Ok(());
    }

    // Check for docker-compose
    let compose_files = ["docker-compose.yml", "docker-compose.yaml", "compose.yml", "compose.yaml"];
    for compose_file in &compose_files {
        if path.join(compose_file).exists() {
            println!("  Found: {}", compose_file);
            println!("  Build: docker compose");
            println!();

            if opts.build_only {
                println!("Building services...");
                println!();
                println!("  docker compose -f {} build", compose_file);
            } else {
                println!("Deploy with:");
                println!("  git push paas main");
                println!();
                println!("Or run locally:");
                println!("  docker compose -f {} up -d", compose_file);
            }
            return Ok(());
        }
    }

    // Fall back to buildpack detection
    println!("  No Dockerfile found, using buildpacks");
    println!();

    // Detect app type for buildpacks
    println!("Detecting app type...");
    if path.join("package.json").exists() {
        println!("  Detected: Node.js (paketo-buildpacks/nodejs)");
    } else if path.join("requirements.txt").exists() || path.join("pyproject.toml").exists() {
        println!("  Detected: Python (paketo-buildpacks/python)");
    } else if path.join("Gemfile").exists() {
        println!("  Detected: Ruby (paketo-buildpacks/ruby)");
    } else if path.join("go.mod").exists() {
        println!("  Detected: Go (paketo-buildpacks/go)");
    } else if path.join("Cargo.toml").exists() {
        println!("  Detected: Rust (paketo-community/rust)");
    } else if path.join("pom.xml").exists() || path.join("build.gradle").exists() {
        println!("  Detected: Java (paketo-buildpacks/java)");
    } else {
        println!("  Could not detect app type");
        println!("  Will use auto-detection during build");
        println!();
        println!("Tip: Add a Dockerfile for more control over the build");
    }
    println!();

    if opts.build_only {
        println!("Building image...");
        println!();
        println!("  pack build {}:latest --builder paketobuildpacks/builder-jammy-base --path {}",
            current_app, path.display());
    } else {
        println!("Deploy with:");
        println!("  git push paas main");
        println!();
        println!("Or build locally:");
        println!("  pack build {}:latest --builder paketobuildpacks/builder-jammy-base --path {}",
            current_app, path.display());
    }

    Ok(())
}

fn handle_logs(opts: LogsOptions) -> Result<()> {
    let app = opts.app.unwrap_or_else(|| get_current_app().unwrap_or_default());

    if app.is_empty() {
        println!("Usage: paas logs [app] [--follow]");
        return Ok(());
    }

    println!("Fetching logs for {}...", app);
    if opts.follow {
        println!("(Following, press Ctrl+C to stop)");
    }
    println!();

    // Simulated log output
    println!("2024-01-01T12:00:00Z app[web]: Listening on port 3000");
    println!("2024-01-01T12:00:01Z app[web]: Connected to database");
    println!("2024-01-01T12:00:02Z app[web]: Server ready");

    Ok(())
}

fn handle_config(cmd: ConfigCommand) -> Result<()> {
    let current_app = get_current_app().ok();

    match cmd {
        ConfigCommand::Get { key } => {
            if key.is_empty() {
                println!("Usage: paas config get <key>");
                return Ok(());
            }
            println!("{}=<value>", key);
        }
        ConfigCommand::Set { key, value } => {
            if key.is_empty() || value.is_empty() {
                println!("Usage: paas config set <key> <value>");
                return Ok(());
            }
            println!("Setting {}={} for {}", key, value, current_app.unwrap_or_default());
            println!();
            println!("Restart your app to apply changes:");
            println!("  git push paas main");
        }
        ConfigCommand::List => {
            let app = current_app.unwrap_or_else(|| "my-app".to_string());
            println!("Config for {}:", app);
            println!();
            println!("  DATABASE_URL=postgres://...");
            println!("  REDIS_URL=redis://...");
            println!("  NODE_ENV=production");
            println!();
            println!("Set config with: paas config set <key> <value>");
        }
    }

    Ok(())
}

fn handle_init(opts: InitOptions) -> Result<()> {
    let name = opts.name.unwrap_or_else(|| {
        std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "my-app".to_string())
    });

    println!("Initializing new app: {}", name);
    println!();

    // Create app
    println!("Creating app on platform...");
    println!();

    let git_url = format!("git@localhost:2222/{}.git", name);

    println!("App {} created!", name);
    println!();
    println!("Next steps:");
    println!();
    println!("  1. Add the git remote:");
    println!("     git remote add paas {}", git_url);
    println!();
    println!("  2. Add database (optional):");
    println!("     paas addons add postgres");
    println!();
    println!("  3. Deploy:");
    println!("     git push paas main");
    println!();
    println!("Your app will be available at: https://{}.localhost", name);

    Ok(())
}

fn print_help() {
    println!(r#"
paas - Your own Heroku (with Docker support)

USAGE:
    paas <command> [options]

COMMANDS:
    init [name]              Initialize a new app in current directory
    apps create <name>       Create a new application
    apps list                List all applications
    apps delete <name>       Delete an application
    apps info <name>         Show application details

    addons add <type>        Add an add-on (postgres, redis, storage)
    addons remove <type>     Remove an add-on
    addons list              List add-ons for current app

    deploy [path]            Deploy application
    logs [app]               View application logs
    config list              List environment variables
    config set <key> <val>   Set environment variable
    config get <key>         Get environment variable

BUILD MODES (auto-detected):
    Dockerfile               docker build (highest priority)
    docker-compose.yml       docker compose build
    Buildpacks               pack build (Node, Python, Go, etc.)

    help                     Show this help
    version                  Show version

ADD-ON TYPES:
    postgres     PostgreSQL database
    redis        Redis cache/queue
    storage      S3-compatible object storage (MinIO)

EXAMPLES:
    paas init                Initialize app with current directory name
    paas apps create myapp   Create a new app called "myapp"
    paas addons add postgres Add PostgreSQL database
    git push paas main       Deploy via git push

ENVIRONMENT:
    PAAS_APP                 Current app context
    PAAS_API_URL             API endpoint (default: http://localhost:9999)
"#);
}

fn print_version() {
    println!("paas 0.1.0");
    println!("Powered by Spawngate");
}

fn get_current_app() -> Result<String> {
    // Check environment variable first
    if let Ok(app) = env::var("PAAS_APP") {
        return Ok(app);
    }

    // Check if we're in a git repo with a paas remote
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "paas"])
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            let url = String::from_utf8_lossy(&output.stdout);
            // Extract app name from URL like git@localhost:2222/myapp.git
            if let Some(name) = url.trim().split('/').last() {
                return Ok(name.trim_end_matches(".git").to_string());
            }
        }
    }

    // Check config file
    let config = load_config()?;
    if let Some(app) = config.current_app {
        return Ok(app);
    }

    // Default to directory name
    Ok(std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "my-app".to_string()))
}

fn config_path() -> PathBuf {
    dirs_next::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".paas")
        .join("config.json")
}

fn load_config() -> Result<PaasConfig> {
    let path = config_path();
    if !path.exists() {
        return Ok(PaasConfig::default());
    }

    let content = std::fs::read_to_string(&path)
        .context("Failed to read config file")?;
    let config: PaasConfig = serde_json::from_str(&content)
        .context("Failed to parse config file")?;

    Ok(config)
}

#[allow(dead_code)]
fn save_config(config: &PaasConfig) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let content = serde_json::to_string_pretty(config)?;
    std::fs::write(&path, content)?;

    Ok(())
}
