//! Alert evaluation engine for monitoring metrics and triggering alerts
//!
//! This module provides:
//! - Alert rule evaluation against metrics data
//! - Condition checking (>, <, >=, <=, ==, !=)
//! - Duration-based alert triggering
//! - Alert lifecycle management (firing, resolved)

use std::sync::Arc;
use anyhow::Result;
use tracing::{debug, info, warn};

use crate::db::{Database, AlertRuleRecord, RequestMetricsRecord, ResourceMetricsRecord};

/// Supported metric types for alerting
#[derive(Debug, Clone, PartialEq)]
pub enum MetricType {
    /// Error rate percentage (error_count / request_count * 100)
    ErrorRate,
    /// Average response time in milliseconds
    ResponseTime,
    /// P95 response time in milliseconds
    ResponseTimeP95,
    /// P99 response time in milliseconds
    ResponseTimeP99,
    /// CPU usage percentage
    CpuUsage,
    /// Memory usage percentage (used / limit * 100)
    MemoryUsage,
    /// Request rate (requests per minute)
    RequestRate,
}

impl MetricType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "error_rate" => Some(MetricType::ErrorRate),
            "response_time" => Some(MetricType::ResponseTime),
            "response_time_p95" => Some(MetricType::ResponseTimeP95),
            "response_time_p99" => Some(MetricType::ResponseTimeP99),
            "cpu_usage" => Some(MetricType::CpuUsage),
            "memory_usage" => Some(MetricType::MemoryUsage),
            "request_rate" => Some(MetricType::RequestRate),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            MetricType::ErrorRate => "error_rate",
            MetricType::ResponseTime => "response_time",
            MetricType::ResponseTimeP95 => "response_time_p95",
            MetricType::ResponseTimeP99 => "response_time_p99",
            MetricType::CpuUsage => "cpu_usage",
            MetricType::MemoryUsage => "memory_usage",
            MetricType::RequestRate => "request_rate",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            MetricType::ErrorRate => "Error Rate",
            MetricType::ResponseTime => "Response Time",
            MetricType::ResponseTimeP95 => "Response Time (P95)",
            MetricType::ResponseTimeP99 => "Response Time (P99)",
            MetricType::CpuUsage => "CPU Usage",
            MetricType::MemoryUsage => "Memory Usage",
            MetricType::RequestRate => "Request Rate",
        }
    }

    pub fn unit(&self) -> &'static str {
        match self {
            MetricType::ErrorRate => "%",
            MetricType::ResponseTime => "ms",
            MetricType::ResponseTimeP95 => "ms",
            MetricType::ResponseTimeP99 => "ms",
            MetricType::CpuUsage => "%",
            MetricType::MemoryUsage => "%",
            MetricType::RequestRate => "req/min",
        }
    }
}

/// Comparison condition for alert rules
#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    GreaterThan,
    LessThan,
    GreaterThanOrEqual,
    LessThanOrEqual,
    Equal,
    NotEqual,
}

impl Condition {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            ">" | "gt" => Some(Condition::GreaterThan),
            "<" | "lt" => Some(Condition::LessThan),
            ">=" | "gte" => Some(Condition::GreaterThanOrEqual),
            "<=" | "lte" => Some(Condition::LessThanOrEqual),
            "==" | "eq" => Some(Condition::Equal),
            "!=" | "ne" => Some(Condition::NotEqual),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Condition::GreaterThan => ">",
            Condition::LessThan => "<",
            Condition::GreaterThanOrEqual => ">=",
            Condition::LessThanOrEqual => "<=",
            Condition::Equal => "==",
            Condition::NotEqual => "!=",
        }
    }

    pub fn evaluate(&self, value: f64, threshold: f64) -> bool {
        match self {
            Condition::GreaterThan => value > threshold,
            Condition::LessThan => value < threshold,
            Condition::GreaterThanOrEqual => value >= threshold,
            Condition::LessThanOrEqual => value <= threshold,
            Condition::Equal => (value - threshold).abs() < f64::EPSILON,
            Condition::NotEqual => (value - threshold).abs() >= f64::EPSILON,
        }
    }
}

/// Alert severity levels
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

impl Severity {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "info" => Some(Severity::Info),
            "warning" | "warn" => Some(Severity::Warning),
            "critical" | "crit" => Some(Severity::Critical),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Critical => "critical",
        }
    }
}

/// Alert evaluator that checks rules against metrics
pub struct AlertEvaluator {
    db: Arc<Database>,
}

impl AlertEvaluator {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Evaluate all enabled alert rules and trigger/resolve alerts as needed
    pub fn evaluate_all(&self) -> Result<EvaluationResult> {
        let rules = self.db.list_enabled_alert_rules()?;
        let mut result = EvaluationResult::default();

        for rule in rules {
            match self.evaluate_rule(&rule) {
                Ok(status) => {
                    match status {
                        RuleStatus::Firing { value, message } => {
                            self.handle_firing(&rule, value, &message)?;
                            result.fired += 1;
                        }
                        RuleStatus::Ok => {
                            self.handle_resolved(&rule)?;
                            result.ok += 1;
                        }
                        RuleStatus::NoData => {
                            result.no_data += 1;
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to evaluate rule {}: {}", rule.id, e);
                    result.errors += 1;
                }
            }
        }

        Ok(result)
    }

    /// Evaluate a single alert rule
    pub fn evaluate_rule(&self, rule: &AlertRuleRecord) -> Result<RuleStatus> {
        let metric_type = MetricType::from_str(&rule.metric_type)
            .ok_or_else(|| anyhow::anyhow!("Unknown metric type: {}", rule.metric_type))?;

        let condition = Condition::from_str(&rule.condition)
            .ok_or_else(|| anyhow::anyhow!("Unknown condition: {}", rule.condition))?;

        let value = self.get_metric_value(&metric_type, rule.app_name.as_deref(), rule.duration_secs)?;

        match value {
            Some(val) => {
                if condition.evaluate(val, rule.threshold) {
                    let message = format!(
                        "{} {} {} {} (threshold: {} {})",
                        metric_type.display_name(),
                        condition.as_str(),
                        val,
                        metric_type.unit(),
                        rule.threshold,
                        metric_type.unit()
                    );
                    Ok(RuleStatus::Firing { value: val, message })
                } else {
                    Ok(RuleStatus::Ok)
                }
            }
            None => Ok(RuleStatus::NoData),
        }
    }

    /// Get the current value for a metric
    fn get_metric_value(&self, metric_type: &MetricType, app_name: Option<&str>, duration_secs: i64) -> Result<Option<f64>> {
        let since = format!("datetime('now', '-{} seconds')", duration_secs);

        match metric_type {
            MetricType::ErrorRate | MetricType::ResponseTime | MetricType::ResponseTimeP95 |
            MetricType::ResponseTimeP99 | MetricType::RequestRate => {
                let app = app_name.ok_or_else(|| anyhow::anyhow!("App name required for request metrics"))?;
                let metrics = self.db.get_request_metrics(app, &since, 100)?;

                if metrics.is_empty() {
                    return Ok(None);
                }

                Ok(Some(self.aggregate_request_metrics(&metrics, metric_type)))
            }
            MetricType::CpuUsage | MetricType::MemoryUsage => {
                let app = app_name.ok_or_else(|| anyhow::anyhow!("App name required for resource metrics"))?;
                let metrics = self.db.get_resource_metrics(app, &since, 100)?;

                if metrics.is_empty() {
                    return Ok(None);
                }

                Ok(Some(self.aggregate_resource_metrics(&metrics, metric_type)))
            }
        }
    }

    fn aggregate_request_metrics(&self, metrics: &[RequestMetricsRecord], metric_type: &MetricType) -> f64 {
        let total_requests: i64 = metrics.iter().map(|m| m.request_count).sum();
        let total_errors: i64 = metrics.iter().map(|m| m.error_count).sum();

        match metric_type {
            MetricType::ErrorRate => {
                if total_requests > 0 {
                    (total_errors as f64 / total_requests as f64) * 100.0
                } else {
                    0.0
                }
            }
            MetricType::ResponseTime => {
                let sum: f64 = metrics.iter().map(|m| m.avg_response_time_ms * m.request_count as f64).sum();
                if total_requests > 0 {
                    sum / total_requests as f64
                } else {
                    0.0
                }
            }
            MetricType::ResponseTimeP95 => {
                let values: Vec<f64> = metrics.iter().filter_map(|m| m.p95_response_time_ms).collect();
                if values.is_empty() {
                    0.0
                } else {
                    values.iter().copied().fold(f64::MIN, f64::max)
                }
            }
            MetricType::ResponseTimeP99 => {
                let values: Vec<f64> = metrics.iter().filter_map(|m| m.p99_response_time_ms).collect();
                if values.is_empty() {
                    0.0
                } else {
                    values.iter().copied().fold(f64::MIN, f64::max)
                }
            }
            MetricType::RequestRate => {
                total_requests as f64
            }
            _ => 0.0,
        }
    }

    fn aggregate_resource_metrics(&self, metrics: &[ResourceMetricsRecord], metric_type: &MetricType) -> f64 {
        if metrics.is_empty() {
            return 0.0;
        }

        match metric_type {
            MetricType::CpuUsage => {
                let sum: f64 = metrics.iter().map(|m| m.cpu_percent).sum();
                sum / metrics.len() as f64
            }
            MetricType::MemoryUsage => {
                let latest = metrics.iter().max_by_key(|m| &m.timestamp);
                if let Some(m) = latest {
                    if m.memory_limit > 0 {
                        (m.memory_used as f64 / m.memory_limit as f64) * 100.0
                    } else {
                        0.0
                    }
                } else {
                    0.0
                }
            }
            _ => 0.0,
        }
    }

    fn handle_firing(&self, rule: &AlertRuleRecord, value: f64, message: &str) -> Result<()> {
        // Check if there's already an active alert for this rule
        if let Some(existing) = self.db.get_active_alert_for_rule(&rule.id)? {
            debug!("Alert already firing for rule {}: event {}", rule.id, existing.id);
            return Ok(());
        }

        // Create new alert event
        info!("Alert firing: {} - {}", rule.name, message);
        let event_id = self.db.create_alert_event(
            &rule.id,
            rule.app_name.as_deref(),
            value,
            rule.threshold,
            Some(message),
        )?;

        // Create notifications for configured channels
        if let Some(channels) = &rule.notification_channels {
            for channel in channels.split(',') {
                let channel = channel.trim();
                if !channel.is_empty() {
                    self.db.create_alert_notification(event_id, channel)?;
                }
            }
        }

        Ok(())
    }

    fn handle_resolved(&self, rule: &AlertRuleRecord) -> Result<()> {
        // Resolve any active alerts for this rule
        if let Some(event) = self.db.get_active_alert_for_rule(&rule.id)? {
            info!("Alert resolved: {} (event {})", rule.name, event.id);
            self.db.resolve_alert_event(event.id)?;
        }
        Ok(())
    }
}

/// Status of a rule after evaluation
#[derive(Debug)]
pub enum RuleStatus {
    /// Rule condition met, alert should fire
    Firing { value: f64, message: String },
    /// Rule condition not met, all clear
    Ok,
    /// No data available to evaluate
    NoData,
}

/// Result of evaluating all rules
#[derive(Debug, Default)]
pub struct EvaluationResult {
    pub fired: usize,
    pub ok: usize,
    pub no_data: usize,
    pub errors: usize,
}

/// Get available metric types with their descriptions
pub fn get_available_metrics() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("error_rate", "Error Rate", "Percentage of requests that resulted in errors"),
        ("response_time", "Response Time", "Average response time in milliseconds"),
        ("response_time_p95", "Response Time (P95)", "95th percentile response time"),
        ("response_time_p99", "Response Time (P99)", "99th percentile response time"),
        ("cpu_usage", "CPU Usage", "Average CPU usage percentage across instances"),
        ("memory_usage", "Memory Usage", "Memory usage as percentage of limit"),
        ("request_rate", "Request Rate", "Number of requests per evaluation window"),
    ]
}

/// Get available conditions with their symbols
pub fn get_available_conditions() -> Vec<(&'static str, &'static str)> {
    vec![
        (">", "Greater than"),
        ("<", "Less than"),
        (">=", "Greater than or equal"),
        ("<=", "Less than or equal"),
        ("==", "Equal to"),
        ("!=", "Not equal to"),
    ]
}

/// Get available severities
pub fn get_available_severities() -> Vec<(&'static str, &'static str)> {
    vec![
        ("info", "Info - Informational alerts"),
        ("warning", "Warning - Should be investigated"),
        ("critical", "Critical - Requires immediate attention"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_condition_evaluate() {
        assert!(Condition::GreaterThan.evaluate(10.0, 5.0));
        assert!(!Condition::GreaterThan.evaluate(5.0, 10.0));

        assert!(Condition::LessThan.evaluate(5.0, 10.0));
        assert!(!Condition::LessThan.evaluate(10.0, 5.0));

        assert!(Condition::GreaterThanOrEqual.evaluate(10.0, 10.0));
        assert!(Condition::GreaterThanOrEqual.evaluate(11.0, 10.0));

        assert!(Condition::LessThanOrEqual.evaluate(10.0, 10.0));
        assert!(Condition::LessThanOrEqual.evaluate(9.0, 10.0));

        assert!(Condition::Equal.evaluate(10.0, 10.0));
        assert!(!Condition::Equal.evaluate(10.0, 11.0));

        assert!(Condition::NotEqual.evaluate(10.0, 11.0));
        assert!(!Condition::NotEqual.evaluate(10.0, 10.0));
    }

    #[test]
    fn test_metric_type_from_str() {
        assert_eq!(MetricType::from_str("error_rate"), Some(MetricType::ErrorRate));
        assert_eq!(MetricType::from_str("cpu_usage"), Some(MetricType::CpuUsage));
        assert_eq!(MetricType::from_str("unknown"), None);
    }

    #[test]
    fn test_severity_ordering() {
        assert!(Severity::Critical > Severity::Warning);
        assert!(Severity::Warning > Severity::Info);
    }
}
