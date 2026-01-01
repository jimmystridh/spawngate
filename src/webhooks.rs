//! Webhooks and CI integration
//!
//! Provides webhook endpoints for GitHub and GitLab to enable
//! automatic deployments on push events.

use anyhow::{Context, Result};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use tracing::{debug, info, warn};

type HmacSha256 = Hmac<Sha256>;

/// Webhook configuration for an app
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Webhook secret for signature verification
    pub secret: String,
    /// Provider (github, gitlab, bitbucket)
    pub provider: WebhookProvider,
    /// Branch to deploy on push (default: main)
    pub deploy_branch: String,
    /// Whether auto-deploy is enabled
    pub auto_deploy: bool,
    /// GitHub/GitLab API token for status updates
    pub status_token: Option<String>,
    /// Repository full name (owner/repo)
    pub repo_name: Option<String>,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            secret: generate_webhook_secret(),
            provider: WebhookProvider::GitHub,
            deploy_branch: "main".to_string(),
            auto_deploy: true,
            status_token: None,
            repo_name: None,
        }
    }
}

/// Supported webhook providers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebhookProvider {
    GitHub,
    GitLab,
    Bitbucket,
    Generic,
}

impl std::fmt::Display for WebhookProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WebhookProvider::GitHub => write!(f, "github"),
            WebhookProvider::GitLab => write!(f, "gitlab"),
            WebhookProvider::Bitbucket => write!(f, "bitbucket"),
            WebhookProvider::Generic => write!(f, "generic"),
        }
    }
}

impl std::str::FromStr for WebhookProvider {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "github" => Ok(WebhookProvider::GitHub),
            "gitlab" => Ok(WebhookProvider::GitLab),
            "bitbucket" => Ok(WebhookProvider::Bitbucket),
            "generic" => Ok(WebhookProvider::Generic),
            _ => anyhow::bail!("Unknown webhook provider: {}", s),
        }
    }
}

/// Parsed webhook event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEvent {
    /// Event type (push, pull_request, etc.)
    pub event_type: String,
    /// Branch name
    pub branch: String,
    /// Commit SHA
    pub commit_sha: String,
    /// Commit message
    pub commit_message: Option<String>,
    /// Author name
    pub author: Option<String>,
    /// Repository name
    pub repo_name: String,
    /// Repository URL
    pub repo_url: Option<String>,
    /// Whether this should trigger a deploy
    pub should_deploy: bool,
}

/// GitHub push event payload
#[derive(Debug, Deserialize)]
pub struct GitHubPushEvent {
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub after: String,
    pub before: String,
    pub repository: GitHubRepository,
    pub pusher: GitHubPusher,
    pub head_commit: Option<GitHubCommit>,
    pub deleted: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubRepository {
    pub full_name: String,
    pub clone_url: String,
    pub ssh_url: String,
    pub default_branch: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubPusher {
    pub name: String,
    pub email: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubCommit {
    pub id: String,
    pub message: String,
    pub author: GitHubAuthor,
}

#[derive(Debug, Deserialize)]
pub struct GitHubAuthor {
    pub name: String,
    pub email: String,
}

/// GitLab push event payload
#[derive(Debug, Deserialize)]
pub struct GitLabPushEvent {
    pub object_kind: String,
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub after: String,
    pub before: String,
    pub project: GitLabProject,
    pub user_name: String,
    pub commits: Vec<GitLabCommit>,
}

#[derive(Debug, Deserialize)]
pub struct GitLabProject {
    pub path_with_namespace: String,
    pub git_http_url: String,
    pub git_ssh_url: String,
    pub default_branch: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GitLabCommit {
    pub id: String,
    pub message: String,
    pub author: GitLabAuthor,
}

#[derive(Debug, Deserialize)]
pub struct GitLabAuthor {
    pub name: String,
    pub email: String,
}

/// Webhook handler for parsing and validating webhooks
pub struct WebhookHandler {
    configs: HashMap<String, WebhookConfig>,
}

impl WebhookHandler {
    pub fn new() -> Self {
        Self {
            configs: HashMap::new(),
        }
    }

    /// Register a webhook configuration for an app
    pub fn register(&mut self, app_name: &str, config: WebhookConfig) {
        info!(app = app_name, provider = %config.provider, "Registered webhook");
        self.configs.insert(app_name.to_string(), config);
    }

    /// Remove webhook configuration for an app
    pub fn unregister(&mut self, app_name: &str) -> Option<WebhookConfig> {
        self.configs.remove(app_name)
    }

    /// Get webhook configuration for an app
    pub fn get_config(&self, app_name: &str) -> Option<&WebhookConfig> {
        self.configs.get(app_name)
    }

    /// Verify GitHub webhook signature
    pub fn verify_github_signature(&self, app_name: &str, payload: &[u8], signature: &str) -> bool {
        let config = match self.configs.get(app_name) {
            Some(c) => c,
            None => {
                warn!(app = app_name, "No webhook config found");
                return false;
            }
        };

        // GitHub signature format: sha256=<hex>
        let expected_prefix = "sha256=";
        if !signature.starts_with(expected_prefix) {
            warn!("Invalid GitHub signature format");
            return false;
        }

        let provided_sig = &signature[expected_prefix.len()..];

        match verify_hmac_sha256(&config.secret, payload, provided_sig) {
            Ok(valid) => {
                if !valid {
                    warn!(app = app_name, "GitHub signature verification failed");
                }
                valid
            }
            Err(e) => {
                warn!(app = app_name, error = %e, "Signature verification error");
                false
            }
        }
    }

    /// Verify GitLab webhook token
    pub fn verify_gitlab_token(&self, app_name: &str, token: &str) -> bool {
        let config = match self.configs.get(app_name) {
            Some(c) => c,
            None => {
                warn!(app = app_name, "No webhook config found");
                return false;
            }
        };

        // GitLab uses a simple token comparison
        let valid = constant_time_compare(&config.secret, token);
        if !valid {
            warn!(app = app_name, "GitLab token verification failed");
        }
        valid
    }

    /// Parse GitHub push event
    pub fn parse_github_push(&self, app_name: &str, payload: &[u8]) -> Result<WebhookEvent> {
        let event: GitHubPushEvent = serde_json::from_slice(payload)
            .context("Failed to parse GitHub push event")?;

        let config = self.configs.get(app_name);
        let deploy_branch = config
            .map(|c| c.deploy_branch.as_str())
            .unwrap_or("main");

        // Extract branch name from ref (refs/heads/main -> main)
        let branch = event.ref_name
            .strip_prefix("refs/heads/")
            .unwrap_or(&event.ref_name)
            .to_string();

        // Check if this is a branch deletion
        let is_deleted = event.deleted.unwrap_or(false) || event.after == "0000000000000000000000000000000000000000";

        let should_deploy = !is_deleted
            && branch == deploy_branch
            && config.map(|c| c.auto_deploy).unwrap_or(true);

        debug!(
            app = app_name,
            branch = %branch,
            commit = %event.after,
            should_deploy = should_deploy,
            "Parsed GitHub push event"
        );

        Ok(WebhookEvent {
            event_type: "push".to_string(),
            branch,
            commit_sha: event.after,
            commit_message: event.head_commit.as_ref().map(|c| c.message.clone()),
            author: Some(event.pusher.name),
            repo_name: event.repository.full_name,
            repo_url: Some(event.repository.clone_url),
            should_deploy,
        })
    }

    /// Parse GitLab push event
    pub fn parse_gitlab_push(&self, app_name: &str, payload: &[u8]) -> Result<WebhookEvent> {
        let event: GitLabPushEvent = serde_json::from_slice(payload)
            .context("Failed to parse GitLab push event")?;

        let config = self.configs.get(app_name);
        let deploy_branch = config
            .map(|c| c.deploy_branch.as_str())
            .unwrap_or("main");

        // Extract branch name from ref
        let branch = event.ref_name
            .strip_prefix("refs/heads/")
            .unwrap_or(&event.ref_name)
            .to_string();

        // Check if this is a branch deletion
        let is_deleted = event.after == "0000000000000000000000000000000000000000";

        let should_deploy = !is_deleted
            && branch == deploy_branch
            && config.map(|c| c.auto_deploy).unwrap_or(true);

        let commit_message = event.commits.first().map(|c| c.message.clone());

        debug!(
            app = app_name,
            branch = %branch,
            commit = %event.after,
            should_deploy = should_deploy,
            "Parsed GitLab push event"
        );

        Ok(WebhookEvent {
            event_type: "push".to_string(),
            branch,
            commit_sha: event.after,
            commit_message,
            author: Some(event.user_name),
            repo_name: event.project.path_with_namespace,
            repo_url: Some(event.project.git_http_url),
            should_deploy,
        })
    }

    /// Parse a generic webhook (just extracts basic info)
    pub fn parse_generic_push(&self, app_name: &str, payload: &[u8]) -> Result<WebhookEvent> {
        #[derive(Deserialize)]
        struct GenericPush {
            #[serde(rename = "ref", default)]
            ref_name: String,
            #[serde(default)]
            branch: String,
            #[serde(default)]
            commit: String,
            #[serde(alias = "sha", default)]
            commit_sha: String,
        }

        let event: GenericPush = serde_json::from_slice(payload)
            .context("Failed to parse generic push event")?;

        let branch = if !event.branch.is_empty() {
            event.branch
        } else {
            event.ref_name
                .strip_prefix("refs/heads/")
                .unwrap_or(&event.ref_name)
                .to_string()
        };

        let commit_sha = if !event.commit_sha.is_empty() {
            event.commit_sha
        } else {
            event.commit
        };

        let config = self.configs.get(app_name);
        let deploy_branch = config.map(|c| c.deploy_branch.as_str()).unwrap_or("main");
        let should_deploy = branch == deploy_branch && config.map(|c| c.auto_deploy).unwrap_or(true);

        Ok(WebhookEvent {
            event_type: "push".to_string(),
            branch,
            commit_sha,
            commit_message: None,
            author: None,
            repo_name: app_name.to_string(),
            repo_url: None,
            should_deploy,
        })
    }
}

impl Default for WebhookHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Build/deploy status for reporting back to CI
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeployStatus {
    Pending,
    Building,
    Success,
    Failure,
    Error,
}

impl DeployStatus {
    /// Convert to GitHub status state
    pub fn to_github_state(&self) -> &'static str {
        match self {
            DeployStatus::Pending | DeployStatus::Building => "pending",
            DeployStatus::Success => "success",
            DeployStatus::Failure => "failure",
            DeployStatus::Error => "error",
        }
    }

    /// Convert to GitLab pipeline status
    pub fn to_gitlab_state(&self) -> &'static str {
        match self {
            DeployStatus::Pending => "pending",
            DeployStatus::Building => "running",
            DeployStatus::Success => "success",
            DeployStatus::Failure => "failed",
            DeployStatus::Error => "failed",
        }
    }
}

/// Status notifier for sending build status updates
pub struct StatusNotifier {
    http_client: Option<reqwest::Client>,
}

impl StatusNotifier {
    pub fn new() -> Self {
        Self {
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .ok(),
        }
    }

    /// Send status update to GitHub
    pub async fn notify_github(
        &self,
        token: &str,
        repo: &str,
        sha: &str,
        status: DeployStatus,
        description: &str,
        target_url: Option<&str>,
    ) -> Result<()> {
        let client = self.http_client.as_ref()
            .context("HTTP client not available")?;

        let url = format!(
            "https://api.github.com/repos/{}/statuses/{}",
            repo, sha
        );

        let body = serde_json::json!({
            "state": status.to_github_state(),
            "description": description,
            "context": "paas/deploy",
            "target_url": target_url,
        });

        let response = client
            .post(&url)
            .header("Authorization", format!("token {}", token))
            .header("Accept", "application/vnd.github.v3+json")
            .header("User-Agent", "spawngate-paas")
            .json(&body)
            .send()
            .await
            .context("Failed to send GitHub status")?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("GitHub API error {}: {}", status, text);
        }

        info!(
            repo = repo,
            sha = %sha,
            status = ?status,
            "GitHub status updated"
        );

        Ok(())
    }

    /// Send status update to GitLab
    pub async fn notify_gitlab(
        &self,
        token: &str,
        project_id: &str,
        sha: &str,
        status: DeployStatus,
        description: &str,
        target_url: Option<&str>,
    ) -> Result<()> {
        let client = self.http_client.as_ref()
            .context("HTTP client not available")?;

        let url = format!(
            "https://gitlab.com/api/v4/projects/{}/statuses/{}",
            urlencoding::encode(project_id),
            sha
        );

        let mut params = vec![
            ("state", status.to_gitlab_state().to_string()),
            ("description", description.to_string()),
            ("name", "paas/deploy".to_string()),
        ];

        if let Some(url) = target_url {
            params.push(("target_url", url.to_string()));
        }

        let response = client
            .post(&url)
            .header("PRIVATE-TOKEN", token)
            .form(&params)
            .send()
            .await
            .context("Failed to send GitLab status")?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("GitLab API error {}: {}", status, text);
        }

        info!(
            project = project_id,
            sha = %sha,
            status = ?status,
            "GitLab status updated"
        );

        Ok(())
    }
}

impl Default for StatusNotifier {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate a build status badge SVG
pub fn generate_badge_svg(status: DeployStatus, _app_name: &str) -> String {
    let (status_text, color) = match status {
        DeployStatus::Pending => ("pending", "#dfb317"),
        DeployStatus::Building => ("building", "#007ec6"),
        DeployStatus::Success => ("passing", "#4c1"),
        DeployStatus::Failure => ("failing", "#e05d44"),
        DeployStatus::Error => ("error", "#9f9f9f"),
    };

    // Simple flat badge SVG
    let label_width = 40;
    let status_width = status_text.len() * 7 + 10;
    let total_width = label_width + status_width;

    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{total_width}" height="20">
  <linearGradient id="b" x2="0" y2="100%">
    <stop offset="0" stop-color="#bbb" stop-opacity=".1"/>
    <stop offset="1" stop-opacity=".1"/>
  </linearGradient>
  <clipPath id="a">
    <rect width="{total_width}" height="20" rx="3" fill="#fff"/>
  </clipPath>
  <g clip-path="url(#a)">
    <path fill="#555" d="M0 0h{label_width}v20H0z"/>
    <path fill="{color}" d="M{label_width} 0h{status_width}v20H{label_width}z"/>
    <path fill="url(#b)" d="M0 0h{total_width}v20H0z"/>
  </g>
  <g fill="#fff" text-anchor="middle" font-family="DejaVu Sans,Verdana,Geneva,sans-serif" font-size="11">
    <text x="{label_x}" y="15" fill="#010101" fill-opacity=".3">build</text>
    <text x="{label_x}" y="14">build</text>
    <text x="{status_x}" y="15" fill="#010101" fill-opacity=".3">{status_text}</text>
    <text x="{status_x}" y="14">{status_text}</text>
  </g>
</svg>"##,
        total_width = total_width,
        label_width = label_width,
        status_width = status_width,
        color = color,
        label_x = label_width / 2,
        status_x = label_width + status_width / 2,
        status_text = status_text,
    )
}

/// Generate a random webhook secret
pub fn generate_webhook_secret() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: [u8; 32] = rng.gen();
    hex::encode(bytes)
}

/// Verify HMAC-SHA256 signature
fn verify_hmac_sha256(secret: &str, payload: &[u8], signature_hex: &str) -> Result<bool> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .context("Invalid HMAC key")?;
    mac.update(payload);

    let expected = mac.finalize().into_bytes();
    let expected_hex = hex::encode(expected);

    Ok(constant_time_compare(&expected_hex, signature_hex))
}

/// Constant-time string comparison to prevent timing attacks
fn constant_time_compare(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut result = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        result |= x ^ y;
    }
    result == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webhook_provider_parsing() {
        assert_eq!("github".parse::<WebhookProvider>().unwrap(), WebhookProvider::GitHub);
        assert_eq!("GitLab".parse::<WebhookProvider>().unwrap(), WebhookProvider::GitLab);
        assert_eq!("BITBUCKET".parse::<WebhookProvider>().unwrap(), WebhookProvider::Bitbucket);
    }

    #[test]
    fn test_github_signature_verification() {
        let mut handler = WebhookHandler::new();
        handler.register("myapp", WebhookConfig {
            secret: "test-secret".to_string(),
            ..Default::default()
        });

        let payload = b"test payload";

        // Calculate expected signature
        let mut mac = HmacSha256::new_from_slice(b"test-secret").unwrap();
        mac.update(payload);
        let sig = hex::encode(mac.finalize().into_bytes());
        let signature = format!("sha256={}", sig);

        assert!(handler.verify_github_signature("myapp", payload, &signature));
        assert!(!handler.verify_github_signature("myapp", payload, "sha256=invalid"));
    }

    #[test]
    fn test_gitlab_token_verification() {
        let mut handler = WebhookHandler::new();
        handler.register("myapp", WebhookConfig {
            secret: "my-secret-token".to_string(),
            provider: WebhookProvider::GitLab,
            ..Default::default()
        });

        assert!(handler.verify_gitlab_token("myapp", "my-secret-token"));
        assert!(!handler.verify_gitlab_token("myapp", "wrong-token"));
    }

    #[test]
    fn test_parse_github_push() {
        let mut handler = WebhookHandler::new();
        handler.register("myapp", WebhookConfig::default());

        let payload = serde_json::json!({
            "ref": "refs/heads/main",
            "after": "abc123",
            "before": "def456",
            "repository": {
                "full_name": "owner/repo",
                "clone_url": "https://github.com/owner/repo.git",
                "ssh_url": "git@github.com:owner/repo.git"
            },
            "pusher": {
                "name": "test-user"
            },
            "head_commit": {
                "id": "abc123",
                "message": "Test commit",
                "author": {
                    "name": "Test User",
                    "email": "test@example.com"
                }
            }
        });

        let event = handler.parse_github_push("myapp", payload.to_string().as_bytes()).unwrap();
        assert_eq!(event.branch, "main");
        assert_eq!(event.commit_sha, "abc123");
        assert_eq!(event.repo_name, "owner/repo");
        assert!(event.should_deploy);
    }

    #[test]
    fn test_parse_github_push_feature_branch() {
        let mut handler = WebhookHandler::new();
        handler.register("myapp", WebhookConfig::default());

        let payload = serde_json::json!({
            "ref": "refs/heads/feature/new-thing",
            "after": "abc123",
            "before": "def456",
            "repository": {
                "full_name": "owner/repo",
                "clone_url": "https://github.com/owner/repo.git",
                "ssh_url": "git@github.com:owner/repo.git"
            },
            "pusher": {
                "name": "test-user"
            }
        });

        let event = handler.parse_github_push("myapp", payload.to_string().as_bytes()).unwrap();
        assert_eq!(event.branch, "feature/new-thing");
        assert!(!event.should_deploy); // Not main branch
    }

    #[test]
    fn test_parse_gitlab_push() {
        let mut handler = WebhookHandler::new();
        handler.register("myapp", WebhookConfig {
            provider: WebhookProvider::GitLab,
            ..Default::default()
        });

        let payload = serde_json::json!({
            "object_kind": "push",
            "ref": "refs/heads/main",
            "after": "abc123",
            "before": "def456",
            "project": {
                "path_with_namespace": "group/project",
                "git_http_url": "https://gitlab.com/group/project.git",
                "git_ssh_url": "git@gitlab.com:group/project.git"
            },
            "user_name": "Test User",
            "commits": [
                {
                    "id": "abc123",
                    "message": "Test commit",
                    "author": {
                        "name": "Test User",
                        "email": "test@example.com"
                    }
                }
            ]
        });

        let event = handler.parse_gitlab_push("myapp", payload.to_string().as_bytes()).unwrap();
        assert_eq!(event.branch, "main");
        assert_eq!(event.commit_sha, "abc123");
        assert_eq!(event.repo_name, "group/project");
        assert!(event.should_deploy);
    }

    #[test]
    fn test_deploy_status_conversions() {
        assert_eq!(DeployStatus::Pending.to_github_state(), "pending");
        assert_eq!(DeployStatus::Building.to_github_state(), "pending");
        assert_eq!(DeployStatus::Success.to_github_state(), "success");
        assert_eq!(DeployStatus::Failure.to_github_state(), "failure");

        assert_eq!(DeployStatus::Pending.to_gitlab_state(), "pending");
        assert_eq!(DeployStatus::Building.to_gitlab_state(), "running");
        assert_eq!(DeployStatus::Success.to_gitlab_state(), "success");
        assert_eq!(DeployStatus::Failure.to_gitlab_state(), "failed");
    }

    #[test]
    fn test_badge_generation() {
        let svg = generate_badge_svg(DeployStatus::Success, "myapp");
        assert!(svg.contains("passing"));
        assert!(svg.contains("#4c1")); // Green color

        let svg = generate_badge_svg(DeployStatus::Failure, "myapp");
        assert!(svg.contains("failing"));
        assert!(svg.contains("#e05d44")); // Red color
    }

    #[test]
    fn test_generate_webhook_secret() {
        let secret1 = generate_webhook_secret();
        let secret2 = generate_webhook_secret();

        assert_eq!(secret1.len(), 64); // 32 bytes = 64 hex chars
        assert_ne!(secret1, secret2);
    }

    #[test]
    fn test_constant_time_compare() {
        assert!(constant_time_compare("abc", "abc"));
        assert!(!constant_time_compare("abc", "abd"));
        assert!(!constant_time_compare("abc", "abcd"));
    }
}
