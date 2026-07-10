use worker::*;

use crate::types::BackendConfig;

const KV_BINDING: &str = "BENDY_FS_CONFIGS";
const CONFIG_INDEX_KEY: &str = "__config_index__";

fn configs_kv(env: &Env) -> std::result::Result<worker::kv::KvStore, worker::Error> {
    env.kv(KV_BINDING)
}

async fn load_index(env: &Env) -> Result<Vec<String>> {
    let kv = configs_kv(env)?;
    match kv.get(CONFIG_INDEX_KEY).text().await? {
        Some(text) if !text.is_empty() => {
            Ok(serde_json::from_str(&text).unwrap_or_default())
        }
        _ => Ok(Vec::new()),
    }
}

async fn save_index(env: &Env, ids: &[String]) -> Result<()> {
    let kv = configs_kv(env)?;
    kv.put(CONFIG_INDEX_KEY, serde_json::to_string(ids)?)?
        .execute()
        .await?;
    Ok(())
}

pub async fn list_configs(env: &Env) -> Result<Vec<BackendConfig>> {
    let ids = load_index(env).await?;
    let kv = configs_kv(env)?;
    let mut configs = Vec::new();

    for id in &ids {
        if let Some(text) = kv.get(id).text().await? {
            if let Ok(cfg) = serde_json::from_str::<BackendConfig>(&text) {
                configs.push(cfg);
            }
        }
    }

    configs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(configs)
}

pub async fn get_config(env: &Env, id: &str) -> Result<Option<BackendConfig>> {
    let kv = configs_kv(env)?;
    match kv.get(id).text().await? {
        Some(text) => Ok(serde_json::from_str(&text).ok()),
        None => Ok(None),
    }
}

pub async fn create_config(env: &Env, config: BackendConfig) -> Result<BackendConfig> {
    let kv = configs_kv(env)?;
    let json = serde_json::to_string(&config)?;
    kv.put(&config.id, json)?.execute().await?;

    let mut ids = load_index(env).await?;
    ids.push(config.id.clone());
    save_index(env, &ids).await?;

    Ok(config)
}

pub async fn update_config(env: &Env, id: &str, updated: BackendConfig) -> Result<Option<BackendConfig>> {
    let kv = configs_kv(env)?;

    if get_config(env, id).await?.is_none() {
        return Ok(None);
    }

    let json = serde_json::to_string(&updated)?;
    kv.put(id, json)?.execute().await?;

    Ok(Some(updated))
}

pub async fn delete_config(env: &Env, id: &str) -> Result<bool> {
    let kv = configs_kv(env)?;

    if get_config(env, id).await?.is_none() {
        return Ok(false);
    }

    kv.delete(id).await?;

    let mut ids = load_index(env).await?;
    ids.retain(|i| i != id);
    save_index(env, &ids).await?;

    Ok(true)
}
