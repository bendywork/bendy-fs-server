# maintain.md - Bendy FS Server

## v0.1.0 - 2026-07-10
### 变更内容
- Phase 1: 认证修复 — `require_admin!` 宏返回 JSON 401 而非 500 文本错误
- Phase 2: 后端端点完成 — stats、health、audit-logs + 所有 7 个 CRUD handler 集成审计日志
- Phase 3: admin.html 完整重写（~1100行）
  - GitHub OAuth 登录 / Light-Dark 主题切换
  - 可折叠侧边栏（概览/系统/监控）+ 底部用户卡片
  - Dashboard 仪表盘（4 统计卡片 + 健康摘要 + 最近审计日志）
  - Configs/Tenants/Files CRUD（模态框 + Toast 通知）
  - 健康探测页（全部探测 + 单卡探测）
  - 审计日志分页表格
  - i18n 中英文切换（zh/en），localStorage 持久化

### 影响范围
- `src/auth.rs` — `verify_admin_with_username()` 新增
- `src/admin_handlers.rs` — stats/health/audit-logs handler + 审计日志集成
- `src/db.rs` — D1 数据库操作层（tenant CRUD, file records, audit logs, stats）
- `src/tenant_auth.rs` — 租户 JWT 认证
- `src/tenant_handlers.rs` — 租户端上传/下载/删除 API
- `src/types.rs` — AdminStats, HealthProbeResult, AuditLog 等新类型
- `src/lib.rs` — 路由注册
- `static/admin.html` — 完全重写（128行 → ~1100行）
- `build/worker.mjs` — Worker 入口（ES module + WASM import）
- `wrangler.toml` — D1 + KV 绑定
- `schema.sql` — D1 数据库 schema

### 功能列表
- GitHub OAuth 管理员认证
- 后端配置 CRUD（S3/Dufs/Redis + 连接测试）
- 租户 CRUD（配额进度条 + 启用/禁用）
- 文件记录按租户浏览
- Dashboard 仪表盘（统计卡片 + 健康摘要 + 最近日志）
- 健康探测（全量 + 单后端探测，状态/延迟/错误）
- 审计日志分页查看
- i18n 中英切换
- 可折叠侧边栏 + 用户卡片
- 管理统计 API（/api/admin/stats）
- 健康探测 API（/api/admin/health）
- 审计日志 API（/api/admin/audit-logs）
- 租户端上传/下载/删除代理 API
- Toast 通知 / Modal 对话框 / Light-Dark 主题
