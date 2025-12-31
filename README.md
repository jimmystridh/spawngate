# Spawngate

A serverless-style reverse proxy that spawns backend processes on demand.

Spawngate routes HTTP traffic based on the Host header to configured backends, automatically starting backend processes when requests arrive and shutting them down after a configurable idle timeout. This enables serverless-like behavior for any application without code changes.

## Features

- **On-demand process spawning**: Backends start automatically when traffic arrives
- **Docker container support**: Run backends as Docker containers with full lifecycle management
- **Automatic idle shutdown**: Processes/containers stop after configurable inactivity periods
- **Health monitoring**: Two-phase health checking (startup polling + continuous monitoring)
- **Graceful shutdown**: Drain in-flight requests before stopping backends
- **HTTP/1.1 and HTTP/2 support**: Auto-detection with h2c (HTTP/2 cleartext) prior knowledge
- **Connection pooling**: Efficient HTTP connection reuse to backends
- **WebSocket support**: Full bidirectional WebSocket proxying with upgrade handling
- **Ready callbacks**: Backends can signal readiness via HTTP callback
- **Request tracing**: Automatic X-Request-ID generation and header forwarding
- **Configurable timeouts**: Per-backend startup, request, drain, and grace period settings
- **Hot reload**: Update backend configuration without restarting (SIGHUP)

## Installation

### From Source

```bash
git clone https://github.com/your-org/spawngate.git
cd spawngate
cargo build --release
```

The binary will be at `target/release/spawngate`.

### Requirements

- Rust 1.70 or later
- Linux, macOS, or Windows

## Quick Start

1. Create a configuration file `config.toml`:

```toml
[server]
port = 8080
bind = "127.0.0.1"
admin_port = 9999

[defaults]
idle_timeout_secs = 300
startup_timeout_secs = 30
health_path = "/health"

[backends."myapp.localhost"]
command = "node"
args = ["server.js"]
port = 3000
working_dir = "/path/to/myapp"

[backends."myapp.localhost".env]
NODE_ENV = "production"
```

2. Run the proxy:

```bash
./spawngate config.toml
```

3. Send a request:

```bash
curl -H "Host: myapp.localhost" http://127.0.0.1:8080/
```

Spawngate will automatically start your Node.js server, wait for it to become healthy, and forward the request.

## Configuration

### Server Settings

```toml
[server]
port = 80                      # Proxy listen port
bind = "0.0.0.0"               # Bind address
admin_port = 9999              # Admin API port (internal)
pool_max_idle_per_host = 10    # Max idle connections per backend
pool_idle_timeout_secs = 90    # Idle connection timeout
pid_file = "/var/run/spawngate.pid"  # Optional PID file
```

### Default Backend Settings

These apply to all backends unless overridden:

```toml
[defaults]
idle_timeout_secs = 600              # Stop backend after 10 min idle
startup_timeout_secs = 30            # Max time to wait for health check
health_check_interval_ms = 100       # Poll interval during startup
health_path = "/health"              # Health endpoint path
request_timeout_secs = 30            # Max request duration
shutdown_grace_period_secs = 10      # Time between SIGTERM and SIGKILL
drain_timeout_secs = 30              # Max time to drain in-flight requests
ready_health_check_interval_ms = 5000  # Health poll interval when ready
unhealthy_threshold = 3              # Failures before marking unhealthy
```

### Backend Configuration

Spawngate supports two backend types: **local processes** (default) and **Docker containers**.

#### Local Process Backend

```toml
[backends."api.example.com"]
command = "python"
args = ["-m", "uvicorn", "main:app", "--port", "8000"]
port = 8000
working_dir = "/opt/api"

# Override defaults for this backend
idle_timeout_secs = 120
startup_timeout_secs = 60
health_path = "/healthz"
request_timeout_secs = 120

[backends."api.example.com".env]
DATABASE_URL = "postgres://localhost/mydb"
```

#### Docker Container Backend

```toml
[backends."app.example.com"]
type = "docker"
image = "myapp:latest"
port = 3000

# Optional Docker-specific settings
container_name = "myapp"              # Default: spawngate-{hostname}
pull_policy = "if-not-present"        # Options: always, never, if-not-present
memory = "512m"                       # Memory limit (e.g., 512m, 1g)
cpus = "1.0"                          # CPU limit (e.g., 0.5, 2)
network = "bridge"                    # Docker network mode

# Container command arguments (passed to CMD)
args = ["--workers", "4"]

# Environment variables
[backends."app.example.com".env]
NODE_ENV = "production"
```

## Docker Backend Support

Spawngate can manage Docker containers as backends, providing the same on-demand spawning behavior for containerized applications.

### Docker Configuration Options

| Option | Required | Default | Description |
|--------|----------|---------|-------------|
| `type` | Yes | `local` | Set to `"docker"` for container backends |
| `image` | Yes | - | Docker image to run (e.g., `nginx:latest`) |
| `port` | Yes | - | Port the container listens on |
| `container_name` | No | `spawngate-{hostname}` | Custom container name |
| `pull_policy` | No | `if-not-present` | When to pull: `always`, `never`, `if-not-present` |
| `memory` | No | - | Memory limit (e.g., `256m`, `1g`, `2gb`) |
| `cpus` | No | - | CPU limit (e.g., `0.5`, `1.0`, `2`) |
| `network` | No | - | Docker network mode |
| `docker_host` | No | auto-detect | Docker daemon URL |
| `args` | No | - | Arguments passed to container CMD |

### Docker Daemon Connection

Spawngate auto-detects the Docker socket in these locations:

1. `DOCKER_HOST` environment variable
2. `/var/run/docker.sock` (Linux default)
3. `~/.docker/run/docker.sock` (Docker Desktop on macOS)
4. `~/.colima/default/docker.sock` (Colima on macOS)
5. `~/.rd/docker.sock` (Rancher Desktop)

You can also specify a custom Docker host per-backend:

```toml
[backends."app.example.com"]
type = "docker"
image = "myapp:latest"
port = 3000
docker_host = "unix:///custom/path/docker.sock"
# or for remote Docker:
# docker_host = "tcp://192.168.1.100:2375"
```

### Pull Policies

- **`if-not-present`** (default): Pull only if the image doesn't exist locally
- **`always`**: Always pull the latest image before starting
- **`never`**: Never pull; fail if image doesn't exist locally

### Container Lifecycle

When a request arrives for a Docker backend:

1. Pull the image (based on pull policy)
2. Create container with port mapping, env vars, and resource limits
3. Start the container
4. Poll health endpoint until ready
5. Forward requests to `127.0.0.1:{port}`

On shutdown (idle timeout or proxy stop):

1. Stop container with graceful timeout
2. Remove container

### Container Logs

Container stdout/stderr logs are automatically forwarded to Spawngate's logging output:
- `stdout` messages are logged at INFO level with `target: "container"`
- `stderr` messages are logged at WARN level with `target: "container"`

Log entries include the hostname and stream type for easy filtering:
```
INFO container: hostname="app.example.com" stream="stdout" Starting server on port 3000
WARN container: hostname="app.example.com" stream="stderr" Connection refused
```

Log streaming starts when the container starts and stops automatically when the container is stopped.

### Mixed Backends Example

You can mix local process and Docker backends:

```toml
[backends."api.example.com"]
command = "node"
args = ["server.js"]
port = 3000

[backends."worker.example.com"]
type = "docker"
image = "myworker:latest"
port = 8080
memory = "256m"
cpus = "0.5"
```

### Requirements

- Docker daemon must be running and accessible
- Spawngate needs permission to access the Docker socket
- Images are pulled on first request (may add latency)

## How It Works

### Request Flow

1. Client sends HTTP request with Host header
2. Spawngate checks if a backend is configured for that host
3. If backend is not running, Spawngate starts it
4. Spawngate polls the health endpoint until it returns 2xx
5. Request is forwarded to the backend
6. Response is returned to the client

### Backend Lifecycle

```
Stopped -> Starting -> Ready -> Stopping -> Stopped
              |          |
              |          v
              +------ Unhealthy (auto-restart)
```

- **Stopped**: Process not running
- **Starting**: Process spawned, waiting for health check
- **Ready**: Accepting traffic
- **Unhealthy**: Health checks failing, auto-restart triggered
- **Stopping**: Draining requests before shutdown

### Ready Callback

Backends can optionally signal readiness by POSTing to the admin API. The callback URL is provided via the `SERVERLESS_PROXY_READY_URL` environment variable:

```bash
# In your backend startup script
curl -X POST "$SERVERLESS_PROXY_READY_URL"
```

This is faster than waiting for health check polling.

### Environment Variables

Spawngate sets these environment variables for spawned backends (both local processes and Docker containers):

| Variable | Description |
|----------|-------------|
| `PORT` | Port the backend should listen on |
| `SERVERLESS_PROXY_READY_URL` | Callback URL for ready notification |

For Docker containers, custom environment variables are passed via the `[backends."host".env]` table.

## Proxy Headers

Spawngate adds standard proxy headers to forwarded requests:

| Header | Description |
|--------|-------------|
| `X-Request-ID` | Unique request identifier (generated or propagated) |
| `X-Forwarded-For` | Client IP address chain |
| `X-Forwarded-Host` | Original Host header value |
| `X-Forwarded-Proto` | Protocol (http) |

## WebSocket Support

Spawngate fully supports WebSocket connections. When a client sends an HTTP Upgrade request for WebSocket, Spawngate:

1. Detects the upgrade request via `Connection: Upgrade` and `Upgrade: websocket` headers
2. Establishes a raw TCP connection to the backend
3. Forwards the upgrade handshake to the backend
4. Returns the backend's 101 Switching Protocols response to the client
5. Bidirectionally forwards all WebSocket frames between client and backend

### Usage

No special configuration is required. WebSocket requests are automatically detected and handled:

```javascript
// Client-side JavaScript
const ws = new WebSocket('ws://myapp.localhost:8080/ws');
ws.onmessage = (event) => console.log('Received:', event.data);
ws.send('Hello WebSocket!');
```

### Backend Implementation

Your backend should handle WebSocket upgrades normally. Example with Node.js:

```javascript
const WebSocket = require('ws');
const wss = new WebSocket.Server({ port: 3000, path: '/ws' });

wss.on('connection', (ws) => {
  ws.on('message', (message) => {
    ws.send(`Echo: ${message}`);
  });
});
```

### Connection Tracking

WebSocket connections are tracked as in-flight requests. This means:

- Active WebSocket connections prevent idle shutdown
- Graceful shutdown waits for WebSocket connections to close
- The `drain_timeout_secs` setting applies to WebSocket connections

### Supported Protocols

- WebSocket (ws://) over HTTP
- Any protocol that uses HTTP Upgrade mechanism

Note: WSS (WebSocket Secure) requires TLS termination at a load balancer in front of Spawngate.

## HTTP/2 Support

Spawngate supports HTTP/2 with prior knowledge (h2c - HTTP/2 cleartext). Clients can connect using either HTTP/1.1 or HTTP/2, and the proxy auto-detects the protocol.

### How It Works

- **Client to Proxy**: Supports both HTTP/1.1 and HTTP/2 (auto-detected)
- **Proxy to Backend**: Uses HTTP/1.1 (standard for local backend applications)
- **HTTP/2 Features**: Full support for multiplexing, header compression, and stream prioritization

### Using HTTP/2 with curl

```bash
# HTTP/2 with prior knowledge (h2c)
curl --http2-prior-knowledge -H "Host: myapp.localhost" http://127.0.0.1:8080/

# HTTP/2 upgrade from HTTP/1.1
curl --http2 -H "Host: myapp.localhost" http://127.0.0.1:8080/
```

### Configuration

HTTP/2 is enabled by default with the following settings:

- Max concurrent streams: 250 per connection
- HTTP/1.1 header case preserved for compatibility

### Notes

- HTTP/2 does not support WebSocket upgrades; WebSocket connections use HTTP/1.1
- For HTTP/2 over TLS (h2), use a TLS-terminating load balancer in front of Spawngate
- Backend connections remain HTTP/1.1 as most backend frameworks serve HTTP/1.1

## Admin API

The admin API runs on `admin_port` (default 9999) and provides:

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Admin API health check |
| `/version` | GET | Version information (JSON) |
| `/ready/{hostname}` | POST | Backend ready callback |
| `/backends` | GET | List all backends and their status (JSON) |

### Backends Endpoint

The `/backends` endpoint returns JSON with status information for all configured backends:

```json
{
  "backends": [
    {
      "hostname": "myapp.localhost",
      "state": "ready",
      "port": 3000,
      "in_flight": 2
    },
    {
      "hostname": "api.localhost",
      "state": "stopped",
      "port": 4000,
      "in_flight": 0
    }
  ],
  "count": 2
}
```

Possible states: `stopped`, `starting`, `ready`, `unhealthy`, `stopping`

## Error Responses

Spawngate returns JSON error responses with an `X-Proxy-Error` header:

```json
{
  "code": "BACKEND_START_FAILED",
  "message": "Failed to start backend: Timeout waiting for backend to start",
  "status": 503
}
```

Error codes:

| Code | Status | Description |
|------|--------|-------------|
| `MISSING_HOST_HEADER` | 400 | No Host header in request |
| `UNKNOWN_HOST` | 404 | No backend configured for host |
| `BACKEND_SHUTTING_DOWN` | 503 | Backend is draining |
| `BACKEND_UNHEALTHY` | 503 | Backend failed health checks |
| `BACKEND_START_FAILED` | 503 | Backend failed to start |
| `REQUEST_TIMEOUT` | 504 | Backend response timeout |
| `CONNECTION_FAILED` | 502 | Could not connect to backend |

## Graceful Shutdown

When stopping a backend (idle timeout or proxy shutdown):

### Local Process Backends

1. Mark backend as Stopping (reject new requests with 503)
2. Wait for in-flight requests to complete (up to `drain_timeout_secs`)
3. Send SIGTERM to the process
4. Wait for graceful exit (up to `shutdown_grace_period_secs`)
5. Send SIGKILL if still running

### Docker Container Backends

1. Mark backend as Stopping (reject new requests with 503)
2. Wait for in-flight requests to complete (up to `drain_timeout_secs`)
3. Stop container (sends SIGTERM to PID 1)
4. Wait for container to stop (up to `shutdown_grace_period_secs`)
5. Force kill container if still running
6. Remove container

## Hot Reload

Spawngate supports hot reloading of backend configuration without restarting the proxy. Send a `SIGHUP` signal to reload the configuration file:

```bash
# Reload configuration
kill -HUP $(cat /var/run/spawngate.pid)

# Or by process name
pkill -HUP spawngate
```

### What Can Be Reloaded

| Setting | Hot Reload | Notes |
|---------|------------|-------|
| New backends | ✅ Yes | Available immediately for new requests |
| Removed backends | ✅ Yes | Stopped gracefully with drain |
| Backend settings | ✅ Yes | Takes effect on next backend restart |
| Default timeouts | ✅ Yes | Applies to new requests |
| Server ports | ❌ No | Requires proxy restart |
| TLS certificates | ❌ No | Requires proxy restart |
| ACME settings | ❌ No | Requires proxy restart |

### Reload Behavior

- **New backends**: Added to configuration, available for routing immediately
- **Removed backends**: Gracefully stopped (drain in-flight requests, then shutdown)
- **Modified backends**: Config changes take effect when the backend next starts (idle timeout or manual restart)
- **Running backends**: Continue running with their original configuration until restarted

### Example

```bash
# Initial config with one backend
$ cat config.toml
[backends."app.example.com"]
command = "node"
args = ["server.js"]
port = 3000

# Start Spawngate
$ ./spawngate config.toml &

# Add a new backend to config
$ cat >> config.toml << 'EOF'
[backends."api.example.com"]
command = "python"
args = ["-m", "uvicorn", "main:app"]
port = 8000
EOF

# Reload configuration
$ kill -HUP $(pgrep spawngate)
# Logs: "Configuration reloaded successfully" with added/removed/updated counts
```

## Logging

Spawngate uses structured logging via `tracing`. Set log level with `RUST_LOG`:

```bash
RUST_LOG=spawngate=debug ./spawngate config.toml
RUST_LOG=spawngate=info,spawngate::proxy=debug ./spawngate config.toml
```

## Use Cases

- **Development environments**: Run multiple services without keeping them all running
- **Cost optimization**: Scale to zero when not in use
- **Multi-tenant hosting**: Isolate tenants in separate processes
- **Legacy application hosting**: Add serverless behavior without code changes
- **CI/CD runners**: Start build tools on demand

## Performance Considerations

- Connection pooling reduces latency for subsequent requests
- Health check client reuses connections across checks
- First request to a cold backend incurs startup latency
- Consider `startup_timeout_secs` based on your backend's startup time

## Building from Source

```bash
# Development build
cargo build

# Release build
cargo build --release

# Run tests
cargo test

# Run with logging
RUST_LOG=spawngate=debug cargo run -- config.toml
```

## License

MIT License

Copyright (c) 2024

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
