//! Persistance des états applicatifs nommés (signatures visuelles de zones).
//! Même patron que `layout_store` : un fichier JSON par état, écrit
//! atomiquement dans `memories/states/<context>__<name>.json`.

use std::path::{Path, PathBuf};

use crate::protocol::{UiStateFile, UiStateSummary};

pub fn default_dir() -> PathBuf {
    crate::recorder::default_dir().join("memories").join("states")
}

fn safe_component(value: &str, label: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() { return Err(format!("{label} vide")); }
    if value.len() > 120 { return Err(format!("{label} trop long")); }
    if value.contains(['/', '\\']) || value == "." || value == ".." {
        return Err(format!("{label} contient un séparateur de chemin interdit"));
    }
    let clean: String = value.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    if clean.trim_matches('_').is_empty() {
        return Err(format!("{label} ne contient aucun caractère utilisable"));
    }
    Ok(clean)
}

fn validate(file: &UiStateFile) -> Result<(), String> {
    if file.version != 1 { return Err(format!("version d'état non supportée: {}", file.version)); }
    if file.signatures.is_empty() { return Err("un état requiert au moins une signature de zone".into()); }
    if file.signatures.len() > 500 { return Err("l'état dépasse 500 signatures".into()); }
    for (index, sig) in file.signatures.iter().enumerate() {
        if sig.zone_id.trim().is_empty() { return Err(format!("signature {}: zoneId vide", index + 1)); }
        if u64::from_str_radix(sig.dhash.trim(), 16).is_err() {
            return Err(format!("signature {}: dhash non hexadécimal", index + 1));
        }
        if !sig.mean_luma.is_finite() || !(0.0..=255.0).contains(&sig.mean_luma) {
            return Err(format!("signature {}: meanLuma hors de [0,255]", index + 1));
        }
        let b = sig.bounds;
        for (label, value) in [("x", b.x), ("y", b.y), ("width", b.width), ("height", b.height)] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(format!("signature {}: {label} hors de [0,1]", index + 1));
            }
        }
        if b.width <= 0.0 || b.height <= 0.0 {
            return Err(format!("signature {}: dimensions nulles", index + 1));
        }
    }
    Ok(())
}

pub async fn save(mut file: UiStateFile) -> Result<PathBuf, String> {
    let context_key = safe_component(&file.context.to_lowercase(), "context")?;
    let name_key = safe_component(&file.name, "name")?;
    file.context = file.context.trim().to_string();
    file.name = file.name.trim().to_string();
    validate(&file)?;
    let dir = default_dir();
    tokio::fs::create_dir_all(&dir).await.map_err(|e| format!("création {}: {e}", dir.display()))?;
    let path = dir.join(format!("{context_key}__{name_key}.json"));
    let tmp = dir.join(format!(".{context_key}__{name_key}.{}.tmp", uuid::Uuid::new_v4()));
    let json = serde_json::to_vec_pretty(&file).map_err(|e| format!("sérialisation état: {e}"))?;
    tokio::fs::write(&tmp, json).await.map_err(|e| format!("écriture {}: {e}", tmp.display()))?;
    if tokio::fs::rename(&tmp, &path).await.is_err() {
        let _ = tokio::fs::remove_file(&path).await;
        tokio::fs::rename(&tmp, &path).await.map_err(|e| format!("installation {}: {e}", path.display()))?;
    }
    Ok(path)
}

pub async fn list(context: Option<&str>) -> Result<Vec<UiStateSummary>, String> {
    let context = context.map(|v| safe_component(&v.to_lowercase(), "context")).transpose()?;
    let dir = default_dir();
    if tokio::fs::metadata(&dir).await.is_err() { return Ok(Vec::new()); }
    let mut entries = tokio::fs::read_dir(&dir).await.map_err(|e| format!("lecture {}: {e}", dir.display()))?;
    let mut items = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| format!("lecture états: {e}"))? {
        if entry.path().extension().and_then(|v| v.to_str()) != Some("json") { continue; }
        match load_path(&entry.path()).await {
            Ok(file) if context.as_ref().is_none_or(|wanted| {
                safe_component(&file.context.to_lowercase(), "context")
                    .map(|key| &key == wanted)
                    .unwrap_or(false)
            }) => {
                items.push(UiStateSummary {
                    context: file.context,
                    name: file.name,
                    layout: file.layout,
                    signatures: file.signatures.len(),
                });
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("état ignoré {}: {e}", entry.path().display()),
        }
    }
    items.sort_by(|a, b| a.context.cmp(&b.context).then_with(|| a.name.cmp(&b.name)));
    Ok(items)
}

pub async fn load_context(context: &str) -> Result<Vec<UiStateFile>, String> {
    let wanted = safe_component(&context.to_lowercase(), "context")?;
    let dir = default_dir();
    if tokio::fs::metadata(&dir).await.is_err() { return Ok(Vec::new()); }
    let mut entries = tokio::fs::read_dir(&dir).await.map_err(|e| format!("lecture {}: {e}", dir.display()))?;
    let mut states = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| format!("lecture états: {e}"))? {
        if entry.path().extension().and_then(|v| v.to_str()) != Some("json") { continue; }
        match load_path(&entry.path()).await {
            Ok(file) => {
                let matches = safe_component(&file.context.to_lowercase(), "context")
                    .map(|key| key == wanted)
                    .unwrap_or(false);
                if matches { states.push(file); }
            }
            Err(e) => tracing::warn!("état ignoré {}: {e}", entry.path().display()),
        }
    }
    states.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(states)
}

pub async fn delete(context: &str, name: &str) -> Result<(), String> {
    let context = safe_component(&context.to_lowercase(), "context")?;
    let name = safe_component(name, "name")?;
    let path = default_dir().join(format!("{context}__{name}.json"));
    tokio::fs::remove_file(&path).await.map_err(|e| format!("suppression {}: {e}", path.display()))
}

async fn load_path(path: &Path) -> Result<UiStateFile, String> {
    let bytes = tokio::fs::read(path).await.map_err(|e| format!("lecture {}: {e}", path.display()))?;
    // Tolère un BOM UTF-8 en tête (fichiers édités sous Windows).
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    let file: UiStateFile = serde_json::from_slice(bytes).map_err(|e| format!("JSON d'état invalide: {e}"))?;
    validate(&file)?;
    Ok(file)
}
