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
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;

/// Default API URL
const DEFAULT_API_URL: &str = "http://127.0.0.1:9999";

/// PaaS configuration stored in ~/.paas/config.json
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PaasConfig {
    /// API endpoint
    api_url: Option<String>,
    /// API token
    api_token: Option<String>,
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

/// API response wrapper
#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    success: bool,
    data: Option<T>,
    error: Option<String>,
}

/// App data from API
#[derive(Debug, Deserialize, Serialize)]
struct App {
    name: String,
    status: String,
    git_url: Option<String>,
    image: Option<String>,
    port: u16,
    env: HashMap<String, String>,
    addons: Vec<String>,
    created_at: String,
    deployed_at: Option<String>,
    commit: Option<String>,
    #[serde(default = "default_scale")]
    scale: i32,
}

fn default_scale() -> i32 {
    1
}

/// Add-on instance from API
#[derive(Debug, Deserialize)]
struct AddonInstance {
    id: String,
    addon_type: String,
    plan: String,
    app_name: String,
    container_name: String,
    connection_url: String,
    env_var_name: String,
    status: String,
}

/// Build result from API
#[derive(Debug, Deserialize)]
struct BuildResult {
    success: bool,
    image: String,
    duration_secs: f64,
    logs: Vec<String>,
    error: Option<String>,
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
    Scale(ScaleOptions),
    Ps(PsOptions),
    Restart(RestartOptions),
    Secrets(SecretsCommand),
    Webhooks(WebhooksCommand),
    Help,
    Version,
}

#[derive(Debug)]
struct RestartOptions {
    /// App name (defaults to current app)
    app: Option<String>,
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
    #[allow(dead_code)]
    path: Option<PathBuf>,
    #[allow(dead_code)]
    build_only: bool,
    clear_cache: bool,
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
    ApiUrl { url: Option<String> },
    ApiToken { token: Option<String> },
}

#[derive(Debug)]
enum SecretsCommand {
    List,
    Set { key: String, value: String },
    Delete { key: String },
    Audit,
    Rotate,
}

#[derive(Debug)]
enum WebhooksCommand {
    Show,
    Enable {
        provider: Option<String>,
        branch: Option<String>,
        repo: Option<String>,
    },
    Disable,
    Events,
}

#[derive(Debug)]
struct InitOptions {
    name: Option<String>,
}

#[derive(Debug)]
struct ScaleOptions {
    /// Process type and count, e.g., "web=3"
    formations: Vec<(String, i32)>,
    /// Just show current scale without changing
    show_only: bool,
}

#[derive(Debug)]
struct PsOptions {
    /// App name (defaults to current app)
    app: Option<String>,
}

/// Process info from API
#[derive(Debug, Deserialize)]
struct ProcessInfo {
    id: String,
    process_type: String,
    status: String,
    container_id: Option<String>,
    port: Option<i32>,
    started_at: String,
    health_status: Option<String>,
}

/// Simple HTTP client for API calls
struct ApiClient {
    base_url: String,
    token: String,
}

impl ApiClient {
    fn new() -> Result<Self> {
        let config = load_config()?;
        let base_url = config.api_url
            .or_else(|| env::var("PAAS_API_URL").ok())
            .unwrap_or_else(|| DEFAULT_API_URL.to_string());
        let token = config.api_token
            .or_else(|| env::var("PAAS_API_TOKEN").ok())
            .unwrap_or_else(|| "changeme".to_string());

        Ok(Self { base_url, token })
    }

    fn request(&self, method: &str, path: &str, body: Option<&str>) -> Result<String> {
        // Parse URL
        let url = format!("{}{}", self.base_url, path);
        let url = url.strip_prefix("http://").unwrap_or(&url);
        let (host_port, path) = if let Some(idx) = url.find('/') {
            (&url[..idx], &url[idx..])
        } else {
            (url, "/")
        };

        // Connect
        let mut stream = TcpStream::connect(host_port)
            .context(format!("Failed to connect to API at {}", self.base_url))?;

        stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;

        // Build request
        let body_bytes = body.unwrap_or("");
        let request = format!(
            "{} {} HTTP/1.1\r\n\
             Host: {}\r\n\
             Authorization: Bearer {}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n\
             {}",
            method, path, host_port, self.token, body_bytes.len(), body_bytes
        );

        // Send request
        stream.write_all(request.as_bytes())?;
        stream.flush()?;

        // Read response
        let mut response = String::new();
        stream.read_to_string(&mut response)?;

        // Parse response - find body after headers
        if let Some(idx) = response.find("\r\n\r\n") {
            let body = &response[idx + 4..];
            Ok(body.to_string())
        } else {
            Ok(response)
        }
    }

    fn get(&self, path: &str) -> Result<String> {
        self.request("GET", path, None)
    }

    fn post(&self, path: &str, body: &str) -> Result<String> {
        self.request("POST", path, Some(body))
    }

    fn delete(&self, path: &str) -> Result<String> {
        self.request("DELETE", path, None)
    }

    fn put(&self, path: &str, body: &str) -> Result<String> {
        self.request("PUT", path, Some(body))
    }
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
        Command::Scale(opts) => handle_scale(opts)?,
        Command::Ps(opts) => handle_ps(opts)?,
        Command::Restart(opts) => handle_restart(opts)?,
        Command::Secrets(cmd) => handle_secrets(cmd)?,
        Command::Webhooks(cmd) => handle_webhooks(cmd)?,
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
        "scale" => parse_scale_command(&args[1..]),
        "ps" | "processes" => parse_ps_command(&args[1..]),
        "restart" => parse_restart_command(&args[1..]),
        "secrets" | "secret" => parse_secrets_command(&args[1..]),
        "webhooks" | "webhook" => parse_webhooks_command(&args[1..]),
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
            if args[0].starts_with("secrets:") {
                let sub = args[0].strip_prefix("secrets:").unwrap();
                return parse_secrets_command(&[sub.to_string()].iter().chain(&args[1..]).cloned().collect::<Vec<_>>());
            }
            if args[0].starts_with("webhooks:") {
                let sub = args[0].strip_prefix("webhooks:").unwrap();
                return parse_webhooks_command(&[sub.to_string()].iter().chain(&args[1..]).cloned().collect::<Vec<_>>());
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
    let path = args.iter()
        .find(|a| !a.starts_with('-'))
        .map(PathBuf::from);
    let build_only = args.iter().any(|a| a == "--build-only" || a == "-b");
    let clear_cache = args.iter().any(|a| a == "--no-cache" || a == "--clear-cache");
    Command::Deploy(DeployOptions { path, build_only, clear_cache })
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
        "api-url" | "api_url" => {
            let url = args.get(1).cloned();
            Command::Config(ConfigCommand::ApiUrl { url })
        }
        "api-token" | "api_token" | "token" => {
            let token = args.get(1).cloned();
            Command::Config(ConfigCommand::ApiToken { token })
        }
        _ => Command::Config(ConfigCommand::List),
    }
}

fn parse_init_command(args: &[String]) -> Command {
    let name = args.get(0).filter(|s| !s.starts_with('-')).cloned();
    Command::Init(InitOptions { name })
}

fn parse_scale_command(args: &[String]) -> Command {
    if args.is_empty() {
        return Command::Scale(ScaleOptions {
            formations: vec![],
            show_only: true,
        });
    }

    let mut formations = Vec::new();
    for arg in args {
        if arg.starts_with('-') {
            continue;
        }
        // Parse "web=3" format
        if let Some(idx) = arg.find('=') {
            let process_type = arg[..idx].to_string();
            if let Ok(count) = arg[idx + 1..].parse::<i32>() {
                formations.push((process_type, count));
            }
        }
    }

    Command::Scale(ScaleOptions {
        formations,
        show_only: false,
    })
}

fn parse_ps_command(args: &[String]) -> Command {
    let app = args.get(0).filter(|s| !s.starts_with('-')).cloned();
    Command::Ps(PsOptions { app })
}

fn parse_restart_command(args: &[String]) -> Command {
    let app = args.get(0).filter(|s| !s.starts_with('-')).cloned();
    Command::Restart(RestartOptions { app })
}

fn parse_secrets_command(args: &[String]) -> Command {
    if args.is_empty() {
        return Command::Secrets(SecretsCommand::List);
    }

    match args[0].as_str() {
        "list" | "ls" => Command::Secrets(SecretsCommand::List),
        "set" | "add" => {
            let key = args.get(1).cloned().unwrap_or_default();
            let value = args.get(2).cloned().unwrap_or_default();
            Command::Secrets(SecretsCommand::Set { key, value })
        }
        "delete" | "rm" | "remove" | "unset" => {
            let key = args.get(1).cloned().unwrap_or_default();
            Command::Secrets(SecretsCommand::Delete { key })
        }
        "audit" | "log" | "logs" => Command::Secrets(SecretsCommand::Audit),
        "rotate" | "rotate-key" => Command::Secrets(SecretsCommand::Rotate),
        _ => Command::Secrets(SecretsCommand::List),
    }
}

fn parse_webhooks_command(args: &[String]) -> Command {
    if args.is_empty() {
        return Command::Webhooks(WebhooksCommand::Show);
    }

    match args[0].as_str() {
        "show" | "info" | "status" => Command::Webhooks(WebhooksCommand::Show),
        "enable" | "create" | "add" => {
            let mut provider = None;
            let mut branch = None;
            let mut repo = None;

            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--provider" | "-p" => {
                        provider = args.get(i + 1).cloned();
                        i += 2;
                    }
                    "--branch" | "-b" => {
                        branch = args.get(i + 1).cloned();
                        i += 2;
                    }
                    "--repo" | "-r" => {
                        repo = args.get(i + 1).cloned();
                        i += 2;
                    }
                    _ => i += 1,
                }
            }

            Command::Webhooks(WebhooksCommand::Enable { provider, branch, repo })
        }
        "disable" | "delete" | "remove" => Command::Webhooks(WebhooksCommand::Disable),
        "events" | "log" | "logs" | "history" => Command::Webhooks(WebhooksCommand::Events),
        _ => Command::Webhooks(WebhooksCommand::Show),
    }
}

fn handle_apps(cmd: AppsCommand) -> Result<()> {
    let client = ApiClient::new()?;

    match cmd {
        AppsCommand::Create { name } => {
            println!("Creating app: {}", name);

            let body = serde_json::json!({
                "name": name,
                "port": 3000
            });

            let response = client.post("/apps", &body.to_string())?;
            let result: ApiResponse<App> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            if result.success {
                if let Some(app) = result.data {
                    println!();
                    println!("App {} created successfully!", app.name);
                    println!();
                    if let Some(git_url) = &app.git_url {
                        println!("Git remote:");
                        println!("  {}", git_url);
                        println!();
                        println!("Add the remote to your project:");
                        println!("  git remote add paas {}", git_url);
                    }
                    println!();
                    println!("Deploy with:");
                    println!("  git push paas main");
                    println!();
                    println!("Your app will be available at:");
                    println!("  https://{}.localhost", app.name);

                    // Save to local config
                    let mut config = load_config()?;
                    config.apps.insert(app.name.clone(), AppConfig {
                        name: app.name.clone(),
                        git_url: app.git_url,
                        addons: app.addons,
                        domain: None,
                    });
                    config.current_app = Some(app.name);
                    save_config(&config)?;
                }
            } else {
                println!("Failed to create app: {}", result.error.unwrap_or_default());
            }
        }
        AppsCommand::List => {
            let response = client.get("/apps")?;
            let result: ApiResponse<Vec<App>> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            println!("Apps:");
            println!();

            if result.success {
                if let Some(apps) = result.data {
                    if apps.is_empty() {
                        println!("  No apps yet. Create one with: paas apps create <name>");
                    } else {
                        for app in apps {
                            println!("  {} ({}) - {}.localhost",
                                app.name,
                                app.status,
                                app.name
                            );
                            if !app.addons.is_empty() {
                                println!("    Add-ons: {}", app.addons.join(", "));
                            }
                        }
                    }
                }
            } else {
                // API might not be running, fall back to local config
                let config = load_config()?;
                if config.apps.is_empty() {
                    println!("  No apps yet. Create one with: paas apps create <name>");
                    println!();
                    println!("  Note: API not reachable at {}", client.base_url);
                } else {
                    for (name, app) in &config.apps {
                        println!("  {} (unknown) - {}.localhost", name, name);
                        if !app.addons.is_empty() {
                            println!("    Add-ons: {}", app.addons.join(", "));
                        }
                    }
                    println!();
                    println!("  Note: Status unknown - API not reachable");
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

            // Prompt for confirmation
            print!("Type the app name to confirm: ");
            std::io::stdout().flush()?;
            let mut confirmation = String::new();
            std::io::stdin().read_line(&mut confirmation)?;

            if confirmation.trim() != name {
                println!("Aborted - name did not match");
                return Ok(());
            }

            let response = client.delete(&format!("/apps/{}", name))?;
            let result: ApiResponse<()> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            if result.success {
                println!("App {} deleted.", name);

                // Remove from local config
                let mut config = load_config()?;
                config.apps.remove(&name);
                if config.current_app.as_ref() == Some(&name) {
                    config.current_app = None;
                }
                save_config(&config)?;
            } else {
                println!("Failed to delete app: {}", result.error.unwrap_or_default());
            }
        }
        AppsCommand::Info { name } => {
            let app_name = if name.is_empty() {
                get_current_app()?
            } else {
                name
            };

            let response = client.get(&format!("/apps/{}", app_name))?;
            let result: ApiResponse<App> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            if result.success {
                if let Some(app) = result.data {
                    println!("App: {}", app.name);
                    println!();
                    println!("Status:     {}", app.status);
                    println!("URL:        https://{}.localhost", app.name);
                    if let Some(git_url) = &app.git_url {
                        println!("Git:        {}", git_url);
                    }
                    println!("Port:       {}", app.port);
                    println!("Created:    {}", app.created_at);
                    if let Some(deployed) = &app.deployed_at {
                        println!("Deployed:   {}", deployed);
                    }
                    if let Some(commit) = &app.commit {
                        println!("Commit:     {}", commit);
                    }
                    if let Some(image) = &app.image {
                        println!("Image:      {}", image);
                    }

                    if !app.addons.is_empty() {
                        println!();
                        println!("Add-ons:");
                        for addon in &app.addons {
                            println!("  {}", addon);
                        }
                    }

                    if !app.env.is_empty() {
                        println!();
                        println!("Config:");
                        for (key, _) in &app.env {
                            println!("  {}=<set>", key);
                        }
                    }
                }
            } else {
                println!("App not found: {}", app_name);
            }
        }
    }

    Ok(())
}

fn handle_addons(cmd: AddonsCommand) -> Result<()> {
    let client = ApiClient::new()?;
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

            let body = serde_json::json!({
                "type": addon_type,
                "plan": plan
            });

            let response = client.post(&format!("/apps/{}/addons", current_app), &body.to_string())?;
            let result: ApiResponse<AddonInstance> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            if result.success {
                if let Some(addon) = result.data {
                    println!();
                    println!("{} provisioned!", addon.addon_type);
                    println!();
                    println!("Container:  {}", addon.container_name);
                    println!("Status:     {}", addon.status);
                    println!();
                    println!("Connection info added to your app:");
                    println!("  {}={}", addon.env_var_name, addon.connection_url);
                    println!();
                    println!("Restart your app to apply changes:");
                    println!("  git push paas main");
                }
            } else {
                println!("Failed to add add-on: {}", result.error.unwrap_or_default());
            }
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

            print!("Type 'yes' to confirm: ");
            std::io::stdout().flush()?;
            let mut confirmation = String::new();
            std::io::stdin().read_line(&mut confirmation)?;

            if confirmation.trim() != "yes" {
                println!("Aborted");
                return Ok(());
            }

            let response = client.delete(&format!("/apps/{}/addons/{}", current_app, addon_type))?;
            let result: ApiResponse<()> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            if result.success {
                println!("Add-on {} removed.", addon_type);
            } else {
                println!("Failed to remove add-on: {}", result.error.unwrap_or_default());
            }
        }
        AddonsCommand::List => {
            let response = client.get(&format!("/apps/{}/addons", current_app))?;
            let result: ApiResponse<Vec<AddonInstance>> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            println!("Add-ons for {}:", current_app);
            println!();

            if result.success {
                if let Some(addons) = result.data {
                    if addons.is_empty() {
                        println!("  No add-ons attached.");
                        println!();
                        println!("Add one with: paas addons add <type>");
                    } else {
                        println!("  TYPE       PLAN      STATUS     ENV VAR");
                        for addon in addons {
                            println!("  {:10} {:9} {:10} {}",
                                addon.addon_type,
                                addon.plan,
                                addon.status,
                                addon.env_var_name
                            );
                        }
                    }
                }
            } else {
                println!("  Failed to fetch add-ons: {}", result.error.unwrap_or_default());
            }
        }
    }

    Ok(())
}

fn handle_deploy(opts: DeployOptions) -> Result<()> {
    let client = ApiClient::new()?;
    let current_app = get_current_app()?;

    println!("Deploying {}...", current_app);
    println!();

    let body = serde_json::json!({
        "clear_cache": opts.clear_cache
    });

    let response = client.post(&format!("/apps/{}/deploy", current_app), &body.to_string())?;
    let result: ApiResponse<BuildResult> = serde_json::from_str(&response)
        .context("Failed to parse API response")?;

    if result.success {
        if let Some(build) = result.data {
            if build.success {
                println!("Build successful!");
                println!();
                println!("Image:    {}", build.image);
                println!("Duration: {:.1}s", build.duration_secs);
                println!();
                println!("Build logs:");
                for log in build.logs.iter().take(20) {
                    println!("  {}", log);
                }
                if build.logs.len() > 20 {
                    println!("  ... ({} more lines)", build.logs.len() - 20);
                }
            } else {
                println!("Build failed!");
                println!();
                if let Some(error) = build.error {
                    println!("Error: {}", error);
                }
                println!();
                println!("Build logs:");
                for log in build.logs.iter().rev().take(20).collect::<Vec<_>>().into_iter().rev() {
                    println!("  {}", log);
                }
            }
        }
    } else {
        println!("Deployment failed: {}", result.error.unwrap_or_default());
        println!();
        println!("Make sure to push your code first:");
        println!("  git push paas main");
    }

    Ok(())
}

fn handle_logs(opts: LogsOptions) -> Result<()> {
    let client = ApiClient::new()?;
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

    let response = client.get(&format!("/apps/{}/logs", app))?;
    let result: ApiResponse<Vec<String>> = serde_json::from_str(&response)
        .context("Failed to parse API response")?;

    if result.success {
        if let Some(logs) = result.data {
            if logs.is_empty() {
                println!("No logs available yet.");
            } else {
                for log in logs {
                    println!("{}", log);
                }
            }
        }
    } else {
        println!("Failed to fetch logs: {}", result.error.unwrap_or_default());
    }

    Ok(())
}

fn handle_config(cmd: ConfigCommand) -> Result<()> {
    let mut config = load_config()?;

    match cmd {
        ConfigCommand::Get { key } => {
            if key.is_empty() {
                println!("Usage: paas config get <key>");
                return Ok(());
            }

            let client = ApiClient::new()?;
            let current_app = get_current_app()?;

            let response = client.get(&format!("/apps/{}/config", current_app))?;
            let result: ApiResponse<HashMap<String, String>> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            if result.success {
                if let Some(env) = result.data {
                    if let Some(value) = env.get(&key) {
                        println!("{}={}", key, value);
                    } else {
                        println!("Key not found: {}", key);
                    }
                }
            } else {
                println!("Failed to fetch config: {}", result.error.unwrap_or_default());
            }
        }
        ConfigCommand::Set { key, value } => {
            if key.is_empty() {
                println!("Usage: paas config set <key> <value>");
                return Ok(());
            }

            let client = ApiClient::new()?;
            let current_app = get_current_app()?;

            let body = serde_json::json!({
                "env": {
                    key.clone(): value.clone()
                }
            });

            let response = client.put(&format!("/apps/{}/config", current_app), &body.to_string())?;
            let result: ApiResponse<()> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            if result.success {
                if value.is_empty() {
                    println!("Removed {} from {}", key, current_app);
                } else {
                    println!("Set {}={} for {}", key, value, current_app);
                }
                println!();
                println!("Restart your app to apply changes:");
                println!("  git push paas main");
            } else {
                println!("Failed to set config: {}", result.error.unwrap_or_default());
            }
        }
        ConfigCommand::List => {
            let client = ApiClient::new()?;
            let current_app = get_current_app().ok();

            if let Some(app) = current_app {
                let response = client.get(&format!("/apps/{}/config", app))?;
                let result: ApiResponse<HashMap<String, String>> = serde_json::from_str(&response)
                    .unwrap_or(ApiResponse { success: false, data: None, error: Some("API error".to_string()) });

                println!("Config for {}:", app);
                println!();

                if result.success {
                    if let Some(env) = result.data {
                        if env.is_empty() {
                            println!("  No config set.");
                        } else {
                            for (key, value) in &env {
                                // Mask sensitive values
                                let display_value = if key.contains("PASSWORD") || key.contains("SECRET") || key.contains("KEY") {
                                    "<hidden>".to_string()
                                } else if value.len() > 50 {
                                    format!("{}...", &value[..50])
                                } else {
                                    value.clone()
                                };
                                println!("  {}={}", key, display_value);
                            }
                        }
                    }
                } else {
                    println!("  Failed to fetch config from API");
                }
            } else {
                println!("No app selected. Use: paas apps info <name>");
            }

            println!();
            println!("CLI Config:");
            println!("  API URL:   {}", config.api_url.as_deref().unwrap_or(DEFAULT_API_URL));
            println!("  API Token: {}", if config.api_token.is_some() { "<set>" } else { "<not set>" });
        }
        ConfigCommand::ApiUrl { url } => {
            if let Some(url) = url {
                config.api_url = Some(url.clone());
                save_config(&config)?;
                println!("API URL set to: {}", url);
            } else {
                println!("Current API URL: {}", config.api_url.as_deref().unwrap_or(DEFAULT_API_URL));
            }
        }
        ConfigCommand::ApiToken { token } => {
            if let Some(token) = token {
                config.api_token = Some(token);
                save_config(&config)?;
                println!("API token updated");
            } else {
                println!("Current API token: {}", if config.api_token.is_some() { "<set>" } else { "<not set>" });
            }
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

    // Create the app via API
    let client = ApiClient::new()?;
    let body = serde_json::json!({
        "name": name,
        "port": 3000
    });

    let response = client.post("/apps", &body.to_string())?;
    let result: ApiResponse<App> = serde_json::from_str(&response)
        .context("Failed to parse API response")?;

    if result.success {
        if let Some(app) = result.data {
            println!("App {} created!", app.name);
            println!();

            // Save to local config
            let mut config = load_config()?;
            config.apps.insert(app.name.clone(), AppConfig {
                name: app.name.clone(),
                git_url: app.git_url.clone(),
                addons: app.addons,
                domain: None,
            });
            config.current_app = Some(app.name.clone());
            save_config(&config)?;

            println!("Next steps:");
            println!();
            if let Some(git_url) = &app.git_url {
                println!("  1. Add the git remote:");
                println!("     git remote add paas {}", git_url);
            }
            println!();
            println!("  2. Add database (optional):");
            println!("     paas addons add postgres");
            println!();
            println!("  3. Deploy:");
            println!("     git push paas main");
            println!();
            println!("Your app will be available at: https://{}.localhost", app.name);
        }
    } else {
        let error = result.error.unwrap_or_else(|| "Unknown error".to_string());
        if error.contains("already exists") {
            println!("App {} already exists. Connecting to it...", name);

            let mut config = load_config()?;
            config.current_app = Some(name.clone());
            save_config(&config)?;

            println!();
            println!("Connected to existing app: {}", name);
        } else {
            println!("Failed to create app: {}", error);
            println!();
            println!("Make sure the PaaS API is running:");
            println!("  spawngate --api-port 9999 config.toml");
        }
    }

    Ok(())
}

fn handle_scale(opts: ScaleOptions) -> Result<()> {
    let client = ApiClient::new()?;
    let current_app = get_current_app()?;

    if opts.show_only || opts.formations.is_empty() {
        // Just show current scale
        let response = client.get(&format!("/apps/{}", current_app))?;
        let result: ApiResponse<App> = serde_json::from_str(&response)
            .context("Failed to parse API response")?;

        if result.success {
            if let Some(app) = result.data {
                println!("Formation for {}:", current_app);
                println!();
                println!("  web: {}", app.scale);
                println!();
                println!("Use 'paas scale web=N' to change dyno count");
            }
        } else {
            println!("Failed to get app info: {}", result.error.unwrap_or_default());
        }
    } else {
        // Set new scale
        for (process_type, count) in &opts.formations {
            if *count < 0 {
                println!("Error: scale count cannot be negative");
                return Ok(());
            }
            if *count > 100 {
                println!("Error: scale count cannot exceed 100");
                return Ok(());
            }

            println!("Scaling {} to {} {}...", current_app, count, process_type);

            let body = serde_json::json!({
                "scale": count
            });

            let response = client.post(&format!("/apps/{}/scale", current_app), &body.to_string())?;
            let result: ApiResponse<()> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            if result.success {
                println!();
                if *count == 0 {
                    println!("App {} scaled down to 0 dynos (idle)", current_app);
                    println!("The app will start when it receives traffic");
                } else {
                    println!("App {} scaled to {} {} {}",
                        current_app,
                        count,
                        process_type,
                        if *count == 1 { "dyno" } else { "dynos" }
                    );
                }
            } else {
                println!("Failed to scale: {}", result.error.unwrap_or_default());
            }
        }
    }

    Ok(())
}

fn handle_ps(opts: PsOptions) -> Result<()> {
    let client = ApiClient::new()?;
    let app = opts.app.unwrap_or_else(|| get_current_app().unwrap_or_default());

    if app.is_empty() {
        println!("Usage: paas ps [app]");
        return Ok(());
    }

    println!("Processes for {}:", app);
    println!();

    let response = client.get(&format!("/apps/{}/processes", app))?;
    let result: ApiResponse<Vec<ProcessInfo>> = serde_json::from_str(&response)
        .context("Failed to parse API response")?;

    if result.success {
        if let Some(processes) = result.data {
            if processes.is_empty() {
                println!("  No processes running.");
                println!();
                println!("  Your app may be idle. Send a request to wake it up.");
            } else {
                println!("  ID                 TYPE   STATUS    HEALTH    PORT");
                println!("  ──────────────────────────────────────────────────");
                for proc in &processes {
                    let health = proc.health_status.as_deref().unwrap_or("unknown");
                    let port = proc.port.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string());
                    println!("  {:18} {:6} {:9} {:9} {}",
                        &proc.id[..std::cmp::min(18, proc.id.len())],
                        proc.process_type,
                        proc.status,
                        health,
                        port
                    );
                }
                println!();
                println!("  Total: {} running", processes.len());
            }
        }
    } else {
        println!("Failed to get processes: {}", result.error.unwrap_or_default());
    }

    Ok(())
}

/// Rolling deploy result from API
#[derive(Debug, Deserialize)]
struct RollingDeployResult {
    app_name: String,
    total_dynos: usize,
    successful: usize,
    failed: usize,
}

fn handle_restart(opts: RestartOptions) -> Result<()> {
    let client = ApiClient::new()?;
    let app = opts.app.unwrap_or_else(|| get_current_app().unwrap_or_default());

    if app.is_empty() {
        println!("Usage: paas restart [app]");
        return Ok(());
    }

    println!("Restarting all dynos for {}...", app);
    println!();

    let response = client.post(&format!("/apps/{}/restart", app), "{}")?;
    let result: ApiResponse<RollingDeployResult> = serde_json::from_str(&response)
        .context("Failed to parse API response")?;

    if result.success {
        if let Some(deploy) = result.data {
            if deploy.total_dynos == 0 {
                println!("No running dynos to restart.");
                println!("Scale up your app with 'paas scale web=1'");
            } else if deploy.failed == 0 {
                println!("✓ Rolling deploy complete!");
                println!("  {} {} restarted successfully",
                    deploy.successful,
                    if deploy.successful == 1 { "dyno" } else { "dynos" }
                );
            } else {
                println!("⚠ Rolling deploy completed with errors");
                println!("  Successful: {}", deploy.successful);
                println!("  Failed: {}", deploy.failed);
            }
        }
    } else {
        println!("Failed to restart: {}", result.error.unwrap_or_default());
    }

    Ok(())
}

/// Response type for secrets list
#[derive(Debug, Deserialize)]
struct SecretsListResponse {
    app: String,
    secrets: Vec<String>,
    count: usize,
}

/// Response type for secrets set
#[derive(Debug, Deserialize)]
struct SecretsSetResponse {
    app: String,
    secrets_set: usize,
}

/// Audit log entry
#[derive(Debug, Deserialize)]
struct SecretAuditEntry {
    id: i64,
    app_name: String,
    secret_key: String,
    action: String,
    actor: Option<String>,
    #[allow(dead_code)]
    ip_address: Option<String>,
    created_at: String,
}

/// Key rotation response
#[derive(Debug, Deserialize)]
struct KeyRotationResponse {
    old_key_id: String,
    new_key_id: String,
    secrets_re_encrypted: usize,
}

fn handle_secrets(cmd: SecretsCommand) -> Result<()> {
    let client = ApiClient::new()?;
    let current_app = get_current_app()?;

    match cmd {
        SecretsCommand::List => {
            println!("Secrets for {}:", current_app);
            println!();

            let response = client.get(&format!("/apps/{}/secrets", current_app))?;
            let result: ApiResponse<SecretsListResponse> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            if result.success {
                if let Some(data) = result.data {
                    if data.secrets.is_empty() {
                        println!("  No secrets configured.");
                        println!();
                        println!("Use 'paas secrets set <KEY> <VALUE>' to add a secret.");
                    } else {
                        for key in &data.secrets {
                            println!("  {} = <encrypted>", key);
                        }
                        println!();
                        println!("{} secret(s) configured", data.count);
                    }
                }
            } else {
                println!("Failed to list secrets: {}", result.error.unwrap_or_default());
            }
        }
        SecretsCommand::Set { key, value } => {
            if key.is_empty() || value.is_empty() {
                println!("Usage: paas secrets set <KEY> <VALUE>");
                println!();
                println!("Example: paas secrets set DATABASE_PASSWORD mysecretpassword");
                return Ok(());
            }

            let body = serde_json::json!({
                "secrets": {
                    key.clone(): value
                }
            });

            let response = client.post(&format!("/apps/{}/secrets", current_app), &body.to_string())?;
            let result: ApiResponse<SecretsSetResponse> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            if result.success {
                println!("✓ Secret '{}' set for {}", key, current_app);
                println!();
                println!("The value is encrypted at rest.");
                println!("Restart your app to apply the changes:");
                println!("  paas restart");
            } else {
                println!("Failed to set secret: {}", result.error.unwrap_or_default());
            }
        }
        SecretsCommand::Delete { key } => {
            if key.is_empty() {
                println!("Usage: paas secrets delete <KEY>");
                return Ok(());
            }

            let response = client.delete(&format!("/apps/{}/secrets/{}", current_app, key))?;
            let result: ApiResponse<()> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            if result.success {
                println!("✓ Secret '{}' deleted from {}", key, current_app);
                println!();
                println!("Restart your app to apply the changes:");
                println!("  paas restart");
            } else {
                println!("Failed to delete secret: {}", result.error.unwrap_or_default());
            }
        }
        SecretsCommand::Audit => {
            println!("Secrets audit log for {}:", current_app);
            println!();

            let response = client.get(&format!("/apps/{}/secrets/audit", current_app))?;
            let result: ApiResponse<Vec<SecretAuditEntry>> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            if result.success {
                if let Some(entries) = result.data {
                    if entries.is_empty() {
                        println!("  No audit log entries.");
                    } else {
                        println!("  {:<20} {:<15} {:<10} {}", "Time", "Key", "Action", "Actor");
                        println!("  {}", "-".repeat(60));
                        for entry in entries {
                            println!("  {:<20} {:<15} {:<10} {}",
                                entry.created_at,
                                entry.secret_key,
                                entry.action,
                                entry.actor.unwrap_or_else(|| "-".to_string())
                            );
                        }
                    }
                }
            } else {
                println!("Failed to fetch audit log: {}", result.error.unwrap_or_default());
            }
        }
        SecretsCommand::Rotate => {
            println!("Rotating encryption key...");
            println!();
            println!("WARNING: This will re-encrypt all secrets with a new key.");
            println!("Press Ctrl+C to cancel or wait 3 seconds to continue...");
            std::thread::sleep(std::time::Duration::from_secs(3));

            let response = client.post("/secrets/rotate", "{}")?;
            let result: ApiResponse<KeyRotationResponse> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            if result.success {
                if let Some(data) = result.data {
                    println!("✓ Key rotation complete!");
                    println!();
                    println!("  Old key ID: {}...", &data.old_key_id[..8]);
                    println!("  New key ID: {}...", &data.new_key_id[..8]);
                    println!("  Secrets re-encrypted: {}", data.secrets_re_encrypted);
                }
            } else {
                println!("Failed to rotate key: {}", result.error.unwrap_or_default());
            }
        }
    }

    Ok(())
}

/// Response for webhook config
#[derive(Debug, Deserialize)]
struct WebhookConfigResponse {
    enabled: bool,
    provider: Option<String>,
    deploy_branch: Option<String>,
    auto_deploy: Option<bool>,
    webhook_url: Option<String>,
    has_status_token: Option<bool>,
}

/// Response for webhook creation
#[derive(Debug, Deserialize)]
struct WebhookCreateResponse {
    webhook_url: String,
    secret: String,
    provider: String,
    deploy_branch: String,
}

/// Webhook event
#[derive(Debug, Deserialize)]
struct WebhookEventResponse {
    #[allow(dead_code)]
    id: Option<i64>,
    event_type: String,
    provider: String,
    branch: Option<String>,
    commit_sha: Option<String>,
    commit_message: Option<String>,
    triggered_deploy: bool,
    created_at: Option<String>,
}

fn handle_webhooks(cmd: WebhooksCommand) -> Result<()> {
    let client = ApiClient::new()?;
    let current_app = get_current_app()?;

    match cmd {
        WebhooksCommand::Show => {
            println!("Webhook configuration for {}:", current_app);
            println!();

            let response = client.get(&format!("/apps/{}/webhook", current_app))?;
            let result: ApiResponse<WebhookConfigResponse> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            if result.success {
                if let Some(config) = result.data {
                    if config.enabled {
                        println!("  Status:        Enabled ✓");
                        println!("  Provider:      {}", config.provider.unwrap_or_default());
                        println!("  Deploy branch: {}", config.deploy_branch.unwrap_or_default());
                        println!("  Auto-deploy:   {}", if config.auto_deploy.unwrap_or(false) { "Yes" } else { "No" });
                        println!("  Status token:  {}", if config.has_status_token.unwrap_or(false) { "Configured" } else { "Not set" });
                        println!();
                        println!("Webhook URL:");
                        println!("  {}", config.webhook_url.unwrap_or_default());
                        println!();
                        println!("Add this URL to your GitHub/GitLab repository settings.");
                    } else {
                        println!("  Webhooks not configured.");
                        println!();
                        println!("Enable webhooks with:");
                        println!("  paas webhooks enable");
                    }
                }
            } else {
                println!("Failed to fetch webhook config: {}", result.error.unwrap_or_default());
            }
        }
        WebhooksCommand::Enable { provider, branch, repo } => {
            println!("Enabling webhooks for {}...", current_app);
            println!();

            let body = serde_json::json!({
                "provider": provider.unwrap_or_else(|| "github".to_string()),
                "deploy_branch": branch.unwrap_or_else(|| "main".to_string()),
                "auto_deploy": true,
                "repo_name": repo,
            });

            let response = client.post(&format!("/apps/{}/webhook", current_app), &body.to_string())?;
            let result: ApiResponse<WebhookCreateResponse> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            if result.success {
                if let Some(config) = result.data {
                    println!("✓ Webhooks enabled!");
                    println!();
                    println!("Configuration:");
                    println!("  Provider:      {}", config.provider);
                    println!("  Deploy branch: {}", config.deploy_branch);
                    println!();
                    println!("Webhook URL:");
                    println!("  {}", config.webhook_url);
                    println!();
                    println!("Webhook Secret (save this, shown only once):");
                    println!("  {}", config.secret);
                    println!();
                    println!("Add the URL and secret to your repository settings.");
                }
            } else {
                println!("Failed to enable webhooks: {}", result.error.unwrap_or_default());
            }
        }
        WebhooksCommand::Disable => {
            println!("Disabling webhooks for {}...", current_app);

            let response = client.delete(&format!("/apps/{}/webhook", current_app))?;
            let result: ApiResponse<()> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            if result.success {
                println!("✓ Webhooks disabled");
                println!();
                println!("Remember to remove the webhook from your repository settings.");
            } else {
                println!("Failed to disable webhooks: {}", result.error.unwrap_or_default());
            }
        }
        WebhooksCommand::Events => {
            println!("Recent webhook events for {}:", current_app);
            println!();

            let response = client.get(&format!("/apps/{}/webhook/events", current_app))?;
            let result: ApiResponse<Vec<WebhookEventResponse>> = serde_json::from_str(&response)
                .context("Failed to parse API response")?;

            if result.success {
                if let Some(events) = result.data {
                    if events.is_empty() {
                        println!("  No webhook events yet.");
                    } else {
                        println!("  {:<20} {:<10} {:<12} {:<8} {}", "Time", "Provider", "Branch", "Deploy", "Commit");
                        println!("  {}", "-".repeat(75));
                        for event in events {
                            let deploy_status = if event.triggered_deploy { "✓" } else { "-" };
                            let commit = event.commit_sha
                                .as_ref()
                                .map(|s| &s[..7.min(s.len())])
                                .unwrap_or("-");
                            println!("  {:<20} {:<10} {:<12} {:<8} {}",
                                event.created_at.as_deref().unwrap_or("-"),
                                event.provider,
                                event.branch.as_deref().unwrap_or("-"),
                                deploy_status,
                                commit,
                            );
                        }
                    }
                }
            } else {
                println!("Failed to fetch events: {}", result.error.unwrap_or_default());
            }
        }
    }

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

    scale [type=N ...]       Scale app processes (e.g., web=3)
    ps                       List running processes
    restart [app]            Rolling restart all dynos

    deploy [path]            Deploy application
    logs [app]               View application logs

    config list              List environment variables
    config set <key> <val>   Set environment variable
    config get <key>         Get environment variable
    config api-url [url]     Set/get API URL
    config api-token [token] Set/get API token

    secrets list             List secret keys (values hidden)
    secrets set <key> <val>  Set encrypted secret
    secrets delete <key>     Delete a secret
    secrets audit            View secrets audit log
    secrets rotate           Rotate encryption key

    webhooks                 Show webhook configuration
    webhooks enable          Enable GitHub/GitLab webhooks
    webhooks disable         Disable webhooks
    webhooks events          Show recent webhook events

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
    paas scale web=3         Scale to 3 web dynos
    paas ps                  Show running processes
    git push paas main       Deploy via git push

ENVIRONMENT:
    PAAS_APP                 Current app context
    PAAS_API_URL             API endpoint (default: http://localhost:9999)
    PAAS_API_TOKEN           API authentication token
"#);
}

fn print_version() {
    println!("paas {}", env!("CARGO_PKG_VERSION"));
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

fn save_config(config: &PaasConfig) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let content = serde_json::to_string_pretty(config)?;
    std::fs::write(&path, content)?;

    Ok(())
}
