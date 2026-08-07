use web_time::{SystemTime, UNIX_EPOCH};
use worker::*;

use crate::config_store;
use crate::db;
use crate::dufs_proxy;
use crate::redis_proxy;
use crate::s3_proxy;
use crate::s3_signer;
use crate::tenant_auth;
use crate::types::{BackendType, FileRecord, TenantPresignInput};

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Load tenant + its default backend config. Returns (tenant_id, config_id).
async fn resolve_tenant_backend(req: &Request, env: &Env) -> Result<(String, String)> {
    let vt = tenant_auth::verify_tenant_token(req, env)?;
    let tenant = db::get_tenant(env, &vt.tenant_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Tenant not found".into()))?;

    if tenant.is_active == 0 {
        return Err(worker::Error::RustError("Tenant is disabled".into()));
    }

    Ok((tenant.id, tenant.default_backend_config_id))
}

/// POST /api/t/upload — multipart form upload
pub async fn handle_tenant_upload(mut req: Request, env: &Env) -> Result<Response> {
    let (tenant_id, config_id) = resolve_tenant_backend(&req, env).await?;

    let config = config_store::get_config(env, &config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Backend config not found".into()))?;

    // Read FormData directly — we cannot delegate to proxy_upload because
    // the proxy would also try to read FormData and the body is one-shot.
    let form = req.form_data().await?;

    let key = match form.get("key").ok_or_else(|| {
        worker::Error::RustError("Missing 'key' field in form data".into())
    })? {
        FormEntry::Field(t) => t,
        FormEntry::File(_) => return Err(worker::Error::RustError("'key' must be text".into())),
    };

    let file_entry = form.get("file").ok_or_else(|| {
        worker::Error::RustError("Missing 'file' field in form data".into())
    })?;

    let (file_bytes, filename, mime_type) = match file_entry {
        FormEntry::File(f) => {
            let name = f.name().to_string();
            let ct = f.type_().to_string();
            let bytes = f.bytes().await?;
            (bytes, name, ct)
        }
        FormEntry::Field(_) => {
            return Err(worker::Error::RustError("'file' must be a file".into()));
        }
    };

    let size = file_bytes.len() as i64;

    // Check quota before uploading
    db::check_and_update_quota(env, &tenant_id, size).await?;

    // Perform direct upload (inline S3/Dufs proxy logic)
    let resp = match config.backend_type {
        BackendType::S3 => {
            let auth_header = s3_signer::build_auth_header(
                &config, "PUT", &key, Some(&mime_type), &file_bytes,
            );
            let (long_date, _) = s3_signer::now_utc();
            let payload_hash = s3_signer::sha256_hex(&file_bytes);
            let url = s3_signer::build_object_url(&config, &key);

            let mut hdrs = Headers::new();
            hdrs.set("Authorization", &auth_header)?;
            hdrs.set("Content-Type", &mime_type)?;
            hdrs.set("x-amz-content-sha256", &payload_hash)?;
            hdrs.set("x-amz-date", &long_date)?;
            hdrs.set("Host", &s3_signer::extract_host(&config.endpoint))?;

            let mut init = RequestInit::new();
            init.method = Method::Put;
            init.headers = hdrs;
            init.body = Some(file_bytes.into());

            let fetch_req = Request::new_with_init(&url, &init)?;
            Fetch::Request(fetch_req).send().await
        }
        BackendType::Dufs => {
            let url = format!(
                "{}/{}",
                config.endpoint.trim_end_matches('/'),
                key.trim_start_matches('/')
            );

            let mut hdrs = Headers::new();
            hdrs.set("Content-Type", "application/octet-stream")?;
            if let (Some(u), Some(p)) = (&config.username, &config.password) {
                let creds = base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    format!("{}:{}", u, p),
                );
                hdrs.set("Authorization", &format!("Basic {}", creds))?;
            }

            let mut init = RequestInit::new();
            init.method = Method::Put;
            init.headers = hdrs;
            init.body = Some(file_bytes.into());

            let fetch_req = Request::new_with_init(&url, &init)?;
            Fetch::Request(fetch_req).send().await
        }
        BackendType::Redis => {
            return Err(worker::Error::RustError("Upload not supported for Redis backends".into()));
        }
    }?;

    if resp.status_code() >= 200 && resp.status_code() < 300 {
        let preview_url = if config.backend_type == BackendType::S3 && mime_type.starts_with("image/") {
            Some(s3_signer::presign_get(&config, &key, 604800, None))
        } else {
            None
        };
        let record = FileRecord {
            id: uuid::Uuid::new_v4().to_string(),
            tenant_id: tenant_id.clone(),
            file_key: key.clone(),
            original_name: filename.clone(),
            mime_type: mime_type.clone(),
            size_bytes: size,
            backend_type: config.backend_type.as_str().to_string(),
            backend_config_id: config_id.clone(),
            preview_url,
            created_at: now_ts(),
        };
        db::create_file_record(env, &record).await?;
        db::increment_quota_after_upload(env, &tenant_id, size).await?;

        // Build response with file URL
        let tenant = db::get_tenant(env, &tenant_id).await?.unwrap();
        let public_url = if tenant.public_files == 1 {
            Some(format!("/files/{}/{}", tenant_id, key))
        } else {
            None
        };

        return Response::from_json(&serde_json::json!({
            "success": true,
            "data": {
                "key": key,
                "filename": filename,
                "mime_type": mime_type,
                "size_bytes": size,
                "public_url": public_url,
            }
        }));
    }

    Ok(resp)
}

/// POST /api/t/presign-upload — generate presigned upload URL (S3 only)
pub async fn handle_tenant_presign_upload(mut req: Request, env: &Env) -> Result<Response> {
    let (tenant_id, config_id) = resolve_tenant_backend(&req, env).await?;

    let config = config_store::get_config(env, &config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Backend config not found".into()))?;

    if config.backend_type != BackendType::S3 {
        return Ok(Response::from_json(&crate::types::ApiResponse::<()>::err(
            "Presigned URLs are only available for S3 backends",
            "UNSUPPORTED_BACKEND",
        ))?.with_status(400));
    }

    let body = req.text().await?;
    let input: TenantPresignInput = serde_json::from_str(&body)
        .map_err(|e| worker::Error::RustError(format!("Invalid JSON: {}", e)))?;

    // Check quota
    db::check_and_update_quota(env, &tenant_id, 0).await?;

    let expires = input.expires_in_seconds.unwrap_or(3600);
    let ct = input.content_type.as_deref().unwrap_or("application/octet-stream");

    let url = crate::s3_signer::presign_put(&config, &input.key, ct, expires);

    Response::from_json(&serde_json::json!({
        "success": true,
        "data": {
            "url": url,
            "key": input.key,
            "expires_in": expires
        }
    }))
}

/// POST /api/t/presign-download — generate presigned download URL (S3 only)
pub async fn handle_tenant_presign_download(mut req: Request, env: &Env) -> Result<Response> {
    let (_tenant_id, config_id) = resolve_tenant_backend(&req, env).await?;

    let config = config_store::get_config(env, &config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Backend config not found".into()))?;

    if config.backend_type != BackendType::S3 {
        return Ok(Response::from_json(&crate::types::ApiResponse::<()>::err(
            "Presigned URLs are only available for S3 backends",
            "UNSUPPORTED_BACKEND",
        ))?.with_status(400));
    }

    let body = req.text().await?;
    let input: TenantPresignInput = serde_json::from_str(&body)
        .map_err(|e| worker::Error::RustError(format!("Invalid JSON: {}", e)))?;

    let expires = input.expires_in_seconds.unwrap_or(3600);
    let url = crate::s3_signer::presign_get(&config, &input.key, expires, input.filename.as_deref());

    Response::from_json(&serde_json::json!({
        "success": true,
        "data": {
            "url": url,
            "key": input.key,
            "expires_in": expires
        }
    }))
}

/// GET /api/t/download/*key — proxy download through backend
pub async fn handle_tenant_download(req: Request, env: &Env, key: &str) -> Result<Response> {
    let (_tenant_id, config_id) = resolve_tenant_backend(&req, env).await?;

    let config = config_store::get_config(env, &config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Backend config not found".into()))?;

    match config.backend_type {
        BackendType::S3 => s3_proxy::proxy_download(req, env, &config_id, key).await,
        BackendType::Dufs => dufs_proxy::proxy_download(req, env, &config_id, key).await,
        BackendType::Redis => redis_proxy::proxy_get(req, env, &config_id, key).await,
    }
}

/// DELETE /api/t/delete/*key — proxy delete through backend + cleanup D1 record
pub async fn handle_tenant_delete(req: Request, env: &Env, key: &str) -> Result<Response> {
    let (tenant_id, config_id) = resolve_tenant_backend(&req, env).await?;

    let config = config_store::get_config(env, &config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Backend config not found".into()))?;

    let resp = match config.backend_type {
        BackendType::S3 => s3_proxy::proxy_delete(env, &config_id, key).await,
        BackendType::Dufs => dufs_proxy::proxy_delete(env, &config_id, key).await,
        BackendType::Redis => redis_proxy::proxy_delete(env, &config_id, key).await,
    }?;

    if resp.status_code() >= 200 && resp.status_code() < 300 || resp.status_code() == 204 {
        // Get file record to know size for quota decrement
        if let Ok(Some(record)) = db::get_file_record(env, &tenant_id, key).await {
            db::decrement_quota_after_delete(env, &tenant_id, record.size_bytes).await?;
        }
        db::delete_file_record(env, &tenant_id, key).await?;
    }

    Ok(resp)
}

/// GET /api/t/files — list file records for the tenant
pub async fn handle_tenant_list_files(req: Request, env: &Env) -> Result<Response> {
    let vt = tenant_auth::verify_tenant_token(&req, env)?;

    let url = req.url()?;
    let offset: i64 = url
        .query_pairs()
        .find(|(k, _)| k == "offset")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let limit: i64 = url
        .query_pairs()
        .find(|(k, _)| k == "limit")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(50);

    let records = db::list_file_records(env, &vt.tenant_id, offset, limit).await?;
    let total = db::count_file_records(env, &vt.tenant_id).await?;

    let items: Vec<crate::types::FileRecordData> = records.iter().map(|r| r.into()).collect();

    Response::from_json(&serde_json::json!({
        "success": true,
        "data": {
            "files": items,
            "total": total,
            "offset": offset,
            "limit": limit
        }
    }))
}

/// GET /files/:tenant_id/*key — public download (no auth, requires tenant.public_files=1)
pub async fn handle_public_download(req: Request, env: &Env, tenant_id: &str, key: &str) -> Result<Response> {
    let tenant = db::get_tenant(env, tenant_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Tenant not found".into()))?;

    if tenant.is_active == 0 || tenant.public_files == 0 {
        return Ok(Response::empty()?.with_status(404));
    }

    let config = config_store::get_config(env, &tenant.default_backend_config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Backend config not found".into()))?;

    match config.backend_type {
        BackendType::S3 => s3_proxy::proxy_download(req, env, &tenant.default_backend_config_id, key).await,
        BackendType::Dufs => dufs_proxy::proxy_download(req, env, &tenant.default_backend_config_id, key).await,
        BackendType::Redis => redis_proxy::proxy_get(req, env, &tenant.default_backend_config_id, key).await,
    }
}
pub async fn handle_tenant_oss_preview(mut req: Request, env: &Env) -> Result<Response> {
    let (tenant_id, config_id) = resolve_tenant_backend(&req, env).await?;

    let config = config_store::get_config(env, &config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Backend config not found".into()))?;

    let body = req.text().await?;
    let input: crate::types::TenantOssPreviewInput = serde_json::from_str(&body)
        .map_err(|e| worker::Error::RustError(format!("Invalid JSON: {}", e)))?;

    let rpc_url = if config.oss_preview_rpc_url.is_empty() {
        let fallback = config.endpoint.trim_end_matches('/').to_string();
        format!("{}/api/oss/rpc/json-rpc", fallback)
    } else {
        config.oss_preview_rpc_url.clone()
    };

    let rpc_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "preview",
        "params": {
            "key": input.key,
            "fileName": input.file_name,
            "mimeType": input.mime_type,
            "sizeBytes": input.size_bytes
        }
    });

    let mut headers = Headers::new();
    headers.set("Content-Type", "application/json")?;
    if !config.oss_preview_cookie.is_empty() {
        headers.set("Cookie", &config.oss_preview_cookie)?;
    }

    let mut init = RequestInit::new();
    init.method = Method::Post;
    init.headers = headers;
    init.body = Some(serde_json::to_string(&rpc_request).unwrap().into_bytes().into());

    let fetch_req = Request::new_with_init(&rpc_url, &init)?;
    let mut resp = Fetch::Request(fetch_req).send().await?;

    if resp.status_code() >= 200 && resp.status_code() < 300 {
        let preview_url: serde_json::Value = resp.json().await?;
        let preview_result = preview_url.get("result").cloned();

        // Update existing file record with preview URL
        if let Some(ref result) = preview_result {
            if let Some(url) = result.get("url").and_then(|v| v.as_str()) {
                if let Ok(Some(existing)) = db::get_file_record(env, &tenant_id, &input.key).await {
                    let mut updated = existing;
                    updated.preview_url = Some(url.to_string());
                    // Re-create the record with updated preview URL
                    db::delete_file_record(env, &tenant_id, &input.key).await?;
                    db::create_file_record(env, &updated).await?;
                }
            }
        }

        Response::from_json(&serde_json::json!({
            "success": true,
            "data": preview_result
        }))
    } else {
        let status = resp.status_code();
        let err_body = resp.text().await.unwrap_or_default();
        Ok(Response::from_json(&serde_json::json!({
            "success": false,
            "error": format!("OSS preview failed ({}): {}", status, err_body),
            "code": "OSS_PREVIEW_FAILED"
        }))?.with_status(status))
    }
}
