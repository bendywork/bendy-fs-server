use serde::{Deserialize, Deserializer, Serialize};
use serde::de::{self, Visitor};
use std::fmt;

fn int_or_bool_to_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    struct IntBoolVisitor;
    impl<'de> Visitor<'de> for IntBoolVisitor {
        type Value = bool;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a boolean or an integer (0 or 1)")
        }
        fn visit_bool<E: de::Error>(self, v: bool) -> Result<bool, E> {
            Ok(v)
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<bool, E> {
            Ok(v != 0)
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<bool, E> {
            Ok(v != 0)
        }
        fn visit_f64<E: de::Error>(self, v: f64) -> Result<bool, E> {
            Ok(v != 0.0)
        }
    }
    deserializer.deserialize_any(IntBoolVisitor)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendType {
    S3,
    Dufs,
    Redis,
}

impl BackendType {
    pub fn as_str(&self) -> &'static str {
        match self {
            BackendType::S3 => "s3",
            BackendType::Dufs => "dufs",
            BackendType::Redis => "redis",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendConfig {
    pub id: String,
    pub name: String,
    pub backend_type: BackendType,
    pub endpoint: String,
    pub created_at: u64,

    // S3-specific
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub access_key_id: String,
    #[serde(default)]
    #[serde(skip_serializing)]
    pub secret_access_key: String,
    #[serde(default)]
    pub force_path_style: bool,

    // Dufs-specific
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,

    // Redis-specific
    #[serde(default)]
    pub redis_password: Option<String>,
    #[serde(default)]
    pub db_index: Option<u32>,

    // OSS Preview (for backends that have a preview service, e.g. hi168)
    #[serde(default)]
    pub oss_preview_rpc_url: String,
    #[serde(default)]
    pub oss_preview_cookie: String,
}

/// Backward-compat alias
pub type S3Config = BackendConfig;

#[derive(Debug, Deserialize)]
pub struct CreateBackendConfigInput {
    pub name: String,
    pub backend_type: BackendType,
    pub endpoint: String,

    // S3
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub access_key_id: String,
    #[serde(default)]
    pub secret_access_key: String,
    #[serde(default)]
    pub force_path_style: bool,

    // Dufs
    pub username: Option<String>,
    pub password: Option<String>,

    // Redis
    pub redis_password: Option<String>,
    pub db_index: Option<u32>,

    // OSS Preview
    #[serde(default)]
    pub oss_preview_rpc_url: String,
    #[serde(default)]
    pub oss_preview_cookie: String,
}

/// Backward-compat alias
pub type CreateConfigInput = CreateBackendConfigInput;

#[derive(Debug, Deserialize)]
pub struct UpdateBackendConfigInput {
    pub name: Option<String>,
    pub backend_type: Option<BackendType>,
    pub endpoint: Option<String>,

    // S3
    pub region: Option<String>,
    pub bucket: Option<String>,
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    pub force_path_style: Option<bool>,

    // Dufs
    pub username: Option<String>,
    pub password: Option<String>,

    // Redis
    pub redis_password: Option<String>,
    pub db_index: Option<u32>,

    // OSS Preview
    pub oss_preview_rpc_url: Option<String>,
    pub oss_preview_cookie: Option<String>,
}

/// Backward-compat alias
pub type UpdateConfigInput = UpdateBackendConfigInput;

#[derive(Debug, Deserialize)]
pub struct PresignedUrlInput {
    pub key: String,
    pub content_type: Option<String>,
    pub expires_in_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct PresignedDownloadInput {
    pub key: String,
    pub expires_in_seconds: Option<u64>,
    pub filename: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PresignedUrlOutput {
    pub url: String,
    pub key: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteObjectInput {
    pub key: String,
}

#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            code: None,
        }
    }

    pub fn err(error: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error.into()),
            code: Some(code.into()),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ConfigListData {
    pub configs: Vec<ConfigListItem>,
}

#[derive(Debug, Serialize)]
pub struct ConfigListItem {
    pub id: String,
    pub name: String,
    pub backend_type: BackendType,
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub force_path_style: bool,
    pub created_at: u64,
}

impl From<&BackendConfig> for ConfigListItem {
    fn from(c: &BackendConfig) -> Self {
        Self {
            id: c.id.clone(),
            name: c.name.clone(),
            backend_type: c.backend_type.clone(),
            endpoint: c.endpoint.clone(),
            region: c.region.clone(),
            bucket: c.bucket.clone(),
            force_path_style: c.force_path_style,
            created_at: c.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TestResult {
    pub ok: bool,
    pub message: String,
}

// ── Tenant types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    pub id: String,
    pub name: String,
    pub api_key: String,
    #[serde(skip_serializing)]
    pub api_secret: String,
    pub default_backend_config_id: String,
    pub max_requests_per_day: i64,
    pub max_storage_bytes: i64,
    pub requests_used_today: i64,
    pub storage_used_bytes: i64,
    pub last_request_date: String,
    #[serde(deserialize_with = "int_or_bool_to_bool")]
    pub is_active: bool,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub id: String,
    pub tenant_id: String,
    pub file_key: String,
    pub original_name: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub backend_type: String,
    pub backend_config_id: String,
    pub preview_url: Option<String>,
    pub created_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TenantClaims {
    pub tenant_id: String,
    pub name: String,
    pub iat: u64,
    pub exp: u64,
}

#[derive(Debug, Deserialize)]
pub struct CreateTenantInput {
    pub name: String,
    pub default_backend_config_id: String,
    #[serde(default = "default_max_requests")]
    pub max_requests_per_day: i64,
    #[serde(default = "default_max_storage")]
    pub max_storage_bytes: i64,
}

fn default_max_requests() -> i64 { 10000 }
fn default_max_storage() -> i64 { 10737418240 }

#[derive(Debug, Deserialize)]
pub struct UpdateTenantInput {
    pub name: Option<String>,
    pub default_backend_config_id: Option<String>,
    pub max_requests_per_day: Option<i64>,
    pub max_storage_bytes: Option<i64>,
    pub is_active: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct TenantAuthInput {
    pub api_key: String,
    pub api_secret: String,
}

#[derive(Debug, Serialize)]
pub struct TenantAuthOutput {
    pub access_token: String,
    pub expires_in: u64,
}

#[derive(Debug, Serialize)]
pub struct FileRecordData {
    pub id: String,
    pub file_key: String,
    pub original_name: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub backend_type: String,
    pub preview_url: Option<String>,
    pub created_at: u64,
}

impl From<&FileRecord> for FileRecordData {
    fn from(r: &FileRecord) -> Self {
        Self {
            id: r.id.clone(),
            file_key: r.file_key.clone(),
            original_name: r.original_name.clone(),
            mime_type: r.mime_type.clone(),
            size_bytes: r.size_bytes,
            backend_type: r.backend_type.clone(),
            preview_url: r.preview_url.clone(),
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct TenantUploadInput {
    pub key: String,
    pub content_type: Option<String>,
    pub filename: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TenantPresignInput {
    pub key: String,
    pub content_type: Option<String>,
    pub filename: Option<String>,
    pub expires_in_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct TenantOssPreviewInput {
    pub key: String,
    pub file_name: String,
    pub mime_type: String,
    pub size_bytes: i64,
}

// ── Dashboard / Health / Audit types ──

#[derive(Debug, Serialize)]
pub struct AdminStats {
    pub total_configs: usize,
    pub total_tenants: i64,
    pub active_tenants: i64,
    pub total_files: i64,
    pub total_storage_used_bytes: i64,
}

#[derive(Debug, Serialize)]
pub struct HealthProbeResult {
    pub config_id: String,
    pub config_name: String,
    pub backend_type: String,
    pub status: String,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLog {
    pub id: String,
    pub action: String,
    pub username: String,
    pub detail: String,
    pub created_at: u64,
}
