use worker::*;

use crate::config_store;
use crate::types::BackendConfig;

fn build_auth_header(config: &BackendConfig) -> String {
    config
        .redis_password
        .as_deref()
        .unwrap_or("")
        .to_string()
}

fn redis_url(config: &BackendConfig, path: &str) -> String {
    format!(
        "{}/{}",
        config.endpoint.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

/// GET {endpoint}/get/{key} — get a value from Redis
pub async fn proxy_get(
    _req: Request,
    env: &Env,
    config_id: &str,
    key: &str,
) -> Result<Response> {
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Redis config not found".into()))?;

    let url = redis_url(&config, &format!("get/{}", key));
    let token = build_auth_header(&config);

    let mut headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {}", token))?;

    let mut init = RequestInit::new();
    init.method = Method::Get;
    init.headers = headers;

    let fetch_req = Request::new_with_init(&url, &init)?;
    let mut resp = Fetch::Request(fetch_req).send().await?;

    if resp.status_code() >= 200 && resp.status_code() < 300 {
        let body = resp.json::<serde_json::Value>().await?;
        Response::from_json(&serde_json::json!({
            "success": true,
            "data": { "key": key, "value": body.get("result") }
        }))
    } else {
        let status = resp.status_code();
        let body = resp.text().await.unwrap_or_default();
        Ok(Response::from_json(&serde_json::json!({
            "success": false,
            "error": format!("Redis GET failed ({}): {}", status, body),
            "code": "REDIS_GET_FAILED"
        }))?.with_status(status))
    }
}

/// POST {endpoint}/set/{key} — set a value in Redis
pub async fn proxy_set(
    mut req: Request,
    env: &Env,
    config_id: &str,
    key: &str,
) -> Result<Response> {
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Redis config not found".into()))?;

    let body = req.bytes().await?;
    let url = redis_url(&config, &format!("set/{}", key));
    let token = build_auth_header(&config);

    let mut headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {}", token))?;
    headers.set("Content-Type", "application/octet-stream")?;

    let mut init = RequestInit::new();
    init.method = Method::Post;
    init.headers = headers;
    init.body = Some(body.into());

    let fetch_req = Request::new_with_init(&url, &init)?;
    let mut resp = Fetch::Request(fetch_req).send().await?;

    if resp.status_code() >= 200 && resp.status_code() < 300 {
        let result = resp.json::<serde_json::Value>().await?;
        Response::from_json(&serde_json::json!({
            "success": true,
            "data": { "key": key, "result": result.get("result") }
        }))
    } else {
        let status = resp.status_code();
        let body = resp.text().await.unwrap_or_default();
        Ok(Response::from_json(&serde_json::json!({
            "success": false,
            "error": format!("Redis SET failed ({}): {}", status, body),
            "code": "REDIS_SET_FAILED"
        }))?.with_status(status))
    }
}

/// POST {endpoint}/del/{key} — delete a key from Redis
pub async fn proxy_delete(
    env: &Env,
    config_id: &str,
    key: &str,
) -> Result<Response> {
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Redis config not found".into()))?;

    let url = redis_url(&config, &format!("del/{}", key));
    let token = build_auth_header(&config);

    let mut headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {}", token))?;

    let mut init = RequestInit::new();
    init.method = Method::Post;
    init.headers = headers;

    let fetch_req = Request::new_with_init(&url, &init)?;
    let mut resp = Fetch::Request(fetch_req).send().await?;

    let status = resp.status_code();
    if status >= 200 && status < 300 || status == 204 {
        Response::from_json(&serde_json::json!({
            "success": true,
            "data": { "deleted": true, "key": key }
        }))
    } else {
        let body = resp.text().await.unwrap_or_default();
        Ok(Response::from_json(&serde_json::json!({
            "success": false,
            "error": format!("Redis DEL failed ({}): {}", status, body),
            "code": "REDIS_DEL_FAILED"
        }))?.with_status(status))
    }
}

/// Test connection to Redis endpoint via Upstash REST API
pub async fn test_connection(env: &Env, config_id: &str) -> Result<Response> {
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Redis config not found".into()))?;

    let url = redis_url(&config, "ping");
    let token = build_auth_header(&config);

    let mut headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {}", token))?;

    let mut init = RequestInit::new();
    init.method = Method::Get;
    init.headers = headers;

    let fetch_req = Request::new_with_init(&url, &init)?;

    match Fetch::Request(fetch_req).send().await {
        Ok(mut resp) => {
            let status = resp.status_code();
            let reachable = status == 200;
            let body = resp.text().await.unwrap_or_default();
            Response::from_json(&serde_json::json!({
                "success": true,
                "data": {
                    "ok": reachable,
                    "message": if reachable {
                        format!("Redis endpoint reachable: {}", body)
                    } else {
                        format!("Redis ping failed (HTTP {}): {}", status, body)
                    }
                }
            }))
        }
        Err(e) => Response::from_json(&serde_json::json!({
            "success": true,
            "data": {
                "ok": false,
                "message": format!("Connection failed: {}", e)
            }
        })),
    }
}
