use worker::*;

use crate::github_oauth;

pub fn extract_token(req: &Request) -> Option<String> {
    req.headers()
        .get("Authorization")
        .ok()
        .flatten()
        .and_then(|v| v.strip_prefix("Bearer ").map(|t| t.to_string()))
}

/// Verify via GitHub OAuth session cookie (primary method)
pub fn verify_admin_session(req: &Request, env: &Env) -> Result<String> {
    github_oauth::verify_session(req, env)
}

/// Verify admin access: try GitHub OAuth session first, fall back to token
pub fn verify_admin(req: &Request, env: &Env) -> Result<()> {
    // Try GitHub OAuth session first
    if github_oauth::verify_session(req, env).is_ok() {
        return Ok(());
    }

    // Fallback to legacy ADMIN_TOKEN
    let expected = env.secret("ADMIN_TOKEN")?.to_string();

    if expected.is_empty() {
        return Err(worker::Error::RustError(
            "ADMIN_TOKEN secret is not set. Run: wrangler secret put ADMIN_TOKEN".into(),
        ));
    }

    let token = extract_token(req).ok_or_else(|| {
        worker::Error::RustError("Missing Authorization: Bearer <token> header".into())
    })?;

    if token != expected {
        return Err(worker::Error::RustError("Invalid admin token".into()));
    }

    Ok(())
}
