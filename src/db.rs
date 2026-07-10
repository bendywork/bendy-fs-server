use serde::Deserialize;
use wasm_bindgen::JsValue;
use worker::*;

use crate::types::{FileRecord, Tenant, AuditLog, AdminStats};

const DB_BINDING: &str = "BENDY_FS_DB";

fn db(env: &Env) -> Result<worker::d1::D1Database> {
    env.d1(DB_BINDING)
}

#[derive(Debug, Deserialize)]
struct CountResult {
    #[serde(alias = "count", alias = "COUNT(*)")]
    count: i64,
}

pub async fn list_tenants(env: &Env) -> Result<Vec<Tenant>> {
    let d = db(env)?;
    let result = d.prepare("SELECT * FROM tenants ORDER BY created_at DESC").all().await?;
    result.results::<Tenant>()
        .map_err(|e| worker::Error::RustError(format!("Failed to deserialize tenants: {}", e)))
}

pub async fn get_tenant(env: &Env, tenant_id: &str) -> Result<Option<Tenant>> {
    let d = db(env)?;
    d.prepare("SELECT * FROM tenants WHERE id = ?1")
        .bind(&[tenant_id.into()])?
        .first::<Tenant>(None).await
}

pub async fn get_tenant_by_api_key(env: &Env, api_key: &str) -> Result<Option<Tenant>> {
    let d = db(env)?;
    d.prepare("SELECT * FROM tenants WHERE api_key = ?1")
        .bind(&[api_key.into()])?
        .first::<Tenant>(None).await
}

pub async fn verify_tenant_credentials(
    env: &Env,
    api_key: &str,
    api_secret: &str,
) -> Result<Tenant> {
    let tenant = get_tenant_by_api_key(env, api_key)
        .await?
        .ok_or_else(|| worker::Error::RustError("Invalid API key".into()))?;

    if tenant.is_active == 0 {
        return Err(worker::Error::RustError("Tenant is disabled".into()));
    }

    if tenant.api_secret != api_secret {
        return Err(worker::Error::RustError("Invalid API secret".into()));
    }

    Ok(tenant)
}

pub async fn check_and_update_quota(
    env: &Env,
    tenant_id: &str,
    request_size_bytes: i64,
) -> Result<()> {
    let tenant = get_tenant(env, tenant_id)
        .await?
        .ok_or_else(|| worker::Error::RustError("Tenant not found".into()))?;

    if tenant.is_active == 0 {
        return Err(worker::Error::RustError("Tenant is disabled".into()));
    }

    let today = today_date();
    let d = db(env)?;

    if tenant.last_request_date != today {
        // Reset daily counter for a new day
        d.prepare(
            "UPDATE tenants SET requests_used_today = 1, last_request_date = ?1 WHERE id = ?2"
        )
        .bind(&[today.into(), tenant_id.into()])?
        .run().await?;
    } else {
        if tenant.requests_used_today >= tenant.max_requests_per_day {
            return Err(worker::Error::RustError(
                "Daily request quota exceeded".into(),
            ));
        }
        d.prepare(
            "UPDATE tenants SET requests_used_today = requests_used_today + 1 WHERE id = ?1"
        )
        .bind(&[tenant_id.into()])?
        .run().await?;
    }

    // Check storage quota
    if request_size_bytes > 0 {
        let new_total = tenant.storage_used_bytes + request_size_bytes;
        if new_total > tenant.max_storage_bytes {
            return Err(worker::Error::RustError(
                format!(
                    "Storage quota exceeded ({} / {} bytes)",
                    new_total, tenant.max_storage_bytes
                ),
            ));
        }
    }

    Ok(())
}

pub async fn increment_quota_after_upload(env: &Env, tenant_id: &str, bytes_added: i64) -> Result<()> {
    let d = db(env)?;
    d.prepare(
        "UPDATE tenants SET storage_used_bytes = storage_used_bytes + ?1 WHERE id = ?2"
    )
    .bind(&[JsValue::from_f64(bytes_added as f64), tenant_id.into()])?
    .run().await?;
    Ok(())
}

pub async fn decrement_quota_after_delete(env: &Env, tenant_id: &str, bytes_removed: i64) -> Result<()> {
    let d = db(env)?;
    d.prepare(
        "UPDATE tenants SET storage_used_bytes = MAX(0, storage_used_bytes - ?1) WHERE id = ?2"
    )
    .bind(&[JsValue::from_f64(bytes_removed as f64), tenant_id.into()])?
    .run().await?;
    Ok(())
}

pub async fn create_file_record(env: &Env, record: &FileRecord) -> Result<()> {
    let d = db(env)?;
    let preview = record.preview_url.as_deref().unwrap_or("");
    d.prepare(
        "INSERT INTO file_records (id, tenant_id, file_key, original_name, mime_type, \
         size_bytes, backend_type, backend_config_id, preview_url, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
    )
    .bind(&[
        record.id.as_str().into(),
        record.tenant_id.as_str().into(),
        record.file_key.as_str().into(),
        record.original_name.as_str().into(),
        record.mime_type.as_str().into(),
        JsValue::from_f64(record.size_bytes as f64),
        record.backend_type.as_str().into(),
        record.backend_config_id.as_str().into(),
        preview.into(),
        JsValue::from_f64(record.created_at as f64),
    ])?
    .run().await?;
    Ok(())
}

pub async fn delete_file_record(env: &Env, tenant_id: &str, file_key: &str) -> Result<bool> {
    let d = db(env)?;
    d.prepare("DELETE FROM file_records WHERE tenant_id = ?1 AND file_key = ?2")
        .bind(&[tenant_id.into(), file_key.into()])?
        .run().await?;
    Ok(true)
}

pub async fn get_file_record(
    env: &Env,
    tenant_id: &str,
    file_key: &str,
) -> Result<Option<FileRecord>> {
    let d = db(env)?;
    d.prepare("SELECT * FROM file_records WHERE tenant_id = ?1 AND file_key = ?2")
        .bind(&[tenant_id.into(), file_key.into()])?
        .first::<FileRecord>(None).await
}

pub async fn list_file_records(
    env: &Env,
    tenant_id: &str,
    offset: i64,
    limit: i64,
) -> Result<Vec<FileRecord>> {
    let d = db(env)?;
    let result = d.prepare(
        "SELECT * FROM file_records WHERE tenant_id = ?1 ORDER BY created_at DESC LIMIT ?2 OFFSET ?3"
    )
    .bind(&[tenant_id.into(), JsValue::from_f64(limit as f64), JsValue::from_f64(offset as f64)])?
    .all().await?;
    result.results::<FileRecord>()
}

pub async fn count_file_records(env: &Env, tenant_id: &str) -> Result<i64> {
    let d = db(env)?;
    let result = d.prepare("SELECT COUNT(*) as count FROM file_records WHERE tenant_id = ?1")
        .bind(&[tenant_id.into()])?
        .first::<CountResult>(None).await?;
    Ok(result.map(|r| r.count).unwrap_or(0))
}

pub async fn create_tenant(env: &Env, tenant: &Tenant) -> Result<()> {
    let d = db(env)?;
    let is_active_int = tenant.is_active as f64;
    d.prepare(
        "INSERT INTO tenants (id, name, api_key, api_secret, default_backend_config_id, \
         max_requests_per_day, max_storage_bytes, requests_used_today, storage_used_bytes, \
         last_request_date, is_active, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0, '', ?8, ?9, ?10)"
    )
    .bind(&[
        tenant.id.as_str().into(),
        tenant.name.as_str().into(),
        tenant.api_key.as_str().into(),
        tenant.api_secret.as_str().into(),
        tenant.default_backend_config_id.as_str().into(),
        JsValue::from_f64(tenant.max_requests_per_day as f64),
        JsValue::from_f64(tenant.max_storage_bytes as f64),
        JsValue::from_f64(is_active_int),
        JsValue::from_f64(tenant.created_at as f64),
        JsValue::from_f64(tenant.updated_at as f64),
    ])?
    .run().await?;
    Ok(())
}

pub async fn update_tenant(env: &Env, tenant_id: &str, updated: &Tenant) -> Result<()> {
    let d = db(env)?;
    let is_active_int = updated.is_active as f64;
    d.prepare(
        "UPDATE tenants SET name = ?1, default_backend_config_id = ?2, \
         max_requests_per_day = ?3, max_storage_bytes = ?4, is_active = ?5, \
         updated_at = ?6 WHERE id = ?7"
    )
    .bind(&[
        updated.name.as_str().into(),
        updated.default_backend_config_id.as_str().into(),
        JsValue::from_f64(updated.max_requests_per_day as f64),
        JsValue::from_f64(updated.max_storage_bytes as f64),
        JsValue::from_f64(is_active_int),
        JsValue::from_f64(updated.updated_at as f64),
        tenant_id.into(),
    ])?
    .run().await?;
    Ok(())
}

pub async fn delete_tenant(env: &Env, tenant_id: &str) -> Result<()> {
    let d = db(env)?;
    // Delete file records first
    d.prepare("DELETE FROM file_records WHERE tenant_id = ?1")
        .bind(&[tenant_id.into()])?
        .run().await?;
    // Delete tenant
    d.prepare("DELETE FROM tenants WHERE id = ?1")
        .bind(&[tenant_id.into()])?
        .run().await?;
    Ok(())
}

// ── Audit log ──

pub async fn ensure_schema(env: &Env) -> Result<()> {
    let d = db(env)?;
    // tenants
    d.prepare(
        "CREATE TABLE IF NOT EXISTS tenants (\
         id TEXT PRIMARY KEY, \
         name TEXT NOT NULL, \
         api_key TEXT NOT NULL UNIQUE, \
         api_secret TEXT NOT NULL, \
         default_backend_config_id TEXT NOT NULL, \
         max_requests_per_day INTEGER DEFAULT 10000, \
         max_storage_bytes INTEGER DEFAULT 10737418240, \
         requests_used_today INTEGER DEFAULT 0, \
         storage_used_bytes INTEGER DEFAULT 0, \
         last_request_date TEXT DEFAULT '', \
         is_active INTEGER DEFAULT 1, \
         created_at INTEGER NOT NULL, \
         updated_at INTEGER NOT NULL)"
    ).run().await?;
    // file_records
    d.prepare(
        "CREATE TABLE IF NOT EXISTS file_records (\
         id TEXT PRIMARY KEY, \
         tenant_id TEXT NOT NULL REFERENCES tenants(id), \
         file_key TEXT NOT NULL, \
         original_name TEXT NOT NULL, \
         mime_type TEXT NOT NULL DEFAULT 'application/octet-stream', \
         size_bytes INTEGER NOT NULL DEFAULT 0, \
         backend_type TEXT NOT NULL, \
         backend_config_id TEXT NOT NULL, \
         preview_url TEXT, \
         created_at INTEGER NOT NULL, \
         UNIQUE(tenant_id, file_key))"
    ).run().await?;
    // audit_logs
    d.prepare(
        "CREATE TABLE IF NOT EXISTS audit_logs (\
         id TEXT PRIMARY KEY, \
         action TEXT NOT NULL, \
         username TEXT NOT NULL, \
         detail TEXT NOT NULL DEFAULT '', \
         created_at INTEGER NOT NULL)"
    ).run().await?;
    d.prepare(
        "CREATE INDEX IF NOT EXISTS idx_audit_logs_created ON audit_logs(created_at DESC)"
    ).run().await?;
    Ok(())
}

pub async fn insert_audit_log(env: &Env, log: &AuditLog) -> Result<()> {
    let d = db(env)?;
    d.prepare(
        "INSERT INTO audit_logs (id, action, username, detail, created_at) VALUES (?1, ?2, ?3, ?4, ?5)"
    )
    .bind(&[
        log.id.as_str().into(),
        log.action.as_str().into(),
        log.username.as_str().into(),
        log.detail.as_str().into(),
        JsValue::from_f64(log.created_at as f64),
    ])?
    .run().await?;
    Ok(())
}

pub async fn list_audit_logs(env: &Env, offset: i64, limit: i64) -> Result<Vec<AuditLog>> {
    let d = db(env)?;
    let result = d.prepare(
        "SELECT * FROM audit_logs ORDER BY created_at DESC LIMIT ?1 OFFSET ?2"
    )
    .bind(&[JsValue::from_f64(limit as f64), JsValue::from_f64(offset as f64)])?
    .all().await?;
    result.results::<AuditLog>()
}

pub async fn count_audit_logs(env: &Env) -> Result<i64> {
    let d = db(env)?;
    let result = d.prepare("SELECT COUNT(*) as count FROM audit_logs")
        .first::<CountResult>(None).await?;
    Ok(result.map(|r| r.count).unwrap_or(0))
}

pub async fn get_stats(env: &Env) -> Result<AdminStats> {
    let d = db(env)?;

    let total_tenants = d.prepare("SELECT COUNT(*) as count FROM tenants")
        .first::<CountResult>(None).await?
        .map(|r| r.count).unwrap_or(0);

    let active_tenants = d.prepare("SELECT COUNT(*) as count FROM tenants WHERE is_active = 1")
        .first::<CountResult>(None).await?
        .map(|r| r.count).unwrap_or(0);

    let total_files = d.prepare("SELECT COUNT(*) as count FROM file_records")
        .first::<CountResult>(None).await?
        .map(|r| r.count).unwrap_or(0);

    #[derive(Debug, Deserialize)]
    struct SumResult { total: Option<f64> }

    let total_storage_bytes = d.prepare("SELECT COALESCE(SUM(storage_used_bytes), 0) as total FROM tenants")
        .first::<SumResult>(None).await?
        .map(|r| r.total.unwrap_or(0.0) as i64)
        .unwrap_or(0);

    Ok(AdminStats {
        total_configs: 0, // filled by caller from KV
        total_tenants,
        active_tenants,
        total_files,
        total_storage_used_bytes: total_storage_bytes,
    })
}

fn today_date() -> String {
    let now = web_time::SystemTime::now()
        .duration_since(web_time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let days = secs / 86400;
    let mut y = 1970i64;
    let mut remaining = days as i64;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }
    let month_days = if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut m = 0usize;
    while m < 12 && remaining >= month_days[m] {
        remaining -= month_days[m];
        m += 1;
    }
    let month = m + 1;
    let day = remaining + 1;
    format!("{:04}-{:02}-{:02}", y, month, day)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}
