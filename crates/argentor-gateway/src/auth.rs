//! JWT/OAuth2 authentication module for the Argentor gateway.
//!
//! Provides multi-mode authentication including JWT bearer tokens, API key
//! validation, and OAuth2 provider configuration. Includes an axum-compatible
//! middleware layer that extracts and validates credentials from incoming requests.
//!
//! # Authentication modes
//!
//! - [`AuthMode::None`] — No authentication required (development/testing).
//! - [`AuthMode::ApiKey`] — API key in `X-Api-Key` header.
//! - [`AuthMode::Jwt`] — JWT Bearer token in `Authorization` header.
//! - [`AuthMode::OAuth2`] — OAuth2 with external provider.
//! - [`AuthMode::Combined`] — Accepts either API key or JWT.

use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tracing::warn;

// ─── Base64url encoding/decoding (no-padding, URL-safe) ─────────────────────

/// Base64url alphabet (RFC 4648 §5).
const B64URL_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Encode bytes to base64url (no padding).
fn base64url_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity((input.len() * 4).div_ceil(3));
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(B64URL_CHARS[((triple >> 18) & 0x3F) as usize] as char);
        out.push(B64URL_CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(B64URL_CHARS[((triple >> 6) & 0x3F) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(B64URL_CHARS[(triple & 0x3F) as usize] as char);
        }
    }
    out
}

/// Decode a base64url (no padding) string to bytes.
fn base64url_decode(input: &str) -> Result<Vec<u8>, AuthError> {
    fn char_value(c: u8) -> Result<u8, AuthError> {
        match c {
            b'A'..=b'Z' => Ok(c - b'A'),
            b'a'..=b'z' => Ok(c - b'a' + 26),
            b'0'..=b'9' => Ok(c - b'0' + 52),
            b'-' => Ok(62),
            b'_' => Ok(63),
            _ => Err(AuthError::InvalidToken(
                "invalid base64url character".into(),
            )),
        }
    }

    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity((bytes.len() * 3) / 4);
    let chunks = bytes.chunks(4);

    for chunk in chunks {
        let vals: Vec<u8> = chunk
            .iter()
            .map(|&b| char_value(b))
            .collect::<Result<_, _>>()?;

        if vals.len() >= 2 {
            out.push((vals[0] << 2) | (vals[1] >> 4));
        }
        if vals.len() >= 3 {
            out.push((vals[1] << 4) | (vals[2] >> 2));
        }
        if vals.len() >= 4 {
            out.push((vals[2] << 6) | vals[3]);
        }
    }

    Ok(out)
}

// ─── HMAC-SHA256 (RFC 2104) ─────────────────────────────────────────────────

/// Compute HMAC-SHA256 of `message` using `key`.
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let block_size = 64;
    let mut key_padded = vec![0u8; block_size];

    if key.len() > block_size {
        let mut hasher = Sha256::new();
        hasher.update(key);
        let hashed = hasher.finalize();
        key_padded[..32].copy_from_slice(&hashed);
    } else {
        key_padded[..key.len()].copy_from_slice(key);
    }

    let mut ipad = vec![0x36u8; block_size];
    let mut opad = vec![0x5cu8; block_size];

    for i in 0..block_size {
        ipad[i] ^= key_padded[i];
        opad[i] ^= key_padded[i];
    }

    let mut inner_hasher = Sha256::new();
    inner_hasher.update(&ipad);
    inner_hasher.update(message);
    let inner_hash = inner_hasher.finalize();

    let mut outer_hasher = Sha256::new();
    outer_hasher.update(&opad);
    outer_hasher.update(inner_hash);
    let result = outer_hasher.finalize();

    let mut output = [0u8; 32];
    output.copy_from_slice(&result);
    output
}

/// Constant-time byte comparison to prevent timing attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ─── Error types ─────────────────────────────────────────────────────────────

/// Authentication errors.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The provided token or key is invalid.
    #[error("Invalid token: {0}")]
    InvalidToken(String),

    /// The token has expired.
    #[error("Token expired")]
    TokenExpired,

    /// No credentials were provided.
    #[error("Missing credentials")]
    MissingCredentials,

    /// The credentials are valid but insufficient for the requested operation.
    #[error("Insufficient permissions: {0}")]
    InsufficientPermissions(String),

    /// Internal error during auth processing.
    #[error("Auth internal error: {0}")]
    Internal(String),
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let status = match &self {
            AuthError::InvalidToken(_)
            | AuthError::TokenExpired
            | AuthError::MissingCredentials => StatusCode::UNAUTHORIZED,
            AuthError::InsufficientPermissions(_) => StatusCode::FORBIDDEN,
            AuthError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let body = serde_json::json!({
            "error": "auth_error",
            "message": self.to_string(),
        });

        (status, body.to_string()).into_response()
    }
}

// ─── Configuration types ─────────────────────────────────────────────────────

/// Authentication mode for the gateway.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuthMode {
    /// No authentication required.
    #[default]
    None,
    /// API key in `X-Api-Key` header.
    ApiKey,
    /// JWT Bearer token in `Authorization` header.
    Jwt,
    /// OAuth2 with external provider.
    OAuth2,
    /// Accepts either API key or JWT.
    Combined,
}

/// Top-level authentication configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Which authentication mode to use.
    pub mode: AuthMode,
    /// HMAC secret for JWT signing (required for `Jwt` and `Combined` modes).
    pub jwt_secret: Option<String>,
    /// Expected `iss` claim in JWTs.
    pub jwt_issuer: Option<String>,
    /// Expected `aud` claim in JWTs.
    pub jwt_audience: Option<String>,
    /// Token lifetime in seconds (default: 3600).
    pub token_expiry_secs: u64,
    /// Registered API keys.
    pub api_keys: Vec<ApiKeyConfig>,
    /// OAuth2 provider configurations.
    pub oauth2_providers: Vec<OAuth2ProviderConfig>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: AuthMode::None,
            jwt_secret: None,
            jwt_issuer: None,
            jwt_audience: None,
            token_expiry_secs: 3600,
            api_keys: Vec::new(),
            oauth2_providers: Vec::new(),
        }
    }
}

/// Configuration for a registered API key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyConfig {
    /// SHA-256 hex hash of the key — never store plaintext.
    pub key_hash: String,
    /// Human-readable name for this key.
    pub name: String,
    /// Permissions granted to this key.
    pub permissions: Vec<String>,
    /// Optional rate limit in requests per minute.
    pub rate_limit: Option<u32>,
    /// Optional expiration timestamp.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Configuration for an OAuth2 provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2ProviderConfig {
    /// Provider name (e.g., "github", "google", "custom").
    pub name: String,
    /// OAuth2 client ID.
    pub client_id: String,
    /// Authorization endpoint URL.
    pub auth_url: String,
    /// Token exchange endpoint URL.
    pub token_url: String,
    /// Requested scopes.
    pub scopes: Vec<String>,
}

// ─── JWT claims ──────────────────────────────────────────────────────────────

/// JWT claims payload for Argentor authentication tokens.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JwtClaims {
    /// Subject — typically a user ID.
    pub sub: String,
    /// Expiration time (seconds since UNIX epoch).
    pub exp: u64,
    /// Issued-at time (seconds since UNIX epoch).
    pub iat: u64,
    /// Issuer.
    pub iss: String,
    /// Audience.
    pub aud: String,
    /// Permissions granted to this token.
    pub permissions: Vec<String>,
    /// Agent roles this user can access.
    pub agent_roles: Vec<String>,
}

/// JWT header (always `{"alg":"HS256","typ":"JWT"}`).
#[derive(Debug, Serialize, Deserialize)]
struct JwtHeader {
    alg: String,
    typ: String,
}

impl Default for JwtHeader {
    fn default() -> Self {
        Self {
            alg: "HS256".into(),
            typ: "JWT".into(),
        }
    }
}

// ─── AuthService ─────────────────────────────────────────────────────────────

/// Central authentication service.
///
/// Handles JWT generation/validation and API key verification.
#[derive(Clone)]
pub struct AuthService {
    config: AuthConfig,
}

impl AuthService {
    /// Create a new `AuthService` from the given configuration.
    pub fn new(config: AuthConfig) -> Self {
        Self { config }
    }

    /// Return a reference to the underlying config.
    pub fn config(&self) -> &AuthConfig {
        &self.config
    }

    // ── JWT operations ──────────────────────────────────────────────────

    /// Generate a signed JWT for the given user.
    ///
    /// # Errors
    ///
    /// Returns `AuthError::Internal` if no JWT secret is configured or
    /// serialization fails.
    pub fn generate_jwt(
        &self,
        user_id: &str,
        permissions: Vec<String>,
        agent_roles: Vec<String>,
    ) -> Result<String, AuthError> {
        let secret = self
            .config
            .jwt_secret
            .as_deref()
            .ok_or_else(|| AuthError::Internal("JWT secret not configured".into()))?;

        let now = Utc::now().timestamp() as u64;

        let claims = JwtClaims {
            sub: user_id.to_string(),
            exp: now + self.config.token_expiry_secs,
            iat: now,
            iss: self
                .config
                .jwt_issuer
                .clone()
                .unwrap_or_else(|| "argentor".into()),
            aud: self
                .config
                .jwt_audience
                .clone()
                .unwrap_or_else(|| "argentor-gateway".into()),
            permissions,
            agent_roles,
        };

        Self::encode_jwt(&claims, secret)
    }

    /// Encode a `JwtClaims` into a signed JWT string.
    fn encode_jwt(claims: &JwtClaims, secret: &str) -> Result<String, AuthError> {
        let header = JwtHeader::default();
        let header_json = serde_json::to_vec(&header)
            .map_err(|e| AuthError::Internal(format!("header serialization failed: {e}")))?;
        let payload_json = serde_json::to_vec(claims)
            .map_err(|e| AuthError::Internal(format!("claims serialization failed: {e}")))?;

        let header_b64 = base64url_encode(&header_json);
        let payload_b64 = base64url_encode(&payload_json);
        let signing_input = format!("{header_b64}.{payload_b64}");

        let signature = hmac_sha256(secret.as_bytes(), signing_input.as_bytes());
        let sig_b64 = base64url_encode(&signature);

        Ok(format!("{signing_input}.{sig_b64}"))
    }

    /// Validate a JWT string and return the decoded claims.
    ///
    /// Verifies the HMAC-SHA256 signature, checks expiration, and optionally
    /// validates issuer and audience claims against the configuration.
    ///
    /// # Errors
    ///
    /// Returns `AuthError::InvalidToken` for malformed or tampered tokens,
    /// `AuthError::TokenExpired` for expired tokens.
    pub fn validate_jwt(&self, token: &str) -> Result<JwtClaims, AuthError> {
        let secret = self
            .config
            .jwt_secret
            .as_deref()
            .ok_or_else(|| AuthError::Internal("JWT secret not configured".into()))?;

        let claims = Self::decode_jwt(token, secret)?;

        // Check expiration
        if Self::is_expired(&claims) {
            return Err(AuthError::TokenExpired);
        }

        // Validate issuer if configured
        if let Some(expected_iss) = &self.config.jwt_issuer {
            if claims.iss != *expected_iss {
                return Err(AuthError::InvalidToken(format!(
                    "issuer mismatch: expected '{expected_iss}', got '{}'",
                    claims.iss
                )));
            }
        }

        // Validate audience if configured
        if let Some(expected_aud) = &self.config.jwt_audience {
            if claims.aud != *expected_aud {
                return Err(AuthError::InvalidToken(format!(
                    "audience mismatch: expected '{expected_aud}', got '{}'",
                    claims.aud
                )));
            }
        }

        Ok(claims)
    }

    /// Decode and verify a JWT string, returning the claims.
    fn decode_jwt(token: &str, secret: &str) -> Result<JwtClaims, AuthError> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(AuthError::InvalidToken(
                "JWT must have 3 dot-separated parts".into(),
            ));
        }

        let signing_input = format!("{}.{}", parts[0], parts[1]);
        let provided_sig = base64url_decode(parts[2])
            .map_err(|_| AuthError::InvalidToken("invalid signature encoding".into()))?;

        // Verify signature
        let expected_sig = hmac_sha256(secret.as_bytes(), signing_input.as_bytes());
        if !constant_time_eq(&provided_sig, &expected_sig) {
            return Err(AuthError::InvalidToken(
                "signature verification failed".into(),
            ));
        }

        // Decode payload
        let payload_bytes = base64url_decode(parts[1])
            .map_err(|_| AuthError::InvalidToken("invalid payload encoding".into()))?;

        let claims: JwtClaims = serde_json::from_slice(&payload_bytes)
            .map_err(|e| AuthError::InvalidToken(format!("invalid claims JSON: {e}")))?;

        Ok(claims)
    }

    /// Check whether the given claims have expired.
    pub fn is_expired(claims: &JwtClaims) -> bool {
        let now = Utc::now().timestamp() as u64;
        claims.exp <= now
    }

    // ── API key operations ──────────────────────────────────────────────

    /// Validate an API key against the configured keys.
    ///
    /// Returns the matching `ApiKeyConfig` if the key is valid and not expired.
    ///
    /// # Errors
    ///
    /// Returns `AuthError::InvalidToken` if no matching key is found, or
    /// `AuthError::TokenExpired` if the key has expired.
    pub fn validate_api_key(&self, key: &str) -> Result<ApiKeyConfig, AuthError> {
        let hash = Self::hash_api_key(key);

        let config = self
            .config
            .api_keys
            .iter()
            .find(|k| constant_time_eq(k.key_hash.as_bytes(), hash.as_bytes()))
            .ok_or_else(|| AuthError::InvalidToken("unknown API key".into()))?;

        // Check expiration
        if let Some(expires_at) = config.expires_at {
            if Utc::now() >= expires_at {
                return Err(AuthError::TokenExpired);
            }
        }

        Ok(config.clone())
    }

    /// Compute the SHA-256 hex hash of an API key.
    pub fn hash_api_key(key: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        hasher.update(b"argentor-api-key-v1");
        hex::encode(hasher.finalize())
    }

    /// Generate a new API key pair: `(plaintext_key, sha256_hash)`.
    ///
    /// The plaintext key is a 32-byte random value encoded as hex (64 chars),
    /// prefixed with `agtr_` for easy identification.
    pub fn generate_api_key() -> (String, String) {
        let mut buf = [0u8; 32];
        getrandom::getrandom(&mut buf).unwrap_or_else(|_| {
            // Fallback: timestamp-based (NOT for production)
            use std::time::SystemTime;
            let seed = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(42) as u64;
            for (i, byte) in buf.iter_mut().enumerate() {
                *byte = ((seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(i as u64))
                    >> 33) as u8;
            }
        });

        let plaintext = format!("agtr_{}", hex::encode(buf));
        let hash = Self::hash_api_key(&plaintext);
        (plaintext, hash)
    }

    // ── Credential extraction ───────────────────────────────────────────

    /// Extract and validate credentials from request headers.
    ///
    /// The authentication mode determines which credential types are accepted:
    /// - `None` — always succeeds with no claims.
    /// - `ApiKey` — requires `X-Api-Key` header.
    /// - `Jwt` — requires `Authorization: Bearer <token>` header.
    /// - `OAuth2` — same as `Jwt` (tokens are issued after OAuth2 flow).
    /// - `Combined` — accepts either `X-Api-Key` or `Authorization: Bearer`.
    pub fn authenticate(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<AuthenticatedUser>, AuthError> {
        match &self.config.mode {
            AuthMode::None => Ok(None),
            AuthMode::ApiKey => {
                let key = extract_api_key(headers)?;
                let key_config = self.validate_api_key(&key)?;
                Ok(Some(AuthenticatedUser::from_api_key(&key_config)))
            }
            AuthMode::Jwt | AuthMode::OAuth2 => {
                let token = extract_bearer_token(headers)?;
                let claims = self.validate_jwt(&token)?;
                Ok(Some(AuthenticatedUser::from_jwt(claims)))
            }
            AuthMode::Combined => {
                // Try JWT first, then API key
                if let Some(token) = try_extract_bearer_token(headers) {
                    let claims = self.validate_jwt(&token)?;
                    return Ok(Some(AuthenticatedUser::from_jwt(claims)));
                }
                if let Some(key) = try_extract_api_key(headers) {
                    let key_config = self.validate_api_key(&key)?;
                    return Ok(Some(AuthenticatedUser::from_api_key(&key_config)));
                }
                Err(AuthError::MissingCredentials)
            }
        }
    }
}

// ─── Authenticated user (injected into request extensions) ───────────────────

/// Represents an authenticated user, injected into request extensions by the
/// auth middleware.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticatedUser {
    /// User identifier (from JWT `sub` or API key name).
    pub user_id: String,
    /// Granted permissions.
    pub permissions: Vec<String>,
    /// Agent roles accessible to this user.
    pub agent_roles: Vec<String>,
    /// How the user authenticated.
    pub auth_method: String,
}

impl AuthenticatedUser {
    /// Build from validated JWT claims.
    fn from_jwt(claims: JwtClaims) -> Self {
        Self {
            user_id: claims.sub,
            permissions: claims.permissions,
            agent_roles: claims.agent_roles,
            auth_method: "jwt".into(),
        }
    }

    /// Build from a validated API key config.
    fn from_api_key(config: &ApiKeyConfig) -> Self {
        Self {
            user_id: config.name.clone(),
            permissions: config.permissions.clone(),
            agent_roles: Vec::new(),
            auth_method: "api_key".into(),
        }
    }

    /// Check whether this user has the given permission.
    pub fn has_permission(&self, permission: &str) -> bool {
        self.permissions.iter().any(|p| p == permission || p == "*")
    }

    /// Check whether this user can access the given agent role.
    pub fn has_agent_role(&self, role: &str) -> bool {
        self.agent_roles.iter().any(|r| r == role || r == "*")
    }
}

// ─── Header extraction helpers ───────────────────────────────────────────────

/// Extract the Bearer token from the `Authorization` header, or error.
fn extract_bearer_token(headers: &HeaderMap) -> Result<String, AuthError> {
    try_extract_bearer_token(headers).ok_or(AuthError::MissingCredentials)
}

/// Try to extract the Bearer token; returns `None` if not present.
fn try_extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(std::string::ToString::to_string)
}

/// Extract the API key from the `X-Api-Key` header, or error.
fn extract_api_key(headers: &HeaderMap) -> Result<String, AuthError> {
    try_extract_api_key(headers).ok_or(AuthError::MissingCredentials)
}

/// Try to extract the API key; returns `None` if not present.
fn try_extract_api_key(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(std::string::ToString::to_string)
}

// ─── Axum middleware ─────────────────────────────────────────────────────────

/// Shared state for the auth middleware layer.
#[derive(Clone)]
pub struct AuthMiddlewareState {
    /// The authentication service.
    pub auth_service: Arc<AuthService>,
}

/// Axum middleware that authenticates incoming requests.
///
/// On success, injects [`AuthenticatedUser`] into request extensions so
/// downstream handlers can access it via `Extension<AuthenticatedUser>`.
///
/// On failure, returns `401 Unauthorized` or `403 Forbidden`.
pub async fn auth_middleware(
    State(state): State<Arc<AuthMiddlewareState>>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Response {
    match state.auth_service.authenticate(&headers) {
        Ok(Some(user)) => {
            request.extensions_mut().insert(user);
            next.run(request).await
        }
        Ok(None) => {
            // AuthMode::None — no user injected, pass through
            next.run(request).await
        }
        Err(e) => {
            warn!(error = %e, "Authentication failed");
            e.into_response()
        }
    }
}

/// Helper to create a `require_permission` middleware closure.
///
/// Use this to protect specific routes that need a particular permission.
///
/// # Example
///
/// ```rust,ignore
/// use argentor_gateway::auth::{require_permission, AuthMiddlewareState};
///
/// let protected = Router::new()
///     .route("/admin", get(admin_handler))
///     .layer(axum::middleware::from_fn(require_permission("admin:write")));
/// ```
pub fn check_permission(user: &AuthenticatedUser, permission: &str) -> Result<(), AuthError> {
    if user.has_permission(permission) {
        Ok(())
    } else {
        Err(AuthError::InsufficientPermissions(format!(
            "requires '{permission}'"
        )))
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::Duration;

    /// Helper: build an `AuthService` configured for JWT mode.
    fn jwt_service() -> AuthService {
        AuthService::new(AuthConfig {
            mode: AuthMode::Jwt,
            jwt_secret: Some("test-secret-key-for-unit-tests".into()),
            jwt_issuer: Some("argentor-test".into()),
            jwt_audience: Some("argentor-gateway".into()),
            token_expiry_secs: 3600,
            api_keys: Vec::new(),
            oauth2_providers: Vec::new(),
        })
    }

    /// Helper: build an `AuthService` configured for API key mode with one key.
    fn api_key_service(plaintext_key: &str) -> AuthService {
        let hash = AuthService::hash_api_key(plaintext_key);
        AuthService::new(AuthConfig {
            mode: AuthMode::ApiKey,
            jwt_secret: None,
            jwt_issuer: None,
            jwt_audience: None,
            token_expiry_secs: 3600,
            api_keys: vec![ApiKeyConfig {
                key_hash: hash,
                name: "test-key".into(),
                permissions: vec!["read".into(), "write".into()],
                rate_limit: Some(100),
                expires_at: None,
            }],
            oauth2_providers: Vec::new(),
        })
    }

    // ── JWT tests ────────────────────────────────────────────────────────

    #[test]
    fn jwt_generate_and_validate_roundtrip() {
        let service = jwt_service();
        let token = service
            .generate_jwt("user-42", vec!["read".into()], vec!["worker".into()])
            .unwrap();

        let claims = service.validate_jwt(&token).unwrap();
        assert_eq!(claims.sub, "user-42");
        assert_eq!(claims.permissions, vec!["read"]);
        assert_eq!(claims.agent_roles, vec!["worker"]);
        assert_eq!(claims.iss, "argentor-test");
        assert_eq!(claims.aud, "argentor-gateway");
    }

    #[test]
    fn jwt_expiration_detection() {
        // Build claims that expired 10 seconds ago.
        let now = Utc::now().timestamp() as u64;
        let expired_claims = JwtClaims {
            sub: "user-1".into(),
            exp: now - 10,
            iat: now - 3610,
            iss: "argentor-test".into(),
            aud: "argentor-gateway".into(),
            permissions: vec![],
            agent_roles: vec![],
        };
        assert!(AuthService::is_expired(&expired_claims));

        // Build claims that expire in the future.
        let valid_claims = JwtClaims {
            sub: "user-1".into(),
            exp: now + 3600,
            iat: now,
            iss: "argentor-test".into(),
            aud: "argentor-gateway".into(),
            permissions: vec![],
            agent_roles: vec![],
        };
        assert!(!AuthService::is_expired(&valid_claims));
    }

    #[test]
    fn jwt_rejects_expired_token() {
        let service = jwt_service();
        let secret = service.config.jwt_secret.as_deref().unwrap();
        let now = Utc::now().timestamp() as u64;

        let expired_claims = JwtClaims {
            sub: "user-1".into(),
            exp: now - 1, // already expired
            iat: now - 3601,
            iss: "argentor-test".into(),
            aud: "argentor-gateway".into(),
            permissions: vec![],
            agent_roles: vec![],
        };

        let token = AuthService::encode_jwt(&expired_claims, secret).unwrap();
        let err = service.validate_jwt(&token).unwrap_err();
        assert!(matches!(err, AuthError::TokenExpired));
    }

    #[test]
    fn jwt_rejects_invalid_signature() {
        let service = jwt_service();
        let token = service.generate_jwt("user-1", vec![], vec![]).unwrap();

        // Tamper with the signature by replacing the last segment.
        let mut parts: Vec<&str> = token.split('.').collect();
        parts[2] = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let tampered = parts.join(".");

        let err = service.validate_jwt(&tampered).unwrap_err();
        assert!(matches!(err, AuthError::InvalidToken(_)));
    }

    #[test]
    fn jwt_rejects_tampered_payload() {
        let service = jwt_service();
        let token = service
            .generate_jwt("user-1", vec!["read".into()], vec![])
            .unwrap();

        // Tamper with the payload (middle segment).
        let parts: Vec<&str> = token.split('.').collect();
        // Modify the payload to a different user
        let fake_claims = JwtClaims {
            sub: "admin".into(),
            exp: (Utc::now().timestamp() as u64) + 9999,
            iat: Utc::now().timestamp() as u64,
            iss: "argentor-test".into(),
            aud: "argentor-gateway".into(),
            permissions: vec!["*".into()],
            agent_roles: vec!["*".into()],
        };
        let fake_payload = base64url_encode(&serde_json::to_vec(&fake_claims).unwrap());

        let tampered = format!("{}.{}.{}", parts[0], fake_payload, parts[2]);
        let err = service.validate_jwt(&tampered).unwrap_err();
        assert!(matches!(err, AuthError::InvalidToken(_)));
    }

    #[test]
    fn jwt_rejects_wrong_secret() {
        let service1 = jwt_service();
        let token = service1.generate_jwt("user-1", vec![], vec![]).unwrap();

        // Validate with a different secret
        let service2 = AuthService::new(AuthConfig {
            mode: AuthMode::Jwt,
            jwt_secret: Some("different-secret".into()),
            jwt_issuer: Some("argentor-test".into()),
            jwt_audience: Some("argentor-gateway".into()),
            token_expiry_secs: 3600,
            api_keys: Vec::new(),
            oauth2_providers: Vec::new(),
        });

        let err = service2.validate_jwt(&token).unwrap_err();
        assert!(matches!(err, AuthError::InvalidToken(_)));
    }

    #[test]
    fn jwt_rejects_malformed_token() {
        let service = jwt_service();

        // Too few parts
        let err = service.validate_jwt("only.two").unwrap_err();
        assert!(matches!(err, AuthError::InvalidToken(_)));

        // Too many parts
        let err = service.validate_jwt("a.b.c.d").unwrap_err();
        assert!(matches!(err, AuthError::InvalidToken(_)));

        // Empty
        let err = service.validate_jwt("").unwrap_err();
        assert!(matches!(err, AuthError::InvalidToken(_)));
    }

    #[test]
    fn jwt_validates_issuer_mismatch() {
        let service = AuthService::new(AuthConfig {
            mode: AuthMode::Jwt,
            jwt_secret: Some("secret".into()),
            jwt_issuer: Some("expected-issuer".into()),
            jwt_audience: None,
            token_expiry_secs: 3600,
            api_keys: Vec::new(),
            oauth2_providers: Vec::new(),
        });

        let secret = "secret";
        let now = Utc::now().timestamp() as u64;
        let claims = JwtClaims {
            sub: "user".into(),
            exp: now + 3600,
            iat: now,
            iss: "wrong-issuer".into(),
            aud: "any".into(),
            permissions: vec![],
            agent_roles: vec![],
        };

        let token = AuthService::encode_jwt(&claims, secret).unwrap();
        let err = service.validate_jwt(&token).unwrap_err();
        assert!(matches!(err, AuthError::InvalidToken(_)));
    }

    // ── API key tests ────────────────────────────────────────────────────

    #[test]
    fn api_key_hash_and_validate() {
        let plaintext = "agtr_test_key_123";
        let service = api_key_service(plaintext);

        let config = service.validate_api_key(plaintext).unwrap();
        assert_eq!(config.name, "test-key");
        assert_eq!(config.permissions, vec!["read", "write"]);
    }

    #[test]
    fn api_key_rejects_unknown_key() {
        let service = api_key_service("agtr_real_key");

        let err = service.validate_api_key("agtr_wrong_key").unwrap_err();
        assert!(matches!(err, AuthError::InvalidToken(_)));
    }

    #[test]
    fn api_key_generation_produces_valid_pair() {
        let (plaintext, hash) = AuthService::generate_api_key();

        // Key starts with "agtr_"
        assert!(plaintext.starts_with("agtr_"));
        // Key is 5 prefix chars + 64 hex chars = 69 chars
        assert_eq!(plaintext.len(), 69);
        // Hash matches
        assert_eq!(AuthService::hash_api_key(&plaintext), hash);
        // Hash is a 64-char hex string
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn api_key_hash_is_deterministic() {
        let key = "agtr_deterministic_test";
        let h1 = AuthService::hash_api_key(key);
        let h2 = AuthService::hash_api_key(key);
        assert_eq!(h1, h2);
    }

    #[test]
    fn api_key_rejects_expired_key() {
        let plaintext = "agtr_expired_key";
        let hash = AuthService::hash_api_key(plaintext);
        let expired = Utc::now() - Duration::hours(1);

        let service = AuthService::new(AuthConfig {
            mode: AuthMode::ApiKey,
            jwt_secret: None,
            jwt_issuer: None,
            jwt_audience: None,
            token_expiry_secs: 3600,
            api_keys: vec![ApiKeyConfig {
                key_hash: hash,
                name: "expired-key".into(),
                permissions: vec!["read".into()],
                rate_limit: None,
                expires_at: Some(expired),
            }],
            oauth2_providers: Vec::new(),
        });

        let err = service.validate_api_key(plaintext).unwrap_err();
        assert!(matches!(err, AuthError::TokenExpired));
    }

    // ── AuthMode::None tests ─────────────────────────────────────────────

    #[test]
    fn auth_mode_none_allows_all() {
        let service = AuthService::new(AuthConfig::default());
        assert_eq!(service.config().mode, AuthMode::None);

        let headers = HeaderMap::new();
        let result = service.authenticate(&headers).unwrap();
        assert!(result.is_none()); // No user injected, but allowed
    }

    // ── AuthMode::Combined tests ─────────────────────────────────────────

    #[test]
    fn combined_mode_accepts_jwt() {
        let service = AuthService::new(AuthConfig {
            mode: AuthMode::Combined,
            jwt_secret: Some("combined-secret".into()),
            jwt_issuer: None,
            jwt_audience: None,
            token_expiry_secs: 3600,
            api_keys: Vec::new(),
            oauth2_providers: Vec::new(),
        });

        let token = service
            .generate_jwt(
                "jwt-user",
                vec!["admin".into()],
                vec!["orchestrator".into()],
            )
            .unwrap();

        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse().unwrap());

        let user = service.authenticate(&headers).unwrap().unwrap();
        assert_eq!(user.user_id, "jwt-user");
        assert_eq!(user.auth_method, "jwt");
        assert_eq!(user.permissions, vec!["admin"]);
    }

    #[test]
    fn combined_mode_accepts_api_key() {
        let plaintext = "agtr_combined_key";
        let hash = AuthService::hash_api_key(plaintext);

        let service = AuthService::new(AuthConfig {
            mode: AuthMode::Combined,
            jwt_secret: Some("combined-secret".into()),
            jwt_issuer: None,
            jwt_audience: None,
            token_expiry_secs: 3600,
            api_keys: vec![ApiKeyConfig {
                key_hash: hash,
                name: "combo-key".into(),
                permissions: vec!["read".into()],
                rate_limit: None,
                expires_at: None,
            }],
            oauth2_providers: Vec::new(),
        });

        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", plaintext.parse().unwrap());

        let user = service.authenticate(&headers).unwrap().unwrap();
        assert_eq!(user.user_id, "combo-key");
        assert_eq!(user.auth_method, "api_key");
    }

    #[test]
    fn combined_mode_rejects_no_credentials() {
        let service = AuthService::new(AuthConfig {
            mode: AuthMode::Combined,
            jwt_secret: Some("secret".into()),
            jwt_issuer: None,
            jwt_audience: None,
            token_expiry_secs: 3600,
            api_keys: Vec::new(),
            oauth2_providers: Vec::new(),
        });

        let headers = HeaderMap::new();
        let err = service.authenticate(&headers).unwrap_err();
        assert!(matches!(err, AuthError::MissingCredentials));
    }

    // ── Permission checking tests ────────────────────────────────────────

    #[test]
    fn permission_check_exact_match() {
        let user = AuthenticatedUser {
            user_id: "u1".into(),
            permissions: vec!["read".into(), "write".into()],
            agent_roles: vec!["worker".into()],
            auth_method: "jwt".into(),
        };

        assert!(user.has_permission("read"));
        assert!(user.has_permission("write"));
        assert!(!user.has_permission("admin"));
    }

    #[test]
    fn permission_check_wildcard() {
        let user = AuthenticatedUser {
            user_id: "admin".into(),
            permissions: vec!["*".into()],
            agent_roles: vec!["*".into()],
            auth_method: "jwt".into(),
        };

        assert!(user.has_permission("anything"));
        assert!(user.has_agent_role("any-role"));
    }

    #[test]
    fn check_permission_helper_works() {
        let user = AuthenticatedUser {
            user_id: "u1".into(),
            permissions: vec!["read".into()],
            agent_roles: vec![],
            auth_method: "api_key".into(),
        };

        assert!(check_permission(&user, "read").is_ok());
        assert!(check_permission(&user, "write").is_err());
    }

    // ── Base64url roundtrip tests ────────────────────────────────────────

    #[test]
    fn base64url_encode_decode_roundtrip() {
        let inputs: Vec<&[u8]> = vec![
            b"",
            b"f",
            b"fo",
            b"foo",
            b"foob",
            b"fooba",
            b"foobar",
            b"hello world! this is a test of base64url encoding.",
            &[0, 1, 2, 255, 254, 253],
        ];

        for input in inputs {
            let encoded = base64url_encode(input);
            let decoded = base64url_decode(&encoded).unwrap();
            assert_eq!(decoded, input, "roundtrip failed for input {input:?}");
        }
    }

    // ── OAuth2 config test ───────────────────────────────────────────────

    #[test]
    fn oauth2_provider_config_serialization() {
        let provider = OAuth2ProviderConfig {
            name: "github".into(),
            client_id: "gh-client-id".into(),
            auth_url: "https://github.com/login/oauth/authorize".into(),
            token_url: "https://github.com/login/oauth/access_token".into(),
            scopes: vec!["read:user".into(), "repo".into()],
        };

        let json = serde_json::to_string(&provider).unwrap();
        let deserialized: OAuth2ProviderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "github");
        assert_eq!(deserialized.scopes.len(), 2);
    }

    // ── AuthConfig default test ──────────────────────────────────────────

    #[test]
    fn auth_config_default_is_none_mode() {
        let config = AuthConfig::default();
        assert_eq!(config.mode, AuthMode::None);
        assert!(config.jwt_secret.is_none());
        assert!(config.api_keys.is_empty());
        assert!(config.oauth2_providers.is_empty());
        assert_eq!(config.token_expiry_secs, 3600);
    }
}
