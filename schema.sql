CREATE TABLE IF NOT EXISTS tenants (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  api_key TEXT NOT NULL UNIQUE,
  api_secret TEXT NOT NULL,
  default_backend_config_id TEXT NOT NULL,
  max_requests_per_day INTEGER DEFAULT 10000,
  max_storage_bytes INTEGER DEFAULT 10737418240,
  requests_used_today INTEGER DEFAULT 0,
  storage_used_bytes INTEGER DEFAULT 0,
  last_request_date TEXT DEFAULT '',
  is_active INTEGER DEFAULT 1,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS file_records (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL REFERENCES tenants(id),
  file_key TEXT NOT NULL,
  original_name TEXT NOT NULL,
  mime_type TEXT NOT NULL DEFAULT 'application/octet-stream',
  size_bytes INTEGER NOT NULL DEFAULT 0,
  backend_type TEXT NOT NULL,
  backend_config_id TEXT NOT NULL,
  preview_url TEXT,
  created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_file_records_tenant ON file_records(tenant_id);
CREATE INDEX IF NOT EXISTS idx_file_records_key ON file_records(tenant_id, file_key);
