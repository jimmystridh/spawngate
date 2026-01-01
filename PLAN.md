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

## 3. Horizontal Scaling
Run multiple instances of an app with load balancing.

- [x] Add `scale` command to CLI (`paas scale web=3`)
- [x] Spawn multiple containers per app
- [ ] Integrate with spawngate proxy for load balancing
- [ ] Health checks for each instance
- [ ] Rolling deploys

## 4. Secrets Management
Encrypted environment variables.

- [ ] Encrypt sensitive config vars at rest
- [ ] Separate secrets from regular config
- [ ] Key rotation support
- [ ] Audit log for secret access

## 5. Webhooks/CI Integration
GitHub webhooks for auto-deploy on push.

- [ ] Webhook endpoint for GitHub/GitLab
- [ ] Verify webhook signatures
- [ ] Auto-deploy on push to main branch
- [ ] Deploy status notifications
- [ ] Build status badges

## 6. Custom Domains
Map custom domains to apps with automatic SSL.

- [ ] Add domain to app (`paas domains add example.com`)
- [ ] DNS verification
- [ ] Automatic SSL via Let's Encrypt (ACME)
- [ ] Wildcard subdomain support
- [ ] Domain routing in spawngate proxy
