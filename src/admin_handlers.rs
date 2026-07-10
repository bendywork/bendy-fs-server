use std::time::{SystemTime, UNIX_EPOCH};
use worker::*;

use crate::auth;
use crate::config_store;
use crate::dufs_proxy;
use crate::redis_proxy;
use crate::s3_proxy;
use crate::types::{
    ApiResponse, BackendConfig, BackendType, ConfigListData, ConfigListItem,
    CreateBackendConfigInput, PresignedDownloadInput, PresignedUrlInput,
    UpdateBackendConfigInput,
};
use crate::s3_signer;

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn backend_not_supported_err(expected: &str, actual: &BackendType) -> worker::Error {
    worker::Error::RustError(format!(
        "Expected {} backend, got {}",
        expected,
        actual.as_str()
    ))
}

/// POST /api/admin/configs/:id/test — test connection by backend type
pub async fn handle_test(req: Request, env: &Env, config_id: &str) -> Result<Response> {
    auth::verify_admin(&req, env)?;
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Config not found".into()))?;

    match config.backend_type {
        BackendType::S3 => s3_proxy::test_connection(env, config_id).await,
        BackendType::Dufs => dufs_proxy::test_connection(env, config_id).await,
        BackendType::Redis => redis_proxy::test_connection(env, config_id).await,
    }
}

/// GET /api/admin/configs — list all configs (without secret keys)
pub async fn handle_list(req: Request, env: &Env) -> Result<Response> {
    auth::verify_admin(&req, env)?;
    let configs = config_store::list_configs(env).await?;
    let items: Vec<ConfigListItem> = configs.iter().map(ConfigListItem::from).collect();
    Response::from_json(&ApiResponse::ok(ConfigListData { configs: items }))
}

/// GET /api/admin/configs/:id — get single config (with secret key)
pub async fn handle_get(req: Request, env: &Env, config_id: &str) -> Result<Response> {
    auth::verify_admin(&req, env)?;
    match config_store::get_config(env, config_id).await? {
        Some(cfg) => Response::from_json(&ApiResponse::ok(cfg)),
        None => Ok(Response::from_json(&ApiResponse::<()>::err(
            "Config not found",
            "NOT_FOUND",
        ))?
        .with_status(404)),
    }
}

/// POST /api/admin/configs — create config
pub async fn handle_create(mut req: Request, env: &Env) -> Result<Response> {
    auth::verify_admin(&req, env)?;
    let body = req.text().await?;
    let input: CreateBackendConfigInput = serde_json::from_str(&body)
        .map_err(|e| worker::Error::RustError(format!("Invalid JSON: {}", e)))?;

    let config = BackendConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name: input.name,
        backend_type: input.backend_type,
        endpoint: input.endpoint,
        region: input.region,
        bucket: input.bucket,
        access_key_id: input.access_key_id,
        secret_access_key: input.secret_access_key,
        force_path_style: input.force_path_style,
        username: input.username,
        password: input.password,
        redis_password: input.redis_password,
        db_index: input.db_index,
        created_at: now_ts(),
    };

    let created = config_store::create_config(env, config).await?;
    Ok(Response::from_json(&ApiResponse::ok(created))?.with_status(201))
}

/// PUT /api/admin/configs/:id — update config
pub async fn handle_update(mut req: Request, env: &Env, config_id: &str) -> Result<Response> {
    auth::verify_admin(&req, env)?;
    let body = req.text().await?;
    let input: UpdateBackendConfigInput = serde_json::from_str(&body)
        .map_err(|e| worker::Error::RustError(format!("Invalid JSON: {}", e)))?;

    let existing = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Config not found".into()))?;

    let updated = BackendConfig {
        id: existing.id,
        name: input.name.unwrap_or(existing.name),
        backend_type: input.backend_type.unwrap_or(existing.backend_type),
        endpoint: input.endpoint.unwrap_or(existing.endpoint),
        region: input.region.unwrap_or(existing.region),
        bucket: input.bucket.unwrap_or(existing.bucket),
        access_key_id: input.access_key_id.unwrap_or(existing.access_key_id),
        secret_access_key: input.secret_access_key.unwrap_or(existing.secret_access_key),
        force_path_style: input.force_path_style.unwrap_or(existing.force_path_style),
        username: input.username.or(existing.username),
        password: input.password.or(existing.password),
        redis_password: input.redis_password.or(existing.redis_password),
        db_index: input.db_index.or(existing.db_index),
        created_at: existing.created_at,
    };

    match config_store::update_config(env, config_id, updated).await? {
        Some(cfg) => Response::from_json(&ApiResponse::ok(cfg)),
        None => Ok(Response::from_json(&ApiResponse::<()>::err(
            "Config not found",
            "NOT_FOUND",
        ))?
        .with_status(404)),
    }
}

/// DELETE /api/admin/configs/:id — delete config
pub async fn handle_delete(req: Request, env: &Env, config_id: &str) -> Result<Response> {
    auth::verify_admin(&req, env)?;
    match config_store::delete_config(env, config_id).await? {
        true => Response::from_json(&ApiResponse::ok(serde_json::json!({"deleted": true}))),
        false => Ok(Response::from_json(&ApiResponse::<()>::err(
            "Config not found",
            "NOT_FOUND",
        ))?
        .with_status(404)),
    }
}

/// POST /api/fs/:config_id/presign-upload — S3-only
pub async fn handle_presign_upload(mut req: Request, env: &Env, config_id: &str) -> Result<Response> {
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Config not found".into()))?;

    if config.backend_type != BackendType::S3 {
        return Ok(Response::from_json(&ApiResponse::<()>::err(
            "Presigned URLs are only available for S3 backends",
            "UNSUPPORTED_BACKEND",
        ))?.with_status(400));
    }

    let body = req.text().await?;
    let input: PresignedUrlInput = serde_json::from_str(&body)
        .map_err(|e| worker::Error::RustError(format!("Invalid JSON: {}", e)))?;

    let expires = input.expires_in_seconds.unwrap_or(3600);
    let ct = input.content_type.as_deref().unwrap_or("application/octet-stream");

    let url = s3_signer::presign_put(&config, &input.key, ct, expires);

    Response::from_json(&ApiResponse::ok(serde_json::json!({
        "url": url,
        "key": input.key,
        "expires_in": expires
    })))
}

/// POST /api/fs/:config_id/presign-download — S3-only
pub async fn handle_presign_download(mut req: Request, env: &Env, config_id: &str) -> Result<Response> {
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Config not found".into()))?;

    if config.backend_type != BackendType::S3 {
        return Ok(Response::from_json(&ApiResponse::<()>::err(
            "Presigned URLs are only available for S3 backends",
            "UNSUPPORTED_BACKEND",
        ))?.with_status(400));
    }

    let body = req.text().await?;
    let input: PresignedDownloadInput = serde_json::from_str(&body)
        .map_err(|e| worker::Error::RustError(format!("Invalid JSON: {}", e)))?;

    let expires = input.expires_in_seconds.unwrap_or(3600);
    let url = s3_signer::presign_get(
        &config,
        &input.key,
        expires,
        input.filename.as_deref(),
    );

    Response::from_json(&ApiResponse::ok(serde_json::json!({
        "url": url,
        "key": input.key,
        "expires_in": expires
    })))
}

/// POST /api/fs/:config_id/upload — proxy upload (dispatch by backend_type)
pub async fn handle_proxy_upload(req: Request, env: &Env, config_id: &str) -> Result<Response> {
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Config not found".into()))?;

    match config.backend_type {
        BackendType::S3 => s3_proxy::proxy_upload(req, env, config_id).await,
        BackendType::Dufs => dufs_proxy::proxy_upload(req, env, config_id).await,
        BackendType::Redis => {
            Err(backend_not_supported_err("S3 or Dufs", &config.backend_type).into())
        }
    }
}

/// GET /api/fs/:config_id/download/:key+ — proxy download (dispatch by backend_type)
pub async fn handle_proxy_download(
    req: Request,
    env: &Env,
    config_id: &str,
    key: &str,
) -> Result<Response> {
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Config not found".into()))?;

    match config.backend_type {
        BackendType::S3 => s3_proxy::proxy_download(req, env, config_id, key).await,
        BackendType::Dufs => dufs_proxy::proxy_download(req, env, config_id, key).await,
        BackendType::Redis => {
            redis_proxy::proxy_get(req, env, config_id, key).await
        }
    }
}

/// DELETE /api/fs/:config_id/delete/:key+ — proxy delete (dispatch by backend_type)
pub async fn handle_proxy_delete(
    req: Request,
    env: &Env,
    config_id: &str,
    key: &str,
) -> Result<Response> {
    auth::verify_admin(&req, env)?;
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Config not found".into()))?;

    match config.backend_type {
        BackendType::S3 => s3_proxy::proxy_delete(env, config_id, key).await,
        BackendType::Dufs => dufs_proxy::proxy_delete(env, config_id, key).await,
        BackendType::Redis => {
            redis_proxy::proxy_delete(env, config_id, key).await
        }
    }
}

/// POST /api/fs/:config_id/set — Redis SET (Redis-only)
pub async fn handle_redis_set(
    mut req: Request,
    env: &Env,
    config_id: &str,
) -> Result<Response> {
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Config not found".into()))?;

    if config.backend_type != BackendType::Redis {
        return Ok(Response::from_json(&ApiResponse::<()>::err(
            "SET is only available for Redis backends",
            "UNSUPPORTED_BACKEND",
        ))?.with_status(400));
    }

    let body = req.text().await?;
    let input: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| worker::Error::RustError(format!("Invalid JSON: {}", e)))?;

    let key = input["key"].as_str().ok_or_else(|| {
        worker::Error::RustError("Missing 'key' field".into())
    })?;
    let value = input["value"].as_str().unwrap_or("");

    let url = format!(
        "{}/set/{}",
        config.endpoint.trim_end_matches('/'),
        key
    );
    let token = config.redis_password.as_deref().unwrap_or("");

    let mut headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {}", token))?;
    headers.set("Content-Type", "application/octet-stream")?;

    let mut init = RequestInit::new();
    init.method = Method::Post;
    init.headers = headers;
    init.body = Some(value.as_bytes().to_vec().into());

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
