# Bendy FS Server

多后端统一文件存储代理服务，运行在 Cloudflare Workers 上，基于 Rust/WASM 构建。

## 概述

Bendy FS Server 是一个轻量级的文件存储代理层，为客户端提供统一的 API 来访问不同类型的存储后端。它支持三种后端类型：

| 后端 | 说明 |
|------|------|
| **S3** | 兼容 S3 的对象存储（AWS S3、Cloudflare R2、MinIO 等），使用 AWS Signature V4 签名 |
| **Dufs** | 自托管的 [Dufs](https://github.com/sigoden/dufs) 文件服务器，支持 Basic Auth |
| **Redis** | 基于 Upstash Redis REST API 的键值存储 |

### 核心功能

- **多租户隔离** — 每个租户有独立的 API Key/Secret、配额、存储空间
- **GitHub OAuth 登录** — 管理后台通过 GitHub OAuth 认证，支持 JWT Session Cookie
- **租户 JWT 鉴权** — 租户端通过 HMAC-SHA256 JWT 访问 API
- **预签名 URL** — S3 后端支持预签名上传/下载链接
- **配额控制** — 每日请求次数限制 + 总存储空间限制
- **公开文件** — 租户可开启 public_files，通过公开链接直接下载
- **OSS 预览** — 支持对接 OSS 预览服务生成文档/图片预览链接
- **审计日志** — 记录所有管理操作
- **健康检查** — 定时探测后端可用性

## 技术栈

- **Runtime**: Cloudflare Workers
- **Language**: Rust → WASM (via `wasm-pack build --target web`)
- **数据库**: Cloudflare D1 (SQLite)
- **KV 存储**: Cloudflare Workers KV（配置索引）
- **前端**: 原生 HTML/JS 管理后台 (SPA)

## 项目结构

```
src/
├── lib.rs              # 路由注册、CORS 中间件、入口
├── types.rs            # 数据类型定义（BackendConfig, Tenant, FileRecord 等）
├── config_store.rs     # KV 配置存储（索引 + CRUD）
├── db.rs               # D1 数据库操作（租户/文件/审计/配额/schema）
├── s3_signer.rs        # AWS Signature V4 签名实现（含预签名 URL）
├── s3_proxy.rs         # S3 后端代理（上传/下载/删除/连接测试）
├── dufs_proxy.rs       # Dufs 后端代理（上传/下载/删除/连接测试）
├── redis_proxy.rs      # Redis 后端代理（GET/SET/DEL/PING）
├── auth.rs             # 管理员鉴权中间件
├── github_oauth.rs     # GitHub OAuth 完整流程
├── tenant_auth.rs      # 租户 JWT 鉴权
├── tenant_handlers.rs  # 租户端 API（上传/下载/预签名/OSS 预览）
├── admin_handlers.rs   # 管理后台 API（配置 CRUD、租户 CRUD、统计、健康检查）
static/
└── admin.html          # 管理后台 SPA
```

## API 文档

### 管理后台 API（需管理员登录）

#### 认证
| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/auth/github/login` | 跳转 GitHub OAuth 授权页 |
| GET | `/api/github/callback` | OAuth 回调（GitHub 重定向到此） |
| GET | `/api/auth/session` | 获取当前登录用户信息 |
| POST | `/api/auth/logout` | 登出 |

#### 后端配置 CRUD
| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/admin/configs` | 列出所有配置 |
| POST | `/api/admin/configs` | 创建配置 |
| GET | `/api/admin/configs/:id` | 获取指定配置 |
| PUT | `/api/admin/configs/:id` | 更新配置 |
| DELETE | `/api/admin/configs/:id` | 删除配置 |
| POST | `/api/admin/configs/:id/test` | 测试后端连接 |

#### 租户管理
| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/admin/tenants` | 列出所有租户 |
| POST | `/api/admin/tenants` | 创建租户 |
| GET | `/api/admin/tenants/:id` | 获取租户详情 |
| PUT | `/api/admin/tenants/:id` | 更新租户 |
| DELETE | `/api/admin/tenants/:id` | 删除租户 |
| GET | `/api/admin/tenants/:id/files` | 列出租户文件 |

#### 仪表盘
| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/admin/stats` | 统计概览 |
| GET | `/api/admin/health` | 全量后端健康检查 |
| GET | `/api/admin/audit-logs` | 审计日志 |

### 租户 API（JWT 鉴权）

所有租户 API 路径以 `/api/t/` 开头，请求头需携带 `Authorization: Bearer <access_token>`。

#### 认证
| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/api/t/auth` | 用 api_key + api_secret 换取 JWT（1 小时有效） |

#### 文件操作
| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/api/t/upload` | 上传文件（multipart form: file + key + content_type） |
| GET | `/api/t/download/*key` | 下载文件 |
| DELETE | `/api/t/delete/*key` | 删除文件 |
| GET | `/api/t/files` | 列出文件（支持 ?page=&limit=） |
| POST | `/api/t/presign-upload` | 获取预签名上传 URL（S3） |
| POST | `/api/t/presign-download` | 获取预签名下载 URL（S3） |
| POST | `/api/t/oss-preview` | 获取 OSS 预览链接 |

#### 公开下载
| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/files/:tenant_id/*key` | 公开文件下载（无需鉴权，租户需开启 public_files） |

### 统一代理 API（管理后台使用）

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/api/fs/:id/upload` | 通过配置 ID 上传文件 |
| GET | `/api/fs/:id/download/*key` | 通过配置 ID 下载文件 |
| DELETE | `/api/fs/:id/delete/*key` | 删除文件 |
| POST | `/api/fs/:id/presign-upload` | 预签名上传 URL |
| POST | `/api/fs/:id/presign-download` | 预签名下载 URL |
| POST | `/api/fs/:id/set` | Redis SET 操作 |

> `/api/s3/:id/...` 路径作为向后兼容别名保留。

## 配置

### Cloudflare 资源

需要在 `wrangler.toml` 和 Cloudflare Dashboard 中配置以下绑定：

| 绑定名 | 类型 | 用途 |
|--------|------|------|
| `BENDY_FS_DB` | D1 | 主数据库（租户、文件记录、审计日志） |
| `BENDY_FS_CONFIGS` | KV | 后端配置存储（索引 + JSON） |

### 环境变量（Secret）

需通过 `wrangler secret put` 或 Dashboard 设置：

| 变量 | 说明 |
|------|------|
| `ADMIN_TOKEN` | 旧版管理 API Token（GitHub OAuth 启用后作为后备方案） |
| `GITHUB_CLIENT_ID` | GitHub OAuth App Client ID |
| `GITHUB_CLIENT_SECRET` | GitHub OAuth App Client Secret |
| `GITHUB_ORG` | 允许登录的 GitHub 组织名（用于验证用户归属） |
| `JWT_SECRET` | 管理员 JWT 签名密钥 |
| `SESSION_COOKIE_NAME` | Session Cookie 名称（默认 `bendy_session`） |

### GitHub OAuth App 配置

在 GitHub Settings → Developer settings → OAuth Apps 创建应用：

- **Homepage URL**: `https://your-worker.workers.dev`
- **Authorization callback URL**: `https://your-worker.workers.dev/api/github/callback`

### 创建后端配置示例

**S3 兼容存储（如 Cloudflare R2）**：
```json
{
  "name": "My R2 Bucket",
  "backend_type": "s3",
  "endpoint": "https://<account_id>.r2.cloudflarestorage.com",
  "region": "auto",
  "bucket": "my-bucket",
  "access_key_id": "<r2_access_key>",
  "secret_access_key": "<r2_secret>",
  "force_path_style": true
}
```

**Dufs 文件服务器**：
```json
{
  "name": "My Dufs Server",
  "backend_type": "dufs",
  "endpoint": "https://dufs.example.com",
  "username": "admin",
  "password": "secret"
}
```

**Redis（Upstash）**：
```json
{
  "name": "My Redis",
  "backend_type": "redis",
  "endpoint": "https://xxx.upstash.io",
  "redis_password": "<upstash_token>"
}
```

## 数据库 Schema

D1 数据库在 Worker 启动时通过 `ensure_schema()` 自动创建表。

### tenants
| 列 | 类型 | 说明 |
|----|------|------|
| id | TEXT PK | UUID |
| name | TEXT | 租户名称 |
| api_key | TEXT UNIQUE | 租户 API Key |
| api_secret | TEXT | 租户 API Secret |
| default_backend_config_id | TEXT | 默认后端配置 ID |
| max_requests_per_day | INTEGER | 每日请求配额（默认 10000） |
| max_storage_bytes | INTEGER | 存储配额（默认 10GB） |
| requests_used_today | INTEGER | 今日已用请求数 |
| storage_used_bytes | INTEGER | 已用存储字节数 |
| last_request_date | TEXT | 上次请求日期（用于重置日配额） |
| is_active | INTEGER | 是否启用（1/0） |
| public_files | INTEGER | 是否开启公开文件下载 |
| created_at | INTEGER | Unix 时间戳 |
| updated_at | INTEGER | Unix 时间戳 |

### file_records
| 列 | 类型 | 说明 |
|----|------|------|
| id | TEXT PK | UUID |
| tenant_id | TEXT FK | 所属租户 |
| file_key | TEXT | 文件路径/key |
| original_name | TEXT | 原始文件名 |
| mime_type | TEXT | MIME 类型 |
| size_bytes | INTEGER | 文件大小 |
| backend_type | TEXT | 后端类型 |
| backend_config_id | TEXT | 后端配置 ID |
| preview_url | TEXT | 预览链接 |
| created_at | INTEGER | Unix 时间戳 |

UNIQUE(tenant_id, file_key)

### audit_logs
| 列 | 类型 | 说明 |
|----|------|------|
| id | TEXT PK | UUID |
| action | TEXT | 操作类型 |
| username | TEXT | 操作用户 |
| detail | TEXT | 操作详情 |
| created_at | INTEGER | Unix 时间戳 |

## 本地开发

### 前置条件

- Rust toolchain（stable）
- `wasm-pack` 已安装
- Node.js / wrangler CLI

### 开发流程

```bash
# 安装 wrangler
npm install -g wrangler

# 构建 WASM
wasm-pack build --target web --no-opt

# 本地运行
wrangler dev

# 部署到 Cloudflare
wrangler deploy
```

### 使用 Cargo 类型检查（不构建 WASM）

```bash
cargo check --target wasm32-unknown-unknown
```

## 部署

CI/CD 通过 GitHub Actions 自动部署。推送到 `main` 分支触发：

1. 安装 Rust + wasm32 target
2. 安装 wasm-pack
3. `wasm-pack build --target web --no-opt`
4. 通过 `cloudflare/wrangler-action` 部署到 Cloudflare Workers

需在 GitHub Secrets 中配置 `CLOUDFLARE_API_TOKEN`。

## 安全模型

- **管理后台**：GitHub OAuth → 组织验证 → JWT Session Cookie
- **租户 API**：api_key + api_secret → HMAC-SHA256 JWT（1h TTL）
- **公开文件**：通过 `/files/:tenant_id/*key` 访问，需租户开启 `public_files=1`
- **S3 签名**：所有 S3 请求通过 AWS Signature V4 进行身份验证，secret 不暴露给客户端
