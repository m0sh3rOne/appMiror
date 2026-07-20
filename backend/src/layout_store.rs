//! Persistance des plans d'interface utilisateur créés dans le module Layout.

use std::path::{Path, PathBuf};

use crate::protocol::{ActivationStep, LayoutFile, LayoutSummary, LayoutZone, SaveLayoutArgs};

pub fn default_dir() -> PathBuf {
    crate::recorder::default_dir().join("memories").join("layouts")
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

fn validate_zone(zone: &LayoutZone, index: usize) -> Result<(), String> {
    if zone.id.trim().is_empty() { return Err(format!("zone {}: id vide", index + 1)); }
    if zone.name.trim().is_empty() { return Err(format!("zone {}: nom vide", index + 1)); }
    let b = zone.bounds;
    for (label, value) in [("x", b.x), ("y", b.y), ("width", b.width), ("height", b.height)] {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(format!("zone {}: {label} hors de [0,1]", index + 1));
        }
    }
    if b.width <= 0.0 || b.height <= 0.0 {
        return Err(format!("zone {}: dimensions nulles", index + 1));
    }
    const EPSILON: f64 = 1e-6;
    if b.x + b.width > 1.0 + EPSILON || b.y + b.height > 1.0 + EPSILON {
        return Err(format!("zone {}: limites hors de la surface", index + 1));
    }
    for (field, value) in [("label", &zone.label), ("expectedState", &zone.expected_state)] {
        if let Some(value) = value {
            if value.trim().is_empty() { return Err(format!("zone {}: {field} vide (omettre plutôt)", index + 1)); }
            if value.len() > 120 { return Err(format!("zone {}: {field} trop long", index + 1)); }
        }
    }
    if let Some(steps) = &zone.activation {
        if steps.is_empty() { return Err(format!("zone {}: séquence d'activation vide (omettre plutôt)", index + 1)); }
        if steps.len() > 10 { return Err(format!("zone {}: plus de 10 étapes d'activation", index + 1)); }
        for step in steps {
            match step {
                ActivationStep::Key { code, hold_ms, .. } | ActivationStep::FocusThenKey { code, hold_ms, .. } => {
                    if code.trim().is_empty() || code.len() > 40 {
                        return Err(format!("zone {}: code de touche invalide", index + 1));
                    }
                    if hold_ms.is_some_and(|v| v > 5_000) {
                        return Err(format!("zone {}: holdMs > 5 000", index + 1));
                    }
                }
                ActivationStep::WaitForState { state, timeout_ms } => {
                    if state.trim().is_empty() {
                        return Err(format!("zone {}: waitForState sans état", index + 1));
                    }
                    if timeout_ms.is_some_and(|v| v > 60_000) {
                        return Err(format!("zone {}: timeoutMs > 60 000", index + 1));
                    }
                }
                ActivationStep::WaitMs { duration_ms } if *duration_ms > 60_000 => {
                    return Err(format!("zone {}: durationMs > 60 000", index + 1));
                }
                _ => {}
            }
            if step.delay_after_ms().is_some_and(|v| v > 60_000) {
                return Err(format!("zone {}: delayAfterMs > 60 000", index + 1));
            }
        }
    }
    if let Some(anchor) = &zone.anchor {
        for (field, value) in [("dx", anchor.dx), ("dy", anchor.dy)] {
            if !value.is_finite() || !(-1.0..=1.0).contains(&value) {
                return Err(format!("zone {}: anchor.{field} hors de [-1,1]", index + 1));
            }
        }
    }
    Ok(())
}

/// Les ancres doivent référencer une zone existante, jamais elles-mêmes,
/// et la chaîne d'ancrage ne doit pas boucler.
fn validate_anchors(zones: &[LayoutZone]) -> Result<(), String> {
    for (index, zone) in zones.iter().enumerate() {
        let Some(anchor) = &zone.anchor else { continue };
        if anchor.relative_to == zone.id {
            return Err(format!("zone {}: ancre sur elle-même", index + 1));
        }
        let mut current = anchor.relative_to.as_str();
        let mut hops = 0usize;
        loop {
            let Some(target) = zones.iter().find(|z| z.id == current) else {
                return Err(format!("zone {}: ancre vers une zone inconnue ({current})", index + 1));
            };
            match &target.anchor {
                None => break,
                Some(next) => {
                    hops += 1;
                    if next.relative_to == zone.id || hops > zones.len() {
                        return Err(format!("zone {}: chaîne d'ancres cyclique", index + 1));
                    }
                    current = next.relative_to.as_str();
                }
            }
        }
    }
    Ok(())
}

fn validate(file: &LayoutFile) -> Result<(), String> {
    if file.version != 1 { return Err(format!("version layout non supportée: {}", file.version)); }
    if file.source_width == 0 || file.source_height == 0 {
        return Err("résolution source invalide".into());
    }
    if file.zones.len() > 2_000 { return Err("le layout dépasse 2 000 zones".into()); }
    for (index, zone) in file.zones.iter().enumerate() { validate_zone(zone, index)?; }
    validate_anchors(&file.zones)?;
    Ok(())
}

pub async fn save(args: SaveLayoutArgs) -> Result<PathBuf, String> {
    let context_key = safe_component(&args.context.to_lowercase(), "context")?;
    let name_key = safe_component(&args.name, "name")?;
    let context = args.context.trim().to_string();
    let name = args.name.trim().to_string();
    let file = LayoutFile {
        version: 1,
        context: context.clone(),
        name: name.clone(),
        source_width: args.source_width,
        source_height: args.source_height,
        zones: args.zones,
    };
    validate(&file)?;
    let dir = default_dir();
    tokio::fs::create_dir_all(&dir).await.map_err(|e| format!("création {}: {e}", dir.display()))?;
    let path = dir.join(format!("{context_key}__{name_key}.json"));
    let tmp = dir.join(format!(".{context_key}__{name_key}.{}.tmp", uuid::Uuid::new_v4()));
    let json = serde_json::to_vec_pretty(&file).map_err(|e| format!("sérialisation layout: {e}"))?;
    tokio::fs::write(&tmp, json).await.map_err(|e| format!("écriture {}: {e}", tmp.display()))?;
    if tokio::fs::rename(&tmp, &path).await.is_err() {
        let _ = tokio::fs::remove_file(&path).await;
        tokio::fs::rename(&tmp, &path).await.map_err(|e| format!("installation {}: {e}", path.display()))?;
    }
    Ok(path)
}

pub async fn list(context: Option<&str>) -> Result<Vec<LayoutSummary>, String> {
    let context = context.map(|v| safe_component(&v.to_lowercase(), "context")).transpose()?;
    let dir = default_dir();
    if tokio::fs::metadata(&dir).await.is_err() { return Ok(Vec::new()); }
    let mut entries = tokio::fs::read_dir(&dir).await.map_err(|e| format!("lecture {}: {e}", dir.display()))?;
    let mut items = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| format!("lecture layouts: {e}"))? {
        if entry.path().extension().and_then(|v| v.to_str()) != Some("json") { continue; }
        match load_path(&entry.path()).await {
            Ok(file) if context.as_ref().is_none_or(|wanted| {
                safe_component(&file.context.to_lowercase(), "context")
                    .map(|key| &key == wanted)
                    .unwrap_or(false)
            }) => {
                items.push(LayoutSummary { context: file.context, name: file.name, zones: file.zones.len() });
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("layout ignoré {}: {e}", entry.path().display()),
        }
    }
    items.sort_by(|a, b| a.context.cmp(&b.context).then_with(|| a.name.cmp(&b.name)));
    Ok(items)
}

pub async fn load_named(context: &str, name: &str) -> Result<LayoutFile, String> {
    let context = safe_component(&context.to_lowercase(), "context")?;
    let name = safe_component(name, "name")?;
    load_path(&default_dir().join(format!("{context}__{name}.json"))).await
}

pub async fn load_context(context: &str) -> Result<Vec<LayoutFile>, String> {
    let summaries = list(Some(context)).await?;
    let mut layouts = Vec::with_capacity(summaries.len());
    for summary in summaries {
        layouts.push(load_named(&summary.context, &summary.name).await?);
    }
    Ok(layouts)
}

async fn load_path(path: &Path) -> Result<LayoutFile, String> {
    let bytes = tokio::fs::read(path).await.map_err(|e| format!("lecture {}: {e}", path.display()))?;
    // Tolère un BOM UTF-8 en tête (fichiers édités sous Windows).
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    let file: LayoutFile = serde_json::from_slice(bytes).map_err(|e| format!("JSON layout invalide: {e}"))?;
    validate(&file)?;
    Ok(file)
}
