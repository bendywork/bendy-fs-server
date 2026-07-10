# plan.md - Bendy FS Server

- [x] Phase 3: Rewrite admin.html — sidebar sections, dashboard, health probes, audit logs, i18n ✅ 2026-07-10
- [ ] Add tenant file preview/delete in admin panel
- [ ] Add tenant rate-limit bypass for admin
- [ ] Rename directory from bendy-s3-server to bendy-fs-server

---

## Phase 3: Admin Panel Rewrite (Detailed Plan)

### Context
Current `admin.html` (961 lines) provides basic CRUD for configs, tenants, and files. Phase 1 & 2 added backend endpoints for stats, health probes, and audit logs. Phase 3 rewrites the admin panel to expose all these features with a polished UI.

### Backend API Endpoints Available
| Endpoint | Method | Response |
|---|---|---|
| `/api/admin/stats` | GET | `{ total_configs, total_tenants, active_tenants, total_files, total_storage_used_bytes }` |
| `/api/admin/health` | GET | `{ results: [{ config_id, config_name, backend_type, status, latency_ms, error? }], ok_count, error_count }` |
| `/api/admin/audit-logs?offset=&limit=` | GET | `{ logs: [{ id, action, username, detail, created_at }], total, offset, limit }` |
| `/api/admin/configs` | GET/POST | CRUD |
| `/api/admin/configs/:id` | GET/PUT/DELETE | Single config |
| `/api/admin/configs/:id/test` | POST | Test backend connection |
| `/api/admin/tenants` | GET/POST | CRUD |
| `/api/admin/tenants/:id` | GET/PUT/DELETE | Single tenant |
| `/api/admin/tenants/:id/files` | GET | Tenant file records |
| `/api/auth/session` | GET | Current session |
| `/api/auth/github/login` | GET | GitHub OAuth login |
| `/api/auth/logout` | POST | Logout |

### What We're Building
Single-file `static/admin.html` (~3000 lines) with Tailwind CDN + vanilla JS, preserving existing glassmorphism CSS theme.

### Implementation Steps

#### 1. CSS Foundation (preserve + extend)
- Keep ALL existing CSS custom properties and classes
- Add new classes for: collapsible sidebar sections, user card, pagination, health status indicators, i18n toggle

#### 2. HTML Shell Layout
```
┌──────────────────────────────────────────┐
│ Header: Logo | i18n toggle | Theme toggle │
├──────────┬───────────────────────────────┤
│ Sidebar  │ Main Content Area             │
│ ─────── │                                 │
│ OVERVIEW │ (switches per sidebar tab)     │
│  Dashboard│                                │
│           │                                 │
│ SYSTEM   │                                 │
│  Configs │                                 │
│  Tenants │                                 │
│  Files   │                                 │
│           │                                 │
│ MONITOR  │                                 │
│  Health  │                                 │
│  Audit   │                                 │
│ ─────── │                                 │
│ User Card│                                 │
└──────────┴───────────────────────────────┘
```

#### 3. Sidebar Sections (collapsible)
- Overview: Dashboard
- System Settings: Configs, Tenants, File Records
- Monitoring: Health Probes, Audit Logs
- User Card at bottom: avatar placeholder, username, logout dropdown

#### 4. Dashboard Tab
- 4 stat cards: Total Configs, Total Tenants (active/total), Total Files, Storage Used
- Health summary bar
- Recent 5 audit logs, "View All" link

#### 5. Configs Tab (enhance existing)
- Card-list layout + create/edit modal + Refresh button + loading spinner

#### 6. Tenants Tab (enhance existing)
- Card layout with progress bars + Refresh button

#### 7. Files Tab (enhance existing)
- Table layout + Refresh button + tenant name in header

#### 8. Health Probes Tab (NEW)
- Per-backend status cards with status/latency/error
- "Probe All" button + per-card probe button

#### 9. Audit Logs Tab (NEW)
- Table with pagination (Previous/Next) + Refresh button + total count

#### 10. i18n System (NEW)
- zh/en dictionaries covering all UI strings
- Toggle button in header, persist to localStorage
- `t(key)` function

#### 11. Preserved Utilities (keep exactly)
- `api()`, `toast()`, `esc()`, `fmt()`, `fmtTs()`, `badge()`
- GitHub OAuth login flow
- Theme toggle with localStorage
- Modal patterns (config, tenant, delete confirm)

#### 12. JavaScript Architecture
- Init: checkSession → showLogin or showApp
- Tab switching: switchTab(name)
- Each tab: loadXxx() + renderXxx()
- Health: probeAll() + probeOne(configId)
- Audit: loadAuditLogs(offset) with pagination
- i18n: setLang(lang) re-renders visible text

### Verification
1. Build: `wasm-pack build --target web --out-dir build --no-opt`
2. Run: `wrangler dev`
3. Test: login → dashboard → all tabs → CRUD → health probes → audit logs → i18n → theme → logout
