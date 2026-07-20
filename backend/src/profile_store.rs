//! Stockage JSON durable des profils de capacités d'injection par application.

use std::path::{Path, PathBuf};
use crate::protocol::AppProfile;

pub fn default_dir() -> PathBuf {
    crate::recorder::default_dir().join("memories").join("profiles")
}

fn safe_name(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 120 { return Err("nom de contexte vide ou trop long".into()); }
    if value.contains(['/', '\\']) || matches!(value, "." | "..") { return Err("nom de contexte invalide".into()); }
    let clean: String = value.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect();
    if clean.trim_matches('_').is_empty() { return Err("nom de contexte invalide".into()); }
    Ok(clean)
}

pub fn validate(profile: &AppProfile) -> Result<(), String> {
    let context = safe_name(&profile.context)?;
    if context.is_empty() { return Err("contexte de profil vide".into()); }
    Ok(())
}

pub async fn save(profile: &AppProfile) -> Result<PathBuf, String> {
    validate(profile)?;
    let context_clean = safe_name(&profile.context)?;
    let dir = default_dir();
    tokio::fs::create_dir_all(&dir).await.map_err(|e| format!("création {}: {e}", dir.display()))?;
    let path = dir.join(format!("{context_clean}.json"));
    let tmp = dir.join(format!(".{context_clean}.{}.tmp", uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(profile).map_err(|e| format!("sérialisation: {e}"))?;
    tokio::fs::write(&tmp, bytes).await.map_err(|e| format!("écriture {}: {e}", tmp.display()))?;
    if tokio::fs::rename(&tmp, &path).await.is_err() {
        let _ = tokio::fs::remove_file(&path).await;
        tokio::fs::rename(&tmp, &path).await.map_err(|e| format!("installation {}: {e}", path.display()))?;
    }
    Ok(path)
}

pub async fn load(context: &str) -> Result<AppProfile, String> {
    let context_clean = safe_name(context)?;
    let path = default_dir().join(format!("{context_clean}.json"));
    load_path(&path).await
}

pub async fn load_or_default(context: &str) -> AppProfile {
    load(context).await.unwrap_or_else(|_| AppProfile {
        context: context.to_string(),
        foreground_required: false,
        preferred_activation: "auto".to_string(),
        background_mouse: "supported".to_string(),
        background_keyboard: "supported".to_string(),
        pause_on_focus_loss: false,
        default_post_click_delay_ms: None,
        default_key_hold_ms: None,
        default_post_action_delay_ms: None,
    })
}

async fn load_path(path: &Path) -> Result<AppProfile, String> {
    let bytes = tokio::fs::read(path).await.map_err(|e| format!("lecture {}: {e}", path.display()))?;
    let slice = if bytes.starts_with(b"\xEF\xBB\xBF") { &bytes[3..] } else { &bytes[..] };
    let profile: AppProfile = serde_json::from_slice(slice).map_err(|e| format!("profil JSON invalide: {e}"))?;
    validate(&profile)?;
    Ok(profile)
}
