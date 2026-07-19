//! Stockage JSON durable des workflows enregistrés.

use std::path::{Path, PathBuf};

use crate::protocol::{MacroFile, MacroStep, MacroSummary, SaveMacroArgs};

pub fn default_dir() -> PathBuf {
    crate::recorder::default_dir().join("memories").join("macros")
}

fn safe_component(value: &str, label: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{label} ne peut pas être vide"));
    }
    if value.len() > 120 {
        return Err(format!("{label} est trop long"));
    }
    if value.contains(['/', '\\']) || value == "." || value == ".." {
        return Err(format!("{label} contient un séparateur de chemin interdit"));
    }
    let clean: String = value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    if clean.trim_matches('_').is_empty() {
        return Err(format!("{label} ne contient aucun caractère utilisable"));
    }
    Ok(clean)
}

fn validate_coord(value: f64, label: &str) -> Result<(), String> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(format!("coordonnée {label} hors de [0,1]"))
    }
}

fn validate_step(step: &MacroStep) -> Result<(), String> {
    match step {
        MacroStep::MouseMove { x, y }
        | MacroStep::MouseButton { x, y, .. }
        | MacroStep::Wheel { x, y, .. } => {
            validate_coord(*x, "x")?;
            validate_coord(*y, "y")
        }
        MacroStep::Key { code, .. } if code.trim().is_empty() => Err("code clavier vide".into()),
        MacroStep::Text { text } if text.is_empty() => Err("texte vide".into()),
        _ => Ok(()),
    }
}

pub async fn save(args: SaveMacroArgs) -> Result<PathBuf, String> {
    if args.steps.is_empty() {
        return Err("la macro ne contient aucune étape".into());
    }
    if args.steps.len() > 10_000 {
        return Err("la macro dépasse 10 000 étapes".into());
    }
    for (index, step) in args.steps.iter().enumerate() {
        validate_step(step).map_err(|e| format!("étape {}: {e}", index + 1))?;
    }
    let context = safe_component(&args.context.to_lowercase(), "context")?;
    let name = safe_component(&args.name, "name")?;
    let file = MacroFile { version: 1, context: context.clone(), name: name.clone(), window_id: args.window_id, steps: args.steps };
    let dir = default_dir();
    tokio::fs::create_dir_all(&dir).await.map_err(|e| format!("création {}: {e}", dir.display()))?;
    let path = dir.join(format!("{context}__{name}.json"));
    let tmp = dir.join(format!(".{context}__{name}.{}.tmp", uuid::Uuid::new_v4()));
    let json = serde_json::to_vec_pretty(&file).map_err(|e| format!("sérialisation macro: {e}"))?;
    tokio::fs::write(&tmp, json).await.map_err(|e| format!("écriture {}: {e}", tmp.display()))?;
    if tokio::fs::rename(&tmp, &path).await.is_err() {
        let _ = tokio::fs::remove_file(&path).await;
        tokio::fs::rename(&tmp, &path).await.map_err(|e| format!("installation {}: {e}", path.display()))?;
    }
    Ok(path)
}

pub async fn load_by_name(name: &str) -> Result<MacroFile, String> {
    let name = safe_component(name, "name")?;
    let dir = default_dir();
    let mut entries = tokio::fs::read_dir(&dir).await.map_err(|e| format!("lecture {}: {e}", dir.display()))?;
    let suffix = format!("__{name}.json");
    let mut matches = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| format!("lecture macros: {e}"))? {
        let filename = entry.file_name().to_string_lossy().to_string();
        if filename.ends_with(&suffix) { matches.push(entry.path()); }
    }
    match matches.len() {
        0 => Err(format!("macro '{name}' introuvable")),
        1 => load(&matches[0]).await,
        _ => Err(format!("plusieurs contextes définissent '{name}'; utiliser un nom unique")),
    }
}

pub async fn load_named(context: &str, name: &str) -> Result<MacroFile, String> {
    let context = safe_component(&context.to_lowercase(), "context")?;
    let name = safe_component(name, "name")?;
    load(&default_dir().join(format!("{context}__{name}.json"))).await
}

pub async fn list() -> Result<Vec<MacroSummary>, String> {
    let dir = default_dir();
    if tokio::fs::metadata(&dir).await.is_err() { return Ok(Vec::new()); }
    let mut entries = tokio::fs::read_dir(&dir).await.map_err(|e| format!("lecture {}: {e}", dir.display()))?;
    let mut items = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| format!("lecture macros: {e}"))? {
        if entry.path().extension().and_then(|v| v.to_str()) != Some("json") { continue; }
        match load(&entry.path()).await {
            Ok(file) => items.push(MacroSummary { context: file.context, name: file.name, steps: file.steps.len() }),
            Err(e) => tracing::warn!("macro ignorée {}: {e}", entry.path().display()),
        }
    }
    items.sort_by(|a, b| a.context.cmp(&b.context).then_with(|| a.name.cmp(&b.name)));
    Ok(items)
}

async fn load(path: &Path) -> Result<MacroFile, String> {
    let bytes = tokio::fs::read(path).await.map_err(|e| format!("lecture {}: {e}", path.display()))?;
    let file: MacroFile = serde_json::from_slice(&bytes).map_err(|e| format!("JSON macro invalide: {e}"))?;
    if file.version != 1 { return Err(format!("version macro non supportée: {}", file.version)); }
    for (i, step) in file.steps.iter().enumerate() { validate_step(step).map_err(|e| format!("étape {}: {e}", i + 1))?; }
    Ok(file)
}
