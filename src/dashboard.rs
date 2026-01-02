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
        <h2>Instances</h2>
        <div id="instances-list" hx-get="/dashboard/apps/{0}/instances" hx-trigger="load" hx-swap="innerHTML">
            <div class="loading">Loading instances...</div>
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

<div class="tab-content" x-show="activeTab === 'deploy'" x-data="{{
    deploying: false,
    deploymentId: null,
    deployProgress: null,
    pollInterval: null,
    async startDeploy(event) {{
        event.preventDefault();
        const form = event.target;
        const formData = new FormData(form);
        this.deploying = true;
        this.deployProgress = {{ status: 'pending', step: 'Initializing...', logs: [] }};
        try {{
            const res = await fetch('/apps/{0}/deploy', {{
                method: 'POST',
                headers: {{ 'Content-Type': 'application/json' }},
                body: JSON.stringify({{
                    git_url: formData.get('git_url'),
                    git_ref: formData.get('git_ref')
                }})
            }});
            const data = await res.json();
            if (data.success && data.data) {{
                this.deploymentId = data.data.id || data.data.deployment_id;
                this.startPolling();
            }} else {{
                this.deployProgress = {{ status: 'failed', step: data.error || 'Failed to start deployment', logs: [] }};
                this.deploying = false;
            }}
        }} catch (e) {{
            this.deployProgress = {{ status: 'failed', step: 'Network error', logs: [] }};
            this.deploying = false;
        }}
    }},
    startPolling() {{
        this.pollInterval = setInterval(async () => {{
            try {{
                const res = await fetch('/apps/{0}/deployments/' + this.deploymentId);
                const data = await res.json();
                if (data.success && data.data) {{
                    const d = data.data;
                    this.deployProgress = {{
                        status: d.status,
                        step: this.getStepFromStatus(d.status),
                        image: d.image,
                        duration: d.duration_secs,
                        logs: d.build_logs ? d.build_logs.split('\\n').slice(-20) : []
                    }};
                    if (d.status === 'success' || d.status === 'failed') {{
                        this.stopPolling();
                        htmx.trigger('#deployments-list', 'reload');
                    }}
                }}
            }} catch (e) {{ }}
        }}, 2000);
    }},
    stopPolling() {{
        if (this.pollInterval) {{
            clearInterval(this.pollInterval);
            this.pollInterval = null;
        }}
        this.deploying = false;
    }},
    getStepFromStatus(status) {{
        switch(status) {{
            case 'pending': return 'Queued...';
            case 'cloning': return 'Cloning repository...';
            case 'building': return 'Building image...';
            case 'pushing': return 'Pushing to registry...';
            case 'deploying': return 'Deploying containers...';
            case 'success': return 'Deployment complete!';
            case 'failed': return 'Deployment failed';
            default: return status;
        }}
    }},
    closeDeploy() {{
        this.stopPolling();
        this.deployProgress = null;
        this.deploymentId = null;
    }}
}}">
    <!-- Active Deployment Progress -->
    <div class="card deploy-progress-card" x-show="deployProgress" x-cloak>
        <div class="card-header">
            <h2>Deployment in Progress</h2>
            <button class="btn btn-icon" @click="closeDeploy()" x-show="!deploying" title="Close">
                <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="20" height="20">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/>
                </svg>
            </button>
        </div>
        <div class="deploy-progress">
            <div class="deploy-status" :class="'deploy-status-' + (deployProgress?.status || 'pending')">
                <div class="deploy-status-icon">
                    <svg x-show="deploying" class="spinner" fill="none" stroke="currentColor" viewBox="0 0 24 24" width="24" height="24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"/>
                    </svg>
                    <svg x-show="deployProgress?.status === 'success'" fill="none" stroke="currentColor" viewBox="0 0 24 24" width="24" height="24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"/>
                    </svg>
                    <svg x-show="deployProgress?.status === 'failed'" fill="none" stroke="currentColor" viewBox="0 0 24 24" width="24" height="24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10 14l2-2m0 0l2-2m-2 2l-2-2m2 2l2 2m7-2a9 9 0 11-18 0 9 9 0 0118 0z"/>
                    </svg>
                </div>
                <div class="deploy-status-text">
                    <span class="deploy-step" x-text="deployProgress?.step"></span>
                    <span class="deploy-duration" x-show="deployProgress?.duration" x-text="deployProgress?.duration?.toFixed(1) + 's'"></span>
                </div>
            </div>
            <div class="deploy-progress-bar" x-show="deploying">
                <div class="deploy-progress-fill"></div>
            </div>
            <div class="deploy-logs" x-show="deployProgress?.logs?.length > 0">
                <div class="deploy-logs-header">Build Output</div>
                <pre class="deploy-logs-content"><template x-for="line in deployProgress?.logs || []"><span x-text="line + '\\n'"></span></template></pre>
            </div>
        </div>
    </div>

    <div class="card" x-show="!deployProgress">
        <h2>Deploy from Git</h2>
        <form @submit.prevent="startDeploy($event)" class="deploy-form">
            <div class="form-group">
                <label for="git-url">Git Repository URL</label>
                <input type="url" id="git-url" name="git_url" placeholder="https://github.com/user/repo.git" value="{6}" class="input">
                <small>HTTPS or SSH URL to your repository</small>
            </div>
            <div class="form-group">
                <label for="git-ref">Branch/Tag/Commit</label>
                <input type="text" id="git-ref" name="git_ref" placeholder="main" value="main" class="input">
                <small>Branch name, tag, or commit SHA</small>
            </div>
            <button type="submit" class="btn btn-primary" :disabled="deploying">
                <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M7 16a4 4 0 01-.88-7.903A5 5 0 1115.9 6L16 6a5 5 0 011 9.9M15 13l-3-3m0 0l-3 3m3-3v12"/>
                </svg>
                Deploy
            </button>
        </form>
    </div>

    <div class="card">
        <div class="card-header">
            <h2>Deployment History</h2>
            <button class="btn btn-secondary btn-sm" hx-get="/dashboard/apps/{0}/deployments" hx-target="#deployments-list" hx-swap="innerHTML">
                <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="14" height="14">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"/>
                </svg>
                Refresh
            </button>
        </div>
        <div id="deployments-list" hx-get="/dashboard/apps/{0}/deployments" hx-trigger="load, reload from:body" hx-swap="innerHTML">
            <div class="loading">Loading deployments...</div>
        </div>
    </div>
</div>

<div class="tab-content" x-show="activeTab === 'logs'" x-data="{{
    logSource: 'all',
    logLevel: 'all',
    searchQuery: '',
    followLogs: true,
    allLogs: [],
    filteredLogs: [],
    init() {{
        this.loadLogs();
        this.$watch('logSource', () => this.filterLogs());
        this.$watch('logLevel', () => this.filterLogs());
        this.$watch('searchQuery', () => this.filterLogs());
    }},
    async loadLogs() {{
        try {{
            const res = await fetch('/dashboard/apps/{0}/logs?limit=500');
            const data = await res.json();
            if (data.logs) {{
                this.allLogs = data.logs;
                this.filterLogs();
            }}
        }} catch (e) {{ }}
        if (this.followLogs) {{
            setTimeout(() => this.loadLogs(), 2000);
        }}
    }},
    filterLogs() {{
        this.filteredLogs = this.allLogs.filter(log => {{
            if (this.logSource !== 'all' && log.source !== this.logSource) return false;
            if (this.logLevel !== 'all' && log.level !== this.logLevel) return false;
            if (this.searchQuery && !log.message.toLowerCase().includes(this.searchQuery.toLowerCase())) return false;
            return true;
        }});
    }},
    toggleFollow() {{
        this.followLogs = !this.followLogs;
        if (this.followLogs) this.loadLogs();
    }},
    clearLogs() {{
        this.allLogs = [];
        this.filteredLogs = [];
    }},
    downloadLogs() {{
        const content = this.filteredLogs.map(l => `[${{l.timestamp}}] [${{l.source}}] [${{l.level}}] ${{l.message}}`).join('\\n');
        const blob = new Blob([content], {{ type: 'text/plain' }});
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = '{0}-logs-' + new Date().toISOString().slice(0,10) + '.txt';
        a.click();
        URL.revokeObjectURL(url);
        showToast('Logs downloaded', 'success');
    }},
    getLogClass(level) {{
        switch(level) {{
            case 'error': return 'log-error';
            case 'warn': return 'log-warn';
            case 'info': return 'log-info';
            case 'debug': return 'log-debug';
            default: return '';
        }}
    }}
}}">
    <div class="card logs-card">
        <div class="card-header">
            <h2>Application Logs</h2>
            <div class="logs-toolbar">
                <div class="logs-search">
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z"/>
                    </svg>
                    <input type="text" x-model="searchQuery" placeholder="Search logs..." class="input input-sm">
                </div>
                <select class="select select-sm" x-model="logSource">
                    <option value="all">All Sources</option>
                    <option value="app">App</option>
                    <option value="router">Router</option>
                    <option value="build">Build</option>
                    <option value="instance">Instance</option>
                </select>
                <select class="select select-sm" x-model="logLevel">
                    <option value="all">All Levels</option>
                    <option value="error">Error</option>
                    <option value="warn">Warning</option>
                    <option value="info">Info</option>
                    <option value="debug">Debug</option>
                </select>
            </div>
        </div>
        <div class="logs-actions">
            <span class="logs-count" x-text="filteredLogs.length + ' logs'"></span>
            <div class="logs-buttons">
                <button class="btn btn-secondary btn-sm" @click="clearLogs()">
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="14" height="14">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"/>
                    </svg>
                    Clear
                </button>
                <button class="btn btn-secondary btn-sm" @click="downloadLogs()">
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="14" height="14">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4"/>
                    </svg>
                    Download
                </button>
                <button class="btn btn-sm" :class="followLogs ? 'btn-primary' : 'btn-secondary'" @click="toggleFollow()">
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="14" height="14">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 14l-7 7m0 0l-7-7m7 7V3"/>
                    </svg>
                    <span x-text="followLogs ? 'Following' : 'Follow'"></span>
                </button>
            </div>
        </div>
        <div class="logs-container" id="logs-container">
            <template x-if="filteredLogs.length === 0">
                <div class="logs-empty">
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="32" height="32">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"/>
                    </svg>
                    <p x-show="allLogs.length === 0">No logs yet</p>
                    <p x-show="allLogs.length > 0">No logs match your filters</p>
                </div>
            </template>
            <template x-for="log in filteredLogs" :key="log.id || log.timestamp">
                <div class="log-line" :class="getLogClass(log.level)">
                    <span class="log-timestamp" x-text="log.timestamp"></span>
                    <span class="log-source" x-text="log.source"></span>
                    <span class="log-level" x-text="log.level"></span>
                    <span class="log-message" x-text="log.message"></span>
                </div>
            </template>
        </div>
    </div>
</div>

<div class="tab-content" x-show="activeTab === 'settings'">
    <div class="card">
        <h2>General Settings</h2>
        <form hx-put="/apps/{0}" hx-swap="none" hx-on::after-request="showToast('Settings saved!', 'success')" class="settings-form">
            <div class="form-row">
                <div class="form-group">
                    <label for="settings-port">Application Port</label>
                    <input type="number" id="settings-port" name="port" value="{3}" min="1" max="65535" class="input">
                    <small>The port your application listens on inside the container</small>
                </div>
            </div>
            <div class="form-actions">
                <button type="submit" class="btn btn-primary">Save Changes</button>
            </div>
        </form>
    </div>

    <div class="card" x-data="{{ minScale: 0, maxScale: 10, currentScale: 1 }}" x-init="
        fetch('/apps/{0}').then(r => r.json()).then(d => {{
            if (d.data) {{
                minScale = d.data.min_scale || 0;
                maxScale = d.data.max_scale || 10;
                currentScale = d.data.scale || 1;
            }}
        }})
    ">
        <h2>Scaling</h2>
        <div class="settings-form">
            <div class="form-row">
                <div class="form-group">
                    <label>Current Scale</label>
                    <div class="scale-display">
                        <span class="scale-value" x-text="currentScale"></span>
                        <span class="scale-label">instances</span>
                    </div>
                </div>
            </div>
            <div class="form-row two-col">
                <div class="form-group">
                    <label for="settings-min-scale">Minimum Instances</label>
                    <input type="number" id="settings-min-scale" x-model.number="minScale" min="0" max="10" class="input">
                    <small>Scale to 0 to enable idle shutdown</small>
                </div>
                <div class="form-group">
                    <label for="settings-max-scale">Maximum Instances</label>
                    <input type="number" id="settings-max-scale" x-model.number="maxScale" min="1" max="100" class="input">
                    <small>Maximum instances for auto-scaling</small>
                </div>
            </div>
            <div class="form-actions">
                <button type="button" class="btn btn-primary"
                    @click="fetch('/apps/{0}/scale', {{
                        method: 'POST',
                        headers: {{ 'Content-Type': 'application/json' }},
                        body: JSON.stringify({{ min_scale: minScale, max_scale: maxScale }})
                    }}).then(() => showToast('Scaling settings saved!', 'success'))">
                    Save Scaling
                </button>
            </div>
        </div>
    </div>

    <div class="card">
        <h2>Maintenance Mode</h2>
        <div class="settings-row" x-data="{{ maintenanceMode: false }}">
            <div class="settings-info">
                <strong>Enable Maintenance Mode</strong>
                <small>Show a maintenance page to all visitors while you make changes</small>
            </div>
            <label class="toggle">
                <input type="checkbox" x-model="maintenanceMode"
                    @change="fetch('/apps/{0}/maintenance', {{
                        method: 'PUT',
                        headers: {{ 'Content-Type': 'application/json' }},
                        body: JSON.stringify({{ enabled: maintenanceMode }})
                    }}).then(() => showToast(maintenanceMode ? 'Maintenance mode enabled' : 'Maintenance mode disabled', 'success'))">
                <span class="toggle-slider"></span>
            </label>
        </div>
    </div>

    <div class="card danger-zone">
        <h2>Danger Zone</h2>
        <div class="danger-item">
            <div class="danger-info">
                <strong>Transfer Ownership</strong>
                <small>Transfer this app to another user or team</small>
            </div>
            <button class="btn btn-outline-danger" disabled title="Coming soon">Transfer</button>
        </div>
        <div class="danger-item">
            <div class="danger-info">
                <strong>Delete App</strong>
                <small>Once deleted, all data, deployments, and add-ons will be permanently removed</small>
            </div>
            <button class="btn btn-danger" @click="confirmDeleteApp()">Delete App</button>
        </div>
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
        return r##"<div class="empty-state small">
            <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="32" height="32">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 6V4m0 2a2 2 0 100 4m0-4a2 2 0 110 4m-6 8a2 2 0 100-4m0 4a2 2 0 110-4m0 4v2m0-6V4m6 6v10m6-2a2 2 0 100-4m0 4a2 2 0 110-4m0 4v2m0-6V4"/>
            </svg>
            <p>No config vars set</p>
            <p class="text-muted">Add environment variables to configure your app</p>
        </div>"##.to_string();
    }

    let items: Vec<String> = env.unwrap().iter().map(|(key, value)| {
        let val = value.as_str().unwrap_or("");
        let is_secret = key.contains("PASSWORD") || key.contains("SECRET") || key.contains("KEY") || key.contains("TOKEN") || key.contains("API");
        let display_value = if is_secret {
            "••••••••".to_string()
        } else if val.len() > 50 {
            format!("{}...", &val[..50])
        } else {
            val.to_string()
        };
        let escaped_value = val.replace('\\', "\\\\").replace('\'', "\\'").replace('\n', "\\n");

        format!(
            r##"<div class="config-item" x-data="{{ showValue: false }}">
            <div class="config-info">
                <span class="config-key">{0}</span>
                <span class="config-value" x-show="!showValue">{1}</span>
                <span class="config-value config-value-revealed" x-show="showValue" x-cloak>{2}</span>
            </div>
            <div class="config-actions">
                <button class="btn btn-icon" @click="showValue = !showValue" title="Toggle visibility" x-show="{3}">
                    <svg x-show="!showValue" fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"/>
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z"/>
                    </svg>
                    <svg x-show="showValue" x-cloak fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.88 9.88l-3.29-3.29m7.532 7.532l3.29 3.29M3 3l3.59 3.59m0 0A9.953 9.953 0 0112 5c4.478 0 8.268 2.943 9.543 7a10.025 10.025 0 01-4.132 5.411m0 0L21 21"/>
                    </svg>
                </button>
                <button class="btn btn-icon" @click="openEditConfig('{0}', '{4}')" title="Edit">
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z"/>
                    </svg>
                </button>
                <button class="btn btn-icon btn-danger" title="Delete"
                    hx-delete="/apps/current/config/{0}"
                    hx-confirm="Delete {0}?"
                    hx-swap="none"
                    hx-on::after-request="htmx.trigger('#config-list', 'reload')">
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"/>
                    </svg>
                </button>
            </div>
        </div>"##,
            key, display_value, val, is_secret, escaped_value
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
        return r##"<div class="empty-state small">
            <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="32" height="32">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M7 16a4 4 0 01-.88-7.903A5 5 0 1115.9 6L16 6a5 5 0 011 9.9M15 13l-3-3m0 0l-3 3m3-3v12"/>
            </svg>
            <p>No deployments yet</p>
            <p class="text-muted">Deploy your app using git push or the form above</p>
        </div>"##.to_string();
    }

    let items: Vec<String> = deployments.iter().enumerate().take(10).map(|(idx, deploy)| {
        let id = deploy["id"].as_str().unwrap_or("");
        let status = deploy["status"].as_str().unwrap_or("pending");
        let image = deploy["image"].as_str().unwrap_or("N/A");
        let commit = deploy["commit_hash"].as_str().map(|c| if c.len() > 7 { &c[..7] } else { c }).unwrap_or("");
        let duration = deploy["duration_secs"].as_f64().map(|d| format!("{:.1}s", d)).unwrap_or_else(|| "-".to_string());
        let created = deploy["created_at"].as_str().unwrap_or("");

        let status_class = match status {
            "success" => "status-running",
            "building" | "pending" | "cloning" | "pushing" | "deploying" => "status-building",
            "failed" => "status-failed",
            _ => "status-idle",
        };

        let status_icon = match status {
            "success" => r##"<svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>"##,
            "building" | "pending" | "cloning" | "pushing" | "deploying" => r##"<svg class="spinner" fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"/></svg>"##,
            "failed" => r##"<svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10 14l2-2m0 0l2-2m-2 2l-2-2m2 2l2 2m7-2a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>"##,
            _ => r##"<svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>"##,
        };

        let is_current = idx == 0 && status == "success";
        let current_badge = if is_current { r##"<span class="badge badge-current">current</span>"## } else { "" };

        let rollback_btn = if !is_current && status == "success" && !image.is_empty() && image != "N/A" {
            format!(
                r##"<button class="btn btn-sm btn-secondary"
                    hx-post="/apps/current/rollback"
                    hx-vals='{{"deployment_id": "{0}"}}'
                    hx-confirm="Rollback to this deployment?"
                    hx-swap="none"
                    hx-on::after-request="showToast('Rollback initiated', 'success'); htmx.trigger('#deployments-list', 'reload')">
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="14" height="14">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 10h10a8 8 0 018 8v2M3 10l6 6m-6-6l6-6"/>
                    </svg>
                    Rollback
                </button>"##,
                id
            )
        } else {
            String::new()
        };

        let commit_display = if !commit.is_empty() {
            format!(r##"<span class="deployment-commit" title="Commit">{0}</span>"##, commit)
        } else {
            String::new()
        };

        format!(
            r##"<div class="deployment-item" data-deployment-id="{0}">
            <div class="deployment-main">
                <div class="deployment-status {1}">
                    {2}
                </div>
                <div class="deployment-info">
                    <div class="deployment-header">
                        <span class="deployment-image">{3}</span>
                        {4}
                        {5}
                    </div>
                    <div class="deployment-meta">
                        <span class="deployment-time">{6}</span>
                        <span class="deployment-duration">{7}</span>
                    </div>
                </div>
            </div>
            <div class="deployment-actions">
                {8}
            </div>
        </div>"##,
            id, status_class, status_icon, image, commit_display, current_badge, created, duration, rollback_btn
        )
    }).collect();

    format!(r##"<div class="deployments-list">{}</div>"##, items.join(""))
}

/// Generate HTML for instances list with scaling controls
pub fn render_instances_list(
    app_name: &str,
    instances: &[serde_json::Value],
    scale: i64,
    min_scale: i64,
    max_scale: i64,
) -> String {
    // Group instances by process type
    let mut web_instances: Vec<&serde_json::Value> = Vec::new();
    let mut worker_instances: Vec<&serde_json::Value> = Vec::new();
    let mut other_instances: Vec<&serde_json::Value> = Vec::new();

    for instance in instances {
        let process_type = instance["process_type"].as_str().unwrap_or("web");
        match process_type {
            "web" => web_instances.push(instance),
            "worker" => worker_instances.push(instance),
            _ => other_instances.push(instance),
        }
    }

    let running_count = instances
        .iter()
        .filter(|i| i["status"].as_str() == Some("running"))
        .count();

    // Scale slider component
    let scale_slider = format!(
        r##"<div class="scale-control" x-data="{{ targetScale: {0}, isScaling: false }}">
    <div class="scale-header">
        <div class="scale-info">
            <span class="scale-label">Instance Count</span>
            <span class="scale-current">{1} running / {0} target</span>
        </div>
    </div>
    <div class="scale-slider-container">
        <input type="range" class="scale-slider"
            min="{2}" max="{3}" x-model="targetScale"
            @change="if(targetScale != {0}) {{ isScaling = true; $dispatch('scale-app', {{ count: targetScale }}) }}">
        <div class="scale-input-group">
            <input type="number" class="scale-input"
                min="{2}" max="{3}" x-model="targetScale"
                @keyup.enter="if(targetScale != {0}) {{ isScaling = true; $dispatch('scale-app', {{ count: targetScale }}) }}">
            <button class="btn btn-primary btn-sm"
                @click="if(targetScale != {0}) {{ isScaling = true; $dispatch('scale-app', {{ count: targetScale }}) }}"
                :disabled="targetScale == {0} || isScaling"
                x-text="isScaling ? 'Scaling...' : 'Apply'">Apply</button>
        </div>
    </div>
    <div class="scale-limits">
        <span>Min: {2}</span>
        <span>Max: {3}</span>
    </div>
</div>
<div x-show="isScaling" class="scale-progress">
    <div class="loading-spinner"></div>
    <span>Scaling in progress...</span>
</div>
<div class="htmx-listener"
    hx-post="/apps/{4}/scale"
    hx-trigger="scale-app from:body"
    hx-swap="none"
    hx-vals="js:{{count: event.detail.count}}"
    hx-on::after-request="showToast('Scale updated', 'success'); setTimeout(() => htmx.trigger('#instances-list', 'reload'), 1000)"
    ></div>"##,
        scale, running_count, min_scale, max_scale, app_name
    );

    // Render process type cards
    let web_card = render_process_type_card(app_name, "web", &web_instances);
    let worker_card = if !worker_instances.is_empty() {
        render_process_type_card(app_name, "worker", &worker_instances)
    } else {
        String::new()
    };
    let other_card = if !other_instances.is_empty() {
        render_process_type_card(app_name, "other", &other_instances)
    } else {
        String::new()
    };

    // Resource usage overview section
    let resource_overview = format!(
        r##"<div class="resource-overview">
    <h3>Resource Usage</h3>
    <div class="resource-graphs" x-data="resourceMonitor('{0}')" x-init="startPolling()">
        <div class="resource-card">
            <div class="resource-header">
                <span class="resource-label">CPU Usage</span>
                <span class="resource-value" x-text="cpuUsage + '%'">--</span>
            </div>
            <div class="resource-bar">
                <div class="resource-bar-fill cpu" :style="'width: ' + cpuUsage + '%'" :class="{{'warning': cpuUsage > 70, 'danger': cpuUsage > 90}}"></div>
            </div>
        </div>
        <div class="resource-card">
            <div class="resource-header">
                <span class="resource-label">Memory Usage</span>
                <span class="resource-value" x-text="memoryUsage + '%'">--</span>
            </div>
            <div class="resource-bar">
                <div class="resource-bar-fill memory" :style="'width: ' + memoryUsage + '%'" :class="{{'warning': memoryUsage > 70, 'danger': memoryUsage > 90}}"></div>
            </div>
        </div>
        <div class="resource-details">
            <div class="resource-detail">
                <span class="detail-label">CPU Cores</span>
                <span class="detail-value" x-text="cpuCores">--</span>
            </div>
            <div class="resource-detail">
                <span class="detail-label">Memory</span>
                <span class="detail-value" x-text="memoryUsed + ' / ' + memoryLimit">--</span>
            </div>
        </div>
    </div>
</div>"##,
        app_name
    );

    format!(
        r##"<div class="instances-container" hx-trigger="reload from:body" hx-get="/dashboard/apps/{0}/instances" hx-swap="innerHTML">
    {1}
    {5}
    <div class="process-types">
        {2}
        {3}
        {4}
    </div>
</div>"##,
        app_name, scale_slider, web_card, worker_card, other_card, resource_overview
    )
}

fn render_process_type_card(app_name: &str, process_type: &str, instances: &[&serde_json::Value]) -> String {
    let type_icon = match process_type {
        "web" => r##"<svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="20" height="20"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M21 12a9 9 0 01-9 9m9-9a9 9 0 00-9-9m9 9H3m9 9a9 9 0 01-9-9m9 9c1.657 0 3-4.03 3-9s-1.343-9-3-9m0 18c-1.657 0-3-4.03-3-9s1.343-9 3-9m-9 9a9 9 0 019-9"/></svg>"##,
        "worker" => r##"<svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="20" height="20"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"/><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"/></svg>"##,
        _ => r##"<svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="20" height="20"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4"/></svg>"##,
    };

    let type_label = match process_type {
        "web" => "Web",
        "worker" => "Worker",
        _ => "Other",
    };

    let running_count = instances.iter().filter(|i| i["status"].as_str() == Some("running")).count();
    let total_count = instances.len();

    let instance_items: Vec<String> = instances.iter().map(|instance| {
        let id = instance["id"].as_str().unwrap_or("");
        let status = instance["status"].as_str().unwrap_or("unknown");
        let health = instance["health_status"].as_str().unwrap_or("unknown");
        let port = instance["port"].as_i64().unwrap_or(0);
        let _started_at = instance["started_at"].as_str().unwrap_or("");

        let status_class = match status {
            "running" => "status-running",
            "starting" => "status-building",
            "stopped" => "status-idle",
            _ => "status-failed",
        };

        let health_class = match health {
            "healthy" => "health-healthy",
            "unhealthy" => "health-unhealthy",
            _ => "health-unknown",
        };

        let health_icon = match health {
            "healthy" => r##"<svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="14" height="14"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>"##,
            "unhealthy" => r##"<svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="14" height="14"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4m0 4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>"##,
            _ => r##"<svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="14" height="14"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8.228 9c.549-1.165 2.03-2 3.772-2 2.21 0 4 1.343 4 3 0 1.4-1.278 2.575-3.006 2.907-.542.104-.994.54-.994 1.093m0 3h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>"##,
        };

        format!(
            r##"<div class="instance-item {0}">
    <div class="instance-main">
        <div class="instance-id">{1}</div>
        <div class="instance-meta">
            <span class="instance-status {0}">{2}</span>
            <span class="instance-health {3}" title="Health: {4}">{5}</span>
            <span class="instance-port" title="Port">:{6}</span>
        </div>
    </div>
    <div class="instance-actions">
        <button class="btn btn-icon btn-sm" title="Restart instance"
            hx-post="/apps/{7}/instances/{1}/restart"
            hx-swap="none"
            hx-on::after-request="showToast('Instance restarting', 'success'); setTimeout(() => htmx.trigger('#instances-list', 'reload'), 2000)">
            <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"/>
            </svg>
        </button>
        <button class="btn btn-icon btn-sm btn-danger" title="Stop instance"
            hx-post="/apps/{7}/instances/{1}/stop"
            hx-confirm="Stop this instance?"
            hx-swap="none"
            hx-on::after-request="showToast('Instance stopped', 'success'); setTimeout(() => htmx.trigger('#instances-list', 'reload'), 1000)">
            <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M21 12a9 9 0 11-18 0 9 9 0 0118 0z"/>
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 10a1 1 0 011-1h4a1 1 0 011 1v4a1 1 0 01-1 1h-4a1 1 0 01-1-1v-4z"/>
            </svg>
        </button>
    </div>
</div>"##,
            status_class, id, status, health_class, health, health_icon, port, app_name
        )
    }).collect();

    let empty_state = if instances.is_empty() {
        r##"<div class="empty-state small">
            <p>No instances running</p>
        </div>"##.to_string()
    } else {
        String::new()
    };

    format!(
        r##"<div class="process-type-card">
    <div class="process-type-header">
        <div class="process-type-info">
            {0}
            <span class="process-type-name">{1}</span>
        </div>
        <div class="process-type-count">
            <span class="count-running">{2}</span>/<span class="count-total">{3}</span>
        </div>
    </div>
    <div class="instances-list">
        {4}
        {5}
    </div>
</div>"##,
        type_icon, type_label, running_count, total_count, instance_items.join(""), empty_state
    )
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
                <nav class="breadcrumbs" x-show="breadcrumbs.length > 0">
                    <template x-for="(crumb, index) in breadcrumbs" :key="index">
                        <span class="breadcrumb-item">
                            <span class="breadcrumb-separator" x-show="index > 0">/</span>
                            <a :href="crumb.href" x-text="crumb.label"
                                x-bind:hx-get="crumb.hxGet"
                                hx-target="#main-content"
                                hx-push-url="true"
                                @click="updateBreadcrumbs(breadcrumbs.slice(0, index + 1))"
                                :class="{ 'breadcrumb-current': index === breadcrumbs.length - 1 }"></a>
                        </span>
                    </template>
                </nav>
                <div class="search-box" x-show="breadcrumbs.length === 0">
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

    <!-- Create App Modal (Multi-step Wizard) -->
    <div class="modal-backdrop" x-show="showModal === 'create-app'" x-cloak
        x-transition:enter="modal-enter" x-transition:leave="modal-leave"
        @click.self="showModal = null" @keydown.escape.window="showModal = null"
        x-data="{
            step: 1,
            appName: '',
            appPort: 3000,
            envVars: [],
            newEnvKey: '',
            newEnvValue: '',
            createdApp: null,
            isCreating: false,
            error: null,
            addEnvVar() {
                if (this.newEnvKey && this.newEnvValue) {
                    this.envVars.push({ key: this.newEnvKey, value: this.newEnvValue });
                    this.newEnvKey = '';
                    this.newEnvValue = '';
                }
            },
            removeEnvVar(index) {
                this.envVars.splice(index, 1);
            },
            async createApp() {
                this.isCreating = true;
                this.error = null;
                const env = {};
                this.envVars.forEach(v => env[v.key] = v.value);
                try {
                    const res = await fetch('/apps', {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json' },
                        body: JSON.stringify({ name: this.appName, port: this.appPort, env })
                    });
                    const data = await res.json();
                    if (data.success) {
                        this.createdApp = data.data;
                        this.step = 3;
                        htmx.trigger('#apps-list', 'reload');
                    } else {
                        this.error = data.error || 'Failed to create app';
                    }
                } catch (e) {
                    this.error = 'Failed to create app';
                }
                this.isCreating = false;
            },
            reset() {
                this.step = 1;
                this.appName = '';
                this.appPort = 3000;
                this.envVars = [];
                this.createdApp = null;
                this.error = null;
            }
        }"
        @close-modal.window="reset()">
        <div class="modal modal-wizard" @click.stop>
            <div class="modal-header">
                <h2 x-text="step === 3 ? 'App Created!' : 'Create New App'"></h2>
                <button class="close-btn" @click="showModal = null; reset()">&times;</button>
            </div>

            <!-- Step indicator -->
            <div class="wizard-steps" x-show="step < 3">
                <div class="wizard-step" :class="{ active: step >= 1, current: step === 1 }">
                    <span class="step-number">1</span>
                    <span class="step-label">Basics</span>
                </div>
                <div class="wizard-step" :class="{ active: step >= 2, current: step === 2 }">
                    <span class="step-number">2</span>
                    <span class="step-label">Config</span>
                </div>
                <div class="wizard-step" :class="{ active: step >= 3, current: step === 3 }">
                    <span class="step-number">3</span>
                    <span class="step-label">Deploy</span>
                </div>
            </div>

            <!-- Step 1: Basic Info -->
            <div class="modal-body" x-show="step === 1">
                <div class="form-group">
                    <label for="app-name">App Name</label>
                    <input type="text" id="app-name" x-model="appName" required pattern="[a-z0-9-]+"
                        placeholder="my-awesome-app" class="input" autofocus>
                    <small>Lowercase letters, numbers, and hyphens only</small>
                </div>
                <div class="form-group">
                    <label for="app-port">Port</label>
                    <input type="number" id="app-port" x-model.number="appPort" min="1" max="65535" class="input">
                    <small>The port your application listens on (default: 3000)</small>
                </div>
            </div>

            <!-- Step 2: Environment Variables -->
            <div class="modal-body" x-show="step === 2">
                <p class="text-muted mb-4">Add environment variables (optional). You can add more later.</p>

                <div class="env-var-list" x-show="envVars.length > 0">
                    <template x-for="(env, index) in envVars" :key="index">
                        <div class="env-var-item">
                            <span class="env-key" x-text="env.key"></span>
                            <span class="env-value">••••••••</span>
                            <button type="button" class="btn btn-icon btn-danger" @click="removeEnvVar(index)">
                                <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="14" height="14">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/>
                                </svg>
                            </button>
                        </div>
                    </template>
                </div>

                <div class="env-var-add">
                    <input type="text" x-model="newEnvKey" placeholder="KEY" class="input input-sm"
                        pattern="[A-Za-z_][A-Za-z0-9_]*" @keydown.enter.prevent="addEnvVar()">
                    <input type="text" x-model="newEnvValue" placeholder="value" class="input input-sm"
                        @keydown.enter.prevent="addEnvVar()">
                    <button type="button" class="btn btn-secondary btn-sm" @click="addEnvVar()"
                        :disabled="!newEnvKey || !newEnvValue">Add</button>
                </div>
            </div>

            <!-- Step 3: Success & Deploy Instructions -->
            <div class="modal-body" x-show="step === 3">
                <div class="success-icon">
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="48" height="48">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"/>
                    </svg>
                </div>
                <p class="text-center mb-4">Your app <strong x-text="appName"></strong> is ready!</p>

                <div class="deploy-instructions">
                    <h4>Deploy your code:</h4>
                    <div class="code-block">
                        <code>
cd your-project<br>
git init<br>
git remote add spawngate <span x-text="createdApp?.git_url || 'git@localhost:' + appName + '.git'"></span><br>
git add .<br>
git commit -m "Initial commit"<br>
git push spawngate main
                        </code>
                        <button class="btn btn-icon copy-btn" @click="navigator.clipboard.writeText('cd your-project && git init && git remote add spawngate ' + (createdApp?.git_url || '') + ' && git add . && git commit -m \'Initial commit\' && git push spawngate main'); showToast('Copied to clipboard', 'success')" title="Copy">
                            <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z"/>
                            </svg>
                        </button>
                    </div>
                </div>
            </div>

            <!-- Error display -->
            <div class="modal-error" x-show="error" x-text="error"></div>

            <!-- Footer -->
            <div class="modal-footer">
                <template x-if="step === 1">
                    <div class="footer-buttons">
                        <button type="button" class="btn btn-secondary" @click="showModal = null; reset()">Cancel</button>
                        <button type="button" class="btn btn-primary" @click="step = 2" :disabled="!appName">
                            Next
                            <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7"/>
                            </svg>
                        </button>
                    </div>
                </template>
                <template x-if="step === 2">
                    <div class="footer-buttons">
                        <button type="button" class="btn btn-secondary" @click="step = 1">
                            <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="16" height="16">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 19l-7-7 7-7"/>
                            </svg>
                            Back
                        </button>
                        <button type="button" class="btn btn-primary" @click="createApp()" :disabled="isCreating">
                            <span x-show="!isCreating">Create App</span>
                            <span x-show="isCreating">Creating...</span>
                        </button>
                    </div>
                </template>
                <template x-if="step === 3">
                    <div class="footer-buttons">
                        <button type="button" class="btn btn-secondary" @click="showModal = null; reset(); loadApp(appName)">
                            View App
                        </button>
                        <button type="button" class="btn btn-primary" @click="showModal = null; reset()">Done</button>
                    </div>
                </template>
            </div>
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

    <!-- Delete App Confirmation Modal -->
    <div class="modal-backdrop" x-show="showModal === 'delete-app'" x-cloak
        @click.self="showModal = null" @keydown.escape.window="showModal = null">
        <div class="modal modal-danger" @click.stop>
            <div class="modal-header">
                <h2>Delete App</h2>
                <button class="close-btn" @click="showModal = null">&times;</button>
            </div>
            <div class="modal-body">
                <div class="delete-warning">
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24" width="48" height="48">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2"
                            d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z"/>
                    </svg>
                    <p class="warning-title">Are you absolutely sure?</p>
                    <p class="warning-text">This action <strong>cannot be undone</strong>. This will permanently delete the <strong x-text="currentApp"></strong> app, including:</p>
                    <ul class="warning-list">
                        <li>All config variables and secrets</li>
                        <li>All custom domains</li>
                        <li>All deployment history</li>
                        <li>All attached add-ons and their data</li>
                    </ul>
                </div>
                <div class="form-group">
                    <label>Please type <strong x-text="currentApp"></strong> to confirm:</label>
                    <input type="text" class="input" x-model="deleteConfirmInput"
                        :placeholder="currentApp" autocomplete="off">
                </div>
            </div>
            <div class="modal-footer">
                <button type="button" class="btn btn-secondary" @click="showModal = null; deleteConfirmInput = ''">Cancel</button>
                <button type="button" class="btn btn-danger"
                    :disabled="deleteConfirmInput !== currentApp"
                    @click="deleteApp()">
                    Delete this app
                </button>
            </div>
        </div>
    </div>

    <!-- Edit Config Var Modal -->
    <div class="modal-backdrop" x-show="showModal === 'edit-config'" x-cloak
        @click.self="showModal = null" @keydown.escape.window="showModal = null">
        <div class="modal" @click.stop>
            <div class="modal-header">
                <h2>Edit Config Variable</h2>
                <button class="close-btn" @click="showModal = null">&times;</button>
            </div>
            <form :hx-put="'/apps/' + currentApp + '/config'" hx-swap="none" @htmx:after-request="handleConfigUpdated($event)">
                <div class="modal-body">
                    <div class="form-group">
                        <label>Key</label>
                        <input type="text" class="input" :value="editingConfigKey" readonly disabled>
                        <input type="hidden" name="key" :value="editingConfigKey">
                    </div>
                    <div class="form-group">
                        <label for="edit-config-value">Value</label>
                        <textarea id="edit-config-value" name="value" required class="input textarea" rows="3"
                            x-model="editingConfigValue"></textarea>
                    </div>
                </div>
                <div class="modal-footer">
                    <button type="button" class="btn btn-secondary" @click="showModal = null">Cancel</button>
                    <button type="submit" class="btn btn-primary">Save Changes</button>
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

.breadcrumbs {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    font-size: 0.875rem;
}

.breadcrumb-item {
    display: flex;
    align-items: center;
}

.breadcrumb-separator {
    color: var(--text-muted);
    margin: 0 0.5rem;
}

.breadcrumbs a {
    color: var(--text-secondary);
    text-decoration: none;
    transition: color 0.15s ease;
}

.breadcrumbs a:hover {
    color: var(--primary);
}

.breadcrumb-current {
    color: var(--text-primary) !important;
    font-weight: 500;
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

.config-value-revealed {
    color: var(--text-primary);
}

.config-actions {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    flex-shrink: 0;
}

.config-actions .btn-icon {
    padding: 0.375rem;
    background: transparent;
    border: none;
    border-radius: 0.375rem;
    color: var(--text-muted);
    cursor: pointer;
    transition: all 0.15s ease;
}

.config-actions .btn-icon:hover {
    background: var(--bg-tertiary);
    color: var(--text-primary);
}

.config-actions .btn-icon.btn-danger:hover {
    background: rgba(239, 68, 68, 0.1);
    color: var(--danger);
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

.logs-card .card-header {
    flex-direction: column;
    gap: 0.75rem;
    align-items: stretch;
}

.logs-toolbar {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
}

.logs-search {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex: 1;
    min-width: 150px;
    background: var(--bg-secondary);
    border-radius: 0.375rem;
    padding: 0 0.5rem;
}

.logs-search svg {
    color: var(--text-muted);
    flex-shrink: 0;
}

.logs-search .input {
    border: none;
    background: transparent;
    padding-left: 0;
}

.logs-actions {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.5rem 0;
    border-bottom: 1px solid var(--border-color);
    margin-bottom: 0.5rem;
}

.logs-count {
    font-size: 0.75rem;
    color: var(--text-muted);
}

.logs-buttons {
    display: flex;
    gap: 0.5rem;
}

.logs-container {
    flex: 1;
    background: #0f172a;
    border-radius: 0.5rem;
    padding: 1rem;
    overflow-y: auto;
    font-family: 'SF Mono', Monaco, Consolas, monospace;
    font-size: 0.8125rem;
    line-height: 1.5;
    color: #e2e8f0;
}

.logs-empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: #64748b;
    gap: 0.5rem;
}

.log-line {
    display: flex;
    gap: 0.75rem;
    padding: 0.25rem 0;
    border-bottom: 1px solid rgba(255,255,255,0.05);
}

.log-line:hover {
    background: rgba(255,255,255,0.02);
}

.log-timestamp {
    color: #64748b;
    flex-shrink: 0;
    font-size: 0.75rem;
}

.log-source {
    color: #a78bfa;
    flex-shrink: 0;
    min-width: 50px;
    font-size: 0.75rem;
}

.log-level {
    flex-shrink: 0;
    min-width: 40px;
    font-size: 0.6875rem;
    text-transform: uppercase;
    font-weight: 500;
}

.log-message {
    flex: 1;
    word-break: break-all;
}

.log-error .log-level,
.log-error .log-message {
    color: #fca5a5;
}

.log-warn .log-level,
.log-warn .log-message {
    color: #fcd34d;
}

.log-info .log-level {
    color: #93c5fd;
}

.log-debug .log-level {
    color: #86efac;
}

.select-sm {
    padding: 0.375rem 0.625rem;
    font-size: 0.8125rem;
}

/* Deployment Progress */
.deploy-progress-card {
    border: 2px solid var(--primary);
}

.deploy-progress {
    padding: 1rem 0;
}

.deploy-status {
    display: flex;
    align-items: center;
    gap: 1rem;
    padding: 1rem;
    background: var(--bg-secondary);
    border-radius: 0.5rem;
    margin-bottom: 1rem;
}

.deploy-status-icon {
    flex-shrink: 0;
}

.deploy-status-icon svg {
    color: var(--primary);
}

.deploy-status-success .deploy-status-icon svg {
    color: var(--success);
}

.deploy-status-failed .deploy-status-icon svg {
    color: var(--danger);
}

.deploy-status-text {
    flex: 1;
    display: flex;
    justify-content: space-between;
    align-items: center;
}

.deploy-step {
    font-weight: 500;
    color: var(--text-primary);
}

.deploy-duration {
    font-size: 0.875rem;
    color: var(--text-muted);
}

.deploy-progress-bar {
    height: 4px;
    background: var(--bg-tertiary);
    border-radius: 2px;
    overflow: hidden;
    margin-bottom: 1rem;
}

.deploy-progress-fill {
    height: 100%;
    width: 30%;
    background: var(--primary);
    border-radius: 2px;
    animation: progress-indeterminate 1.5s ease-in-out infinite;
}

@keyframes progress-indeterminate {
    0% { transform: translateX(-100%); width: 30%; }
    50% { transform: translateX(100%); width: 60%; }
    100% { transform: translateX(300%); width: 30%; }
}

.deploy-logs {
    background: #0f172a;
    border-radius: 0.5rem;
    overflow: hidden;
}

.deploy-logs-header {
    padding: 0.5rem 1rem;
    background: #1e293b;
    font-size: 0.75rem;
    font-weight: 500;
    color: #94a3b8;
    text-transform: uppercase;
    letter-spacing: 0.05em;
}

.deploy-logs-content {
    padding: 1rem;
    margin: 0;
    font-family: 'SF Mono', Monaco, Consolas, monospace;
    font-size: 0.75rem;
    line-height: 1.6;
    color: #e2e8f0;
    max-height: 200px;
    overflow-y: auto;
}

.deploy-form {
    display: flex;
    flex-direction: column;
    gap: 1rem;
}

.deploy-form .btn {
    align-self: flex-start;
    display: flex;
    align-items: center;
    gap: 0.5rem;
}

/* Spinner animation */
.spinner {
    animation: spin 1s linear infinite;
}

@keyframes spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
}

/* Enhanced Deployments List */
.deployment-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.875rem 1rem;
    background: var(--bg-secondary);
    border-radius: 0.5rem;
    gap: 1rem;
}

.deployment-main {
    display: flex;
    align-items: center;
    gap: 0.875rem;
    flex: 1;
    min-width: 0;
}

.deployment-status {
    flex-shrink: 0;
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
    background: var(--bg-tertiary);
}

.deployment-status.status-running svg {
    color: var(--success);
}

.deployment-status.status-building svg {
    color: var(--primary);
}

.deployment-status.status-failed svg {
    color: var(--danger);
}

.deployment-info {
    flex: 1;
    min-width: 0;
}

.deployment-header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
}

.deployment-image {
    font-family: 'SF Mono', Monaco, Consolas, monospace;
    font-size: 0.8125rem;
    font-weight: 500;
    color: var(--text-primary);
}

.deployment-commit {
    font-family: 'SF Mono', Monaco, Consolas, monospace;
    font-size: 0.75rem;
    color: var(--text-muted);
    background: var(--bg-tertiary);
    padding: 0.125rem 0.375rem;
    border-radius: 0.25rem;
}

.badge-current {
    font-size: 0.625rem;
    text-transform: uppercase;
    font-weight: 600;
    letter-spacing: 0.05em;
    padding: 0.125rem 0.375rem;
    background: var(--success);
    color: white;
    border-radius: 0.25rem;
}

.deployment-meta {
    display: flex;
    gap: 1rem;
    font-size: 0.75rem;
    color: var(--text-muted);
    margin-top: 0.25rem;
}

.deployment-actions {
    flex-shrink: 0;
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

/* Delete Warning Modal */
.modal-danger .modal-header {
    background: var(--danger-light);
    border-bottom-color: var(--danger);
}

.modal-danger .modal-header h2 {
    color: var(--danger);
}

.delete-warning {
    text-align: center;
    padding: 1rem 0;
}

.delete-warning svg {
    color: var(--danger);
    margin-bottom: 1rem;
}

.delete-warning .warning-title {
    font-size: 1.25rem;
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: 0.75rem;
}

.delete-warning .warning-text {
    color: var(--text-secondary);
    margin-bottom: 1rem;
}

.delete-warning .warning-list {
    text-align: left;
    color: var(--text-secondary);
    padding-left: 1.5rem;
    margin: 0 auto 1.5rem;
    max-width: 300px;
}

.delete-warning .warning-list li {
    margin-bottom: 0.25rem;
}

.modal-footer button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
}

/* Wizard Modal */
.modal-wizard {
    max-width: 500px;
}

.wizard-steps {
    display: flex;
    justify-content: center;
    gap: 2rem;
    padding: 1rem 1.5rem;
    border-bottom: 1px solid var(--border-color);
}

.wizard-step {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.375rem;
    opacity: 0.4;
    transition: opacity 0.2s ease;
}

.wizard-step.active {
    opacity: 1;
}

.wizard-step .step-number {
    width: 1.75rem;
    height: 1.75rem;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
    font-size: 0.75rem;
    font-weight: 600;
    background: var(--bg-tertiary);
    color: var(--text-muted);
    border: 2px solid var(--border-color);
}

.wizard-step.current .step-number {
    background: var(--primary);
    color: white;
    border-color: var(--primary);
}

.wizard-step .step-label {
    font-size: 0.75rem;
    color: var(--text-muted);
}

.wizard-step.current .step-label {
    color: var(--text-primary);
    font-weight: 500;
}

/* Env var list in wizard */
.env-var-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin-bottom: 1rem;
}

.env-var-item {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.5rem 0.75rem;
    background: var(--bg-secondary);
    border-radius: 0.375rem;
}

.env-var-item .env-key {
    font-family: 'SF Mono', Monaco, Consolas, monospace;
    font-size: 0.8125rem;
    font-weight: 500;
    color: var(--text-primary);
}

.env-var-item .env-value {
    flex: 1;
    font-family: 'SF Mono', Monaco, Consolas, monospace;
    font-size: 0.8125rem;
    color: var(--text-muted);
}

.env-var-add {
    display: flex;
    gap: 0.5rem;
}

.env-var-add .input {
    flex: 1;
}

.input-sm {
    padding: 0.375rem 0.625rem;
    font-size: 0.8125rem;
}

.btn-sm {
    padding: 0.375rem 0.75rem;
    font-size: 0.8125rem;
}

/* Success state in wizard */
.success-icon {
    display: flex;
    justify-content: center;
    margin-bottom: 1rem;
}

.success-icon svg {
    color: var(--success);
}

.text-center {
    text-align: center;
}

.mb-4 {
    margin-bottom: 1rem;
}

/* Deploy instructions */
.deploy-instructions h4 {
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--text-primary);
    margin-bottom: 0.75rem;
}

.code-block {
    position: relative;
    background: var(--bg-tertiary);
    border-radius: 0.5rem;
    padding: 1rem;
    font-family: 'SF Mono', Monaco, Consolas, monospace;
    font-size: 0.75rem;
    line-height: 1.6;
    color: var(--text-primary);
    overflow-x: auto;
}

.code-block code {
    display: block;
}

.code-block .copy-btn {
    position: absolute;
    top: 0.5rem;
    right: 0.5rem;
    padding: 0.25rem;
    opacity: 0.6;
}

.code-block .copy-btn:hover {
    opacity: 1;
}

/* Modal error */
.modal-error {
    padding: 0.75rem 1.5rem;
    background: var(--danger-light);
    color: var(--danger);
    font-size: 0.875rem;
    border-top: 1px solid var(--danger);
}

/* Footer buttons wrapper */
.footer-buttons {
    display: flex;
    gap: 0.75rem;
    width: 100%;
    justify-content: flex-end;
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

.danger-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 1rem 0;
    border-bottom: 1px solid rgba(239, 68, 68, 0.2);
}

.danger-item:last-child {
    border-bottom: none;
    padding-bottom: 0;
}

.danger-item:first-child {
    padding-top: 0;
}

.danger-info {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
}

.danger-info strong {
    color: var(--text-primary);
    font-size: 0.875rem;
}

.danger-info small {
    color: var(--text-muted);
    font-size: 0.75rem;
}

.btn-outline-danger {
    background: transparent;
    border: 1px solid var(--danger);
    color: var(--danger);
}

.btn-outline-danger:hover:not(:disabled) {
    background: var(--danger);
    color: white;
}

/* Settings Form */
.settings-form {
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
}

.form-row {
    display: flex;
    gap: 1.5rem;
}

.form-row.two-col > .form-group {
    flex: 1;
}

.form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.75rem;
    padding-top: 0.5rem;
}

/* Settings row with toggle */
.settings-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 1rem;
}

.settings-info {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
}

.settings-info strong {
    font-size: 0.875rem;
    color: var(--text-primary);
}

.settings-info small {
    font-size: 0.75rem;
    color: var(--text-muted);
}

/* Scale display */
.scale-display {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
}

.scale-value {
    font-size: 2rem;
    font-weight: 600;
    color: var(--primary);
}

.scale-label {
    font-size: 0.875rem;
    color: var(--text-muted);
}

/* Toggle switch */
.toggle {
    position: relative;
    display: inline-block;
    width: 48px;
    height: 26px;
    flex-shrink: 0;
}

.toggle input {
    opacity: 0;
    width: 0;
    height: 0;
}

.toggle-slider {
    position: absolute;
    cursor: pointer;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background: var(--bg-tertiary);
    border-radius: 26px;
    transition: 0.2s;
}

.toggle-slider:before {
    position: absolute;
    content: "";
    height: 20px;
    width: 20px;
    left: 3px;
    bottom: 3px;
    background: white;
    border-radius: 50%;
    transition: 0.2s;
}

.toggle input:checked + .toggle-slider {
    background: var(--primary);
}

.toggle input:checked + .toggle-slider:before {
    transform: translateX(22px);
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

/* Instance Manager Styles */
.instances-container {
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
}

.scale-control {
    background: var(--bg-secondary);
    border: 1px solid var(--border-color);
    border-radius: 0.75rem;
    padding: 1.5rem;
}

.scale-header {
    margin-bottom: 1rem;
}

.scale-info {
    display: flex;
    justify-content: space-between;
    align-items: center;
}

.scale-label {
    font-weight: 600;
    color: var(--text-primary);
}

.scale-current {
    font-size: 0.875rem;
    color: var(--text-secondary);
}

.scale-slider-container {
    display: flex;
    align-items: center;
    gap: 1rem;
}

.scale-slider {
    flex: 1;
    height: 8px;
    border-radius: 4px;
    background: var(--bg-tertiary);
    appearance: none;
    cursor: pointer;
}

.scale-slider::-webkit-slider-thumb {
    appearance: none;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--primary);
    cursor: pointer;
    transition: transform 0.15s ease;
}

.scale-slider::-webkit-slider-thumb:hover {
    transform: scale(1.1);
}

.scale-slider::-moz-range-thumb {
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--primary);
    cursor: pointer;
    border: none;
}

.scale-input-group {
    display: flex;
    align-items: center;
    gap: 0.5rem;
}

.scale-input {
    width: 60px;
    padding: 0.5rem;
    border: 1px solid var(--border-color);
    border-radius: 0.375rem;
    background: var(--bg-primary);
    color: var(--text-primary);
    text-align: center;
    font-size: 0.875rem;
}

.scale-input:focus {
    outline: none;
    border-color: var(--primary);
    box-shadow: 0 0 0 3px var(--primary-light);
}

.scale-limits {
    display: flex;
    justify-content: space-between;
    margin-top: 0.5rem;
    font-size: 0.75rem;
    color: var(--text-muted);
}

.scale-progress {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 1rem;
    background: var(--primary-light);
    border-radius: 0.5rem;
    margin-top: 1rem;
    color: var(--primary);
    font-size: 0.875rem;
}

.loading-spinner {
    width: 16px;
    height: 16px;
    border: 2px solid var(--primary);
    border-top-color: transparent;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
}

@keyframes spin {
    to { transform: rotate(360deg); }
}

.htmx-listener {
    display: none;
}

.process-types {
    display: flex;
    flex-direction: column;
    gap: 1rem;
}

.process-type-card {
    background: var(--bg-primary);
    border: 1px solid var(--border-color);
    border-radius: 0.75rem;
    overflow: hidden;
}

.process-type-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 1rem 1.25rem;
    background: var(--bg-secondary);
    border-bottom: 1px solid var(--border-color);
}

.process-type-info {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    color: var(--text-primary);
}

.process-type-info svg {
    color: var(--text-secondary);
}

.process-type-name {
    font-weight: 600;
}

.process-type-count {
    font-size: 0.875rem;
    color: var(--text-secondary);
}

.process-type-count .count-running {
    color: var(--success);
    font-weight: 600;
}

.instances-list {
    padding: 0.5rem;
}

.instance-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.875rem 1rem;
    border-radius: 0.5rem;
    transition: background 0.15s ease;
}

.instance-item:hover {
    background: var(--bg-secondary);
}

.instance-main {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
}

.instance-id {
    font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', monospace;
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--text-primary);
}

.instance-meta {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    font-size: 0.75rem;
}

.instance-status {
    display: inline-flex;
    align-items: center;
    padding: 0.125rem 0.5rem;
    border-radius: 9999px;
    font-weight: 500;
    text-transform: capitalize;
}

.instance-status.status-running {
    background: var(--success-light);
    color: var(--success);
}

.instance-status.status-building {
    background: var(--warning-light);
    color: var(--warning);
}

.instance-status.status-idle {
    background: var(--bg-tertiary);
    color: var(--text-secondary);
}

.instance-status.status-failed {
    background: var(--danger-light);
    color: var(--danger);
}

.instance-health {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
}

.instance-health.health-healthy {
    color: var(--success);
}

.instance-health.health-unhealthy {
    color: var(--danger);
}

.instance-health.health-unknown {
    color: var(--text-muted);
}

.instance-port {
    color: var(--text-muted);
    font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', monospace;
}

.instance-actions {
    display: flex;
    gap: 0.5rem;
    opacity: 0;
    transition: opacity 0.15s ease;
}

.instance-item:hover .instance-actions {
    opacity: 1;
}

.btn-icon {
    padding: 0.5rem;
    display: flex;
    align-items: center;
    justify-content: center;
}

.btn-icon.btn-sm {
    padding: 0.375rem;
}

.btn-danger {
    background: var(--danger);
    color: white;
}

.btn-danger:hover {
    background: #dc2626;
}

.empty-state.small {
    padding: 1.5rem;
    text-align: center;
}

.empty-state.small p {
    color: var(--text-muted);
    font-size: 0.875rem;
}

/* Resource Usage Styles */
.resource-overview {
    background: var(--bg-secondary);
    border: 1px solid var(--border-color);
    border-radius: 0.75rem;
    padding: 1.5rem;
}

.resource-overview h3 {
    font-size: 1rem;
    font-weight: 600;
    margin-bottom: 1rem;
    color: var(--text-primary);
}

.resource-graphs {
    display: flex;
    flex-direction: column;
    gap: 1rem;
}

.resource-card {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
}

.resource-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
}

.resource-label {
    font-size: 0.875rem;
    color: var(--text-secondary);
}

.resource-value {
    font-size: 0.875rem;
    font-weight: 600;
    color: var(--text-primary);
    font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', monospace;
}

.resource-bar {
    height: 8px;
    background: var(--bg-tertiary);
    border-radius: 4px;
    overflow: hidden;
}

.resource-bar-fill {
    height: 100%;
    border-radius: 4px;
    transition: width 0.3s ease;
}

.resource-bar-fill.cpu {
    background: var(--primary);
}

.resource-bar-fill.memory {
    background: var(--success);
}

.resource-bar-fill.warning {
    background: var(--warning);
}

.resource-bar-fill.danger {
    background: var(--danger);
}

.resource-details {
    display: flex;
    gap: 2rem;
    margin-top: 0.5rem;
    padding-top: 0.75rem;
    border-top: 1px solid var(--border-color);
}

.resource-detail {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
}

.detail-label {
    font-size: 0.75rem;
    color: var(--text-muted);
}

.detail-value {
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--text-primary);
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
        breadcrumbs: [],
        deleteConfirmInput: '',
        editingConfigKey: '',
        editingConfigValue: '',

        init() {
            document.body.classList.add(this.theme);
            this.setupHtmxHandlers();
            this.setupKeyboardShortcuts();
            this.initBreadcrumbs();

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

        initBreadcrumbs() {
            const path = window.location.pathname;
            this.breadcrumbs = this.buildBreadcrumbs(path);
        },

        buildBreadcrumbs(path) {
            const crumbs = [];

            // App detail pages
            const appMatch = path.match(/\/dashboard\/apps\/([^\/]+)/);
            if (appMatch) {
                crumbs.push({ label: 'Apps', href: '/dashboard', hxGet: '/dashboard/apps' });
                crumbs.push({ label: appMatch[1], href: `/dashboard/apps/${appMatch[1]}`, hxGet: `/dashboard/apps/${appMatch[1]}` });
            }

            return crumbs;
        },

        updateBreadcrumbs(crumbs) {
            this.breadcrumbs = crumbs;
        },

        navigateToApp(appName) {
            this.currentApp = appName;
            this.breadcrumbs = [
                { label: 'Apps', href: '/dashboard', hxGet: '/dashboard/apps' },
                { label: appName, href: `/dashboard/apps/${appName}`, hxGet: `/dashboard/apps/${appName}` }
            ];
        },

        navigateToApps() {
            this.currentApp = null;
            this.breadcrumbs = [];
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

            // Update current app and breadcrumbs from URL changes
            document.body.addEventListener('htmx:pushedIntoHistory', (e) => {
                const path = e.detail.path;
                const appMatch = path.match(/\/dashboard\/apps\/([^\/]+)/);
                if (appMatch) {
                    this.currentApp = appMatch[1];
                    this.breadcrumbs = this.buildBreadcrumbs(path);
                } else if (path === '/dashboard' || path === '/dashboard/') {
                    this.currentApp = null;
                    this.breadcrumbs = [];
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
        },

        handleConfigUpdated(event) {
            if (event.detail.successful) {
                this.showModal = null;
                this.editingConfigKey = '';
                this.editingConfigValue = '';
                showToast('Config variable updated!', 'success');
                htmx.trigger('#config-list', 'htmx:trigger');
            }
        },

        openEditConfig(key, value) {
            this.editingConfigKey = key;
            this.editingConfigValue = value;
            this.showModal = 'edit-config';
        },

        deleteApp() {
            if (this.deleteConfirmInput !== this.currentApp) return;

            fetch(`/apps/${this.currentApp}`, {
                method: 'DELETE',
                headers: { 'Content-Type': 'application/json' }
            })
            .then(response => {
                if (response.ok) {
                    showToast('App deleted successfully', 'success');
                    this.showModal = null;
                    this.deleteConfirmInput = '';
                    this.currentApp = null;
                    this.breadcrumbs = [];
                    htmx.ajax('GET', '/dashboard/apps', '#main-content');
                    history.pushState({}, '', '/dashboard');
                } else {
                    response.json().then(data => {
                        showToast(data.error || 'Failed to delete app', 'error');
                    });
                }
            })
            .catch(() => {
                showToast('Failed to delete app', 'error');
            });
        },

        confirmDeleteApp() {
            this.deleteConfirmInput = '';
            this.showModal = 'delete-app';
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

// Resource monitoring component for Alpine.js
function resourceMonitor(appName) {
    return {
        appName: appName,
        cpuUsage: 0,
        memoryUsage: 0,
        cpuCores: '--',
        memoryUsed: '--',
        memoryLimit: '--',
        pollingInterval: null,

        async startPolling() {
            // Initial fetch
            await this.fetchMetrics();
            // Poll every 5 seconds
            this.pollingInterval = setInterval(() => this.fetchMetrics(), 5000);
        },

        stopPolling() {
            if (this.pollingInterval) {
                clearInterval(this.pollingInterval);
                this.pollingInterval = null;
            }
        },

        async fetchMetrics() {
            try {
                const response = await fetch(`/apps/${this.appName}/metrics`);
                if (response.ok) {
                    const data = await response.json();
                    if (data.status === 'ok' && data.data) {
                        const metrics = data.data;
                        this.cpuUsage = Math.round(metrics.cpu_percent || 0);
                        this.memoryUsage = Math.round(metrics.memory_percent || 0);
                        this.cpuCores = metrics.cpu_cores || '--';
                        this.memoryUsed = this.formatBytes(metrics.memory_used || 0);
                        this.memoryLimit = this.formatBytes(metrics.memory_limit || 0);
                    }
                }
            } catch (e) {
                // Silently fail - metrics may not be available
                console.debug('Failed to fetch metrics:', e);
            }
        },

        formatBytes(bytes) {
            if (bytes === 0) return '0 B';
            const k = 1024;
            const sizes = ['B', 'KB', 'MB', 'GB'];
            const i = Math.floor(Math.log(bytes) / Math.log(k));
            return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
        },

        destroy() {
            this.stopPolling();
        }
    };
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
        assert!(html.contains("••••••••")); // API_KEY should be masked
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
        assert!(html.contains("status-running")); // success status class
        assert!(html.contains("app:v1.2.3"));
        assert!(html.contains("45.5s"));
        assert!(html.contains("badge-current")); // first successful deploy is current
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
