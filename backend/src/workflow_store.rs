//! Validation et stockage JSON des graphes de workflow.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::protocol::*;

pub fn default_dir() -> PathBuf {
    crate::recorder::default_dir().join("memories").join("workflows")
}

fn safe_name(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 120 { return Err("nom de workflow vide ou trop long".into()); }
    if value.contains(['/', '\\']) || matches!(value, "." | "..") { return Err("nom de workflow invalide".into()); }
    let clean: String = value.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect();
    if clean.trim_matches('_').is_empty() { return Err("nom de workflow invalide".into()); }
    Ok(clean)
}

fn allowed_handle(kind: WorkflowNodeType, handle: WorkflowHandle) -> bool {
    matches!((kind, handle),
        (WorkflowNodeType::Start, WorkflowHandle::Next)
        | (WorkflowNodeType::Macro, WorkflowHandle::OnSuccess | WorkflowHandle::OnError)
        | (WorkflowNodeType::Loop, WorkflowHandle::LoopCycle | WorkflowHandle::Out)
        | (WorkflowNodeType::Condition, WorkflowHandle::True | WorkflowHandle::False)
        | (WorkflowNodeType::Wait, WorkflowHandle::WaitDone)
        | (WorkflowNodeType::Activate, WorkflowHandle::OnSuccess | WorkflowHandle::OnError)
        | (WorkflowNodeType::WaitForState, WorkflowHandle::OnSuccess | WorkflowHandle::OnError))
}

fn data_matches(node: &WorkflowNode) -> bool {
    matches!((&node.node_type, &node.data),
        (WorkflowNodeType::Start, WorkflowNodeData::Start(_))
        | (WorkflowNodeType::Macro, WorkflowNodeData::Macro(_))
        | (WorkflowNodeType::Loop, WorkflowNodeData::Loop(_))
        | (WorkflowNodeType::Condition, WorkflowNodeData::Condition(_))
        | (WorkflowNodeType::Wait, WorkflowNodeData::Wait(_))
        | (WorkflowNodeType::Activate, WorkflowNodeData::Activate(_))
        | (WorkflowNodeType::WaitForState, WorkflowNodeData::WaitForState(_)))
}

pub fn validate(graph: &WorkflowGraph) -> Result<(), String> {
    if graph.nodes.is_empty() { return Err("le workflow ne contient aucun nœud".into()); }
    if graph.nodes.len() > 1_000 || graph.edges.len() > 5_000 { return Err("workflow trop volumineux".into()); }
    let starts = graph.nodes.values().filter(|n| n.node_type == WorkflowNodeType::Start).count();
    if starts != 1 { return Err(format!("le workflow doit contenir exactement un START (trouvé: {starts})")); }
    for (key, node) in &graph.nodes {
        if key != &node.id || node.id.trim().is_empty() { return Err(format!("identifiant de nœud incohérent: {key}")); }
        if !node.position.x.is_finite() || !node.position.y.is_finite() { return Err(format!("position invalide: {}", node.id)); }
        if !data_matches(node) { return Err(format!("data incompatible avec le type du nœud {}", node.id)); }
        match &node.data {
            WorkflowNodeData::Macro(d) if d.macro_name.trim().is_empty() => return Err(format!("macro absente: {}", node.id)),
            WorkflowNodeData::Loop(d) if d.repetitions == 0 || d.repetitions > 10_000 => return Err(format!("répétitions invalides: {}", node.id)),
            WorkflowNodeData::Condition(d) if d.expression.trim().is_empty() => return Err(format!("condition vide: {}", node.id)),
            WorkflowNodeData::Wait(d) if d.duration_ms > 3_600_000 => return Err(format!("attente trop longue: {}", node.id)),
            WorkflowNodeData::Activate(d) => {
                if d.element.trim().is_empty() { return Err(format!("élément à activer absent: {}", node.id)); }
                if d.retries > 10 { return Err(format!("tentatives (retries) > 10: {}", node.id)); }
                if d.timeout_ms > 300_000 { return Err(format!("timeout > 300 000 ms: {}", node.id)); }
            }
            WorkflowNodeData::WaitForState(d) => {
                if d.state.trim().is_empty() { return Err(format!("nom d'état absent: {}", node.id)); }
                if d.timeout_ms > 300_000 { return Err(format!("timeout > 300 000 ms: {}", node.id)); }
            }
            _ => {}
        }
    }
    let mut edge_ids = HashSet::new();
    let mut outputs = HashSet::new();
    for edge in &graph.edges {
        if !edge_ids.insert(&edge.id) { return Err(format!("arête dupliquée: {}", edge.id)); }
        let source = graph.nodes.get(&edge.source_node_id).ok_or_else(|| format!("source absente: {}", edge.source_node_id))?;
        if !graph.nodes.contains_key(&edge.target_node_id) { return Err(format!("cible absente: {}", edge.target_node_id)); }
        if !allowed_handle(source.node_type, edge.source_handle) { return Err(format!("handle invalide sur {}", source.id)); }
        if !outputs.insert((edge.source_node_id.clone(), edge.source_handle)) { return Err(format!("plusieurs arêtes sur {:?} de {}", edge.source_handle, edge.source_node_id)); }
    }
    Ok(())
}

pub async fn save(name: &str, graph: WorkflowGraph) -> Result<PathBuf, String> {
    validate(&graph)?;
    let name = safe_name(name)?;
    let file = WorkflowFile { version: 1, name: name.clone(), graph };
    let dir = default_dir();
    tokio::fs::create_dir_all(&dir).await.map_err(|e| format!("création {}: {e}", dir.display()))?;
    let path = dir.join(format!("{name}.json"));
    let tmp = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(&file).map_err(|e| format!("sérialisation: {e}"))?;
    tokio::fs::write(&tmp, bytes).await.map_err(|e| format!("écriture {}: {e}", tmp.display()))?;
    if tokio::fs::rename(&tmp, &path).await.is_err() {
        let _ = tokio::fs::remove_file(&path).await;
        tokio::fs::rename(&tmp, &path).await.map_err(|e| format!("installation {}: {e}", path.display()))?;
    }
    Ok(path)
}

pub async fn load(name: &str) -> Result<WorkflowFile, String> {
    let name = safe_name(name)?;
    load_path(&default_dir().join(format!("{name}.json"))).await
}

async fn load_path(path: &Path) -> Result<WorkflowFile, String> {
    let bytes = tokio::fs::read(path).await.map_err(|e| format!("lecture {}: {e}", path.display()))?;
    let file: WorkflowFile = serde_json::from_slice(&bytes).map_err(|e| format!("workflow JSON invalide: {e}"))?;
    if file.version != 1 { return Err(format!("version workflow non supportée: {}", file.version)); }
    validate(&file.graph)?;
    Ok(file)
}

pub async fn list() -> Result<Vec<WorkflowSummary>, String> {
    let dir = default_dir();
    if tokio::fs::metadata(&dir).await.is_err() { return Ok(Vec::new()); }
    let mut entries = tokio::fs::read_dir(&dir).await.map_err(|e| format!("lecture {}: {e}", dir.display()))?;
    let mut items = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| format!("lecture workflows: {e}"))? {
        if entry.path().extension().and_then(|v| v.to_str()) != Some("json") { continue; }
        match load_path(&entry.path()).await {
            Ok(file) => items.push(WorkflowSummary { name: file.name, nodes: file.graph.nodes.len(), edges: file.graph.edges.len() }),
            Err(e) => tracing::warn!("workflow ignoré {}: {e}", entry.path().display()),
        }
    }
    items.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(items)
}
