use worker::*;

use crate::config_store;
use crate::s3_signer;

/// Proxy an upload: receive multipart FormData from client, forward to S3
pub async fn proxy_upload(
    mut req: Request,
    env: &Env,
    config_id: &str,
) -> Result<Response> {
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("S3 config not found".into()))?;

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
            return Err(worker::Error::RustError(
                "'key' field must be text".into(),
            ));
        }
    };

    let content_type = match form.get("content_type") {
        Some(FormEntry::Field(t)) => t,
        _ => "application/octet-stream".to_string(),
    };

    let auth_header = s3_signer::build_auth_header(
        &config,
        "PUT",
        &key,
        Some(&content_type),
        &file_bytes,
    );
    let (long_date, _) = s3_signer::now_utc();
    let payload_hash = s3_signer::sha256_hex(&file_bytes);

    let url = s3_signer::build_object_url(&config, &key);

    let mut headers = Headers::new();
    headers.set("Authorization", &auth_header)?;
    headers.set("Content-Type", &content_type)?;
    headers.set("x-amz-content-sha256", &payload_hash)?;
    headers.set("x-amz-date", &long_date)?;
    headers.set("Host", &s3_signer::extract_host(&config.endpoint))?;

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
            "error": format!("S3 upload failed ({}): {}", status, body),
            "code": "S3_UPLOAD_FAILED"
        }))?
        .with_status(status))
    }
}

/// Proxy a download: fetch from S3 and stream back to client
pub async fn proxy_download(
    _req: Request,
    env: &Env,
    config_id: &str,
    key: &str,
) -> Result<Response> {
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("S3 config not found".into()))?;

    let auth_header = s3_signer::build_auth_header(&config, "GET", key, None, &[]);

    let url = s3_signer::build_object_url(&config, key);
    let (long_date, _) = s3_signer::now_utc();
    let empty_hash = s3_signer::sha256_hex(&[]);

    let mut headers = Headers::new();
    headers.set("Authorization", &auth_header)?;
    headers.set("x-amz-content-sha256", &empty_hash)?;
    headers.set("x-amz-date", &long_date)?;
    headers.set("Host", &s3_signer::extract_host(&config.endpoint))?;

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
            "error": format!("S3 download failed ({}): {}", status, body),
            "code": "S3_DOWNLOAD_FAILED"
        }))?
        .with_status(status))
    }
}

/// Delete an object from S3
pub async fn proxy_delete(
    env: &Env,
    config_id: &str,
    key: &str,
) -> Result<Response> {
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("S3 config not found".into()))?;

    let auth_header = s3_signer::build_auth_header(&config, "DELETE", key, None, &[]);

    let url = s3_signer::build_object_url(&config, key);
    let (long_date, _) = s3_signer::now_utc();
    let empty_hash = s3_signer::sha256_hex(&[]);

    let mut headers = Headers::new();
    headers.set("Authorization", &auth_header)?;
    headers.set("x-amz-content-sha256", &empty_hash)?;
    headers.set("x-amz-date", &long_date)?;
    headers.set("Host", &s3_signer::extract_host(&config.endpoint))?;

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
            "error": format!("S3 delete failed ({}): {}", status, body),
            "code": "S3_DELETE_FAILED"
        }))?
        .with_status(status))
    }
}

/// Test connection to the configured S3 endpoint
pub async fn test_connection(env: &Env, config_id: &str) -> Result<Response> {
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("S3 config not found".into()))?;

    let auth_header = s3_signer::build_auth_header(&config, "HEAD", "", None, &[]);

    let url = format!(
        "{}/{}",
        config.endpoint.trim_end_matches('/'),
        config.bucket
    );
    let (long_date, _) = s3_signer::now_utc();
    let empty_hash = s3_signer::sha256_hex(&[]);

    let mut headers = Headers::new();
    headers.set("Authorization", &auth_header)?;
    headers.set("x-amz-content-sha256", &empty_hash)?;
    headers.set("x-amz-date", &long_date)?;
    headers.set("Host", &s3_signer::extract_host(&config.endpoint))?;

    let mut init = RequestInit::new();
    init.method = Method::Head;
    init.headers = headers;

    let fetch_req = Request::new_with_init(&url, &init)?;

    match Fetch::Request(fetch_req).send().await {
        Ok(resp) => {
            let status = resp.status_code();
            let reachable = status == 200 || status == 403 || status == 404;
            Response::from_json(&serde_json::json!({
                "success": true,
                "data": {
                    "ok": reachable,
                    "message": if reachable {
                        format!("S3 endpoint reachable (HTTP {})", status)
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
