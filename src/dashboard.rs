//! Web Dashboard for PaaS Platform
//!
//! Provides a comprehensive web UI using HTMX + Tailwind CSS + Alpine.js
//! for managing apps, viewing logs, and monitoring the platform.

use hyper::body::Bytes;
use hyper::header::CONTENT_TYPE;
use hyper::{Response, StatusCode};
use http_body_util::Full;

/// Serve the main dashboard HTML
pub fn serve_dashboard() -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Full::new(Bytes::from(DASHBOARD_HTML)))
        .unwrap()
}

/// Serve the login page
pub fn serve_login() -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Full::new(Bytes::from(LOGIN_HTML)))
        .unwrap()
}

/// Serve dashboard CSS
pub fn serve_css() -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/css")
        .body(Full::new(Bytes::from(DASHBOARD_CSS)))
        .unwrap()
}

/// Serve dashboard JavaScript
pub fn serve_js() -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/javascript")
        .body(Full::new(Bytes::from(DASHBOARD_JS)))
        .unwrap()
}

/// Generate HTML fragment for apps list (HTMX partial)
pub fn render_apps_list(apps: &[serde_json::Value]) -> String {
    if apps.is_empty() {
        return r#"
        <div class="empty-state">
            <svg class="empty-icon" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2"
                    d="M20 7l-8-4-8 4m16 0l-8 4m8-4v10l-8 4m0-10L4 7m8 4v10M4 7v10l8 4"/>
            </svg>
            <h3>No apps yet</h3>
            <p>Create your first app to get started</p>
            <button class="btn btn-primary" @click="showModal = 'create-app'">
                Create App
            </button>
        </div>
        "#.to_string();
    }

    let cards: Vec<String> = apps.iter().map(|app| {
        let name = app["name"].as_str().unwrap_or("unknown");
        let status = app["status"].as_str().unwrap_or("idle");
        let port = app["port"].as_u64().unwrap_or(3000);

        let status_class = match status {
            "running" => "status-running",
            "building" => "status-building",
            "failed" => "status-failed",
            _ => "status-idle",
        };

        format!(
            r##"<a href="/dashboard/apps/{0}" class="app-card" hx-get="/dashboard/apps/{0}" hx-target="#main-content" hx-push-url="true"><div class="app-card-header"><span class="app-name">{0}</span><span class="status-badge {1}">{2}</span></div><div class="app-card-meta"><span>Port {3}</span></div></a>"##,
            name, status_class, status, port
        )
    }).collect();

    format!(r#"<div class="apps-grid">{}</div>"#, cards.join(""))
}

/// Generate HTML fragment for app detail page
pub fn render_app_detail(app: &serde_json::Value) -> String {
    let name = app["name"].as_str().unwrap_or("unknown");
    let status = app["status"].as_str().unwrap_or("idle");
    let port = app["port"].as_u64().unwrap_or(3000);
    let git_url = app["git_url"].as_str().unwrap_or("");
    let image = app["image"].as_str().unwrap_or("Not deployed");

    let status_class = match status {
        "running" => "status-running",
        "building" => "status-building",
        "failed" => "status-failed",
        _ => "status-idle",
    };

    let git_display = if git_url.is_empty() { "Not configured" } else { git_url };

    format!(
        r##"<div class="app-detail" x-data="{{ activeTab: 'overview' }}">
<div class="app-header">
    <a href="/dashboard" class="back-link" hx-get="/dashboard/apps" hx-target="#main-content" hx-push-url="/dashboard">
        <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="20" height="20">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 19l-7-7 7-7"/>
        </svg>
        Back to Apps
    </a>
    <div class="app-title">
        <h1>{0}</h1>
        <span class="status-badge {1}">{2}</span>
    </div>
    <div class="app-actions">
        <button class="btn btn-secondary" hx-post="/apps/{0}/restart" hx-swap="none" hx-on::after-request="showToast('App restarting...', 'success')">Restart</button>
        <button class="btn btn-primary" @click="showModal = 'deploy'">Deploy</button>
    </div>
</div>

<div class="tabs">
    <button class="tab" :class="{{ 'active': activeTab === 'overview' }}" @click="activeTab = 'overview'">Overview</button>
    <button class="tab" :class="{{ 'active': activeTab === 'resources' }}" @click="activeTab = 'resources'">Resources</button>
    <button class="tab" :class="{{ 'active': activeTab === 'config' }}" @click="activeTab = 'config'">Config Vars</button>
    <button class="tab" :class="{{ 'active': activeTab === 'domains' }}" @click="activeTab = 'domains'">Domains</button>
    <button class="tab" :class="{{ 'active': activeTab === 'deploy' }}" @click="activeTab = 'deploy'">Deploy</button>
    <button class="tab" :class="{{ 'active': activeTab === 'logs' }}" @click="activeTab = 'logs'">Logs</button>
    <button class="tab" :class="{{ 'active': activeTab === 'settings' }}" @click="activeTab = 'settings'">Settings</button>
</div>

<div class="tab-content" x-show="activeTab === 'overview'">
    <div class="card">
        <h2>App Info</h2>
        <div class="info-grid">
            <div class="info-item">
                <span class="info-label">Status</span>
                <span class="info-value"><span class="status-badge {1}">{2}</span></span>
            </div>
            <div class="info-item">
                <span class="info-label">Port</span>
                <span class="info-value">{3}</span>
            </div>
            <div class="info-item">
                <span class="info-label">Git Repository</span>
                <span class="info-value">{4}</span>
            </div>
            <div class="info-item">
                <span class="info-label">Image</span>
                <span class="info-value">{5}</span>
            </div>
        </div>
    </div>
    <div class="card">
        <h2>Recent Activity</h2>
        <div id="activity-feed" hx-get="/dashboard/apps/{0}/activity" hx-trigger="load" hx-swap="innerHTML">
            <div class="loading">Loading activity...</div>
        </div>
    </div>
    <div class="metrics-grid">
        <div class="metric-card"><span class="metric-label">Requests/min</span><span class="metric-value" id="metric-requests">--</span></div>
        <div class="metric-card"><span class="metric-label">Memory</span><span class="metric-value" id="metric-memory">--</span></div>
        <div class="metric-card"><span class="metric-label">CPU</span><span class="metric-value" id="metric-cpu">--</span></div>
        <div class="metric-card"><span class="metric-label">Uptime</span><span class="metric-value" id="metric-uptime">--</span></div>
    </div>
</div>

<div class="tab-content" x-show="activeTab === 'resources'">
    <div class="card">
        <h2>Dynos</h2>
        <div id="dynos-list" hx-get="/dashboard/apps/{0}/dynos" hx-trigger="load" hx-swap="innerHTML">
            <div class="loading">Loading dynos...</div>
        </div>
    </div>
    <div class="card">
        <h2>Add-ons</h2>
        <div id="addons-list" hx-get="/dashboard/apps/{0}/addons" hx-trigger="load" hx-swap="innerHTML">
            <div class="loading">Loading add-ons...</div>
        </div>
        <button class="btn btn-secondary mt-4" @click="showModal = 'add-addon'">Add Add-on</button>
    </div>
</div>

<div class="tab-content" x-show="activeTab === 'config'">
    <div class="card">
        <div class="card-header">
            <h2>Config Vars</h2>
            <button class="btn btn-secondary" @click="showModal = 'add-config'">Add Variable</button>
        </div>
        <div id="config-list" hx-get="/dashboard/apps/{0}/config" hx-trigger="load" hx-swap="innerHTML">
            <div class="loading">Loading config vars...</div>
        </div>
    </div>
</div>

<div class="tab-content" x-show="activeTab === 'domains'">
    <div class="card">
        <div class="card-header">
            <h2>Domains</h2>
            <button class="btn btn-secondary" @click="showModal = 'add-domain'">Add Domain</button>
        </div>
        <div id="domains-list" hx-get="/dashboard/apps/{0}/domains" hx-trigger="load" hx-swap="innerHTML">
            <div class="loading">Loading domains...</div>
        </div>
    </div>
</div>

<div class="tab-content" x-show="activeTab === 'deploy'">
    <div class="card">
        <h2>Deploy from Git</h2>
        <form hx-post="/apps/{0}/deploy" hx-swap="none" hx-on::after-request="showToast('Deployment started!', 'success')">
            <div class="form-group">
                <label for="git-url">Git Repository URL</label>
                <input type="url" id="git-url" name="git_url" placeholder="https://github.com/user/repo.git" value="{6}" class="input">
            </div>
            <div class="form-group">
                <label for="git-ref">Branch/Tag/Commit</label>
                <input type="text" id="git-ref" name="git_ref" placeholder="main" value="main" class="input">
            </div>
            <button type="submit" class="btn btn-primary">Deploy</button>
        </form>
    </div>
    <div class="card">
        <h2>Recent Deployments</h2>
        <div id="deployments-list" hx-get="/dashboard/apps/{0}/deployments" hx-trigger="load" hx-swap="innerHTML">
            <div class="loading">Loading deployments...</div>
        </div>
    </div>
</div>

<div class="tab-content" x-show="activeTab === 'logs'">
    <div class="card logs-card">
        <div class="card-header">
            <h2>Application Logs</h2>
            <div class="logs-controls">
                <select class="select" id="log-source">
                    <option value="all">All Sources</option>
                    <option value="app">App</option>
                    <option value="router">Router</option>
                    <option value="build">Build</option>
                </select>
                <button class="btn btn-secondary" onclick="clearLogs()">Clear</button>
                <button class="btn btn-secondary" id="logs-follow-btn" onclick="toggleLogsFollow()">Follow</button>
            </div>
        </div>
        <div class="logs-container" id="logs-container" hx-get="/dashboard/apps/{0}/logs/stream" hx-trigger="load, every 2s[followLogs]" hx-swap="beforeend"></div>
    </div>
</div>

<div class="tab-content" x-show="activeTab === 'settings'">
    <div class="card">
        <h2>App Settings</h2>
        <form hx-put="/apps/{0}" hx-swap="none" hx-on::after-request="showToast('Settings saved!', 'success')">
            <div class="form-group">
                <label for="app-port">Port</label>
                <input type="number" id="app-port" name="port" value="{3}" class="input">
            </div>
            <button type="submit" class="btn btn-primary">Save Settings</button>
        </form>
    </div>
    <div class="card danger-zone">
        <h2>Danger Zone</h2>
        <p>Once you delete an app, there is no going back. Please be certain.</p>
        <button class="btn btn-danger" hx-delete="/apps/{0}" hx-confirm="Are you sure you want to delete {0}? This cannot be undone." hx-on::after-request="window.location.href='/dashboard'">Delete App</button>
    </div>
</div>
</div>"##,
        name, status_class, status, port, git_display, image, git_url
    )
}

/// Generate HTML for config vars list
pub fn render_config_vars(config: &serde_json::Value) -> String {
    let env = config.as_object();

    if env.is_none() || env.unwrap().is_empty() {
        return r##"<div class="empty-state small">No config vars set</div>"##.to_string();
    }

    let items: Vec<String> = env.unwrap().iter().map(|(key, value)| {
        let val = value.as_str().unwrap_or("");
        let masked = if key.contains("PASSWORD") || key.contains("SECRET") || key.contains("KEY") || key.contains("TOKEN") {
            "--------".to_string()
        } else if val.len() > 40 {
            format!("{}...", &val[..40])
        } else {
            val.to_string()
        };

        format!(
            r##"<div class="config-item">
            <div class="config-info">
                <span class="config-key">{0}</span>
                <span class="config-value">{1}</span>
            </div>
            <button class="btn btn-icon btn-danger"
                hx-delete="/apps/current/config/{0}"
                hx-confirm="Delete {0}?"
                hx-swap="none"
                hx-on::after-request="htmx.trigger('#config-list', 'reload')">
                <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"/>
                </svg>
            </button>
        </div>"##,
            key, masked
        )
    }).collect();

    format!(r##"<div class="config-list">{}</div>"##, items.join(""))
}

/// Generate HTML for domains list
pub fn render_domains_list(domains: &[serde_json::Value]) -> String {
    if domains.is_empty() {
        return r##"<div class="empty-state small">No custom domains configured</div>"##.to_string();
    }

    let items: Vec<String> = domains.iter().map(|domain| {
        let hostname = domain["hostname"].as_str().unwrap_or("unknown");
        let verified = domain["dns_verified"].as_bool().unwrap_or(false);
        let ssl_status = domain["ssl_status"].as_str().unwrap_or("pending");

        let status_class = if verified { "status-running" } else { "status-idle" };
        let status_text = if verified { "Verified" } else { "Pending" };

        format!(
            r##"<div class="domain-item">
            <div class="domain-info">
                <span class="domain-name">{0}</span>
                <div class="domain-badges">
                    <span class="status-badge {1}">{2}</span>
                    <span class="ssl-badge">SSL: {3}</span>
                </div>
            </div>
            <button class="btn btn-icon btn-danger"
                hx-delete="/apps/current/domains/{0}"
                hx-confirm="Remove {0}?"
                hx-swap="none"
                hx-on::after-request="htmx.trigger('#domains-list', 'reload')">
                <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/>
                </svg>
            </button>
        </div>"##,
            hostname, status_class, status_text, ssl_status
        )
    }).collect();

    format!(r##"<div class="domains-list">{}</div>"##, items.join(""))
}

/// Generate HTML for addons list
pub fn render_addons_list(addons: &[serde_json::Value]) -> String {
    if addons.is_empty() {
        return r##"<div class="empty-state small">No add-ons attached</div>"##.to_string();
    }

    let items: Vec<String> = addons.iter().map(|addon| {
        let addon_type = addon["addon_type"].as_str().unwrap_or("unknown");
        let plan = addon["plan"].as_str().unwrap_or("hobby");
        let status = addon["status"].as_str().unwrap_or("provisioning");

        let status_class = match status {
            "running" => "status-running",
            "provisioning" => "status-building",
            "failed" => "status-failed",
            _ => "status-idle",
        };

        let icon = match addon_type {
            "postgres" => r##"<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4"/>"##,
            "redis" => r##"<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 12h14M5 12a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v4a2 2 0 01-2 2M5 12a2 2 0 00-2 2v4a2 2 0 002 2h14a2 2 0 002-2v-4a2 2 0 00-2-2"/>"##,
            _ => r##"<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 8h14M5 8a2 2 0 110-4h14a2 2 0 110 4M5 8v10a2 2 0 002 2h10a2 2 0 002-2V8"/>"##,
        };

        format!(
            r##"<div class="addon-item">
            <div class="addon-info">
                <svg class="addon-icon" fill="none" stroke="currentColor" viewBox="0 0 24 24" width="24" height="24">
                    {0}
                </svg>
                <div class="addon-details">
                    <span class="addon-type">{1}</span>
                    <span class="addon-plan">{2}</span>
                </div>
            </div>
            <div class="addon-actions">
                <span class="status-badge {3}">{4}</span>
                <button class="btn btn-icon btn-danger"
                    hx-delete="/apps/current/addons/{1}"
                    hx-confirm="Remove {1}? This will delete all data!"
                    hx-swap="none"
                    hx-on::after-request="htmx.trigger('#addons-list', 'reload')">
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"/>
                    </svg>
                </button>
            </div>
        </div>"##,
            icon, addon_type, plan, status_class, status
        )
    }).collect();

    format!(r##"<div class="addons-list">{}</div>"##, items.join(""))
}

/// Generate HTML for deployments list
pub fn render_deployments_list(deployments: &[serde_json::Value]) -> String {
    if deployments.is_empty() {
        return r##"<div class="empty-state small">No deployments yet</div>"##.to_string();
    }

    let items: Vec<String> = deployments.iter().take(10).map(|deploy| {
        let status = deploy["status"].as_str().unwrap_or("pending");
        let image = deploy["image"].as_str().unwrap_or("N/A");
        let duration = deploy["duration_secs"].as_f64().map(|d| format!("{:.1}s", d)).unwrap_or_default();
        let created = deploy["created_at"].as_str().unwrap_or("");

        let status_class = match status {
            "success" => "status-running",
            "building" | "pending" => "status-building",
            "failed" => "status-failed",
            _ => "status-idle",
        };

        format!(
            r##"<div class="deployment-item">
            <div class="deployment-info">
                <span class="status-badge {0}">{1}</span>
                <span class="deployment-image">{2}</span>
            </div>
            <div class="deployment-meta">
                <span class="deployment-duration">{3}</span>
                <span class="deployment-time">{4}</span>
            </div>
        </div>"##,
            status_class, status, image, duration, created
        )
    }).collect();

    format!(r##"<div class="deployments-list">{}</div>"##, items.join(""))
}

const LOGIN_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Login - Spawngate</title>
    <link rel="stylesheet" href="/dashboard/style.css">
    <script defer src="https://unpkg.com/alpinejs@3.x.x/dist/cdn.min.js"></script>
</head>
<body class="login-page" x-data="{ theme: localStorage.getItem('theme') || 'dark' }" :class="theme">
    <div class="login-container">
        <div class="login-card">
            <div class="login-header">
                <svg class="logo" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <path d="M13 10V3L4 14h7v7l9-11h-7z"/>
                </svg>
                <h1>Spawngate</h1>
                <p>Sign in to your account</p>
            </div>
            <form action="/dashboard/auth" method="POST" class="login-form">
                <div class="form-group">
                    <label for="token">API Token</label>
                    <input type="password" id="token" name="token" required
                        placeholder="Enter your API token" class="input">
                </div>
                <button type="submit" class="btn btn-primary btn-block">Sign In</button>
            </form>
            <div class="login-footer">
                <p>Don't have a token? Check the server configuration.</p>
            </div>
        </div>
    </div>
    <script>
        document.body.classList.add(localStorage.getItem('theme') || 'dark');
    </script>
</body>
</html>
"##;

const DASHBOARD_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Dashboard - Spawngate</title>
    <link rel="stylesheet" href="/dashboard/style.css">
    <script src="https://unpkg.com/htmx.org@1.9.10"></script>
    <script defer src="https://unpkg.com/alpinejs@3.x.x/dist/cdn.min.js"></script>
</head>
<body x-data="dashboard()" :class="theme">
    <!-- Sidebar -->
    <aside class="sidebar" :class="{ 'collapsed': sidebarCollapsed }">
        <div class="sidebar-header">
            <a href="/dashboard" class="logo-link">
                <svg class="logo" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <path d="M13 10V3L4 14h7v7l9-11h-7z"/>
                </svg>
                <span class="logo-text" x-show="!sidebarCollapsed">Spawngate</span>
            </a>
            <button class="sidebar-toggle" @click="toggleSidebar()">
                <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="20" height="20">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 6h16M4 12h16M4 18h16"/>
                </svg>
            </button>
        </div>

        <nav class="sidebar-nav">
            <a href="/dashboard" class="nav-item" :class="{ 'active': currentPage === 'apps' }"
                hx-get="/dashboard/apps" hx-target="#main-content" hx-push-url="/dashboard"
                @click="currentPage = 'apps'">
                <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="20" height="20">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2"
                        d="M20 7l-8-4-8 4m16 0l-8 4m8-4v10l-8 4m0-10L4 7m8 4v10M4 7v10l8 4"/>
                </svg>
                <span x-show="!sidebarCollapsed">Apps</span>
            </a>
            <a href="/dashboard/pipelines" class="nav-item" :class="{ 'active': currentPage === 'pipelines' }"
                hx-get="/dashboard/pipelines" hx-target="#main-content" hx-push-url="true"
                @click="currentPage = 'pipelines'">
                <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="20" height="20">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2"
                        d="M9 17v-2m3 2v-4m3 4v-6m2 10H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"/>
                </svg>
                <span x-show="!sidebarCollapsed">Pipelines</span>
            </a>
            <a href="/dashboard/addons" class="nav-item" :class="{ 'active': currentPage === 'addons' }"
                hx-get="/dashboard/addons" hx-target="#main-content" hx-push-url="true"
                @click="currentPage = 'addons'">
                <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="20" height="20">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2"
                        d="M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 012-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10"/>
                </svg>
                <span x-show="!sidebarCollapsed">Add-ons</span>
            </a>
            <a href="/dashboard/scheduler" class="nav-item" :class="{ 'active': currentPage === 'scheduler' }"
                hx-get="/dashboard/scheduler" hx-target="#main-content" hx-push-url="true"
                @click="currentPage = 'scheduler'">
                <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="20" height="20">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2"
                        d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z"/>
                </svg>
                <span x-show="!sidebarCollapsed">Scheduler</span>
            </a>
            <a href="/dashboard/metrics" class="nav-item" :class="{ 'active': currentPage === 'metrics' }"
                hx-get="/dashboard/metrics" hx-target="#main-content" hx-push-url="true"
                @click="currentPage = 'metrics'">
                <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="20" height="20">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2"
                        d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z"/>
                </svg>
                <span x-show="!sidebarCollapsed">Metrics</span>
            </a>
        </nav>

        <div class="sidebar-footer">
            <button class="theme-toggle" @click="toggleTheme()" :title="theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'">
                <svg x-show="theme === 'dark'" fill="none" stroke="currentColor" viewBox="0 0 24 24" width="20" height="20">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2"
                        d="M12 3v1m0 16v1m9-9h-1M4 12H3m15.364 6.364l-.707-.707M6.343 6.343l-.707-.707m12.728 0l-.707.707M6.343 17.657l-.707.707M16 12a4 4 0 11-8 0 4 4 0 018 0z"/>
                </svg>
                <svg x-show="theme === 'light'" fill="none" stroke="currentColor" viewBox="0 0 24 24" width="20" height="20">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2"
                        d="M20.354 15.354A9 9 0 018.646 3.646 9.003 9.003 0 0012 21a9.003 9.003 0 008.354-5.646z"/>
                </svg>
                <span x-show="!sidebarCollapsed" x-text="theme === 'dark' ? 'Light Mode' : 'Dark Mode'"></span>
            </button>
            <a href="/dashboard/settings" class="nav-item" :class="{ 'active': currentPage === 'settings' }"
                @click="currentPage = 'settings'">
                <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="20" height="20">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2"
                        d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"/>
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"/>
                </svg>
                <span x-show="!sidebarCollapsed">Settings</span>
            </a>
            <a href="/dashboard/logout" class="nav-item nav-item-danger">
                <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="20" height="20">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2"
                        d="M17 16l4-4m0 0l-4-4m4 4H7m6 4v1a3 3 0 01-3 3H6a3 3 0 01-3-3V7a3 3 0 013-3h4a3 3 0 013 3v1"/>
                </svg>
                <span x-show="!sidebarCollapsed">Logout</span>
            </a>
        </div>
    </aside>

    <!-- Main Content -->
    <main class="main-wrapper" :class="{ 'sidebar-collapsed': sidebarCollapsed }">
        <header class="topbar">
            <div class="topbar-left">
                <button class="mobile-menu-btn" @click="mobileMenuOpen = !mobileMenuOpen">
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="24" height="24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 6h16M4 12h16M4 18h16"/>
                    </svg>
                </button>
                <div class="search-box">
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="20" height="20">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2"
                            d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z"/>
                    </svg>
                    <input type="search" placeholder="Search apps... (Ctrl+K)" class="search-input"
                        @input.debounce.300ms="searchApps($event.target.value)"
                        @keydown.ctrl.k.window.prevent="$el.focus()">
                </div>
            </div>
            <div class="topbar-actions">
                <button class="btn btn-primary" @click="showModal = 'create-app'">
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4"/>
                    </svg>
                    <span class="btn-text">New App</span>
                </button>
            </div>
        </header>

        <div id="main-content" class="content" hx-get="/dashboard/apps" hx-trigger="load" hx-swap="innerHTML">
            <div class="loading-screen">
                <div class="spinner"></div>
                <p>Loading...</p>
            </div>
        </div>
    </main>

    <!-- Create App Modal -->
    <div class="modal-backdrop" x-show="showModal === 'create-app'" x-cloak
        x-transition:enter="modal-enter" x-transition:leave="modal-leave"
        @click.self="showModal = null" @keydown.escape.window="showModal = null">
        <div class="modal" @click.stop>
            <div class="modal-header">
                <h2>Create New App</h2>
                <button class="close-btn" @click="showModal = null">&times;</button>
            </div>
            <form hx-post="/apps" hx-swap="none" @htmx:after-request="handleAppCreated($event)">
                <div class="modal-body">
                    <div class="form-group">
                        <label for="app-name">App Name</label>
                        <input type="text" id="app-name" name="name" required pattern="[a-z0-9-]+"
                            placeholder="my-awesome-app" class="input" autofocus>
                        <small>Lowercase letters, numbers, and hyphens only</small>
                    </div>
                    <div class="form-group">
                        <label for="app-port">Port</label>
                        <input type="number" id="app-port" name="port" value="3000" min="1" max="65535" class="input">
                        <small>The port your application listens on</small>
                    </div>
                </div>
                <div class="modal-footer">
                    <button type="button" class="btn btn-secondary" @click="showModal = null">Cancel</button>
                    <button type="submit" class="btn btn-primary">Create App</button>
                </div>
            </form>
        </div>
    </div>

    <!-- Add Config Var Modal -->
    <div class="modal-backdrop" x-show="showModal === 'add-config'" x-cloak
        @click.self="showModal = null" @keydown.escape.window="showModal = null">
        <div class="modal" @click.stop>
            <div class="modal-header">
                <h2>Add Config Variable</h2>
                <button class="close-btn" @click="showModal = null">&times;</button>
            </div>
            <form :hx-put="'/apps/' + currentApp + '/config'" hx-swap="none" @htmx:after-request="handleConfigAdded($event)">
                <div class="modal-body">
                    <div class="form-group">
                        <label for="config-key">Key</label>
                        <input type="text" id="config-key" name="key" required
                            pattern="[A-Za-z_][A-Za-z0-9_]*" placeholder="DATABASE_URL" class="input">
                        <small>Use UPPER_SNAKE_CASE for consistency</small>
                    </div>
                    <div class="form-group">
                        <label for="config-value">Value</label>
                        <textarea id="config-value" name="value" required class="input textarea" rows="3"
                            placeholder="postgres://user:pass@host/db"></textarea>
                    </div>
                </div>
                <div class="modal-footer">
                    <button type="button" class="btn btn-secondary" @click="showModal = null">Cancel</button>
                    <button type="submit" class="btn btn-primary">Add Variable</button>
                </div>
            </form>
        </div>
    </div>

    <!-- Add Add-on Modal -->
    <div class="modal-backdrop" x-show="showModal === 'add-addon'" x-cloak
        @click.self="showModal = null" @keydown.escape.window="showModal = null">
        <div class="modal modal-lg" @click.stop>
            <div class="modal-header">
                <h2>Add Add-on</h2>
                <button class="close-btn" @click="showModal = null">&times;</button>
            </div>
            <form :hx-post="'/apps/' + currentApp + '/addons'" hx-swap="none" @htmx:after-request="handleAddonAdded($event)">
                <div class="modal-body">
                    <div class="addon-options">
                        <label class="addon-option">
                            <input type="radio" name="type" value="postgres" checked>
                            <div class="addon-card">
                                <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="32" height="32">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2"
                                        d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4"/>
                                </svg>
                                <span class="addon-name">PostgreSQL</span>
                                <span class="addon-desc">Reliable SQL database</span>
                            </div>
                        </label>
                        <label class="addon-option">
                            <input type="radio" name="type" value="redis">
                            <div class="addon-card">
                                <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="32" height="32">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2"
                                        d="M5 12h14M5 12a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v4a2 2 0 01-2 2M5 12a2 2 0 00-2 2v4a2 2 0 002 2h14a2 2 0 002-2v-4a2 2 0 00-2-2m-2-4h.01M17 16h.01"/>
                                </svg>
                                <span class="addon-name">Redis</span>
                                <span class="addon-desc">In-memory cache & queue</span>
                            </div>
                        </label>
                        <label class="addon-option">
                            <input type="radio" name="type" value="storage">
                            <div class="addon-card">
                                <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="32" height="32">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2"
                                        d="M5 8h14M5 8a2 2 0 110-4h14a2 2 0 110 4M5 8v10a2 2 0 002 2h10a2 2 0 002-2V8m-9 4h4"/>
                                </svg>
                                <span class="addon-name">S3 Storage</span>
                                <span class="addon-desc">Object storage (MinIO)</span>
                            </div>
                        </label>
                    </div>
                    <div class="form-group">
                        <label for="addon-plan">Plan</label>
                        <select id="addon-plan" name="plan" class="select">
                            <option value="hobby">Hobby - 256MB RAM</option>
                            <option value="basic">Basic - 512MB RAM</option>
                            <option value="standard" selected>Standard - 1GB RAM</option>
                            <option value="premium">Premium - 2GB RAM</option>
                        </select>
                    </div>
                </div>
                <div class="modal-footer">
                    <button type="button" class="btn btn-secondary" @click="showModal = null">Cancel</button>
                    <button type="submit" class="btn btn-primary">Provision Add-on</button>
                </div>
            </form>
        </div>
    </div>

    <!-- Add Domain Modal -->
    <div class="modal-backdrop" x-show="showModal === 'add-domain'" x-cloak
        @click.self="showModal = null" @keydown.escape.window="showModal = null">
        <div class="modal" @click.stop>
            <div class="modal-header">
                <h2>Add Custom Domain</h2>
                <button class="close-btn" @click="showModal = null">&times;</button>
            </div>
            <form :hx-post="'/apps/' + currentApp + '/domains'" hx-swap="none" @htmx:after-request="handleDomainAdded($event)">
                <div class="modal-body">
                    <div class="form-group">
                        <label for="domain-name">Domain Name</label>
                        <input type="text" id="domain-name" name="domain" required
                            placeholder="app.example.com" class="input">
                        <small>You'll need to configure DNS after adding the domain</small>
                    </div>
                </div>
                <div class="modal-footer">
                    <button type="button" class="btn btn-secondary" @click="showModal = null">Cancel</button>
                    <button type="submit" class="btn btn-primary">Add Domain</button>
                </div>
            </form>
        </div>
    </div>

    <!-- Toast Container -->
    <div id="toast-container"></div>

    <script src="/dashboard/app.js"></script>
</body>
</html>
"##;

const DASHBOARD_CSS: &str = r##"
/* CSS Variables - Light Theme */
:root {
    --bg-primary: #ffffff;
    --bg-secondary: #f8fafc;
    --bg-tertiary: #f1f5f9;
    --text-primary: #0f172a;
    --text-secondary: #475569;
    --text-muted: #94a3b8;
    --border-color: #e2e8f0;
    --primary: #6366f1;
    --primary-hover: #4f46e5;
    --primary-light: #eef2ff;
    --success: #10b981;
    --success-light: #d1fae5;
    --warning: #f59e0b;
    --warning-light: #fef3c7;
    --danger: #ef4444;
    --danger-light: #fee2e2;
    --sidebar-width: 240px;
    --sidebar-collapsed-width: 64px;
    --sidebar-bg: #0f172a;
    --sidebar-text: #94a3b8;
    --sidebar-hover: #1e293b;
    --sidebar-active: #6366f1;
    --topbar-height: 64px;
    --card-shadow: 0 1px 3px rgba(0,0,0,0.1);
    --modal-shadow: 0 25px 50px -12px rgba(0,0,0,0.25);
}

/* Dark Theme */
.dark {
    --bg-primary: #0f172a;
    --bg-secondary: #1e293b;
    --bg-tertiary: #334155;
    --text-primary: #f8fafc;
    --text-secondary: #cbd5e1;
    --text-muted: #64748b;
    --border-color: #334155;
    --primary-light: #312e81;
    --success-light: #064e3b;
    --warning-light: #78350f;
    --danger-light: #7f1d1d;
    --card-shadow: 0 1px 3px rgba(0,0,0,0.3);
}

/* Reset & Base */
*, *::before, *::after {
    box-sizing: border-box;
    margin: 0;
    padding: 0;
}

html {
    font-size: 16px;
    -webkit-font-smoothing: antialiased;
    -moz-osx-font-smoothing: grayscale;
}

body {
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', sans-serif;
    background: var(--bg-secondary);
    color: var(--text-primary);
    line-height: 1.5;
    display: flex;
    min-height: 100vh;
}

/* Sidebar */
.sidebar {
    width: var(--sidebar-width);
    background: var(--sidebar-bg);
    display: flex;
    flex-direction: column;
    position: fixed;
    height: 100vh;
    z-index: 100;
    transition: width 0.2s ease;
}

.sidebar.collapsed {
    width: var(--sidebar-collapsed-width);
}

.sidebar-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 1rem;
    border-bottom: 1px solid rgba(255,255,255,0.1);
    min-height: var(--topbar-height);
}

.logo-link {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    text-decoration: none;
    color: white;
    overflow: hidden;
}

.logo {
    width: 32px;
    height: 32px;
    min-width: 32px;
    color: var(--primary);
}

.logo-text {
    font-size: 1.25rem;
    font-weight: 600;
    white-space: nowrap;
}

.sidebar-toggle {
    background: none;
    border: none;
    color: var(--sidebar-text);
    cursor: pointer;
    padding: 0.5rem;
    border-radius: 0.375rem;
    display: flex;
    align-items: center;
    justify-content: center;
}

.sidebar-toggle:hover {
    color: white;
    background: var(--sidebar-hover);
}

.sidebar-nav {
    flex: 1;
    padding: 1rem 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    overflow-y: auto;
}

.nav-item {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.75rem;
    color: var(--sidebar-text);
    text-decoration: none;
    border-radius: 0.5rem;
    transition: all 0.15s ease;
    white-space: nowrap;
    overflow: hidden;
}

.nav-item svg {
    min-width: 20px;
}

.nav-item:hover {
    background: var(--sidebar-hover);
    color: white;
}

.nav-item.active {
    background: var(--sidebar-active);
    color: white;
}

.nav-item-danger:hover {
    background: var(--danger);
}

.sidebar-footer {
    padding: 0.75rem;
    border-top: 1px solid rgba(255,255,255,0.1);
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
}

.theme-toggle {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.75rem;
    background: none;
    border: none;
    color: var(--sidebar-text);
    cursor: pointer;
    border-radius: 0.5rem;
    width: 100%;
    text-align: left;
    font-size: 0.875rem;
}

.theme-toggle:hover {
    background: var(--sidebar-hover);
    color: white;
}

/* Main Content */
.main-wrapper {
    flex: 1;
    margin-left: var(--sidebar-width);
    min-height: 100vh;
    display: flex;
    flex-direction: column;
    transition: margin-left 0.2s ease;
}

.main-wrapper.sidebar-collapsed {
    margin-left: var(--sidebar-collapsed-width);
}

.topbar {
    background: var(--bg-primary);
    border-bottom: 1px solid var(--border-color);
    padding: 0 1.5rem;
    height: var(--topbar-height);
    display: flex;
    align-items: center;
    justify-content: space-between;
    position: sticky;
    top: 0;
    z-index: 50;
}

.topbar-left {
    display: flex;
    align-items: center;
    gap: 1rem;
}

.mobile-menu-btn {
    display: none;
    background: none;
    border: none;
    color: var(--text-secondary);
    cursor: pointer;
    padding: 0.5rem;
}

.search-box {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    background: var(--bg-secondary);
    border: 1px solid var(--border-color);
    border-radius: 0.5rem;
    padding: 0.5rem 1rem;
    width: 320px;
    transition: all 0.15s ease;
}

.search-box:focus-within {
    border-color: var(--primary);
    box-shadow: 0 0 0 3px var(--primary-light);
}

.search-box svg {
    color: var(--text-muted);
    min-width: 20px;
}

.search-input {
    background: none;
    border: none;
    outline: none;
    color: var(--text-primary);
    width: 100%;
    font-size: 0.875rem;
}

.search-input::placeholder {
    color: var(--text-muted);
}

.topbar-actions {
    display: flex;
    align-items: center;
    gap: 0.75rem;
}

.content {
    flex: 1;
    padding: 1.5rem;
    max-width: 1400px;
    margin: 0 auto;
    width: 100%;
}

/* Cards */
.card {
    background: var(--bg-primary);
    border: 1px solid var(--border-color);
    border-radius: 0.75rem;
    padding: 1.5rem;
    box-shadow: var(--card-shadow);
    margin-bottom: 1.5rem;
}

.card h2 {
    font-size: 1.125rem;
    font-weight: 600;
    margin-bottom: 1rem;
    color: var(--text-primary);
}

.card-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 1rem;
}

.card-header h2 {
    margin-bottom: 0;
}

/* Apps Grid */
.apps-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    gap: 1rem;
}

.app-card {
    background: var(--bg-primary);
    border: 1px solid var(--border-color);
    border-radius: 0.75rem;
    padding: 1.25rem;
    text-decoration: none;
    color: inherit;
    transition: all 0.15s ease;
    display: block;
}

.app-card:hover {
    border-color: var(--primary);
    box-shadow: 0 4px 12px rgba(99, 102, 241, 0.15);
    transform: translateY(-2px);
}

.app-card-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    margin-bottom: 0.75rem;
}

.app-name {
    font-size: 1.125rem;
    font-weight: 600;
    color: var(--text-primary);
}

.app-card-meta {
    font-size: 0.875rem;
    color: var(--text-secondary);
}

/* Status Badges */
.status-badge {
    display: inline-flex;
    align-items: center;
    padding: 0.25rem 0.625rem;
    border-radius: 9999px;
    font-size: 0.75rem;
    font-weight: 500;
}

.status-idle {
    background: var(--bg-tertiary);
    color: var(--text-secondary);
}

.status-running {
    background: var(--success-light);
    color: #059669;
}

.dark .status-running {
    color: #34d399;
}

.status-building {
    background: var(--warning-light);
    color: #d97706;
}

.dark .status-building {
    color: #fbbf24;
}

.status-failed {
    background: var(--danger-light);
    color: #dc2626;
}

.dark .status-failed {
    color: #f87171;
}

/* Buttons */
.btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    padding: 0.5rem 1rem;
    border: none;
    border-radius: 0.5rem;
    font-size: 0.875rem;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
    text-decoration: none;
    white-space: nowrap;
}

.btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
}

.btn-primary {
    background: var(--primary);
    color: white;
}

.btn-primary:hover:not(:disabled) {
    background: var(--primary-hover);
}

.btn-secondary {
    background: var(--bg-secondary);
    color: var(--text-primary);
    border: 1px solid var(--border-color);
}

.btn-secondary:hover:not(:disabled) {
    background: var(--bg-tertiary);
}

.btn-danger {
    background: var(--danger);
    color: white;
}

.btn-danger:hover:not(:disabled) {
    background: #dc2626;
}

.btn-icon {
    padding: 0.5rem;
}

.btn-block {
    width: 100%;
}

/* Forms */
.form-group {
    margin-bottom: 1rem;
}

.form-group label {
    display: block;
    font-size: 0.875rem;
    font-weight: 500;
    margin-bottom: 0.375rem;
    color: var(--text-primary);
}

.form-group small {
    display: block;
    font-size: 0.75rem;
    color: var(--text-muted);
    margin-top: 0.375rem;
}

.input, .select, .textarea {
    width: 100%;
    padding: 0.625rem 0.875rem;
    border: 1px solid var(--border-color);
    border-radius: 0.5rem;
    font-size: 0.875rem;
    background: var(--bg-primary);
    color: var(--text-primary);
    transition: all 0.15s ease;
}

.input:focus, .select:focus, .textarea:focus {
    outline: none;
    border-color: var(--primary);
    box-shadow: 0 0 0 3px var(--primary-light);
}

.textarea {
    resize: vertical;
    min-height: 80px;
    font-family: 'SF Mono', Monaco, Consolas, monospace;
}

.select {
    cursor: pointer;
    appearance: none;
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' fill='none' viewBox='0 0 24 24' stroke='%236b7280'%3E%3Cpath stroke-linecap='round' stroke-linejoin='round' stroke-width='2' d='M19 9l-7 7-7-7'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 0.75rem center;
    background-size: 1rem;
    padding-right: 2.5rem;
}

/* Tabs */
.tabs {
    display: flex;
    gap: 0.25rem;
    border-bottom: 1px solid var(--border-color);
    margin-bottom: 1.5rem;
    overflow-x: auto;
    -webkit-overflow-scrolling: touch;
}

.tab {
    padding: 0.75rem 1.25rem;
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    color: var(--text-secondary);
    font-size: 0.875rem;
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
    transition: all 0.15s ease;
}

.tab:hover {
    color: var(--text-primary);
}

.tab.active {
    color: var(--primary);
    border-bottom-color: var(--primary);
}

/* App Detail */
.app-detail {
    max-width: 1200px;
}

.app-header {
    margin-bottom: 1.5rem;
}

.back-link {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    color: var(--text-secondary);
    text-decoration: none;
    font-size: 0.875rem;
    margin-bottom: 0.75rem;
    transition: color 0.15s ease;
}

.back-link:hover {
    color: var(--primary);
}

.app-title {
    display: flex;
    align-items: center;
    gap: 1rem;
    margin-bottom: 1rem;
}

.app-title h1 {
    font-size: 1.75rem;
    font-weight: 600;
}

.app-actions {
    display: flex;
    gap: 0.75rem;
}

/* Info Grid */
.info-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
    gap: 1rem;
}

.info-item {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
}

.info-label {
    font-size: 0.75rem;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
}

.info-value {
    font-size: 0.875rem;
    color: var(--text-primary);
    font-family: 'SF Mono', Monaco, Consolas, monospace;
    word-break: break-all;
}

/* Metrics Grid */
.metrics-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 1rem;
    margin-top: 1.5rem;
}

.metric-card {
    background: var(--bg-primary);
    border: 1px solid var(--border-color);
    border-radius: 0.75rem;
    padding: 1.25rem;
    text-align: center;
}

.metric-label {
    font-size: 0.75rem;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
}

.metric-value {
    font-size: 1.5rem;
    font-weight: 600;
    color: var(--text-primary);
    display: block;
    margin-top: 0.25rem;
}

/* Config List */
.config-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
}

.config-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.875rem 1rem;
    background: var(--bg-secondary);
    border-radius: 0.5rem;
    gap: 1rem;
}

.config-info {
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
    min-width: 0;
    flex: 1;
}

.config-key {
    font-family: 'SF Mono', Monaco, Consolas, monospace;
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--text-primary);
}

.config-value {
    font-family: 'SF Mono', Monaco, Consolas, monospace;
    font-size: 0.75rem;
    color: var(--text-muted);
    word-break: break-all;
}

/* Domains List */
.domains-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
}

.domain-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.875rem 1rem;
    background: var(--bg-secondary);
    border-radius: 0.5rem;
}

.domain-info {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
}

.domain-name {
    font-family: 'SF Mono', Monaco, Consolas, monospace;
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--text-primary);
}

.domain-badges {
    display: flex;
    gap: 0.5rem;
}

.ssl-badge {
    font-size: 0.75rem;
    color: var(--text-muted);
}

/* Addons List */
.addons-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
}

.addon-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.875rem 1rem;
    background: var(--bg-secondary);
    border-radius: 0.5rem;
}

.addon-info {
    display: flex;
    align-items: center;
    gap: 0.75rem;
}

.addon-icon {
    color: var(--text-muted);
}

.addon-details {
    display: flex;
    flex-direction: column;
}

.addon-type {
    font-weight: 500;
    color: var(--text-primary);
    text-transform: capitalize;
}

.addon-plan {
    font-size: 0.75rem;
    color: var(--text-muted);
    text-transform: capitalize;
}

.addon-actions {
    display: flex;
    align-items: center;
    gap: 0.75rem;
}

/* Deployments List */
.deployments-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
}

.deployment-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.875rem 1rem;
    background: var(--bg-secondary);
    border-radius: 0.5rem;
}

.deployment-info {
    display: flex;
    align-items: center;
    gap: 0.75rem;
}

.deployment-image {
    font-family: 'SF Mono', Monaco, Consolas, monospace;
    font-size: 0.875rem;
    color: var(--text-primary);
}

.deployment-meta {
    display: flex;
    align-items: center;
    gap: 1rem;
    color: var(--text-muted);
    font-size: 0.75rem;
}

/* Logs */
.logs-card {
    display: flex;
    flex-direction: column;
    height: calc(100vh - 280px);
    min-height: 400px;
}

.logs-controls {
    display: flex;
    gap: 0.5rem;
    align-items: center;
}

.logs-controls .select {
    width: auto;
}

.logs-container {
    flex: 1;
    background: #0f172a;
    border-radius: 0.5rem;
    padding: 1rem;
    overflow-y: auto;
    font-family: 'SF Mono', Monaco, Consolas, monospace;
    font-size: 0.8125rem;
    line-height: 1.7;
    color: #e2e8f0;
}

.log-line {
    white-space: pre-wrap;
    word-break: break-all;
}

.log-line.error {
    color: #fca5a5;
}

.log-line.warn {
    color: #fcd34d;
}

.log-line .timestamp {
    color: #64748b;
}

.log-line .source {
    color: #a78bfa;
}

/* Modal */
.modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
    backdrop-filter: blur(4px);
    padding: 1rem;
}

.modal {
    background: var(--bg-primary);
    border-radius: 0.75rem;
    width: 100%;
    max-width: 480px;
    max-height: calc(100vh - 2rem);
    overflow-y: auto;
    box-shadow: var(--modal-shadow);
    animation: modalIn 0.2s ease;
}

.modal-lg {
    max-width: 600px;
}

@keyframes modalIn {
    from {
        opacity: 0;
        transform: scale(0.95) translateY(-10px);
    }
    to {
        opacity: 1;
        transform: scale(1) translateY(0);
    }
}

.modal-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 1.25rem 1.5rem;
    border-bottom: 1px solid var(--border-color);
}

.modal-header h2 {
    font-size: 1.125rem;
    font-weight: 600;
    margin: 0;
}

.close-btn {
    background: none;
    border: none;
    font-size: 1.5rem;
    color: var(--text-muted);
    cursor: pointer;
    line-height: 1;
    padding: 0.25rem;
    transition: color 0.15s ease;
}

.close-btn:hover {
    color: var(--text-primary);
}

.modal-body {
    padding: 1.5rem;
}

.modal-footer {
    display: flex;
    justify-content: flex-end;
    gap: 0.75rem;
    padding: 1rem 1.5rem;
    border-top: 1px solid var(--border-color);
    background: var(--bg-secondary);
    border-radius: 0 0 0.75rem 0.75rem;
}

/* Addon Options */
.addon-options {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 0.75rem;
    margin-bottom: 1.5rem;
}

.addon-option input {
    display: none;
}

.addon-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.5rem;
    padding: 1.25rem 1rem;
    border: 2px solid var(--border-color);
    border-radius: 0.5rem;
    cursor: pointer;
    transition: all 0.15s ease;
    text-align: center;
}

.addon-option input:checked + .addon-card {
    border-color: var(--primary);
    background: var(--primary-light);
}

.addon-card:hover {
    border-color: var(--primary);
}

.addon-card svg {
    color: var(--text-muted);
}

.addon-option input:checked + .addon-card svg {
    color: var(--primary);
}

.addon-name {
    font-size: 0.875rem;
    font-weight: 600;
    color: var(--text-primary);
}

.addon-desc {
    font-size: 0.75rem;
    color: var(--text-muted);
}

/* Danger Zone */
.danger-zone {
    border-color: var(--danger);
    background: var(--danger-light);
}

.danger-zone h2 {
    color: var(--danger);
}

.danger-zone p {
    color: var(--text-secondary);
    margin-bottom: 1rem;
    font-size: 0.875rem;
}

/* Toast */
#toast-container {
    position: fixed;
    bottom: 1.5rem;
    right: 1.5rem;
    z-index: 300;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
}

.toast {
    background: var(--sidebar-bg);
    color: white;
    padding: 0.875rem 1.25rem;
    border-radius: 0.5rem;
    box-shadow: var(--modal-shadow);
    animation: toastIn 0.3s ease;
    display: flex;
    align-items: center;
    gap: 0.75rem;
    max-width: 360px;
}

.toast.success {
    background: var(--success);
}

.toast.error {
    background: var(--danger);
}

.toast.warning {
    background: var(--warning);
}

@keyframes toastIn {
    from {
        transform: translateX(100%);
        opacity: 0;
    }
    to {
        transform: translateX(0);
        opacity: 1;
    }
}

/* Empty State */
.empty-state {
    text-align: center;
    padding: 3rem;
    color: var(--text-secondary);
}

.empty-state.small {
    padding: 1.5rem;
}

.empty-icon {
    width: 48px;
    height: 48px;
    margin: 0 auto 1rem;
    color: var(--text-muted);
}

.empty-state h3 {
    font-size: 1.125rem;
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: 0.5rem;
}

.empty-state p {
    margin-bottom: 1.5rem;
}

/* Loading */
.loading {
    text-align: center;
    padding: 2rem;
    color: var(--text-muted);
}

.loading-screen {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 50vh;
    color: var(--text-muted);
}

.spinner {
    width: 32px;
    height: 32px;
    border: 3px solid var(--border-color);
    border-top-color: var(--primary);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
    margin-bottom: 1rem;
}

@keyframes spin {
    to { transform: rotate(360deg); }
}

/* Login Page */
.login-page {
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 100vh;
    background: var(--bg-secondary);
}

.login-container {
    width: 100%;
    max-width: 400px;
    padding: 1rem;
}

.login-card {
    background: var(--bg-primary);
    border: 1px solid var(--border-color);
    border-radius: 0.75rem;
    padding: 2rem;
    box-shadow: var(--card-shadow);
}

.login-header {
    text-align: center;
    margin-bottom: 2rem;
}

.login-header .logo {
    width: 48px;
    height: 48px;
    margin: 0 auto 1rem;
}

.login-header h1 {
    font-size: 1.5rem;
    font-weight: 600;
    margin-bottom: 0.25rem;
}

.login-header p {
    color: var(--text-secondary);
}

.login-form .form-group {
    margin-bottom: 1.5rem;
}

.login-footer {
    text-align: center;
    margin-top: 1.5rem;
    padding-top: 1.5rem;
    border-top: 1px solid var(--border-color);
}

.login-footer p {
    font-size: 0.875rem;
    color: var(--text-muted);
}

/* Utilities */
.mt-4 {
    margin-top: 1rem;
}

.mb-4 {
    margin-bottom: 1rem;
}

[x-cloak] {
    display: none !important;
}

/* Responsive */
@media (max-width: 1024px) {
    .sidebar {
        transform: translateX(-100%);
        transition: transform 0.2s ease;
    }

    .sidebar.open {
        transform: translateX(0);
    }

    .main-wrapper {
        margin-left: 0 !important;
    }

    .mobile-menu-btn {
        display: flex;
    }
}

@media (max-width: 768px) {
    .search-box {
        width: 100%;
        max-width: none;
    }

    .topbar {
        padding: 0 1rem;
        flex-wrap: wrap;
        height: auto;
        padding-top: 0.75rem;
        padding-bottom: 0.75rem;
        gap: 0.75rem;
    }

    .topbar-left {
        width: 100%;
    }

    .topbar-actions {
        width: 100%;
        justify-content: flex-end;
    }

    .btn-text {
        display: none;
    }

    .apps-grid {
        grid-template-columns: 1fr;
    }

    .addon-options {
        grid-template-columns: 1fr;
    }

    .info-grid {
        grid-template-columns: 1fr;
    }

    .metrics-grid {
        grid-template-columns: repeat(2, 1fr);
    }

    .tabs {
        gap: 0;
    }

    .tab {
        padding: 0.75rem 1rem;
        flex: 1;
        text-align: center;
    }

    .content {
        padding: 1rem;
    }
}

@media (max-width: 480px) {
    .modal {
        max-width: none;
        margin: 0;
        border-radius: 0.75rem 0.75rem 0 0;
        max-height: 90vh;
        position: fixed;
        bottom: 0;
    }

    .modal-backdrop {
        align-items: flex-end;
        padding: 0;
    }

    .metrics-grid {
        grid-template-columns: 1fr;
    }
}
"##;

const DASHBOARD_JS: &str = r##"
// Dashboard Alpine.js Component
function dashboard() {
    return {
        theme: localStorage.getItem('theme') || 'dark',
        sidebarCollapsed: localStorage.getItem('sidebarCollapsed') === 'true',
        mobileMenuOpen: false,
        currentPage: 'apps',
        showModal: null,
        currentApp: null,

        init() {
            document.body.classList.add(this.theme);
            this.setupHtmxHandlers();
            this.setupKeyboardShortcuts();

            // Restore current page from URL
            const path = window.location.pathname;
            if (path.includes('/pipelines')) this.currentPage = 'pipelines';
            else if (path.includes('/addons')) this.currentPage = 'addons';
            else if (path.includes('/scheduler')) this.currentPage = 'scheduler';
            else if (path.includes('/metrics')) this.currentPage = 'metrics';
            else if (path.includes('/settings')) this.currentPage = 'settings';
            else this.currentPage = 'apps';

            // Extract current app from URL
            const appMatch = path.match(/\/apps\/([^\/]+)/);
            if (appMatch) {
                this.currentApp = appMatch[1];
            }
        },

        toggleTheme() {
            this.theme = this.theme === 'dark' ? 'light' : 'dark';
            document.body.classList.remove('dark', 'light');
            document.body.classList.add(this.theme);
            localStorage.setItem('theme', this.theme);
        },

        toggleSidebar() {
            this.sidebarCollapsed = !this.sidebarCollapsed;
            localStorage.setItem('sidebarCollapsed', this.sidebarCollapsed);
        },

        searchApps(query) {
            if (query.length > 0) {
                htmx.ajax('GET', `/dashboard/apps?q=${encodeURIComponent(query)}`, '#main-content');
            } else {
                htmx.ajax('GET', '/dashboard/apps', '#main-content');
            }
        },

        setupHtmxHandlers() {
            // Handle HTMX errors
            document.body.addEventListener('htmx:responseError', (e) => {
                const status = e.detail.xhr.status;
                if (status === 401) {
                    window.location.href = '/dashboard/login';
                } else if (status === 403) {
                    showToast('Permission denied', 'error');
                } else if (status >= 500) {
                    showToast('Server error. Please try again.', 'error');
                } else {
                    try {
                        const data = JSON.parse(e.detail.xhr.responseText);
                        showToast(data.error || 'Request failed', 'error');
                    } catch {
                        showToast('Request failed', 'error');
                    }
                }
            });

            // Update current app from URL changes
            document.body.addEventListener('htmx:pushedIntoHistory', (e) => {
                const appMatch = e.detail.path.match(/\/apps\/([^\/]+)/);
                if (appMatch) {
                    this.currentApp = appMatch[1];
                }
            });
        },

        setupKeyboardShortcuts() {
            document.addEventListener('keydown', (e) => {
                // Ctrl/Cmd + K for search
                if ((e.ctrlKey || e.metaKey) && e.key === 'k') {
                    e.preventDefault();
                    document.querySelector('.search-input')?.focus();
                }

                // Escape to close modals
                if (e.key === 'Escape') {
                    this.showModal = null;
                }

                // N for new app (when not in input)
                if (e.key === 'n' && !['INPUT', 'TEXTAREA'].includes(document.activeElement.tagName)) {
                    e.preventDefault();
                    this.showModal = 'create-app';
                }
            });
        },

        handleAppCreated(event) {
            if (event.detail.successful) {
                this.showModal = null;
                showToast('App created successfully!', 'success');
                htmx.ajax('GET', '/dashboard/apps', '#main-content');
            } else {
                try {
                    const data = JSON.parse(event.detail.xhr.responseText);
                    showToast(data.error || 'Failed to create app', 'error');
                } catch {
                    showToast('Failed to create app', 'error');
                }
            }
        },

        handleConfigAdded(event) {
            if (event.detail.successful) {
                this.showModal = null;
                showToast('Config variable added!', 'success');
                htmx.trigger('#config-list', 'htmx:trigger');
            }
        },

        handleAddonAdded(event) {
            if (event.detail.successful) {
                this.showModal = null;
                showToast('Add-on provisioned!', 'success');
                htmx.trigger('#addons-list', 'htmx:trigger');
            }
        },

        handleDomainAdded(event) {
            if (event.detail.successful) {
                this.showModal = null;
                showToast('Domain added! Configure DNS to complete setup.', 'success');
                htmx.trigger('#domains-list', 'htmx:trigger');
            }
        }
    };
}

// Toast Notifications
function showToast(message, type = 'info') {
    const container = document.getElementById('toast-container');
    if (!container) return;

    const toast = document.createElement('div');
    toast.className = `toast ${type}`;

    const icon = type === 'success'
        ? '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7"/>'
        : type === 'error'
        ? '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/>'
        : type === 'warning'
        ? '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z"/>'
        : '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"/>';

    toast.innerHTML = `
        <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="20" height="20">${icon}</svg>
        <span>${escapeHtml(message)}</span>
    `;

    container.appendChild(toast);

    setTimeout(() => {
        toast.style.animation = 'toastIn 0.3s ease reverse';
        setTimeout(() => toast.remove(), 300);
    }, 4000);
}

// Escape HTML to prevent XSS
function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

// Logs functionality
let followLogs = true;

function toggleLogsFollow() {
    followLogs = !followLogs;
    const btn = document.getElementById('logs-follow-btn');
    if (btn) {
        btn.classList.toggle('btn-primary', followLogs);
        btn.classList.toggle('btn-secondary', !followLogs);
        btn.textContent = followLogs ? 'Following' : 'Follow';
    }
}

function clearLogs() {
    const container = document.getElementById('logs-container');
    if (container) {
        container.innerHTML = '';
    }
}

// Auto-scroll logs when following
document.addEventListener('htmx:afterSwap', (e) => {
    if (e.detail.target.id === 'logs-container' && followLogs) {
        e.detail.target.scrollTop = e.detail.target.scrollHeight;
    }
});

// HTMX Configuration
document.addEventListener('DOMContentLoaded', () => {
    // Configure HTMX
    if (typeof htmx !== 'undefined') {
        htmx.config.defaultSwapStyle = 'innerHTML';
        htmx.config.historyCacheSize = 10;
        htmx.config.refreshOnHistoryMiss = true;

        // Add auth header to all requests
        document.body.addEventListener('htmx:configRequest', (e) => {
            const token = localStorage.getItem('paas_token') || getCookie('paas_token');
            if (token) {
                e.detail.headers['Authorization'] = `Bearer ${token}`;
            }
        });

        // Show loading indicator
        document.body.addEventListener('htmx:beforeRequest', () => {
            document.body.classList.add('htmx-request');
        });

        document.body.addEventListener('htmx:afterRequest', () => {
            document.body.classList.remove('htmx-request');
        });
    }
});

// Helper to get cookie value
function getCookie(name) {
    const value = `; ${document.cookie}`;
    const parts = value.split(`; ${name}=`);
    if (parts.length === 2) return parts.pop().split(';').shift();
    return null;
}

// Format timestamps
function formatDate(timestamp) {
    if (!timestamp) return 'N/A';
    const date = typeof timestamp === 'number'
        ? new Date(timestamp * 1000)
        : new Date(timestamp);
    if (isNaN(date.getTime())) return timestamp;
    return new Intl.DateTimeFormat('en-US', {
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit'
    }).format(date);
}

// Format relative time
function timeAgo(timestamp) {
    const seconds = Math.floor((Date.now() - new Date(timestamp).getTime()) / 1000);

    const intervals = [
        { label: 'year', seconds: 31536000 },
        { label: 'month', seconds: 2592000 },
        { label: 'week', seconds: 604800 },
        { label: 'day', seconds: 86400 },
        { label: 'hour', seconds: 3600 },
        { label: 'minute', seconds: 60 }
    ];

    for (const interval of intervals) {
        const count = Math.floor(seconds / interval.seconds);
        if (count >= 1) {
            return `${count} ${interval.label}${count > 1 ? 's' : ''} ago`;
        }
    }

    return 'just now';
}

// Copy to clipboard
function copyToClipboard(text) {
    navigator.clipboard.writeText(text).then(() => {
        showToast('Copied to clipboard!', 'success');
    }).catch(() => {
        showToast('Failed to copy', 'error');
    });
}

// Confirm dangerous actions
function confirmAction(message, callback) {
    if (confirm(message)) {
        callback();
    }
}
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serve_dashboard() {
        let response = serve_dashboard();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap().contains("text/html"));
    }

    #[test]
    fn test_serve_login() {
        let response = serve_login();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap().contains("text/html"));
    }

    #[test]
    fn test_serve_css() {
        let response = serve_css();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap().contains("text/css"));
    }

    #[test]
    fn test_serve_js() {
        let response = serve_js();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap().contains("javascript"));
    }

    #[test]
    fn test_render_apps_list_empty() {
        let apps: Vec<serde_json::Value> = vec![];
        let html = render_apps_list(&apps);
        assert!(html.contains("No apps yet"));
        assert!(html.contains("Create your first app"));
    }

    #[test]
    fn test_render_apps_list_with_apps() {
        let apps = vec![
            serde_json::json!({
                "name": "test-app",
                "status": "running",
                "port": 3000
            })
        ];
        let html = render_apps_list(&apps);
        assert!(html.contains("test-app"));
        assert!(html.contains("status-running"));
        assert!(html.contains("Port 3000"));
    }

    #[test]
    fn test_render_app_detail() {
        let app = serde_json::json!({
            "name": "my-app",
            "status": "running",
            "port": 8080,
            "git_url": "https://github.com/test/repo",
            "image": "my-app:latest"
        });
        let html = render_app_detail(&app);
        assert!(html.contains("my-app"));
        assert!(html.contains("status-running"));
        assert!(html.contains("8080"));
        assert!(html.contains("https://github.com/test/repo"));
    }

    #[test]
    fn test_render_config_vars_empty() {
        let config = serde_json::json!({});
        let html = render_config_vars(&config);
        assert!(html.contains("No config vars set"));
    }

    #[test]
    fn test_render_config_vars_with_values() {
        let config = serde_json::json!({
            "DATABASE_URL": "postgres://localhost/db",
            "API_KEY": "secret123"
        });
        let html = render_config_vars(&config);
        assert!(html.contains("DATABASE_URL"));
        assert!(html.contains("API_KEY"));
        assert!(html.contains("--------")); // API_KEY should be masked
    }

    #[test]
    fn test_render_domains_list_empty() {
        let domains: Vec<serde_json::Value> = vec![];
        let html = render_domains_list(&domains);
        assert!(html.contains("No custom domains configured"));
    }

    #[test]
    fn test_render_domains_list_with_domains() {
        let domains = vec![
            serde_json::json!({
                "hostname": "app.example.com",
                "dns_verified": true,
                "ssl_status": "active"
            })
        ];
        let html = render_domains_list(&domains);
        assert!(html.contains("app.example.com"));
        assert!(html.contains("Verified"));
        assert!(html.contains("SSL: active"));
    }

    #[test]
    fn test_render_addons_list_empty() {
        let addons: Vec<serde_json::Value> = vec![];
        let html = render_addons_list(&addons);
        assert!(html.contains("No add-ons attached"));
    }

    #[test]
    fn test_render_addons_list_with_addons() {
        let addons = vec![
            serde_json::json!({
                "addon_type": "postgres",
                "plan": "standard",
                "status": "running"
            })
        ];
        let html = render_addons_list(&addons);
        assert!(html.contains("postgres"));
        assert!(html.contains("standard"));
        assert!(html.contains("status-running"));
    }

    #[test]
    fn test_render_deployments_list_empty() {
        let deployments: Vec<serde_json::Value> = vec![];
        let html = render_deployments_list(&deployments);
        assert!(html.contains("No deployments yet"));
    }

    #[test]
    fn test_render_deployments_list_with_deployments() {
        let deployments = vec![
            serde_json::json!({
                "status": "success",
                "image": "app:v1.2.3",
                "duration_secs": 45.5,
                "created_at": "2024-01-15T10:30:00Z"
            })
        ];
        let html = render_deployments_list(&deployments);
        assert!(html.contains("success"));
        assert!(html.contains("app:v1.2.3"));
        assert!(html.contains("45.5s"));
    }

    #[test]
    fn test_css_contains_themes() {
        assert!(DASHBOARD_CSS.contains(":root"));
        assert!(DASHBOARD_CSS.contains(".dark"));
        assert!(DASHBOARD_CSS.contains("--bg-primary"));
        assert!(DASHBOARD_CSS.contains("--primary"));
    }

    #[test]
    fn test_html_includes_htmx() {
        assert!(DASHBOARD_HTML.contains("htmx.org"));
        assert!(DASHBOARD_HTML.contains("alpinejs"));
    }

    #[test]
    fn test_js_contains_dashboard_function() {
        assert!(DASHBOARD_JS.contains("function dashboard()"));
        assert!(DASHBOARD_JS.contains("toggleTheme"));
        assert!(DASHBOARD_JS.contains("showToast"));
        assert!(DASHBOARD_JS.contains("setupKeyboardShortcuts"));
    }

    #[test]
    fn test_login_html_structure() {
        assert!(LOGIN_HTML.contains("Sign in"));
        assert!(LOGIN_HTML.contains("API Token"));
        assert!(LOGIN_HTML.contains("Spawngate"));
    }
}
