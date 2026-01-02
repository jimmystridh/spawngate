# PaaS Platform Roadmap

## 1. App State Persistence ✅
Store app configs in SQLite so they survive restarts.

- [x] Add SQLite database for app state
- [x] Store apps, addons, deployments, config vars
- [x] Migration system for schema changes
- [x] Replace JSON file persistence with SQLite

## 2. Web Dashboard ✅
Simple UI to see apps, logs, metrics.

- [x] Serve static dashboard from API server
- [x] App list view with status indicators
- [x] App detail page (config, addons, deploys)
- [x] Real-time log viewer
- [x] Basic metrics (requests, memory, CPU)

## 3. Horizontal Scaling ✅
Run multiple instances of an app with load balancing.

- [x] Add `scale` command to CLI (`paas scale web=3`)
- [x] Spawn multiple containers per app
- [x] Integrate with spawngate proxy for load balancing
- [x] Health checks for each instance
- [x] Rolling deploys

## 4. Secrets Management ✅
Encrypted environment variables.

- [x] Encrypt sensitive config vars at rest
- [x] Separate secrets from regular config
- [x] Key rotation support
- [x] Audit log for secret access

## 5. Webhooks/CI Integration ✅
GitHub webhooks for auto-deploy on push.

- [x] Webhook endpoint for GitHub/GitLab
- [x] Verify webhook signatures (HMAC-SHA256)
- [x] Auto-deploy on push to main branch
- [x] Deploy status notifications
- [x] Build status badges

## 6. Custom Domains ✅
Map custom domains to apps with automatic SSL.

- [x] Add domain to app (`paas domains add example.com`)
- [x] DNS verification (TXT record based)
- [x] Automatic SSL via Let's Encrypt (ACME) - self-signed for now
- [x] Wildcard subdomain support (*.example.com)
- [x] Domain routing in spawngate proxy (via database lookup)

## 7. Buildpacks ✅
Auto-detect language and build apps without Dockerfile.

- [x] Language detection from project files
- [x] Node.js support (npm, yarn, pnpm)
- [x] Python support (pip, poetry, pipenv)
- [x] Go support (go.mod)
- [x] Ruby support (bundler, Rails)
- [x] Rust support (Cargo)
- [x] Static site support (nginx)
- [x] Procfile parsing for process types
- [x] Framework detection (Express, Next.js, Flask, FastAPI, Django, Rails, etc.)
- [x] Automatic Dockerfile generation
- [x] Extensive integration tests (34 tests)

## 8. Extensive Web Control Panel
Full-featured web UI for managing the PaaS platform.

### Phase 1: Authentication & Core UI ✅
- [x] User authentication system (JWT-based)
- [x] Session management with secure cookies
- [x] Modern responsive UI framework (Tailwind CSS)
- [x] Dark/light theme support
- [x] Navigation with breadcrumbs

### Phase 2: App Management ✅
- [x] App dashboard with status cards
- [x] Create app wizard (multi-step with env vars)
- [x] App settings page (port, scaling, maintenance mode)
- [x] Environment variables editor (with secret masking, visibility toggle)
- [x] Delete app with confirmation (type-to-confirm safety)

### Phase 3: Deployments & Logs ✅
- [x] Real-time deployment progress (polling with status updates, build logs)
- [x] Deployment history with details (status icons, commit hash, current badge)
- [x] One-click rollback (rollback button on successful deployments)
- [x] Live log streaming (auto-refresh with follow mode)
- [x] Log search and filtering (by source, level, and text search)
- [x] Log download (export filtered logs as text file)

### Phase 4: Scaling & Processes
- [ ] Visual dyno manager (slider/input)
- [ ] Process type cards (web, worker, etc.)
- [ ] Restart/stop individual dynos
- [ ] Resource usage graphs (CPU, memory)
- [ ] Formation editor

### Phase 5: Domains & SSL
- [ ] Domain list with status badges
- [ ] Add domain wizard
- [ ] DNS verification helper
- [ ] SSL certificate status
- [ ] Auto-renew indicators

### Phase 6: Add-ons & Services
- [ ] Add-on marketplace view
- [ ] One-click add-on provisioning
- [ ] Add-on credentials (with copy button)
- [ ] Usage metrics per add-on

### Phase 7: Webhooks & CI
- [ ] Webhook configuration UI
- [ ] GitHub/GitLab integration wizard
- [ ] Webhook event log
- [ ] Build status badges preview
- [ ] Deploy key management

### Phase 8: Secrets & Security
- [ ] Secrets manager with audit log
- [ ] Key rotation UI
- [ ] Access audit trail
- [ ] API token management

### Phase 9: Metrics & Monitoring
- [ ] Request rate graphs
- [ ] Response time percentiles
- [ ] Error rate visualization
- [ ] Memory/CPU charts (per dyno)
- [ ] Alerting rules (future)

### Phase 10: Activity & Notifications
- [ ] Activity feed (all actions)
- [ ] Filterable event log
- [ ] User action attribution
- [ ] Email/webhook notifications (future)
