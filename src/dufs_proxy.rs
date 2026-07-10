use base64::Engine;
use worker::*;

use crate::config_store;
use crate::types::BackendConfig;

fn build_auth_header_value(config: &BackendConfig) -> Option<String> {
    match (&config.username, &config.password) {
        (Some(u), Some(p)) => {
            let creds = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", u, p));
            Some(format!("Basic {}", creds))
        }
        _ => None,
    }
}

/// PUT / {path} — upload a file to Dufs
pub async fn proxy_upload(
    mut req: Request,
    env: &Env,
    config_id: &str,
) -> Result<Response> {
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Dufs config not found".into()))?;

    let form = req.form_data().await?;

    let file_entry = form.get("file").ok_or_else(|| {
        worker::Error::RustError("Missing 'file' field in form data".into())
    })?;

    let file_bytes = match file_entry {
        FormEntry::File(f) => f.bytes().await?,
        FormEntry::Field(_) => {
            return Err(worker::Error::RustError(
                "'file' field must be a file".into(),
            ));
        }
    };

    let key = match form.get("key").ok_or_else(|| {
        worker::Error::RustError("Missing 'key' field in form data".into())
    })? {
        FormEntry::Field(t) => t,
        FormEntry::File(_) => {
            return Err(worker::Error::RustError("'key' field must be text".into()));
        }
    };

    let url = format!(
        "{}/{}",
        config.endpoint.trim_end_matches('/'),
        key.trim_start_matches('/')
    );

    let mut headers = Headers::new();
    headers.set("Content-Type", "application/octet-stream")?;
    if let Some(auth) = build_auth_header_value(&config) {
        headers.set("Authorization", &auth)?;
    }

    let mut init = RequestInit::new();
    init.method = Method::Put;
    init.headers = headers;
    init.body = Some(file_bytes.into());

    let fetch_req = Request::new_with_init(&url, &init)?;
    let mut resp = Fetch::Request(fetch_req).send().await?;

    if resp.status_code() >= 200 && resp.status_code() < 300 {
        Response::from_json(&serde_json::json!({
            "success": true,
            "data": { "key": key, "url": url }
        }))
    } else {
        let status = resp.status_code();
        let body = resp.text().await.unwrap_or_default();
        Ok(Response::from_json(&serde_json::json!({
            "success": false,
            "error": format!("Dufs upload failed ({}): {}", status, body),
            "code": "DUFS_UPLOAD_FAILED"
        }))?.with_status(status))
    }
}

/// GET / {path} — download a file from Dufs
pub async fn proxy_download(
    _req: Request,
    env: &Env,
    config_id: &str,
    key: &str,
) -> Result<Response> {
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Dufs config not found".into()))?;

    let url = format!(
        "{}/{}",
        config.endpoint.trim_end_matches('/'),
        key.trim_start_matches('/')
    );

    let mut headers = Headers::new();
    if let Some(auth) = build_auth_header_value(&config) {
        headers.set("Authorization", &auth)?;
    }

    let mut init = RequestInit::new();
    init.method = Method::Get;
    init.headers = headers;

    let fetch_req = Request::new_with_init(&url, &init)?;
    let mut resp = Fetch::Request(fetch_req).send().await?;

    if resp.status_code() >= 200 && resp.status_code() < 300 {
        let body = resp.bytes().await?;
        let content_type = resp
            .headers()
            .get("Content-Type")
            .ok()
            .flatten()
            .unwrap_or_else(|| "application/octet-stream".to_string());

        let mut response_headers = Headers::new();
        response_headers.set("Content-Type", &content_type)?;
        response_headers.set("Cache-Control", "public, max-age=31536000")?;

        Ok(Response::from_bytes(body)?.with_headers(response_headers))
    } else {
        let status = resp.status_code();
        let body = resp.text().await.unwrap_or_default();
        Ok(Response::from_json(&serde_json::json!({
            "success": false,
            "error": format!("Dufs download failed ({}): {}", status, body),
            "code": "DUFS_DOWNLOAD_FAILED"
        }))?.with_status(status))
    }
}

/// DELETE / {path} — delete a file from Dufs
pub async fn proxy_delete(
    env: &Env,
    config_id: &str,
    key: &str,
) -> Result<Response> {
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Dufs config not found".into()))?;

    let url = format!(
        "{}/{}",
        config.endpoint.trim_end_matches('/'),
        key.trim_start_matches('/')
    );

    let mut headers = Headers::new();
    if let Some(auth) = build_auth_header_value(&config) {
        headers.set("Authorization", &auth)?;
    }

    let mut init = RequestInit::new();
    init.method = Method::Delete;
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
            "error": format!("Dufs delete failed ({}): {}", status, body),
            "code": "DUFS_DELETE_FAILED"
        }))?.with_status(status))
    }
}

/// Test connection to Dufs endpoint
pub async fn test_connection(env: &Env, config_id: &str) -> Result<Response> {
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Dufs config not found".into()))?;

    let url = config.endpoint.trim_end_matches('/').to_string();

    let mut headers = Headers::new();
    if let Some(auth) = build_auth_header_value(&config) {
        headers.set("Authorization", &auth)?;
    }

    let mut init = RequestInit::new();
    init.method = Method::Head;
    init.headers = headers;

    let fetch_req = Request::new_with_init(&url, &init)?;

    match Fetch::Request(fetch_req).send().await {
        Ok(resp) => {
            let status = resp.status_code();
            let reachable = status == 200 || status == 401 || status == 403 || status == 404;
            Response::from_json(&serde_json::json!({
                "success": true,
                "data": {
                    "ok": reachable,
                    "message": if reachable {
                        format!("Dufs endpoint reachable (HTTP {})", status)
                    } else {
                        format!("Unexpected response (HTTP {})", status)
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
