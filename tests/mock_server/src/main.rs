//! Mock server for integration testing
//!
//! Environment variables:
//! - PORT: Port to listen on (required)
//! - STARTUP_DELAY_MS: Delay before accepting connections (default: 0)
//! - SERVERLESS_PROXY_READY_URL: URL to POST to when ready (optional)

use base64::Engine;
use sha1::{Digest, Sha1};
use std::env;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// WebSocket magic GUID for handshake
const WS_MAGIC_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Process start time for uptime tracking
static START_TIME: OnceLock<Instant> = OnceLock::new();

fn get_uptime() -> Duration {
    START_TIME.get().map(|t| t.elapsed()).unwrap_or_default()
}

#[tokio::main]
async fn main() {
    START_TIME.set(Instant::now()).ok();

    let port: u16 = env::var("PORT")
        .expect("PORT environment variable required")
        .parse()
        .expect("PORT must be a valid port number");

    let startup_delay: u64 = env::var("STARTUP_DELAY_MS")
        .unwrap_or_else(|_| "0".to_string())
        .parse()
        .unwrap_or(0);

    let ready_url = env::var("SERVERLESS_PROXY_READY_URL").ok();

    // Simulate startup delay
    if startup_delay > 0 {
        eprintln!("Mock server: sleeping for {}ms before starting", startup_delay);
        tokio::time::sleep(Duration::from_millis(startup_delay)).await;
    }

    let listener = TcpListener::bind(format!("127.0.0.1:{}", port))
        .await
        .expect("Failed to bind");

    eprintln!("Mock server: listening on port {}", port);

    // Send ready callback if URL provided
    if let Some(url) = ready_url {
        eprintln!("Mock server: sending ready callback to {}", url);
        // Simple HTTP POST without external dependencies
        if let Err(e) = send_ready_callback(&url).await {
            eprintln!("Mock server: failed to send ready callback: {}", e);
        }
    }

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                eprintln!("Mock server: connection from {}", addr);
                tokio::spawn(async move {
                    handle_connection(stream).await;
                });
            }
            Err(e) => {
                eprintln!("Mock server: accept error: {}", e);
            }
        }
    }
}

async fn handle_connection(mut stream: tokio::net::TcpStream) {
    let mut buf = Vec::new();
    let mut temp = [0u8; 1024];

    // Read HTTP request headers
    loop {
        let n = match stream.read(&mut temp).await {
            Ok(0) => return,
            Ok(n) => n,
            Err(_) => return,
        };
        buf.extend_from_slice(&temp[..n]);

        // Check for end of headers
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 8192 {
            return; // Headers too large
        }
    }

    let request_str = match std::str::from_utf8(&buf) {
        Ok(s) => s,
        Err(_) => return,
    };

    let mut lines = request_str.lines();
    let request_line = match lines.next() {
        Some(l) => l,
        None => return,
    };

    let parts: Vec<&str> = request_line.split(' ').collect();
    let (method, path) = if parts.len() >= 2 {
        (parts[0], parts[1])
    } else {
        ("GET", "/")
    };

    eprintln!("Mock server: {} {}", method, path);

    // Collect headers
    let headers: Vec<&str> = lines.take_while(|l| !l.is_empty()).collect();

    // Check for WebSocket upgrade
    let is_websocket = headers.iter().any(|h| {
        let lower = h.to_lowercase();
        lower.contains("upgrade") && lower.contains("websocket")
    });

    if path == "/ws" && is_websocket {
        // Handle WebSocket upgrade
        let ws_key = headers
            .iter()
            .find(|h| h.to_lowercase().starts_with("sec-websocket-key:"))
            .and_then(|h| h.split_once(':'))
            .map(|(_, v)| v.trim().to_string());

        if let Some(key) = ws_key {
            // Compute accept key
            let accept_key = compute_ws_accept(&key);

            let response = format!(
                "HTTP/1.1 101 Switching Protocols\r\n\
                 Upgrade: websocket\r\n\
                 Connection: Upgrade\r\n\
                 Sec-WebSocket-Accept: {}\r\n\
                 \r\n",
                accept_key
            );
            if stream.write_all(response.as_bytes()).await.is_err() {
                return;
            }

            eprintln!("Mock server: WebSocket upgrade successful");
            handle_websocket(stream).await;
        }
        return;
    }

    // Generate response based on path
    let (status, body) = match path {
        "/health" | "/healthz" | "/ready" => ("200 OK", "ok".to_string()),
        "/echo" => ("200 OK", "echo response".to_string()),
        "/headers" => {
            // Return all headers as JSON
            let mut headers_json = String::from("{");
            for (i, h) in headers.iter().enumerate() {
                if let Some((name, value)) = h.split_once(':') {
                    if i > 0 {
                        headers_json.push_str(",");
                    }
                    headers_json.push_str(&format!(
                        "\"{}\":\"{}\"",
                        name.trim().to_lowercase(),
                        value.trim().replace("\"", "\\\"")
                    ));
                }
            }
            headers_json.push('}');
            ("200 OK", headers_json)
        }
        "/slow" => {
            tokio::time::sleep(Duration::from_secs(2)).await;
            ("200 OK", "slow response".to_string())
        }
        "/error" => ("500 Internal Server Error", "error".to_string()),
        _ => {
            let uptime = get_uptime();
            ("200 OK", format!("Hello! Uptime: {:.1}s", uptime.as_secs_f64()))
        }
    };

    let content_type = if path == "/headers" {
        "application/json"
    } else {
        "text/plain"
    };

    let response = format!(
        "HTTP/1.1 {}\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         X-Mock-Server: true\r\n\
         \r\n\
         {}",
        status,
        content_type,
        body.len(),
        body
    );

    let _ = stream.write_all(response.as_bytes()).await;
}

async fn send_ready_callback(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Parse URL to get host and port
    let url = url.strip_prefix("http://").unwrap_or(url);
    let (host_port, path) = url.split_once('/').unwrap_or((url, ""));
    let path = format!("/{}", path);

    let mut stream = tokio::net::TcpStream::connect(host_port).await?;

    let request = format!(
        "POST {} HTTP/1.1\r\n\
         Host: {}\r\n\
         Content-Length: 0\r\n\
         Connection: close\r\n\
         \r\n",
        path, host_port
    );

    stream.write_all(request.as_bytes()).await?;
    eprintln!("Mock server: ready callback sent successfully");
    Ok(())
}

/// Compute the Sec-WebSocket-Accept header value
fn compute_ws_accept(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WS_MAGIC_GUID.as_bytes());
    let hash = hasher.finalize();
    base64::engine::general_purpose::STANDARD.encode(hash)
}

/// Handle WebSocket connection - echo messages back
async fn handle_websocket(mut stream: tokio::net::TcpStream) {
    loop {
        // Read WebSocket frame header (2 bytes minimum)
        let mut header = [0u8; 2];
        if stream.read_exact(&mut header).await.is_err() {
            break;
        }

        let fin = (header[0] & 0x80) != 0;
        let opcode = header[0] & 0x0F;
        let masked = (header[1] & 0x80) != 0;
        let mut payload_len = (header[1] & 0x7F) as u64;

        // Handle extended payload length
        if payload_len == 126 {
            let mut ext = [0u8; 2];
            if stream.read_exact(&mut ext).await.is_err() {
                break;
            }
            payload_len = u16::from_be_bytes(ext) as u64;
        } else if payload_len == 127 {
            let mut ext = [0u8; 8];
            if stream.read_exact(&mut ext).await.is_err() {
                break;
            }
            payload_len = u64::from_be_bytes(ext);
        }

        // Read mask if present
        let mask = if masked {
            let mut m = [0u8; 4];
            if stream.read_exact(&mut m).await.is_err() {
                break;
            }
            Some(m)
        } else {
            None
        };

        // Read payload
        let mut payload = vec![0u8; payload_len as usize];
        if !payload.is_empty() && stream.read_exact(&mut payload).await.is_err() {
            break;
        }

        // Unmask payload if needed
        if let Some(mask) = mask {
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[i % 4];
            }
        }

        eprintln!(
            "Mock server: WebSocket frame opcode={} fin={} len={}",
            opcode, fin, payload_len
        );

        match opcode {
            0x1 => {
                // Text frame - echo back
                let text = String::from_utf8_lossy(&payload);
                eprintln!("Mock server: WebSocket received text: {}", text);

                // Send back (unmasked - server to client)
                let mut response = Vec::new();
                response.push(0x81); // FIN + text opcode

                if payload.len() < 126 {
                    response.push(payload.len() as u8);
                } else if payload.len() < 65536 {
                    response.push(126);
                    response.extend_from_slice(&(payload.len() as u16).to_be_bytes());
                } else {
                    response.push(127);
                    response.extend_from_slice(&(payload.len() as u64).to_be_bytes());
                }

                response.extend_from_slice(&payload);

                if stream.write_all(&response).await.is_err() {
                    break;
                }
            }
            0x8 => {
                // Close frame
                eprintln!("Mock server: WebSocket close received");
                // Send close frame back
                let close = [0x88, 0x00];
                let _ = stream.write_all(&close).await;
                break;
            }
            0x9 => {
                // Ping - send pong
                let mut pong = Vec::new();
                pong.push(0x8A); // FIN + pong opcode
                if payload.len() < 126 {
                    pong.push(payload.len() as u8);
                } else {
                    pong.push(126);
                    pong.extend_from_slice(&(payload.len() as u16).to_be_bytes());
                }
                pong.extend_from_slice(&payload);
                let _ = stream.write_all(&pong).await;
            }
            0xA => {
                // Pong - ignore
            }
            _ => {
                eprintln!("Mock server: Unknown WebSocket opcode: {}", opcode);
            }
        }
    }
    eprintln!("Mock server: WebSocket connection closed");
}
