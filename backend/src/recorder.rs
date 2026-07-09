//! Sauvegarde des frames JPEG sur disque : capture unique (`Screenshot`) ou
//! séquence d'images (`StartRecording`/`StopRecording`).
//!
//! Les frames sont déjà encodées en JPEG par le pipeline de capture (canal
//! `watch`) : on ne fait qu'écrire les octets, aucune ré-encodage. La séquence
//! suit la même philosophie latest-frame-wins que le streaming — on écrit la
//! frame la plus récente à chaque changement, plafonné à `fps`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

/// Dossier par défaut : `<Images>/appMirror` (ou `<home>/appMirror` à défaut).
pub fn default_dir() -> PathBuf {
    dirs::picture_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("appMirror")
}

/// Résout le dossier demandé (absent ou vide => défaut) et le crée.
pub fn resolve_dir(custom: Option<&str>) -> anyhow::Result<PathBuf> {
    let dir = match custom {
        Some(s) if !s.trim().is_empty() => PathBuf::from(s.trim()),
        _ => default_dir(),
    };
    std::fs::create_dir_all(&dir)
        .map_err(|e| anyhow::anyhow!("création du dossier {}: {e}", dir.display()))?;
    Ok(dir)
}

fn timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d_%H-%M-%S-%3f").to_string()
}

/// Écrit la frame JPEG `bytes` dans `dir` ; retourne le chemin du fichier.
pub async fn save_screenshot(dir: &Path, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let path = dir.join(format!("capture_{}.jpg", timestamp()));
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|e| anyhow::anyhow!("écriture {}: {e}", path.display()))?;
    Ok(path)
}

/// Enregistrement en cours : tâche d'écriture + compteur de frames écrites.
pub struct Recording {
    task: tokio::task::JoinHandle<()>,
    dir: PathBuf,
    count: Arc<AtomicU64>,
}

impl Recording {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Arrête l'enregistrement ; retourne (dossier, nombre de frames).
    pub fn stop(self) -> (PathBuf, u64) {
        self.task.abort();
        (self.dir, self.count.load(Ordering::Relaxed))
    }
}

/// Démarre une séquence d'images dans `<base_dir>/rec_<horodatage>/`.
///
/// Écrit `frame_NNNNNN.jpg` à chaque nouvelle frame reçue, au plus `fps` fois
/// par seconde. La tâche se termine d'elle-même quand la capture s'arrête
/// (émetteur `watch` fermé).
pub fn start_recording(
    base_dir: &Path,
    fps: u32,
    mut rx: watch::Receiver<Vec<u8>>,
) -> anyhow::Result<(Recording, u32)> {
    let fps = fps.clamp(1, 60);
    let dir = base_dir.join(format!("rec_{}", timestamp()));
    std::fs::create_dir_all(&dir)
        .map_err(|e| anyhow::anyhow!("création du dossier {}: {e}", dir.display()))?;

    let count = Arc::new(AtomicU64::new(0));
    let count_task = count.clone();
    let dir_task = dir.clone();

    let task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis((1000 / fps).max(1) as u64));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            // Attend une nouvelle frame ; se termine quand la capture s'arrête.
            if rx.changed().await.is_err() {
                break;
            }
            let frame = rx.borrow_and_update().clone();
            if frame.is_empty() {
                continue;
            }
            let n = count_task.fetch_add(1, Ordering::Relaxed);
            let path = dir_task.join(format!("frame_{n:06}.jpg"));
            if let Err(e) = tokio::fs::write(&path, &frame).await {
                tracing::warn!("écriture frame échouée ({}): {e}", path.display());
            }
            // Plafonne la cadence d'écriture à `fps`.
            interval.tick().await;
        }
        tracing::info!("enregistrement terminé (source fermée)");
    });

    Ok((
        Recording {
            task,
            dir,
            count,
        },
        fps,
    ))
}
