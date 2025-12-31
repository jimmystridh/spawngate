# Security Findings

This document summarizes security findings from a code review performed on 2025-12-30.

## Fixed Issues

### 1. Unauthenticated Admin API (Critical → Fixed)

**Issue:** The admin API endpoints `/ready/{hostname}` and `/backends` had no authentication, allowing any process to mark backends as ready or enumerate backend status.

**Fix:** Added token-based authentication:
- New `admin_token` config option in `[server]` section
- Auto-generates UUID token at startup if not configured
- Requires `Authorization: Bearer <token>` header for sensitive endpoints
- Public endpoints `/health` and `/version` remain unauthenticated

**Location:** `src/admin.rs:153-164`, `src/config.rs`

### 2. Missing Host Header Validation (High → Fixed)

**Issue:** No validation of hostname length or characters could enable DoS attacks via resource exhaustion or injection attacks.

**Fix:** Added hostname validation in proxy:
- Maximum 253 characters (DNS spec limit)
- Only ASCII alphanumeric, hyphens, and dots allowed
- Normalized to lowercase

**Location:** `src/proxy.rs:42-56`

### 3. Information Disclosure via Error Messages (Medium → Fixed)

**Issue:** Detailed error messages including internal paths and system info were returned to clients.

**Fix:** Sanitized error responses:
- Generic error messages returned to clients
- Detailed errors logged server-side for debugging
- Distinguishes between client-facing and internal error paths

**Location:** `src/proxy.rs` (multiple error handlers)

### 4. ACME Private Key File Permissions (Medium → Fixed)

**Issue:** ACME private keys were saved with default file permissions, potentially readable by other users.

**Fix:** Set restrictive permissions (0600) on Unix systems when saving private keys.

**Location:** `src/acme.rs:221-235`

### 5. ACME Cache Directory Path Traversal (High → Fixed)

**Issue:** The `cache_dir` path was used without validation. Path traversal could write files outside intended directory.

**Fix:** Added `validate_cache_dir()` function that:
- Rejects paths containing `..`
- Canonicalizes existing paths to resolve symlinks
- Validates parent directory for new paths

**Location:** `src/acme.rs:611-656`

### 6. Backend Startup Race Condition (High → Fixed)

**Issue:** Time-of-check-time-of-use race between checking backend state and forwarding requests could cause errors during startup transitions.

**Fix:** Modified `increment_in_flight()` to atomically verify backend is in Ready state before accepting requests:
- Returns `bool` indicating if request was accepted
- Holds lock while checking state and incrementing counter
- Proxy returns retry error if state changed

**Location:** `src/process.rs:130-142`, `src/proxy.rs:345-352`

### 7. Command Injection via Configuration (High → Documented)

**Issue:** Backend commands are specified in config files and executed directly. Malicious config files could execute arbitrary commands.

**Mitigation:** Added security documentation warning that config files must be protected with appropriate file permissions. This is an inherent design aspect of the proxy (it needs to execute backend commands) but users must be aware of the security implications.

**Location:** `src/config.rs:234-248`

### 8. Certificate Expiration Checking (Medium → Fixed)

**Issue:** The ACME implementation had a placeholder for certificate expiration checking.

**Fix:** Implemented proper X.509 certificate parsing using `x509-parser` crate:
- Parses certificate to extract `notAfter` timestamp
- Compares against current time to check remaining validity
- Triggers renewal when certificate has less than 30 days remaining
- Logs expiration warnings with remaining days

**Location:** `src/acme.rs:603-651`

### 9. Header Spoofing (Medium → Fixed)

**Issue:** X-Forwarded-* headers from clients were appended to, enabling IP spoofing.

**Fix:** Changed to overwrite (not append) X-Forwarded-* headers:
- `X-Forwarded-For` is set to actual client IP only
- `X-Forwarded-Host` and `X-Forwarded-Proto` are overwritten
- Added security comments explaining the rationale

**Location:** `src/proxy.rs:259-281`

### 10. Unencrypted Account Key Storage (Medium → Documented)

**Issue:** ACME account keys are stored unencrypted on disk.

**Mitigation:** Added security documentation in module header with recommendations:
- Use encrypted filesystem for cache directory
- Restrict directory permissions to service user only
- Consider secrets manager for high-security environments
- Back up cache directory securely

**Location:** `src/acme.rs:7-17`

### 11. Signal Handling (Low → Fixed)

**Issue:** Only SIGINT (Ctrl+C) was handled; SIGTERM (used by systemd/containers) was ignored.

**Fix:** Added proper signal handling for both SIGINT and SIGTERM on Unix:
- Uses `tokio::signal::unix` for SIGTERM
- Graceful shutdown on either signal
- Cross-platform fallback for non-Unix systems

**Location:** `src/main.rs:256-283`

### 12. PID File Locking (Low → Fixed)

**Issue:** PID file creation had a race condition and no locking.

**Fix:** Implemented exclusive file locking using `flock()`:
- Acquires exclusive lock before writing PID
- Fails fast if another instance is running
- Lock held for process lifetime via `PidFile` struct
- Cross-platform fallback for non-Unix systems

**Location:** `src/main.rs:337-392`

## Remaining Issues (Low Priority)

### 13. No Rate Limiting

**Issue:** No rate limiting on any endpoints, enabling DoS attacks.

**Recommendation:**
- Add per-IP rate limiting
- Add rate limiting on admin endpoints
- Consider using token bucket algorithm

## Security Hardening Recommendations

1. **Defense in Depth:** Run the proxy as a non-root user with minimal privileges
2. **Network Isolation:** Bind admin API to localhost only (already done)
3. **TLS Configuration:** Enforce TLS 1.2+ with modern cipher suites
4. **Logging:** Enable security audit logging for auth failures
5. **Config Protection:** Ensure config files are readable only by the service user
6. **Container Security:** If containerized, use read-only filesystem where possible
