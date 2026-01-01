use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, TokenData, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: i64,
    pub iat: i64,
    pub role: String,
}

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub secret: String,
    pub token_expiry_hours: i64,
    pub cookie_name: String,
    pub cookie_secure: bool,
    pub cookie_http_only: bool,
    pub cookie_same_site: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            secret: uuid::Uuid::new_v4().to_string(),
            token_expiry_hours: 24,
            cookie_name: "spawngate_session".to_string(),
            cookie_secure: true,
            cookie_http_only: true,
            cookie_same_site: "Strict".to_string(),
        }
    }
}

#[derive(Clone)]
pub struct AuthManager {
    config: Arc<AuthConfig>,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
}

impl AuthManager {
    pub fn new(config: AuthConfig) -> Self {
        let encoding_key = EncodingKey::from_secret(config.secret.as_bytes());
        let decoding_key = DecodingKey::from_secret(config.secret.as_bytes());
        Self {
            config: Arc::new(config),
            encoding_key,
            decoding_key,
        }
    }

    pub fn create_token(&self, user_id: &str, role: &str) -> Result<String, jsonwebtoken::errors::Error> {
        let now = Utc::now();
        let exp = now + Duration::hours(self.config.token_expiry_hours);

        let claims = Claims {
            sub: user_id.to_string(),
            exp: exp.timestamp(),
            iat: now.timestamp(),
            role: role.to_string(),
        };

        encode(&Header::default(), &claims, &self.encoding_key)
    }

    pub fn verify_token(&self, token: &str) -> Result<TokenData<Claims>, jsonwebtoken::errors::Error> {
        let validation = Validation::default();
        decode::<Claims>(token, &self.decoding_key, &validation)
    }

    pub fn create_session_cookie(&self, token: &str) -> String {
        let mut cookie = format!(
            "{}={}; Path=/; Max-Age={}",
            self.config.cookie_name,
            token,
            self.config.token_expiry_hours * 3600
        );

        if self.config.cookie_http_only {
            cookie.push_str("; HttpOnly");
        }

        if self.config.cookie_secure {
            cookie.push_str("; Secure");
        }

        cookie.push_str(&format!("; SameSite={}", self.config.cookie_same_site));

        cookie
    }

    pub fn create_logout_cookie(&self) -> String {
        format!(
            "{}=; Path=/; Max-Age=0; HttpOnly; SameSite={}",
            self.config.cookie_name,
            self.config.cookie_same_site
        )
    }

    pub fn extract_token_from_cookie(&self, cookie_header: &str) -> Option<String> {
        for cookie in cookie_header.split(';') {
            let cookie = cookie.trim();
            if let Some(value) = cookie.strip_prefix(&format!("{}=", self.config.cookie_name)) {
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
        None
    }

    pub fn extract_token_from_header(&self, auth_header: &str) -> Option<String> {
        auth_header.strip_prefix("Bearer ").map(|s| s.to_string())
    }

    pub fn config(&self) -> &AuthConfig {
        &self.config
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> AuthConfig {
        AuthConfig {
            secret: "test-secret-key-for-jwt-testing".to_string(),
            token_expiry_hours: 24,
            cookie_name: "test_session".to_string(),
            cookie_secure: false,
            cookie_http_only: true,
            cookie_same_site: "Strict".to_string(),
        }
    }

    #[test]
    fn test_create_and_verify_token() {
        let auth = AuthManager::new(test_config());
        let token = auth.create_token("user123", "admin").unwrap();

        let decoded = auth.verify_token(&token).unwrap();
        assert_eq!(decoded.claims.sub, "user123");
        assert_eq!(decoded.claims.role, "admin");
    }

    #[test]
    fn test_token_expiry() {
        let config = AuthConfig {
            token_expiry_hours: 1,
            ..test_config()
        };
        let auth = AuthManager::new(config);
        let token = auth.create_token("user", "user").unwrap();

        let decoded = auth.verify_token(&token).unwrap();
        let now = Utc::now().timestamp();
        assert!(decoded.claims.exp > now);
        assert!(decoded.claims.exp <= now + 3600 + 1);
    }

    #[test]
    fn test_invalid_token() {
        let auth = AuthManager::new(test_config());
        let result = auth.verify_token("invalid.token.here");
        assert!(result.is_err());
    }

    #[test]
    fn test_wrong_secret() {
        let auth1 = AuthManager::new(test_config());
        let token = auth1.create_token("user", "admin").unwrap();

        let config2 = AuthConfig {
            secret: "different-secret".to_string(),
            ..test_config()
        };
        let auth2 = AuthManager::new(config2);
        let result = auth2.verify_token(&token);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_session_cookie() {
        let auth = AuthManager::new(test_config());
        let cookie = auth.create_session_cookie("mytoken123");

        assert!(cookie.contains("test_session=mytoken123"));
        assert!(cookie.contains("Path=/"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(cookie.contains("Max-Age=86400"));
    }

    #[test]
    fn test_create_session_cookie_secure() {
        let config = AuthConfig {
            cookie_secure: true,
            ..test_config()
        };
        let auth = AuthManager::new(config);
        let cookie = auth.create_session_cookie("mytoken");

        assert!(cookie.contains("Secure"));
    }

    #[test]
    fn test_create_logout_cookie() {
        let auth = AuthManager::new(test_config());
        let cookie = auth.create_logout_cookie();

        assert!(cookie.contains("test_session="));
        assert!(cookie.contains("Max-Age=0"));
        assert!(cookie.contains("HttpOnly"));
    }

    #[test]
    fn test_extract_token_from_cookie() {
        let auth = AuthManager::new(test_config());

        let cookie = "test_session=abc123; other=value";
        assert_eq!(auth.extract_token_from_cookie(cookie), Some("abc123".to_string()));

        let cookie = "other=value; test_session=xyz789";
        assert_eq!(auth.extract_token_from_cookie(cookie), Some("xyz789".to_string()));

        let cookie = "other=value";
        assert_eq!(auth.extract_token_from_cookie(cookie), None);

        let cookie = "test_session=";
        assert_eq!(auth.extract_token_from_cookie(cookie), None);
    }

    #[test]
    fn test_extract_token_from_header() {
        let auth = AuthManager::new(test_config());

        assert_eq!(auth.extract_token_from_header("Bearer abc123"), Some("abc123".to_string()));
        assert_eq!(auth.extract_token_from_header("abc123"), None);
        assert_eq!(auth.extract_token_from_header("Basic abc123"), None);
    }

    #[test]
    fn test_default_config() {
        let config = AuthConfig::default();
        assert_eq!(config.token_expiry_hours, 24);
        assert_eq!(config.cookie_name, "spawngate_session");
        assert!(config.cookie_secure);
        assert!(config.cookie_http_only);
        assert_eq!(config.cookie_same_site, "Strict");
        assert!(!config.secret.is_empty());
    }
}
