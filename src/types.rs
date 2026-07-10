use serde::{Deserialize, Serialize};

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
