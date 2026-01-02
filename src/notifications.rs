//! Notification system for sending alerts via email and webhooks
//!
//! Supports multiple notification channels:
//! - Email (SMTP)
//! - Webhooks (HTTP POST)
//! - Slack (via webhook)
//!
//! Notifications are queued in the database and processed asynchronously.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info};

use crate::db::{AlertEventRecord, AlertNotificationRecord, AlertRuleRecord, Database};

/// Notification channel types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelType {
    Email,
    Webhook,
    Slack,
}

impl ChannelType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "email" => Some(Self::Email),
            "webhook" => Some(Self::Webhook),
            "slack" => Some(Self::Slack),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Webhook => "webhook",
            Self::Slack => "slack",
        }
    }
}

/// Configuration for a notification channel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationChannelConfig {
    pub id: String,
    pub name: String,
    pub channel_type: ChannelType,
    pub enabled: bool,
    pub config: ChannelSettings,
}

/// Channel-specific settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChannelSettings {
    Email(EmailSettings),
    Webhook(WebhookSettings),
    Slack(SlackSettings),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailSettings {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    pub smtp_tls: bool,
    pub from_address: String,
    pub to_addresses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSettings {
    pub url: String,
    pub method: String,
    pub headers: Option<std::collections::HashMap<String, String>>,
    pub secret: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackSettings {
    pub webhook_url: String,
    pub channel: Option<String>,
    pub username: Option<String>,
    pub icon_emoji: Option<String>,
}

/// Notification payload sent to channels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationPayload {
    pub alert_id: String,
    pub alert_name: String,
    pub app_name: Option<String>,
    pub severity: String,
    pub status: String,
    pub message: String,
    pub metric_type: String,
    pub metric_value: f64,
    pub threshold: f64,
    pub condition: String,
    pub started_at: String,
    pub resolved_at: Option<String>,
    pub timestamp: String,
}

impl NotificationPayload {
    pub fn from_event(event: &AlertEventRecord, rule: &AlertRuleRecord) -> Self {
        Self {
            alert_id: rule.id.clone(),
            alert_name: rule.name.clone(),
            app_name: rule.app_name.clone(),
            severity: rule.severity.clone(),
            status: event.status.clone(),
            message: event.message.clone().unwrap_or_else(|| format!(
                "{} {} {} (current: {:.2})",
                rule.metric_type, rule.condition, rule.threshold, event.metric_value
            )),
            metric_type: rule.metric_type.clone(),
            metric_value: event.metric_value,
            threshold: event.threshold,
            condition: rule.condition.clone(),
            started_at: event.started_at.clone(),
            resolved_at: event.resolved_at.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// Notification sender handles delivery of notifications
pub struct NotificationSender {
    db: Arc<Database>,
    http_client: reqwest::Client,
}

impl NotificationSender {
    pub fn new(db: Arc<Database>) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self { db, http_client }
    }

    /// Process all pending notifications
    pub async fn process_pending(&self) -> Result<ProcessResult> {
        let pending = self.db.get_pending_notifications()?;
        let total = pending.len();
        let mut sent = 0;
        let mut failed = 0;

        for notification in pending {
            match self.send_notification(&notification).await {
                Ok(()) => {
                    self.db.update_notification_status(notification.id, "sent", None)?;
                    sent += 1;
                    info!(
                        notification_id = notification.id,
                        channel = notification.channel,
                        "Notification sent successfully"
                    );
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    self.db.update_notification_status(
                        notification.id,
                        "failed",
                        Some(&error_msg),
                    )?;
                    failed += 1;
                    error!(
                        notification_id = notification.id,
                        channel = notification.channel,
                        error = %e,
                        "Failed to send notification"
                    );
                }
            }
        }

        Ok(ProcessResult { total, sent, failed })
    }

    /// Send a single notification
    async fn send_notification(&self, notification: &AlertNotificationRecord) -> Result<()> {
        // Get the event and rule for context
        let event = self.db.get_alert_event(notification.alert_event_id)?
            .ok_or_else(|| anyhow!("Alert event not found"))?;

        let rule = self.db.get_alert_rule(&event.rule_id)?
            .ok_or_else(|| anyhow!("Alert rule not found"))?;

        let payload = NotificationPayload::from_event(&event, &rule);

        // Get channel configuration
        let channel_config = self.db.get_notification_channel(&notification.channel)?;

        match channel_config {
            Some(config) => {
                let settings: ChannelSettings = serde_json::from_str(&config.config)?;
                match settings {
                    ChannelSettings::Email(email) => self.send_email(&payload, &email).await,
                    ChannelSettings::Webhook(webhook) => self.send_webhook(&payload, &webhook).await,
                    ChannelSettings::Slack(slack) => self.send_slack(&payload, &slack).await,
                }
            }
            None => {
                // Try to infer channel type from the channel name
                match ChannelType::from_str(&notification.channel) {
                    Some(ChannelType::Webhook) => {
                        // Use channel as URL directly if it looks like a URL
                        if notification.channel.starts_with("http") {
                            let settings = WebhookSettings {
                                url: notification.channel.clone(),
                                method: "POST".to_string(),
                                headers: None,
                                secret: None,
                            };
                            self.send_webhook(&payload, &settings).await
                        } else {
                            Err(anyhow!("Unknown notification channel: {}", notification.channel))
                        }
                    }
                    _ => Err(anyhow!("Unknown notification channel: {}", notification.channel)),
                }
            }
        }
    }

    /// Send email notification
    async fn send_email(&self, payload: &NotificationPayload, settings: &EmailSettings) -> Result<()> {
        use lettre::{
            message::{header::ContentType, Mailbox},
            transport::smtp::authentication::Credentials,
            AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
        };

        let subject = format!(
            "[{}] {} - {}",
            payload.severity.to_uppercase(),
            payload.alert_name,
            payload.status
        );

        let body = self.format_email_body(payload);

        let from: Mailbox = settings.from_address.parse()
            .map_err(|e| anyhow!("Invalid from address: {}", e))?;

        for to_addr in &settings.to_addresses {
            let to: Mailbox = to_addr.parse()
                .map_err(|e| anyhow!("Invalid to address {}: {}", to_addr, e))?;

            let email = Message::builder()
                .from(from.clone())
                .to(to)
                .subject(&subject)
                .header(ContentType::TEXT_PLAIN)
                .body(body.clone())
                .map_err(|e| anyhow!("Failed to build email: {}", e))?;

            let mut transport_builder = if settings.smtp_tls {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&settings.smtp_host)
                    .map_err(|e| anyhow!("Failed to create SMTP transport: {}", e))?
            } else {
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&settings.smtp_host)
            };

            transport_builder = transport_builder.port(settings.smtp_port);

            if let (Some(username), Some(password)) = (&settings.smtp_username, &settings.smtp_password) {
                transport_builder = transport_builder.credentials(Credentials::new(
                    username.clone(),
                    password.clone(),
                ));
            }

            let transport = transport_builder.build();
            transport.send(email).await
                .map_err(|e| anyhow!("Failed to send email to {}: {}", to_addr, e))?;

            debug!(to = to_addr, "Email sent successfully");
        }

        Ok(())
    }

    fn format_email_body(&self, payload: &NotificationPayload) -> String {
        let app_info = payload.app_name.as_ref()
            .map(|a| format!("App: {}\n", a))
            .unwrap_or_default();

        format!(
            r#"Alert: {}
Status: {}
Severity: {}
{}
Metric: {} {} {}
Current Value: {:.2}
Threshold: {:.2}

Message: {}

Started: {}
{}
Timestamp: {}

---
Spawngate PaaS Alert System
"#,
            payload.alert_name,
            payload.status,
            payload.severity,
            app_info,
            payload.metric_type,
            payload.condition,
            payload.threshold,
            payload.metric_value,
            payload.threshold,
            payload.message,
            payload.started_at,
            payload.resolved_at.as_ref()
                .map(|r| format!("Resolved: {}\n", r))
                .unwrap_or_default(),
            payload.timestamp
        )
    }

    /// Send webhook notification
    async fn send_webhook(&self, payload: &NotificationPayload, settings: &WebhookSettings) -> Result<()> {
        let mut request = match settings.method.to_uppercase().as_str() {
            "POST" => self.http_client.post(&settings.url),
            "PUT" => self.http_client.put(&settings.url),
            _ => return Err(anyhow!("Unsupported HTTP method: {}", settings.method)),
        };

        // Add custom headers
        if let Some(headers) = &settings.headers {
            for (key, value) in headers {
                request = request.header(key, value);
            }
        }

        // Add signature if secret is configured
        if let Some(secret) = &settings.secret {
            let body = serde_json::to_string(payload)?;
            let signature = self.compute_hmac_signature(secret, &body);
            request = request.header("X-Signature-256", format!("sha256={}", signature));
        }

        let response = request
            .header("Content-Type", "application/json")
            .header("User-Agent", "Spawngate-Notifications/1.0")
            .json(payload)
            .send()
            .await
            .map_err(|e| anyhow!("Webhook request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("Webhook returned error: {} - {}", status, body));
        }

        debug!(url = settings.url, "Webhook notification sent");
        Ok(())
    }

    /// Send Slack notification
    async fn send_slack(&self, payload: &NotificationPayload, settings: &SlackSettings) -> Result<()> {
        let color = match payload.severity.as_str() {
            "critical" => "#dc2626",
            "warning" => "#f59e0b",
            _ => "#3b82f6",
        };

        let status_emoji = match payload.status.as_str() {
            "firing" => ":fire:",
            "resolved" => ":white_check_mark:",
            _ => ":bell:",
        };

        let app_field = payload.app_name.as_ref().map(|app| {
            serde_json::json!({
                "title": "App",
                "value": app,
                "short": true
            })
        });

        let mut fields = vec![
            serde_json::json!({
                "title": "Status",
                "value": format!("{} {}", status_emoji, payload.status),
                "short": true
            }),
            serde_json::json!({
                "title": "Severity",
                "value": payload.severity,
                "short": true
            }),
            serde_json::json!({
                "title": "Metric",
                "value": format!("{} {} {}", payload.metric_type, payload.condition, payload.threshold),
                "short": true
            }),
            serde_json::json!({
                "title": "Current Value",
                "value": format!("{:.2}", payload.metric_value),
                "short": true
            }),
        ];

        if let Some(app) = app_field {
            fields.insert(0, app);
        }

        let slack_payload = serde_json::json!({
            "channel": settings.channel,
            "username": settings.username.as_deref().unwrap_or("Spawngate Alerts"),
            "icon_emoji": settings.icon_emoji.as_deref().unwrap_or(":rotating_light:"),
            "attachments": [{
                "color": color,
                "title": payload.alert_name,
                "text": payload.message,
                "fields": fields,
                "footer": "Spawngate PaaS",
                "ts": chrono::Utc::now().timestamp()
            }]
        });

        let response = self.http_client
            .post(&settings.webhook_url)
            .header("Content-Type", "application/json")
            .json(&slack_payload)
            .send()
            .await
            .map_err(|e| anyhow!("Slack webhook request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("Slack webhook returned error: {} - {}", status, body));
        }

        debug!(channel = ?settings.channel, "Slack notification sent");
        Ok(())
    }

    fn compute_hmac_signature(&self, secret: &str, body: &str) -> String {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        type HmacSha256 = Hmac<Sha256>;

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(body.as_bytes());
        let result = mac.finalize();
        hex::encode(result.into_bytes())
    }

    /// Queue notifications for an alert event
    pub fn queue_notifications_for_event(&self, event_id: i64, channels: &[String]) -> Result<Vec<i64>> {
        let mut notification_ids = Vec::new();
        for channel in channels {
            let id = self.db.create_alert_notification(event_id, channel)?;
            notification_ids.push(id);
            debug!(event_id, channel, notification_id = id, "Notification queued");
        }
        Ok(notification_ids)
    }
}

/// Result of processing notifications
#[derive(Debug, Clone)]
pub struct ProcessResult {
    pub total: usize,
    pub sent: usize,
    pub failed: usize,
}

/// Notification channel record from database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationChannelRecord {
    pub id: String,
    pub name: String,
    pub channel_type: String,
    pub config: String,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_type_from_str() {
        assert_eq!(ChannelType::from_str("email"), Some(ChannelType::Email));
        assert_eq!(ChannelType::from_str("webhook"), Some(ChannelType::Webhook));
        assert_eq!(ChannelType::from_str("slack"), Some(ChannelType::Slack));
        assert_eq!(ChannelType::from_str("unknown"), None);
    }

    #[test]
    fn test_channel_type_as_str() {
        assert_eq!(ChannelType::Email.as_str(), "email");
        assert_eq!(ChannelType::Webhook.as_str(), "webhook");
        assert_eq!(ChannelType::Slack.as_str(), "slack");
    }

    #[test]
    fn test_notification_payload_serialization() {
        let payload = NotificationPayload {
            alert_id: "alert-123".to_string(),
            alert_name: "High CPU".to_string(),
            app_name: Some("my-app".to_string()),
            severity: "warning".to_string(),
            status: "firing".to_string(),
            message: "CPU usage above threshold".to_string(),
            metric_type: "cpu_usage".to_string(),
            metric_value: 85.5,
            threshold: 80.0,
            condition: ">".to_string(),
            started_at: "2024-01-01T00:00:00Z".to_string(),
            resolved_at: None,
            timestamp: "2024-01-01T00:01:00Z".to_string(),
        };

        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("High CPU"));
        assert!(json.contains("my-app"));
    }
}
