use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use worker::*;

use crate::admin_handlers;

type HmacSha256 = Hmac<Sha256>;

const GITHUB_AUTHORIZE_URL: &str = "https://github.com/login/oauth/authorize";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USER_API: &str = "https://api.github.com/user";
const SESSION_COOKIE: &str = "bendy_fs_session";
const SESSION_TTL_SECS: u64 = 24 * 3600; // 24 hours

#[derive(Debug, Serialize, Deserialize)]
struct SessionClaims {
    username: String,
    iat: u64,
    exp: u64,
}

fn now_secs() -> u64 {
    let now = web_time::SystemTime::now()
        .duration_since(web_time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_secs()
}

fn base64url_encode(data: &[u8]) -> String {
    // Use base64 engine::general_purpose::URL_SAFE_NO_PAD equivalent
    base64::engine::general_purpose::STANDARD
        .encode(data)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

fn base64url_decode(input: &str) -> std::result::Result<Vec<u8>, base64::DecodeError> {
    let mut s = input.replace('-', "+").replace('_', "/");
    // Add padding
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

fn sign_jwt(claims: &SessionClaims, secret: &str) -> String {
    let header = base64url_encode(b"{\"alg\":\"HS256\",\"typ\":\"JWT\"}");
    let payload = base64url_encode(serde_json::to_string(claims).unwrap().as_bytes());
    let signing_input = format!("{}.{}", header, payload);
    let signature = base64url_encode(&hmac_sha256(secret.as_bytes(), signing_input.as_bytes()));
    format!("{}.{}.{}", header, payload, signature)
}

fn verify_jwt(token: &str, secret: &str) -> Result<SessionClaims> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(worker::Error::RustError("Invalid JWT format".into()));
    }

    let signing_input = format!("{}.{}", parts[0], parts[1]);
    let expected_sig = base64url_encode(&hmac_sha256(secret.as_bytes(), signing_input.as_bytes()));

    if expected_sig != parts[2] {
        return Err(worker::Error::RustError("Invalid JWT signature".into()));
    }

    let payload_bytes = base64url_decode(parts[1])
        .map_err(|e| worker::Error::RustError(format!("Invalid JWT payload: {}", e)))?;
    let claims: SessionClaims = serde_json::from_slice(&payload_bytes)
        .map_err(|e| worker::Error::RustError(format!("Invalid JWT claims: {}", e)))?;

    let now = now_secs();
    if claims.exp < now {
        return Err(worker::Error::RustError("Session expired".into()));
    }

    Ok(claims)
}

pub fn get_admin_usernames(env: &Env) -> Result<Vec<String>> {
    let raw = env
        .secret("ADMIN_GITHUB_USERNAMES")
        .map(|s| s.to_string())
        .or_else(|_| env.var("ADMIN_GITHUB_USERNAMES").map(|v| v.to_string()))
        .unwrap_or_default();

    Ok(raw
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect())
}

fn get_oauth_config(env: &Env) -> Result<(String, String, String)> {
    let get_val = |key: &str| -> Result<String> {
        env.secret(key)
            .map(|s| s.to_string())
            .or_else(|_| env.var(key).map(|v| v.to_string()))
            .map_err(|_| worker::Error::RustError(format!("Missing env: {}", key)))
    };

    Ok((
        get_val("GITHUB_CLIENT_ID")?,
        get_val("GITHUB_CLIENT_SECRET")?,
        get_val("GITHUB_REDIRECT_URI")?,
    ))
}

/// GET /api/auth/github/login — redirect to GitHub OAuth authorize page
pub async fn handle_github_login(env: &Env) -> Result<Response> {
    let (client_id, _, redirect_uri) = get_oauth_config(env)?;
    let auth_url = format!(
        "{}?client_id={}&redirect_uri={}&scope=read:user",
        GITHUB_AUTHORIZE_URL,
        url_encode(&client_id),
        url_encode(&redirect_uri),
    );

    let mut headers = Headers::new();
    headers.set("Location", &auth_url)?;
    Ok(Response::empty()?.with_status(302).with_headers(headers))
}

/// Helper: redirect to /admin with an error query param
fn redirect_admin_error(error: &str) -> Result<Response> {
    let mut headers = Headers::new();
    headers.set("Location", &format!("/admin?error={}", error))?;
    Ok(Response::empty()?.with_status(302).with_headers(headers))
}

/// GET /api/github/callback?code=... — exchange code, fetch user, verify admin, set cookie
pub async fn handle_github_callback(req: &Request, env: &Env) -> Result<Response> {
    let result = handle_github_callback_inner(req, env).await;
    match result {
        Ok(resp) => Ok(resp),
        Err(e) => {
            console_log!("OAuth callback error: {:?}", e);
            redirect_admin_error("oauth_failed")
        }
    }
}

async fn handle_github_callback_inner(req: &Request, env: &Env) -> Result<Response> {
    let url = req.url()?;
    let query: Vec<(String, String)> = url
        .query_pairs()
        .into_iter()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    let code = query
        .iter()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.clone())
        .ok_or_else(|| worker::Error::RustError("Missing 'code' parameter".into()))?;

    let (client_id, client_secret, redirect_uri) = get_oauth_config(env)?;

    // Exchange code for access token
    let token_body = format!(
        "client_id={}&client_secret={}&code={}&redirect_uri={}",
        url_encode(&client_id),
        url_encode(&client_secret),
        url_encode(&code),
        url_encode(&redirect_uri),
    );

    let mut headers = Headers::new();
    headers.set("Content-Type", "application/x-www-form-urlencoded")?;
    headers.set("Accept", "application/json")?;

    let mut init = RequestInit::new();
    init.method = Method::Post;
    init.headers = headers;
    init.body = Some(token_body.into_bytes().into());

    let token_req = Request::new_with_init(GITHUB_TOKEN_URL, &init)?;
    let mut token_resp = Fetch::Request(token_req).send().await?;

    if token_resp.status_code() >= 400 {
        return Err(worker::Error::RustError("GitHub token exchange failed".into()));
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: Option<String>,
        error: Option<String>,
        error_description: Option<String>,
    }

    let token_data: TokenResponse = token_resp.json().await.map_err(|e| {
        worker::Error::RustError(format!("Failed to parse token response: {}", e))
    })?;

    if let Some(err) = token_data.error {
        return Err(worker::Error::RustError(format!("GitHub OAuth error: {} - {:?}", err, token_data.error_description)));
    }

    let access_token = token_data
        .access_token
        .ok_or_else(|| worker::Error::RustError("No access_token in response".into()))?;

    // Fetch GitHub user info
    let mut user_headers = Headers::new();
    user_headers.set("Authorization", &format!("Bearer {}", access_token))?;
    user_headers.set("User-Agent", "bendy-fs-server")?;

    let mut user_init = RequestInit::new();
    user_init.method = Method::Get;
    user_init.headers = user_headers;

    let user_req = Request::new_with_init(GITHUB_USER_API, &user_init)?;
    let mut user_resp = Fetch::Request(user_req).send().await?;

    if user_resp.status_code() >= 400 {
        return Err(worker::Error::RustError("Failed to fetch GitHub user".into()));
    }

    #[derive(Deserialize)]
    struct GitHubUser {
        login: String,
    }

    let gh_user: GitHubUser = user_resp.json().await.map_err(|e| {
        worker::Error::RustError(format!("Failed to parse user response: {}", e))
    })?;

    // Check admin whitelist
    let admin_usernames = get_admin_usernames(env)?;
    let username_lower = gh_user.login.to_lowercase();

    if !admin_usernames.contains(&username_lower) {
        let _ = admin_handlers::audit_log(env, &username_lower, "login_failure", "Not in admin whitelist").await;
        return redirect_admin_error("unauthorized");
    }

    // Issue JWT session
    let jwt_secret = env
        .secret("JWT_SECRET")
        .map(|s| s.to_string())
        .or_else(|_| env.var("JWT_SECRET").map(|v| v.to_string()))
        .unwrap_or_default();

    let now = now_secs();
    let claims = SessionClaims {
        username: gh_user.login.clone(),
        iat: now,
        exp: now + SESSION_TTL_SECS,
    };
    let token = sign_jwt(&claims, &jwt_secret);

    // Set cookie and redirect to admin
    let cookie_value = format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        SESSION_COOKIE, token, SESSION_TTL_SECS
    );

    let _ = admin_handlers::audit_log(env, &gh_user.login, "login", "Admin logged in via GitHub OAuth").await;

    let mut resp_headers = Headers::new();
    resp_headers.set("Set-Cookie", &cookie_value)?;
    resp_headers.set("Location", "/admin")?;

    Ok(Response::empty()?
        .with_status(302)
        .with_headers(resp_headers))
}

/// GET /api/auth/session — return current session info
pub async fn handle_get_session(req: &Request, env: &Env) -> Result<Response> {
    match verify_session(req, env) {
        Ok(username) => Response::from_json(&serde_json::json!({
            "success": true,
            "data": { "username": username, "isAdmin": true }
        })),
        Err(_) => Ok(Response::from_json(&serde_json::json!({
            "success": false,
            "error": "No valid session",
            "code": "UNAUTHORIZED"
        }))?
        .with_status(401)),
    }
}

/// POST /api/auth/logout — clear session cookie
pub async fn handle_logout(req: &Request, env: &Env) -> Result<Response> {
    let cookie_value = format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
        SESSION_COOKIE
    );
    let body = br#"{"success":true,"v":"3"}"#;
    let mut resp = Response::from_bytes(body.to_vec()).unwrap_or_else(|_| Response::empty().unwrap());
    let _ = resp.headers_mut().set("Content-Type", "application/json");
    let _ = resp.headers_mut().set("Set-Cookie", &cookie_value);
    Ok(resp)
}

/// Extract JWT from cookie and verify
pub fn extract_session_cookie(req: &Request) -> Option<String> {
    let cookie_header = req.headers().get("Cookie").ok().flatten()?;
    for part in cookie_header.split(';') {
        let kv: Vec<&str> = part.trim().splitn(2, '=').collect();
        if kv.len() == 2 && kv[0].trim() == SESSION_COOKIE {
            return Some(kv[1].trim().to_string());
        }
    }
    None
}

/// Verify session and return username
pub fn verify_session(req: &Request, env: &Env) -> Result<String> {
    let token = extract_session_cookie(req).ok_or_else(|| {
        worker::Error::RustError("No session cookie".into())
    })?;

    let jwt_secret = env
        .secret("JWT_SECRET")
        .map(|s| s.to_string())
        .or_else(|_| env.var("JWT_SECRET").map(|v| v.to_string()))
        .unwrap_or_default();

    let claims = verify_jwt(&token, &jwt_secret)?;

    // Re-verify admin status
    let admin_usernames = get_admin_usernames(env)?;
    if !admin_usernames.contains(&claims.username.to_lowercase()) {
        return Err(worker::Error::RustError("Not an admin user".into()));
    }

    Ok(claims.username)
}

fn url_encode(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}
