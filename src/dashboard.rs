//! Web Dashboard for PaaS Platform
//!
//! Provides a simple embedded web UI for managing apps, viewing logs,
//! and monitoring the platform.

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

const DASHBOARD_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>PaaS Dashboard</title>
    <link rel="stylesheet" href="/dashboard/style.css">
</head>
<body>
    <nav class="navbar">
        <div class="nav-brand">
            <h1>⚡ PaaS Dashboard</h1>
        </div>
        <div class="nav-links">
            <a href="#" class="nav-link active" data-view="apps">Apps</a>
            <a href="#" class="nav-link" data-view="addons">Add-ons</a>
            <a href="#" class="nav-link" data-view="logs">Logs</a>
        </div>
        <div class="nav-actions">
            <button class="btn btn-primary" onclick="showCreateAppModal()">New App</button>
        </div>
    </nav>

    <main class="container">
        <!-- Apps View -->
        <div id="apps-view" class="view active">
            <div class="view-header">
                <h2>Applications</h2>
                <button class="btn btn-secondary" onclick="refreshApps()">Refresh</button>
            </div>
            <div id="apps-list" class="apps-grid">
                <!-- Apps will be loaded here -->
            </div>
        </div>

        <!-- App Detail View -->
        <div id="app-detail-view" class="view">
            <div class="view-header">
                <button class="btn btn-link" onclick="showView('apps')">&larr; Back to Apps</button>
                <h2 id="app-detail-name">App Name</h2>
            </div>
            <div class="app-detail-content">
                <div class="detail-section">
                    <h3>Overview</h3>
                    <div id="app-overview" class="info-grid"></div>
                </div>
                <div class="detail-section">
                    <h3>Environment Variables</h3>
                    <div id="app-config" class="config-list"></div>
                    <button class="btn btn-secondary" onclick="showAddConfigModal()">Add Variable</button>
                </div>
                <div class="detail-section">
                    <h3>Add-ons</h3>
                    <div id="app-addons" class="addons-list"></div>
                    <button class="btn btn-secondary" onclick="showAddAddonModal()">Add Add-on</button>
                </div>
                <div class="detail-section">
                    <h3>Recent Deployments</h3>
                    <div id="app-deployments" class="deployments-list"></div>
                </div>
                <div class="detail-section danger-zone">
                    <h3>Danger Zone</h3>
                    <button class="btn btn-danger" onclick="confirmDeleteApp()">Delete App</button>
                </div>
            </div>
        </div>

        <!-- Logs View -->
        <div id="logs-view" class="view">
            <div class="view-header">
                <h2>Logs</h2>
                <select id="logs-app-select" onchange="loadLogs()">
                    <option value="">Select an app...</option>
                </select>
            </div>
            <div id="logs-container" class="logs-container">
                <pre id="logs-content">Select an app to view logs</pre>
            </div>
        </div>
    </main>

    <!-- Create App Modal -->
    <div id="create-app-modal" class="modal">
        <div class="modal-content">
            <div class="modal-header">
                <h3>Create New App</h3>
                <button class="close-btn" onclick="closeModal('create-app-modal')">&times;</button>
            </div>
            <form onsubmit="createApp(event)">
                <div class="form-group">
                    <label for="app-name">App Name</label>
                    <input type="text" id="app-name" name="name" required pattern="[a-z0-9-]+"
                           placeholder="my-awesome-app">
                    <small>Lowercase letters, numbers, and hyphens only</small>
                </div>
                <div class="form-group">
                    <label for="app-port">Port</label>
                    <input type="number" id="app-port" name="port" value="3000" min="1" max="65535">
                </div>
                <div class="form-actions">
                    <button type="button" class="btn btn-secondary" onclick="closeModal('create-app-modal')">Cancel</button>
                    <button type="submit" class="btn btn-primary">Create App</button>
                </div>
            </form>
        </div>
    </div>

    <!-- Add Addon Modal -->
    <div id="add-addon-modal" class="modal">
        <div class="modal-content">
            <div class="modal-header">
                <h3>Add Add-on</h3>
                <button class="close-btn" onclick="closeModal('add-addon-modal')">&times;</button>
            </div>
            <form onsubmit="addAddon(event)">
                <div class="form-group">
                    <label for="addon-type">Type</label>
                    <select id="addon-type" name="type" required>
                        <option value="postgres">PostgreSQL Database</option>
                        <option value="redis">Redis Cache</option>
                        <option value="storage">S3 Storage (MinIO)</option>
                    </select>
                </div>
                <div class="form-group">
                    <label for="addon-plan">Plan</label>
                    <select id="addon-plan" name="plan">
                        <option value="hobby">Hobby (256MB)</option>
                        <option value="basic">Basic (512MB)</option>
                        <option value="standard">Standard (1GB)</option>
                        <option value="premium">Premium (2GB)</option>
                    </select>
                </div>
                <div class="form-actions">
                    <button type="button" class="btn btn-secondary" onclick="closeModal('add-addon-modal')">Cancel</button>
                    <button type="submit" class="btn btn-primary">Add Add-on</button>
                </div>
            </form>
        </div>
    </div>

    <!-- Add Config Modal -->
    <div id="add-config-modal" class="modal">
        <div class="modal-content">
            <div class="modal-header">
                <h3>Add Environment Variable</h3>
                <button class="close-btn" onclick="closeModal('add-config-modal')">&times;</button>
            </div>
            <form onsubmit="addConfig(event)">
                <div class="form-group">
                    <label for="config-key">Key</label>
                    <input type="text" id="config-key" name="key" required pattern="[A-Z_][A-Z0-9_]*"
                           placeholder="MY_VAR">
                    <small>Uppercase letters, numbers, and underscores</small>
                </div>
                <div class="form-group">
                    <label for="config-value">Value</label>
                    <input type="text" id="config-value" name="value" required placeholder="value">
                </div>
                <div class="form-actions">
                    <button type="button" class="btn btn-secondary" onclick="closeModal('add-config-modal')">Cancel</button>
                    <button type="submit" class="btn btn-primary">Add Variable</button>
                </div>
            </form>
        </div>
    </div>

    <!-- Toast Notifications -->
    <div id="toast-container"></div>

    <script src="/dashboard/app.js"></script>
</body>
</html>
"##;

const DASHBOARD_CSS: &str = r##"
:root {
    --primary: #6366f1;
    --primary-dark: #4f46e5;
    --success: #10b981;
    --warning: #f59e0b;
    --danger: #ef4444;
    --gray-50: #f9fafb;
    --gray-100: #f3f4f6;
    --gray-200: #e5e7eb;
    --gray-300: #d1d5db;
    --gray-400: #9ca3af;
    --gray-500: #6b7280;
    --gray-600: #4b5563;
    --gray-700: #374151;
    --gray-800: #1f2937;
    --gray-900: #111827;
}

* {
    box-sizing: border-box;
    margin: 0;
    padding: 0;
}

body {
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    background: var(--gray-100);
    color: var(--gray-800);
    line-height: 1.5;
}

/* Navbar */
.navbar {
    background: var(--gray-900);
    color: white;
    padding: 0 1.5rem;
    height: 60px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    position: sticky;
    top: 0;
    z-index: 100;
}

.nav-brand h1 {
    font-size: 1.25rem;
    font-weight: 600;
}

.nav-links {
    display: flex;
    gap: 0.5rem;
}

.nav-link {
    color: var(--gray-300);
    text-decoration: none;
    padding: 0.5rem 1rem;
    border-radius: 0.375rem;
    transition: all 0.2s;
}

.nav-link:hover {
    color: white;
    background: var(--gray-700);
}

.nav-link.active {
    color: white;
    background: var(--primary);
}

/* Container */
.container {
    max-width: 1200px;
    margin: 0 auto;
    padding: 1.5rem;
}

/* Views */
.view {
    display: none;
}

.view.active {
    display: block;
}

.view-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 1.5rem;
}

.view-header h2 {
    font-size: 1.5rem;
    font-weight: 600;
}

/* Buttons */
.btn {
    display: inline-flex;
    align-items: center;
    padding: 0.5rem 1rem;
    border: none;
    border-radius: 0.375rem;
    font-size: 0.875rem;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.2s;
}

.btn-primary {
    background: var(--primary);
    color: white;
}

.btn-primary:hover {
    background: var(--primary-dark);
}

.btn-secondary {
    background: white;
    color: var(--gray-700);
    border: 1px solid var(--gray-300);
}

.btn-secondary:hover {
    background: var(--gray-50);
}

.btn-danger {
    background: var(--danger);
    color: white;
}

.btn-danger:hover {
    background: #dc2626;
}

.btn-link {
    background: none;
    color: var(--primary);
}

/* Apps Grid */
.apps-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 1rem;
}

.app-card {
    background: white;
    border-radius: 0.5rem;
    padding: 1.25rem;
    box-shadow: 0 1px 3px rgba(0,0,0,0.1);
    cursor: pointer;
    transition: all 0.2s;
    border: 2px solid transparent;
}

.app-card:hover {
    border-color: var(--primary);
    box-shadow: 0 4px 6px rgba(0,0,0,0.1);
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
    color: var(--gray-900);
}

.status-badge {
    display: inline-flex;
    align-items: center;
    padding: 0.25rem 0.5rem;
    border-radius: 9999px;
    font-size: 0.75rem;
    font-weight: 500;
}

.status-idle {
    background: var(--gray-200);
    color: var(--gray-700);
}

.status-running {
    background: #d1fae5;
    color: #065f46;
}

.status-building {
    background: #fef3c7;
    color: #92400e;
}

.status-failed {
    background: #fee2e2;
    color: #991b1b;
}

.app-card-meta {
    font-size: 0.875rem;
    color: var(--gray-500);
}

.app-card-addons {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.75rem;
}

.addon-badge {
    background: var(--gray-100);
    color: var(--gray-600);
    padding: 0.25rem 0.5rem;
    border-radius: 0.25rem;
    font-size: 0.75rem;
}

/* App Detail */
.app-detail-content {
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
}

.detail-section {
    background: white;
    border-radius: 0.5rem;
    padding: 1.25rem;
    box-shadow: 0 1px 3px rgba(0,0,0,0.1);
}

.detail-section h3 {
    font-size: 1rem;
    font-weight: 600;
    margin-bottom: 1rem;
    color: var(--gray-700);
}

.info-grid {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 1rem;
}

.info-item {
    display: flex;
    flex-direction: column;
}

.info-label {
    font-size: 0.75rem;
    color: var(--gray-500);
    text-transform: uppercase;
    letter-spacing: 0.05em;
}

.info-value {
    font-size: 0.875rem;
    color: var(--gray-900);
    font-family: monospace;
}

.config-list, .addons-list, .deployments-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin-bottom: 1rem;
}

.config-item, .addon-item, .deployment-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.75rem;
    background: var(--gray-50);
    border-radius: 0.375rem;
}

.config-key {
    font-family: monospace;
    font-weight: 500;
}

.config-value {
    font-family: monospace;
    color: var(--gray-500);
}

.danger-zone {
    border: 1px solid var(--danger);
    background: #fef2f2;
}

/* Logs */
.logs-container {
    background: var(--gray-900);
    border-radius: 0.5rem;
    padding: 1rem;
    height: 500px;
    overflow-y: auto;
}

#logs-content {
    color: #a5f3fc;
    font-family: 'SF Mono', Monaco, monospace;
    font-size: 0.8125rem;
    line-height: 1.6;
    white-space: pre-wrap;
    word-break: break-all;
}

/* Modal */
.modal {
    display: none;
    position: fixed;
    top: 0;
    left: 0;
    width: 100%;
    height: 100%;
    background: rgba(0,0,0,0.5);
    z-index: 1000;
    align-items: center;
    justify-content: center;
}

.modal.active {
    display: flex;
}

.modal-content {
    background: white;
    border-radius: 0.5rem;
    width: 100%;
    max-width: 400px;
    box-shadow: 0 20px 25px -5px rgba(0,0,0,0.1);
}

.modal-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 1rem 1.25rem;
    border-bottom: 1px solid var(--gray-200);
}

.modal-header h3 {
    font-size: 1.125rem;
    font-weight: 600;
}

.close-btn {
    background: none;
    border: none;
    font-size: 1.5rem;
    color: var(--gray-400);
    cursor: pointer;
}

.close-btn:hover {
    color: var(--gray-600);
}

form {
    padding: 1.25rem;
}

.form-group {
    margin-bottom: 1rem;
}

.form-group label {
    display: block;
    font-size: 0.875rem;
    font-weight: 500;
    margin-bottom: 0.25rem;
    color: var(--gray-700);
}

.form-group input,
.form-group select {
    width: 100%;
    padding: 0.5rem 0.75rem;
    border: 1px solid var(--gray-300);
    border-radius: 0.375rem;
    font-size: 0.875rem;
}

.form-group input:focus,
.form-group select:focus {
    outline: none;
    border-color: var(--primary);
    box-shadow: 0 0 0 3px rgba(99, 102, 241, 0.1);
}

.form-group small {
    display: block;
    font-size: 0.75rem;
    color: var(--gray-500);
    margin-top: 0.25rem;
}

.form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    margin-top: 1.5rem;
}

/* Toast */
#toast-container {
    position: fixed;
    bottom: 1rem;
    right: 1rem;
    z-index: 2000;
}

.toast {
    background: var(--gray-900);
    color: white;
    padding: 0.75rem 1rem;
    border-radius: 0.375rem;
    margin-top: 0.5rem;
    box-shadow: 0 4px 6px rgba(0,0,0,0.1);
    animation: slideIn 0.3s ease;
}

.toast.success {
    background: var(--success);
}

.toast.error {
    background: var(--danger);
}

@keyframes slideIn {
    from {
        transform: translateX(100%);
        opacity: 0;
    }
    to {
        transform: translateX(0);
        opacity: 1;
    }
}

/* Empty state */
.empty-state {
    text-align: center;
    padding: 3rem;
    color: var(--gray-500);
}

.empty-state p {
    margin-bottom: 1rem;
}

/* Select */
select {
    background: white;
    cursor: pointer;
}
"##;

const DASHBOARD_JS: &str = r##"
// API Configuration
const API_TOKEN = localStorage.getItem('paas_token') || 'changeme';
let currentApp = null;

// Initialize
document.addEventListener('DOMContentLoaded', () => {
    setupNavigation();
    refreshApps();
});

// Navigation
function setupNavigation() {
    document.querySelectorAll('.nav-link').forEach(link => {
        link.addEventListener('click', (e) => {
            e.preventDefault();
            const view = link.dataset.view;
            showView(view);

            document.querySelectorAll('.nav-link').forEach(l => l.classList.remove('active'));
            link.classList.add('active');
        });
    });
}

function showView(viewName) {
    document.querySelectorAll('.view').forEach(v => v.classList.remove('active'));
    const view = document.getElementById(`${viewName}-view`);
    if (view) {
        view.classList.add('active');
    }

    if (viewName === 'logs') {
        populateLogsAppSelect();
    }
}

// API Helpers
async function apiRequest(method, path, body = null) {
    const options = {
        method,
        headers: {
            'Authorization': `Bearer ${API_TOKEN}`,
            'Content-Type': 'application/json'
        }
    };

    if (body) {
        options.body = JSON.stringify(body);
    }

    const response = await fetch(path, options);
    const data = await response.json();

    if (!data.success) {
        throw new Error(data.error || 'API request failed');
    }

    return data.data;
}

// Apps
async function refreshApps() {
    const container = document.getElementById('apps-list');

    try {
        const apps = await apiRequest('GET', '/apps');

        if (apps.length === 0) {
            container.innerHTML = `
                <div class="empty-state">
                    <p>No apps yet</p>
                    <button class="btn btn-primary" onclick="showCreateAppModal()">Create your first app</button>
                </div>
            `;
            return;
        }

        container.innerHTML = apps.map(app => `
            <div class="app-card" onclick="showAppDetail('${app.name}')">
                <div class="app-card-header">
                    <span class="app-name">${app.name}</span>
                    <span class="status-badge status-${app.status}">${app.status}</span>
                </div>
                <div class="app-card-meta">
                    Port ${app.port} · Created ${formatDate(app.created_at)}
                </div>
                ${app.addons.length > 0 ? `
                    <div class="app-card-addons">
                        ${app.addons.map(a => `<span class="addon-badge">${a.split(':')[0]}</span>`).join('')}
                    </div>
                ` : ''}
            </div>
        `).join('');
    } catch (err) {
        showToast(err.message, 'error');
        container.innerHTML = `
            <div class="empty-state">
                <p>Failed to load apps</p>
                <button class="btn btn-secondary" onclick="refreshApps()">Retry</button>
            </div>
        `;
    }
}

async function showAppDetail(appName) {
    try {
        currentApp = await apiRequest('GET', `/apps/${appName}`);

        document.getElementById('app-detail-name').textContent = appName;

        // Overview
        const overview = document.getElementById('app-overview');
        overview.innerHTML = `
            <div class="info-item">
                <span class="info-label">Status</span>
                <span class="info-value"><span class="status-badge status-${currentApp.status}">${currentApp.status}</span></span>
            </div>
            <div class="info-item">
                <span class="info-label">Port</span>
                <span class="info-value">${currentApp.port}</span>
            </div>
            <div class="info-item">
                <span class="info-label">Git URL</span>
                <span class="info-value">${currentApp.git_url || 'N/A'}</span>
            </div>
            <div class="info-item">
                <span class="info-label">Image</span>
                <span class="info-value">${currentApp.image || 'Not deployed'}</span>
            </div>
            <div class="info-item">
                <span class="info-label">Created</span>
                <span class="info-value">${formatDate(currentApp.created_at)}</span>
            </div>
            <div class="info-item">
                <span class="info-label">Last Deploy</span>
                <span class="info-value">${currentApp.deployed_at ? formatDate(currentApp.deployed_at) : 'Never'}</span>
            </div>
        `;

        // Config
        const configList = document.getElementById('app-config');
        const envEntries = Object.entries(currentApp.env || {});
        if (envEntries.length === 0) {
            configList.innerHTML = '<p class="empty-state">No environment variables set</p>';
        } else {
            configList.innerHTML = envEntries.map(([key, value]) => `
                <div class="config-item">
                    <span class="config-key">${key}</span>
                    <span class="config-value">${maskSecret(key, value)}</span>
                </div>
            `).join('');
        }

        // Addons
        await loadAppAddons(appName);

        // Deployments
        await loadAppDeployments(appName);

        showView('app-detail');
    } catch (err) {
        showToast(err.message, 'error');
    }
}

async function loadAppAddons(appName) {
    try {
        const addons = await apiRequest('GET', `/apps/${appName}/addons`);
        const addonsList = document.getElementById('app-addons');

        if (addons.length === 0) {
            addonsList.innerHTML = '<p class="empty-state">No add-ons attached</p>';
        } else {
            addonsList.innerHTML = addons.map(addon => `
                <div class="addon-item">
                    <div>
                        <strong>${addon.addon_type}</strong>
                        <span class="addon-badge">${addon.plan}</span>
                    </div>
                    <span class="status-badge status-${addon.status === 'running' ? 'running' : 'idle'}">${addon.status}</span>
                </div>
            `).join('');
        }
    } catch (err) {
        console.error('Failed to load addons:', err);
    }
}

async function loadAppDeployments(appName) {
    try {
        const deployments = await apiRequest('GET', `/apps/${appName}/deployments`);
        const deploysList = document.getElementById('app-deployments');

        if (deployments.length === 0) {
            deploysList.innerHTML = '<p class="empty-state">No deployments yet</p>';
        } else {
            deploysList.innerHTML = deployments.slice(0, 5).map(deploy => `
                <div class="deployment-item">
                    <div>
                        <span class="status-badge status-${deploy.status === 'success' ? 'running' : 'failed'}">${deploy.status}</span>
                        <span>${deploy.image || 'N/A'}</span>
                    </div>
                    <span>${deploy.duration_secs ? deploy.duration_secs.toFixed(1) + 's' : ''} · ${formatDate(deploy.created_at)}</span>
                </div>
            `).join('');
        }
    } catch (err) {
        console.error('Failed to load deployments:', err);
    }
}

// Create App
function showCreateAppModal() {
    document.getElementById('create-app-modal').classList.add('active');
    document.getElementById('app-name').focus();
}

async function createApp(e) {
    e.preventDefault();

    const form = e.target;
    const name = form.name.value.trim();
    const port = parseInt(form.port.value) || 3000;

    try {
        await apiRequest('POST', '/apps', { name, port });
        showToast(`App "${name}" created successfully!`, 'success');
        closeModal('create-app-modal');
        form.reset();
        refreshApps();
    } catch (err) {
        showToast(err.message, 'error');
    }
}

// Delete App
async function confirmDeleteApp() {
    if (!currentApp) return;

    if (confirm(`Are you sure you want to delete "${currentApp.name}"? This cannot be undone.`)) {
        try {
            await apiRequest('DELETE', `/apps/${currentApp.name}`);
            showToast(`App "${currentApp.name}" deleted`, 'success');
            showView('apps');
            refreshApps();
        } catch (err) {
            showToast(err.message, 'error');
        }
    }
}

// Add-ons
function showAddAddonModal() {
    document.getElementById('add-addon-modal').classList.add('active');
}

async function addAddon(e) {
    e.preventDefault();

    if (!currentApp) return;

    const form = e.target;
    const type = form.type.value;
    const plan = form.plan.value;

    try {
        await apiRequest('POST', `/apps/${currentApp.name}/addons`, { type, plan });
        showToast(`${type} add-on provisioned!`, 'success');
        closeModal('add-addon-modal');
        form.reset();
        await loadAppAddons(currentApp.name);
    } catch (err) {
        showToast(err.message, 'error');
    }
}

// Config
function showAddConfigModal() {
    document.getElementById('add-config-modal').classList.add('active');
    document.getElementById('config-key').focus();
}

async function addConfig(e) {
    e.preventDefault();

    if (!currentApp) return;

    const form = e.target;
    const key = form.key.value.trim().toUpperCase();
    const value = form.value.value;

    try {
        await apiRequest('PUT', `/apps/${currentApp.name}/config`, { env: { [key]: value } });
        showToast(`Config variable ${key} added`, 'success');
        closeModal('add-config-modal');
        form.reset();
        await showAppDetail(currentApp.name);
    } catch (err) {
        showToast(err.message, 'error');
    }
}

// Logs
async function populateLogsAppSelect() {
    try {
        const apps = await apiRequest('GET', '/apps');
        const select = document.getElementById('logs-app-select');
        select.innerHTML = '<option value="">Select an app...</option>' +
            apps.map(app => `<option value="${app.name}">${app.name}</option>`).join('');
    } catch (err) {
        console.error('Failed to load apps for logs:', err);
    }
}

async function loadLogs() {
    const select = document.getElementById('logs-app-select');
    const appName = select.value;
    const logsContent = document.getElementById('logs-content');

    if (!appName) {
        logsContent.textContent = 'Select an app to view logs';
        return;
    }

    try {
        const logs = await apiRequest('GET', `/apps/${appName}/logs`);
        if (logs.length === 0) {
            logsContent.textContent = 'No logs available';
        } else {
            logsContent.textContent = logs.join('\n');
        }
    } catch (err) {
        logsContent.textContent = `Error loading logs: ${err.message}`;
    }
}

// Modal
function closeModal(modalId) {
    document.getElementById(modalId).classList.remove('active');
}

// Close modal on escape key
document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
        document.querySelectorAll('.modal.active').forEach(m => m.classList.remove('active'));
    }
});

// Close modal on backdrop click
document.querySelectorAll('.modal').forEach(modal => {
    modal.addEventListener('click', (e) => {
        if (e.target === modal) {
            modal.classList.remove('active');
        }
    });
});

// Toast
function showToast(message, type = 'info') {
    const container = document.getElementById('toast-container');
    const toast = document.createElement('div');
    toast.className = `toast ${type}`;
    toast.textContent = message;
    container.appendChild(toast);

    setTimeout(() => {
        toast.remove();
    }, 3000);
}

// Helpers
function formatDate(timestamp) {
    if (!timestamp) return 'N/A';

    // Handle Unix timestamp (seconds)
    let date;
    if (/^\d+$/.test(timestamp)) {
        date = new Date(parseInt(timestamp) * 1000);
    } else {
        date = new Date(timestamp);
    }

    if (isNaN(date.getTime())) return timestamp;

    return date.toLocaleDateString() + ' ' + date.toLocaleTimeString();
}

function maskSecret(key, value) {
    if (key.includes('PASSWORD') || key.includes('SECRET') || key.includes('KEY') || key.includes('TOKEN')) {
        return '••••••••';
    }
    if (value.length > 40) {
        return value.substring(0, 40) + '...';
    }
    return value;
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
}
