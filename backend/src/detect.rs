//! Détection d'état applicatif : compare la frame courante aux signatures de
//! zones (dHash + luminance) des états enregistrés pour un contexte.

use serde_json::{json, Value};

use crate::protocol::{NormalizedBounds, StateSignature, UiStateFile};

/// Seuils de correspondance d'une zone : distance de Hamming sur le dHash 8×8
/// et écart de luminance moyenne. Une zone matche si les deux sont sous seuil.
const MAX_HAMMING: u32 = 10;
const MAX_LUMA_DIFF: f64 = 18.0;
/// Score minimal (fraction pondérée de zones qui matchent) pour déclarer un état.
const MIN_CONFIDENCE: f64 = 0.75;

/// dHash 8×8 d'une image (64 bits) : gradients horizontaux sur une réduction 9×8.
pub fn dhash8(img: &image::DynamicImage) -> u64 {
    let small = img.resize_exact(9, 8, image::imageops::FilterType::Triangle).to_luma8();
    let mut bits = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            bits = (bits << 1) | u64::from(small.get_pixel(x, y)[0] > small.get_pixel(x + 1, y)[0]);
        }
    }
    bits
}

/// Crop d'une image selon des bounds normalisés (au moins 1×1 px).
pub fn crop_normalized(img: &image::DynamicImage, bounds: &NormalizedBounds) -> image::DynamicImage {
    let (w, h) = (img.width(), img.height());
    let zx = ((bounds.x * w as f64) as u32).min(w.saturating_sub(1));
    let zy = ((bounds.y * h as f64) as u32).min(h.saturating_sub(1));
    let zw = ((bounds.width * w as f64).ceil() as u32).clamp(1, w - zx);
    let zh = ((bounds.height * h as f64).ceil() as u32).clamp(1, h - zy);
    img.crop_imm(zx, zy, zw, zh)
}

/// Signature d'une zone sur une image décodée : (dHash, luminance moyenne).
pub fn zone_signature(img: &image::DynamicImage, bounds: &NormalizedBounds) -> (u64, f64) {
    let crop = crop_normalized(img, bounds);
    let dhash = dhash8(&crop);
    let luma = crop.resize_exact(16, 16, image::imageops::FilterType::Triangle).to_luma8();
    let total: u64 = luma.as_raw().iter().map(|v| u64::from(*v)).sum();
    let mean = total as f64 / luma.as_raw().len().max(1) as f64;
    (dhash, mean)
}

/// Score d'un état sur une image : moyenne des similarités de zones, où une
/// zone hors seuil compte 0. `zones_matched` = zones sous les deux seuils.
pub struct StateScore {
    pub name: String,
    pub layout: Option<String>,
    pub score: f64,
    pub zones_matched: usize,
    pub zones_total: usize,
}

fn signature_similarity(img: &image::DynamicImage, sig: &StateSignature) -> Option<f64> {
    let expected = u64::from_str_radix(sig.dhash.trim(), 16).ok()?;
    let (actual, mean) = zone_signature(img, &sig.bounds);
    let hamming = (expected ^ actual).count_ones();
    let luma_diff = (mean - sig.mean_luma).abs();
    if hamming > MAX_HAMMING || luma_diff > MAX_LUMA_DIFF {
        return None;
    }
    Some(1.0 - f64::from(hamming) / 64.0)
}

pub fn score_states(jpeg: &[u8], states: &[UiStateFile]) -> Result<Vec<StateScore>, String> {
    let img = image::load_from_memory(jpeg).map_err(|e| format!("décodage frame: {e}"))?;
    let mut scores: Vec<StateScore> = states
        .iter()
        .map(|state| {
            let mut total = 0.0;
            let mut matched = 0usize;
            for sig in &state.signatures {
                if let Some(similarity) = signature_similarity(&img, sig) {
                    total += similarity;
                    matched += 1;
                }
            }
            StateScore {
                name: state.name.clone(),
                layout: state.layout.clone(),
                score: total / state.signatures.len().max(1) as f64,
                zones_matched: matched,
                zones_total: state.signatures.len(),
            }
        })
        .collect();
    scores.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scores)
}

/// Détecte l'état le plus probable. `None` si aucun score n'atteint le seuil.
pub fn detect(jpeg: &[u8], states: &[UiStateFile]) -> Result<(Option<StateScore>, Vec<StateScore>), String> {
    let mut scores = score_states(jpeg, states)?;
    let best = if scores.first().is_some_and(|s| s.score >= MIN_CONFIDENCE) {
        Some(scores.remove(0))
    } else {
        None
    };
    Ok((best, scores))
}

/// Attend qu'un état soit détecté sur une fenêtre (sondage ~4 Hz sur une
/// session de capture dédiée). Renvoie le temps écoulé en ms.
pub async fn wait_for_state_on_window(
    window_id: isize,
    states: &[UiStateFile],
    wanted: &str,
    timeout_ms: u64,
) -> Result<u64, String> {
    if !states.iter().any(|s| s.name == wanted) {
        let known: Vec<&str> = states.iter().map(|s| s.name.as_str()).collect();
        return Err(format!("état '{wanted}' inconnu; états connus: {}", known.join(", ")));
    }
    let timeout_ms = timeout_ms.clamp(250, 300_000);
    let started = std::time::Instant::now();
    let (session, rx) = crate::capture::start(window_id, 80, 1.0, false)
        .map_err(|e| format!("impossible de démarrer la capture: {e}"))?;
    let result = loop {
        let frame = rx.borrow().clone();
        if !frame.is_empty() {
            let (best, _) = detect(&frame, states)?;
            if best.is_some_and(|s| s.name == wanted) {
                break Ok(started.elapsed().as_millis() as u64);
            }
        }
        if started.elapsed().as_millis() as u64 >= timeout_ms {
            break Err(format!("état '{wanted}' non détecté après {timeout_ms} ms"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    };
    let _ = session.stop();
    result
}

/// Vue JSON d'un score, pour les rapports d'outils MCP.
pub fn score_view(score: &StateScore) -> Value {
    json!({
        "state": score.name,
        "layout": score.layout,
        "confidence": (score.score * 1000.0).round() / 1000.0,
        "zonesMatched": score.zones_matched,
        "zonesTotal": score.zones_total,
    })
}
