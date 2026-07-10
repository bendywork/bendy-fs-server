use worker::*;

mod admin_handlers;
mod auth;
mod config_store;
mod db;
mod dufs_proxy;
mod github_oauth;
mod redis_proxy;
mod s3_proxy;
mod s3_signer;
mod tenant_auth;
mod tenant_handlers;
mod types;

fn cors_response() -> Result<Response> {
    let mut headers = Headers::new();
    headers.set("Access-Control-Allow-Origin", "*")?;
    headers.set("Access-Control-Allow-Methods", "GET, POST, PUT, DELETE, OPTIONS")?;
    headers.set("Access-Control-Allow-Headers", "Authorization, Content-Type")?;
    headers.set("Access-Control-Max-Age", "86400")?;
    Ok(Response::empty()?.with_headers(headers))
}

fn add_cors(resp: &mut Response) {
    let _ = resp.headers_mut().set("Access-Control-Allow-Origin", "*");
    let _ = resp
        .headers_mut()
        .set("Access-Control-Allow-Methods", "GET, POST, PUT, DELETE, OPTIONS");
    let _ = resp
        .headers_mut()
        .set("Access-Control-Allow-Headers", "Authorization, Content-Type");
}

/// Helper: build a response and add CORS
/// force rebuild v3 - sidebar layout + fix tenant select
fn ok_with_cors<T: serde::Serialize>(data: T, status: u16) -> Result<Response> {
    let mut resp = Response::from_json(&data)?;
    add_cors(&mut resp);
    Ok(resp.with_status(status))
}

#[event(fetch)]
pub async fn main(req: Request, env: Env, _ctx: worker::Context) -> Result<Response> {
    console_error_panic_hook::set_once();

    db::ensure_schema(&env).await?;

    let router = Router::new();

    router
        // OPTIONS — CORS preflight for all routes
        .options("/*rest", |_, _| cors_response())
        // ── Static ──
        .get_async("/admin", |_req, _ctx| async move {
            let html = include_str!("../static/admin.html");
            let mut headers = Headers::new();
            headers.set("Content-Type", "text/html; charset=utf-8")?;
            Ok(Response::from_html(html)?.with_headers(headers))
        })
        // ── Auth: GitHub OAuth ──
        .get_async("/api/auth/github/login", |_req, ctx| async move {
            github_oauth::handle_github_login(&ctx.env).await
        })
        .get_async("/api/github/callback", |req, ctx| async move {
            github_oauth::handle_github_callback(&req, &ctx.env).await
        })
        .get_async("/api/auth/session", |req, ctx| async move {
            let mut resp = github_oauth::handle_get_session(&req, &ctx.env).await?;
            add_cors(&mut resp);
            Ok(resp)
        })
        .post_async("/api/auth/logout", |_req, _ctx| async move {
            let mut resp = github_oauth::handle_logout().await?;
            add_cors(&mut resp);
            Ok(resp)
        })
        // ── Tenant auth ──
        .post_async("/api/t/auth", |req, ctx| async move {
            let mut resp = tenant_auth::handle_tenant_login(req, &ctx.env).await?;
            add_cors(&mut resp);
            Ok(resp)
        })
        // ── Tenant proxy routes ──
        .post_async("/api/t/upload", |req, ctx| async move {
            let mut resp = tenant_handlers::handle_tenant_upload(req, &ctx.env).await?;
            add_cors(&mut resp);
            Ok(resp)
        })
        .post_async("/api/t/presign-upload", |req, ctx| async move {
            let mut resp = tenant_handlers::handle_tenant_presign_upload(req, &ctx.env).await?;
            add_cors(&mut resp);
            Ok(resp)
        })
        .post_async("/api/t/presign-download", |req, ctx| async move {
            let mut resp = tenant_handlers::handle_tenant_presign_download(req, &ctx.env).await?;
            add_cors(&mut resp);
            Ok(resp)
        })
        .get_async("/api/t/download/*key", |req, ctx| async move {
            if let Some(key) = ctx.param("key") {
                let mut resp = tenant_handlers::handle_tenant_download(req, &ctx.env, key).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing key", 400)
            }
        })
        .delete_async("/api/t/delete/*key", |req, ctx| async move {
            if let Some(key) = ctx.param("key") {
                let mut resp = tenant_handlers::handle_tenant_delete(req, &ctx.env, key).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing key", 400)
            }
        })
        .get_async("/api/t/files", |req, ctx| async move {
            let mut resp = tenant_handlers::handle_tenant_list_files(req, &ctx.env).await?;
            add_cors(&mut resp);
            Ok(resp)
        })
        .post_async("/api/t/oss-preview", |req, ctx| async move {
            let mut resp = tenant_handlers::handle_tenant_oss_preview(req, &ctx.env).await?;
            add_cors(&mut resp);
            Ok(resp)
        })
        // ── Admin: config CRUD ──
        .get_async("/api/admin/configs", |req, ctx| async move {
            admin_handlers::handle_list(req, &ctx.env).await
        })
        .post_async("/api/admin/configs", |req, ctx| async move {
            let mut resp = admin_handlers::handle_create(req, &ctx.env).await?;
            add_cors(&mut resp);
            Ok(resp)
        })
        .get_async("/api/admin/configs/:id", |req, ctx| async move {
            if let Some(id) = ctx.param("id") {
                let mut resp = admin_handlers::handle_get(req, &ctx.env, id).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing config id", 400)
            }
        })
        .put_async("/api/admin/configs/:id", |req, ctx| async move {
            if let Some(id) = ctx.param("id") {
                let mut resp = admin_handlers::handle_update(req, &ctx.env, id).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing config id", 400)
            }
        })
        .delete_async("/api/admin/configs/:id", |req, ctx| async move {
            if let Some(id) = ctx.param("id") {
                let mut resp = admin_handlers::handle_delete(req, &ctx.env, id).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing config id", 400)
            }
        })
        .post_async("/api/admin/configs/:id/test", |req, ctx| async move {
            if let Some(id) = ctx.param("id") {
                let mut resp = admin_handlers::handle_test(req, &ctx.env, id).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing config id", 400)
            }
        })
        // ── Admin: tenant CRUD ──
        .get_async("/api/admin/tenants", |req, ctx| async move {
            let mut resp = admin_handlers::handle_list_tenants(req, &ctx.env).await?;
            add_cors(&mut resp);
            Ok(resp)
        })
        .post_async("/api/admin/tenants", |req, ctx| async move {
            let mut resp = admin_handlers::handle_create_tenant(req, &ctx.env).await?;
            add_cors(&mut resp);
            Ok(resp)
        })
        .get_async("/api/admin/tenants/:id", |req, ctx| async move {
            if let Some(id) = ctx.param("id") {
                let mut resp = admin_handlers::handle_get_tenant(req, &ctx.env, id).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing tenant id", 400)
            }
        })
        .put_async("/api/admin/tenants/:id", |req, ctx| async move {
            if let Some(id) = ctx.param("id") {
                let mut resp = admin_handlers::handle_update_tenant(req, &ctx.env, id).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing tenant id", 400)
            }
        })
        .delete_async("/api/admin/tenants/:id", |req, ctx| async move {
            if let Some(id) = ctx.param("id") {
                let mut resp = admin_handlers::handle_delete_tenant(req, &ctx.env, id).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing tenant id", 400)
            }
        })
        .get_async("/api/admin/tenants/:id/files", |req, ctx| async move {
            if let Some(id) = ctx.param("id") {
                let mut resp = admin_handlers::handle_list_tenant_files(req, &ctx.env, id).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing tenant id", 400)
            }
        })
        // ── Admin: dashboard / health / audit ──
        .get_async("/api/admin/stats", |req, ctx| async move {
            let mut resp = admin_handlers::handle_stats(req, &ctx.env).await?;
            add_cors(&mut resp);
            Ok(resp)
        })
        .get_async("/api/admin/health", |req, ctx| async move {
            let mut resp = admin_handlers::handle_health(req, &ctx.env).await?;
            add_cors(&mut resp);
            Ok(resp)
        })
        .get_async("/api/admin/audit-logs", |req, ctx| async move {
            let mut resp = admin_handlers::handle_audit_logs(req, &ctx.env).await?;
            add_cors(&mut resp);
            Ok(resp)
        })
        // ── FS unified proxy routes ──
        .post_async("/api/fs/:id/presign-upload", |req, ctx| async move {
            if let Some(id) = ctx.param("id") {
                let mut resp =
                    admin_handlers::handle_presign_upload(req, &ctx.env, id).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing config id", 400)
            }
        })
        .post_async("/api/fs/:id/presign-download", |req, ctx| async move {
            if let Some(id) = ctx.param("id") {
                let mut resp =
                    admin_handlers::handle_presign_download(req, &ctx.env, id).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing config id", 400)
            }
        })
        .post_async("/api/fs/:id/upload", |req, ctx| async move {
            if let Some(id) = ctx.param("id") {
                let mut resp = admin_handlers::handle_proxy_upload(req, &ctx.env, id).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing config id", 400)
            }
        })
        .get_async("/api/fs/:id/download/*key", |req, ctx| async move {
            if let (Some(id), Some(key)) = (ctx.param("id"), ctx.param("key")) {
                let mut resp =
                    admin_handlers::handle_proxy_download(req, &ctx.env, id, key).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing config id or key", 400)
            }
        })
        .delete_async("/api/fs/:id/delete/*key", |req, ctx| async move {
            if let (Some(id), Some(key)) = (ctx.param("id"), ctx.param("key")) {
                let mut resp =
                    admin_handlers::handle_proxy_delete(req, &ctx.env, id, key).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing config id or key", 400)
            }
        })
        .post_async("/api/fs/:id/set", |req, ctx| async move {
            if let Some(id) = ctx.param("id") {
                let mut resp = admin_handlers::handle_redis_set(req, &ctx.env, id).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing config id", 400)
            }
        })
        // ── Backward compat: /api/s3/ routes (aliased to /api/fs/) ──
        .post_async("/api/s3/:id/presign-upload", |req, ctx| async move {
            if let Some(id) = ctx.param("id") {
                let mut resp =
                    admin_handlers::handle_presign_upload(req, &ctx.env, id).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing config id", 400)
            }
        })
        .post_async("/api/s3/:id/presign-download", |req, ctx| async move {
            if let Some(id) = ctx.param("id") {
                let mut resp =
                    admin_handlers::handle_presign_download(req, &ctx.env, id).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing config id", 400)
            }
        })
        .post_async("/api/s3/:id/upload", |req, ctx| async move {
            if let Some(id) = ctx.param("id") {
                let mut resp = admin_handlers::handle_proxy_upload(req, &ctx.env, id).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing config id", 400)
            }
        })
        .get_async("/api/s3/:id/download/*key", |req, ctx| async move {
            if let (Some(id), Some(key)) = (ctx.param("id"), ctx.param("key")) {
                let mut resp =
                    admin_handlers::handle_proxy_download(req, &ctx.env, id, key).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing config id or key", 400)
            }
        })
        .delete_async("/api/s3/:id/delete/*key", |req, ctx| async move {
            if let (Some(id), Some(key)) = (ctx.param("id"), ctx.param("key")) {
                let mut resp =
                    admin_handlers::handle_proxy_delete(req, &ctx.env, id, key).await?;
                add_cors(&mut resp);
                Ok(resp)
            } else {
                Response::error("Missing config id or key", 400)
            }
        })
        .run(req, env)
        .await
}
