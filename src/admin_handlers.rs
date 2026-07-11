use web_time::{SystemTime, UNIX_EPOCH};
use worker::*;

use crate::auth;
use crate::config_store;
use crate::db;
use crate::dufs_proxy;
use crate::redis_proxy;
use crate::s3_proxy;
use crate::types::{
    ApiResponse, AuditLog, BackendConfig, BackendType, ConfigListItem,
    CreateBackendConfigInput, CreateTenantInput, HealthProbeResult,
    PresignedDownloadInput, PresignedUrlInput,
    UpdateBackendConfigInput, UpdateTenantInput,
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

/// Checks admin auth; returns JSON 401 instead of propagating to 500 text error
macro_rules! require_admin {
    ($req:expr, $env:expr) => {
        if let Err(_) = auth::verify_admin($req, $env) {
            return Ok(Response::from_json(&ApiResponse::<()>::err(
                "Authentication required",
                "UNAUTHORIZED",
            ))?
            .with_status(401));
        }
    };
}

/// Checks admin auth and captures username; returns JSON 401 on failure
macro_rules! require_admin_uname {
    ($req:expr, $env:expr, $uname:ident) => {
        let $uname = match auth::verify_admin_with_username($req, $env) {
            Ok(u) => u,
            Err(_) => {
                return Ok(Response::from_json(&ApiResponse::<()>::err(
                    "Authentication required",
                    "UNAUTHORIZED",
                ))?
                .with_status(401));
            }
        };
    };
}

pub async fn audit_log(env: &Env, username: &str, action: &str, detail: &str) -> Result<()> {
    let log = AuditLog {
        id: uuid::Uuid::new_v4().to_string(),
        action: action.to_string(),
        username: username.to_string(),
        detail: detail.to_string(),
        created_at: now_ts(),
    };
    db::insert_audit_log(env, &log).await
}

/// POST /api/admin/configs/:id/test — test connection by backend type
pub async fn handle_test(req: Request, env: &Env, config_id: &str) -> Result<Response> {
    require_admin_uname!(&req, env, username);
    let config = config_store::get_config(env, config_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Config not found".into()))?;

    let result = match config.backend_type {
        BackendType::S3 => s3_proxy::test_connection(env, config_id).await,
        BackendType::Dufs => dufs_proxy::test_connection(env, config_id).await,
        BackendType::Redis => redis_proxy::test_connection(env, config_id).await,
    };

    let _ = audit_log(env, &username, "test", &format!("Tested config '{}'", config.name)).await;
    result
}

/// GET /api/admin/configs — list configs with pagination (without secret keys)
pub async fn handle_list(req: Request, env: &Env) -> Result<Response> {
    require_admin!(&req, env);
    let url = req.url()?;
    let offset: usize = url
        .query_pairs()
        .find(|(k, _)| k == "offset")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let limit: usize = url
        .query_pairs()
        .find(|(k, _)| k == "limit")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(20);
    let configs = config_store::list_configs(env, offset, limit).await?;
    let total = config_store::count_configs(env).await?;
    let items: Vec<ConfigListItem> = configs.iter().map(ConfigListItem::from).collect();
    Response::from_json(&ApiResponse::ok(serde_json::json!({
        "configs": items,
        "total": total,
        "offset": offset,
        "limit": limit
    })))
}

/// GET /api/admin/configs/:id — get single config (with secret key)
pub async fn handle_get(req: Request, env: &Env, config_id: &str) -> Result<Response> {
    require_admin!(&req, env);
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
    require_admin_uname!(&req, env, username);
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
        oss_preview_rpc_url: input.oss_preview_rpc_url,
        oss_preview_cookie: input.oss_preview_cookie,
        created_at: now_ts(),
    };

    let config_name = config.name.clone();
    let created = config_store::create_config(env, config).await?;
    let _ = audit_log(env, &username, "create", &format!("Created config '{}'", config_name)).await;
    Ok(Response::from_json(&ApiResponse::ok(created))?.with_status(201))
}

/// PUT /api/admin/configs/:id — update config
pub async fn handle_update(mut req: Request, env: &Env, config_id: &str) -> Result<Response> {
    require_admin_uname!(&req, env, username);
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
        oss_preview_rpc_url: input.oss_preview_rpc_url.unwrap_or(existing.oss_preview_rpc_url),
        oss_preview_cookie: input.oss_preview_cookie.unwrap_or(existing.oss_preview_cookie),
        created_at: existing.created_at,
    };

    let config_name = updated.name.clone();
    match config_store::update_config(env, config_id, updated).await? {
        Some(cfg) => {
            let _ = audit_log(env, &username, "update", &format!("Updated config '{}'", config_name)).await;
            Response::from_json(&ApiResponse::ok(cfg))
        }
        None => Ok(Response::from_json(&ApiResponse::<()>::err(
            "Config not found",
            "NOT_FOUND",
        ))?
        .with_status(404)),
    }
}

/// DELETE /api/admin/configs/:id — delete config
pub async fn handle_delete(req: Request, env: &Env, config_id: &str) -> Result<Response> {
    require_admin_uname!(&req, env, username);

    let config_name = config_store::get_config(env, config_id)
        .await?
        .map(|c| c.name)
        .unwrap_or_else(|| config_id.to_string());

    match config_store::delete_config(env, config_id).await? {
        true => {
            let _ = audit_log(env, &username, "delete", &format!("Deleted config '{}'", config_name)).await;
            Response::from_json(&ApiResponse::ok(serde_json::json!({"deleted": true})))
        }
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
    require_admin!(&req, env);
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

// ── Tenant admin CRUD ──

fn generate_api_key() -> String {
    format!("bndy_{}", uuid::Uuid::new_v4().to_string().replace('-', ""))
}

fn generate_api_secret() -> String {
    uuid::Uuid::new_v4().to_string().replace('-', "")
}

/// GET /api/admin/tenants?offset=&limit=
pub async fn handle_list_tenants(req: Request, env: &Env) -> Result<Response> {
    require_admin!(&req, env);
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
        .unwrap_or(20);
    let tenants = match db::list_tenants(env, offset, limit).await {
        Ok(t) => t,
        Err(e) => {
            return Ok(Response::from_json(&ApiResponse::<()>::err(
                &format!("{}", e),
                "INTERNAL_ERROR",
            ))?.with_status(500));
        }
    };
    let total = db::count_tenants(env).await?;
    Response::from_json(&ApiResponse::ok(serde_json::json!({
        "tenants": tenants,
        "total": total,
        "offset": offset,
        "limit": limit
    })))
}

/// POST /api/admin/tenants
pub async fn handle_create_tenant(mut req: Request, env: &Env) -> Result<Response> {
    require_admin_uname!(&req, env, username);
    let body = req.text().await?;
    let input: CreateTenantInput = serde_json::from_str(&body)
        .map_err(|e| worker::Error::RustError(format!("Invalid JSON: {}", e)))?;

    let now = now_ts();
    let tenant = crate::types::Tenant {
        id: uuid::Uuid::new_v4().to_string(),
        name: input.name,
        api_key: generate_api_key(),
        api_secret: generate_api_secret(),
        default_backend_config_id: input.default_backend_config_id,
        max_requests_per_day: input.max_requests_per_day,
        max_storage_bytes: input.max_storage_bytes,
        requests_used_today: 0,
        storage_used_bytes: 0,
        last_request_date: String::new(),
        is_active: 1,
        created_at: now,
        updated_at: now,
    };

    let tenant_name = tenant.name.clone();
    db::create_tenant(env, &tenant).await?;
    let _ = audit_log(env, &username, "create", &format!("Created tenant '{}'", tenant_name)).await;
    Ok(Response::from_json(&ApiResponse::ok(tenant))?.with_status(201))
}

/// GET /api/admin/tenants/:id
pub async fn handle_get_tenant(req: Request, env: &Env, tenant_id: &str) -> Result<Response> {
    require_admin!(&req, env);
    match db::get_tenant(env, tenant_id).await? {
        Some(t) => {
            let file_count = db::count_file_records(env, tenant_id).await?;
            Response::from_json(&ApiResponse::ok(serde_json::json!({
                "tenant": t,
                "file_count": file_count
            })))
        }
        None => Ok(Response::from_json(&ApiResponse::<()>::err(
            "Tenant not found",
            "NOT_FOUND",
        ))?.with_status(404)),
    }
}

/// PUT /api/admin/tenants/:id
pub async fn handle_update_tenant(mut req: Request, env: &Env, tenant_id: &str) -> Result<Response> {
    require_admin_uname!(&req, env, username);
    let body = req.text().await?;
    let input: UpdateTenantInput = serde_json::from_str(&body)
        .map_err(|e| worker::Error::RustError(format!("Invalid JSON: {}", e)))?;

    let existing = db::get_tenant(env, tenant_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Tenant not found".into()))?;

    let updated = crate::types::Tenant {
        name: input.name.unwrap_or(existing.name),
        default_backend_config_id: input.default_backend_config_id.unwrap_or(existing.default_backend_config_id),
        max_requests_per_day: input.max_requests_per_day.unwrap_or(existing.max_requests_per_day),
        max_storage_bytes: input.max_storage_bytes.unwrap_or(existing.max_storage_bytes),
        is_active: input.is_active.map(|b| if b { 1 } else { 0 }).unwrap_or(existing.is_active),
        updated_at: now_ts(),
        ..existing
    };

    let tenant_name = updated.name.clone();
    db::update_tenant(env, tenant_id, &updated).await?;
    let _ = audit_log(env, &username, "update", &format!("Updated tenant '{}'", tenant_name)).await;
    Response::from_json(&ApiResponse::ok(updated))
}

/// DELETE /api/admin/tenants/:id
pub async fn handle_delete_tenant(req: Request, env: &Env, tenant_id: &str) -> Result<Response> {
    require_admin_uname!(&req, env, username);

    let tenant_name = db::get_tenant(env, tenant_id)
        .await?
        .map(|t| t.name)
        .unwrap_or_else(|| tenant_id.to_string());

    db::delete_tenant(env, tenant_id).await?;
    let _ = audit_log(env, &username, "delete", &format!("Deleted tenant '{}'", tenant_name)).await;
    Response::from_json(&ApiResponse::ok(serde_json::json!({"deleted": true})))
}

/// GET /api/admin/tenants/:id/files
pub async fn handle_list_tenant_files(req: Request, env: &Env, tenant_id: &str) -> Result<Response> {
    require_admin!(&req, env);

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

    let records = db::list_file_records(env, tenant_id, offset, limit).await?;
    let total = db::count_file_records(env, tenant_id).await?;
    let items: Vec<crate::types::FileRecordData> = records.iter().map(|r| r.into()).collect();

    Response::from_json(&ApiResponse::ok(serde_json::json!({
        "files": items,
        "total": total,
        "offset": offset,
        "limit": limit
    })))
}

// ── Dashboard / Health / Audit ──

/// GET /api/admin/stats
pub async fn handle_stats(req: Request, env: &Env) -> Result<Response> {
    require_admin!(&req, env);
    let configs_count = config_store::count_configs(env).await?;
    let mut stats = db::get_stats(env).await?;
    stats.total_configs = configs_count;
    Response::from_json(&ApiResponse::ok(stats))
}

/// GET /api/admin/health
pub async fn handle_health(req: Request, env: &Env) -> Result<Response> {
    require_admin!(&req, env);
    let configs = config_store::list_configs(env, 0, 10000).await?;

    let mut results: Vec<HealthProbeResult> = Vec::new();
    for config in &configs {
        let start = SystemTime::now();
        let probe_resp = match config.backend_type {
            BackendType::S3 => s3_proxy::test_connection(env, &config.id).await,
            BackendType::Dufs => dufs_proxy::test_connection(env, &config.id).await,
            BackendType::Redis => redis_proxy::test_connection(env, &config.id).await,
        };
        let elapsed = start.elapsed().unwrap_or_default().as_millis() as u64;

        match probe_resp {
            Ok(mut resp) => {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    let data_ok = body["data"]["ok"].as_bool().unwrap_or(false);
                    let msg = body["data"]["message"].as_str().unwrap_or("").to_string();
                    results.push(HealthProbeResult {
                        config_id: config.id.clone(),
                        config_name: config.name.clone(),
                        backend_type: config.backend_type.as_str().to_string(),
                        status: if data_ok { "ok".into() } else { "error".into() },
                        latency_ms: elapsed,
                        error: if data_ok { None } else { Some(msg) },
                    });
                } else {
                    results.push(HealthProbeResult {
                        config_id: config.id.clone(),
                        config_name: config.name.clone(),
                        backend_type: config.backend_type.as_str().to_string(),
                        status: "error".into(),
                        latency_ms: elapsed,
                        error: Some("Failed to parse response".into()),
                    });
                }
            }
            Err(e) => {
                results.push(HealthProbeResult {
                    config_id: config.id.clone(),
                    config_name: config.name.clone(),
                    backend_type: config.backend_type.as_str().to_string(),
                    status: "error".into(),
                    latency_ms: elapsed,
                    error: Some(format!("{}", e)),
                });
            }
        }
    }

    let ok_count = results.iter().filter(|r| r.status == "ok").count() as i64;
    let error_count = results.len() as i64 - ok_count;

    Response::from_json(&ApiResponse::ok(serde_json::json!({
        "results": results,
        "ok_count": ok_count,
        "error_count": error_count
    })))
}

/// GET /api/admin/audit-logs?offset=&limit=
pub async fn handle_audit_logs(req: Request, env: &Env) -> Result<Response> {
    require_admin!(&req, env);

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

    let logs = db::list_audit_logs(env, offset, limit).await?;
    let total = db::count_audit_logs(env).await?;

    Response::from_json(&ApiResponse::ok(serde_json::json!({
        "logs": logs,
        "total": total,
        "offset": offset,
        "limit": limit
    })))
}
