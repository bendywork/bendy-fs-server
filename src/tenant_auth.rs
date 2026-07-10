use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use worker::*;

type HmacSha256 = Hmac<Sha256>;

const TENANT_TOKEN_TTL_SECS: u64 = 3600; // 1 hour

// ── JWT primitives (same pattern as github_oauth.rs) ──

fn now_secs() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn base64url_encode(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD
        .encode(data)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

fn base64url_decode(input: &str) -> std::result::Result<Vec<u8>, base64::DecodeError> {
    let mut s = input.replace('-', "+").replace('_', "/");
    while s.len() % 4 != 0 {
        s.push('=');
    }
    base64::engine::general_purpose::STANDARD.decode(&s)
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

#[derive(Debug, Serialize, Deserialize)]
struct TenantClaims {
    tenant_id: String,
    name: String,
    iat: u64,
    exp: u64,
}

fn sign_tenant_jwt(claims: &TenantClaims, secret: &str) -> String {
    let header = base64url_encode(b"{\"alg\":\"HS256\",\"typ\":\"JWT\"}");
    let payload = base64url_encode(serde_json::to_string(claims).unwrap().as_bytes());
    let signing_input = format!("{}.{}", header, payload);
    let signature = base64url_encode(&hmac_sha256(secret.as_bytes(), signing_input.as_bytes()));
    format!("{}.{}.{}", header, payload, signature)
}

fn verify_tenant_jwt(token: &str, secret: &str) -> Result<TenantClaims> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(worker::Error::RustError("Invalid JWT format".into()));
    }

    let signing_input = format!("{}.{}", parts[0], parts[1]);
    let expected_sig =
        base64url_encode(&hmac_sha256(secret.as_bytes(), signing_input.as_bytes()));

    if expected_sig != parts[2] {
        return Err(worker::Error::RustError("Invalid JWT signature".into()));
    }

    let payload_bytes = base64url_decode(parts[1])
        .map_err(|e| worker::Error::RustError(format!("Invalid JWT payload: {}", e)))?;
    let claims: TenantClaims = serde_json::from_slice(&payload_bytes)
        .map_err(|e| worker::Error::RustError(format!("Invalid JWT claims: {}", e)))?;

    let now = now_secs();
    if claims.exp < now {
        return Err(worker::Error::RustError("Token expired".into()));
    }

    Ok(claims)
}

fn get_jwt_secret(env: &Env) -> Result<String> {
    env.secret("JWT_SECRET")
        .map(|s| s.to_string())
        .or_else(|_| env.var("JWT_SECRET").map(|v| v.to_string()))
        .map_err(|_| worker::Error::RustError("JWT_SECRET not configured".into()))
}

// ── Public API ──

pub fn generate_tenant_token(env: &Env, tenant_id: &str, name: &str) -> Result<String> {
    let secret = get_jwt_secret(env)?;
    let now = now_secs();
    let claims = TenantClaims {
        tenant_id: tenant_id.to_string(),
        name: name.to_string(),
        iat: now,
        exp: now + TENANT_TOKEN_TTL_SECS,
    };
    Ok(sign_tenant_jwt(&claims, &secret))
}

pub struct VerifiedTenant {
    pub tenant_id: String,
    pub name: String,
}

/// Extract and verify tenant JWT from Authorization: Bearer <token>
pub fn verify_tenant_token(req: &Request, env: &Env) -> Result<VerifiedTenant> {
    let token = req
        .headers()
        .get("Authorization")
        .ok()
        .flatten()
        .and_then(|v| v.strip_prefix("Bearer ").map(|t| t.to_string()))
        .ok_or_else(|| worker::Error::RustError("Missing Authorization: Bearer <token> header".into()))?;

    let secret = get_jwt_secret(env)?;
    let claims = verify_tenant_jwt(&token, &secret)?;

    Ok(VerifiedTenant {
        tenant_id: claims.tenant_id,
        name: claims.name,
    })
}

/// POST /api/t/auth — tenant login: exchange api_key + api_secret for JWT
pub async fn handle_tenant_login(mut req: Request, env: &Env) -> Result<Response> {
    let body = req.text().await?;
    let input: crate::types::TenantAuthInput = serde_json::from_str(&body)
        .map_err(|e| worker::Error::RustError(format!("Invalid JSON: {}", e)))?;

    let tenant = crate::db::verify_tenant_credentials(&env, &input.api_key, &input.api_secret)
        .await?;

    let access_token = generate_tenant_token(&env, &tenant.id, &tenant.name)?;

    Response::from_json(&crate::types::TenantAuthOutput {
        access_token,
        expires_in: TENANT_TOKEN_TTL_SECS,
    })
}
