//! Activation sémantique des zones de layout : stratégies de repli et
//! vérification visuelle par hash perceptuel de la frame avant/après.
//!
//! C'est le cœur de `activate_ui_element` (MCP). La détection d'état complète
//! (phase 3) branchera ici ; en attendant, la vérification est un détecteur de
//! changement d'écran : dHash + différence moyenne de luminance sur la frame.

use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::watch;

use crate::app::AttachedWindow;
use crate::protocol::{ActivationStep, LayoutFile, LayoutZone, MouseButton, UiStateFile};
use crate::{capture, input};

/// Stratégie demandée par l'agent. `Auto` enchaîne les replis dans l'ordre
/// jusqu'à observer un changement d'écran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Auto,
    Click,
    ClickThenEnter,
    DoubleClick,
}

impl Strategy {
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("auto") {
            "auto" => Ok(Self::Auto),
            "click" => Ok(Self::Click),
            "click_then_enter" => Ok(Self::ClickThenEnter),
            "double_click" => Ok(Self::DoubleClick),
            other => Err(format!("stratégie inconnue: {other}")),
        }
    }

    fn sequence(self) -> Vec<ActivationStep> {
        match self {
            // Auto est déplié en plusieurs séquences dans `planned_attempts`.
            Strategy::Auto | Strategy::Click => vec![ActivationStep::ClickCenter],
            Strategy::ClickThenEnter => vec![
                ActivationStep::ClickCenter,
                ActivationStep::Key { code: "Enter".into() },
            ],
            Strategy::DoubleClick => vec![ActivationStep::DoubleClick],
        }
    }
}

/// Résout `element` (id, nom ou label, insensible à la casse) dans les layouts
/// donnés. `layout` restreint la recherche à un layout nommé.
pub fn resolve_zone<'a>(
    layouts: &'a [LayoutFile],
    layout: Option<&str>,
    element: &str,
) -> Result<(&'a LayoutFile, &'a LayoutZone), String> {
    let wanted = element.trim().to_lowercase();
    if wanted.is_empty() {
        return Err("paramètre 'element' vide".into());
    }
    let candidates: Vec<&LayoutFile> = layouts
        .iter()
        .filter(|file| layout.is_none_or(|name| file.name.eq_ignore_ascii_case(name)))
        .collect();
    if candidates.is_empty() {
        return Err(match layout {
            Some(name) => format!("layout '{name}' introuvable"),
            None => "aucun layout défini pour ce contexte".into(),
        });
    }
    let matches_zone = |zone: &LayoutZone| {
        zone.id.to_lowercase() == wanted
            || zone.name.to_lowercase() == wanted
            || zone.label.as_deref().is_some_and(|l| l.to_lowercase() == wanted)
    };
    let mut found = candidates
        .iter()
        .flat_map(|file| file.zones.iter().filter(|z| matches_zone(z)).map(move |z| (*file, z)));
    let first = found.next().ok_or_else(|| {
        let available: Vec<String> = candidates
            .iter()
            .flat_map(|file| file.zones.iter().map(|z| format!("{}/{}", file.name, z.name)))
            .collect();
        format!("élément '{element}' introuvable; zones connues: {}", available.join(", "))
    })?;
    if found.next().is_some() && layout.is_none() {
        tracing::warn!("élément '{element}' ambigu entre plusieurs layouts; premier retenu ({})", first.0.name);
    }
    Ok(first)
}

/// Centre effectif de la zone en coordonnées normalisées, ancres résolues.
/// Une zone ancrée prend l'origine de sa cible décalée de (dx, dy) et garde
/// ses propres dimensions.
pub fn effective_center(file: &LayoutFile, zone: &LayoutZone) -> (f64, f64) {
    let (x, y) = effective_origin(file, zone, 0);
    (
        (x + zone.bounds.width / 2.0).clamp(0.0, 1.0),
        (y + zone.bounds.height / 2.0).clamp(0.0, 1.0),
    )
}

fn effective_origin(file: &LayoutFile, zone: &LayoutZone, depth: usize) -> (f64, f64) {
    // La validation interdit les cycles ; la borne protège des fichiers non validés.
    if depth > file.zones.len() {
        return (zone.bounds.x, zone.bounds.y);
    }
    match &zone.anchor {
        None => (zone.bounds.x, zone.bounds.y),
        Some(anchor) => match file.zones.iter().find(|z| z.id == anchor.relative_to) {
            None => (zone.bounds.x, zone.bounds.y),
            Some(target) => {
                let (tx, ty) = effective_origin(file, target, depth + 1);
                (tx + anchor.dx, ty + anchor.dy)
            }
        },
    }
}

// ---- Vérification visuelle ----

/// Sonde de frames : une capture temporaire dédiée (sans curseur, pour que le
/// pointeur ne compte pas comme un changement d'écran).
struct Probe {
    session: capture::CaptureSession,
    rx: watch::Receiver<Vec<u8>>,
}

impl Probe {
    fn start(window_id: isize) -> Result<Self, String> {
        let (session, rx) = capture::start(window_id, 80, 1.0, false)
            .map_err(|e| format!("impossible de démarrer la capture de vérification: {e}"))?;
        Ok(Self { session, rx })
    }

    /// Dernière frame disponible, en attendant la première si nécessaire.
    async fn frame(&mut self, timeout: Duration) -> Result<Vec<u8>, String> {
        let current = self.rx.borrow().clone();
        if !current.is_empty() {
            return Ok(current);
        }
        tokio::select! {
            res = self.rx.changed() => {
                res.map_err(|_| "capture de vérification interrompue".to_string())?;
                Ok(self.rx.borrow().clone())
            }
            _ = tokio::time::sleep(timeout) => Err("délai dépassé pour la frame de vérification".into()),
        }
    }

    fn stop(self) {
        let _ = self.session.stop();
    }
}

/// Signature perceptuelle d'une frame JPEG : dHash 8×8 + luminance 32×32 sur
/// la frame entière, et la même paire sur le crop de la zone ciblée — bien
/// plus sensible aux changements locaux (surlignage de bouton, caret, focus).
struct FrameSig {
    dhash: u64,
    luma: Vec<u8>, // 32×32
    zone_dhash: u64,
    zone_luma: Vec<u8>, // 16×16
}

fn frame_signature(jpeg: &[u8], zone: &crate::protocol::NormalizedBounds) -> Result<FrameSig, String> {
    let img = image::load_from_memory(jpeg).map_err(|e| format!("décodage frame: {e}"))?;
    let (dhash, luma) = signature_of(&img, 32);
    let crop = crate::detect::crop_normalized(&img, zone);
    let (zone_dhash, zone_luma) = signature_of(&crop, 16);
    Ok(FrameSig { dhash, luma, zone_dhash, zone_luma })
}

fn signature_of(img: &image::DynamicImage, luma_side: u32) -> (u64, Vec<u8>) {
    let dhash = crate::detect::dhash8(img);
    let luma = img
        .resize_exact(luma_side, luma_side, image::imageops::FilterType::Triangle)
        .to_luma8()
        .into_raw();
    (dhash, luma)
}

fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
    let total: u64 = a.iter().zip(b).map(|(x, y)| u64::from(x.abs_diff(*y))).sum();
    total as f64 / a.len().max(1) as f64
}

struct Diff {
    hamming: u32,
    mean: f64,
    zone_hamming: u32,
    zone_mean: f64,
}

impl Diff {
    fn changed(&self) -> bool {
        self.hamming >= 3 || self.mean > 3.0 || self.zone_hamming >= 2 || self.zone_mean > 2.5
    }
}

fn frame_diff(a: &FrameSig, b: &FrameSig) -> Diff {
    Diff {
        hamming: (a.dhash ^ b.dhash).count_ones(),
        mean: mean_abs_diff(&a.luma, &b.luma),
        zone_hamming: (a.zone_dhash ^ b.zone_dhash).count_ones(),
        zone_mean: mean_abs_diff(&a.zone_luma, &b.zone_luma),
    }
}

// ---- Exécution ----

fn step_label(step: &ActivationStep) -> String {
    match step {
        ActivationStep::ClickCenter => "click_center".into(),
        ActivationStep::DoubleClick => "double_click".into(),
        ActivationStep::Key { code } => format!("key {code}"),
        ActivationStep::FocusThenKey { code } => format!("focus_then_key {code}"),
        ActivationStep::TypeText => "type_text".into(),
    }
}

async fn run_step(
    target: &AttachedWindow,
    step: &ActivationStep,
    px: i32,
    py: i32,
    text: Option<&str>,
) -> Result<(), String> {
    match step {
        ActivationStep::ClickCenter => {
            input::mouse_move(target.id, px, py);
            input::mouse_button(target.id, MouseButton::Left, true, px, py);
            input::mouse_button(target.id, MouseButton::Left, false, px, py);
        }
        ActivationStep::DoubleClick => {
            input::mouse_move(target.id, px, py);
            for _ in 0..2 {
                input::mouse_button(target.id, MouseButton::Left, true, px, py);
                input::mouse_button(target.id, MouseButton::Left, false, px, py);
            }
        }
        ActivationStep::Key { code } => {
            input::key(Some(target.id), code, true);
            input::key(Some(target.id), code, false);
        }
        ActivationStep::FocusThenKey { code } => {
            input::focus_window(target.id);
            tokio::time::sleep(Duration::from_millis(150)).await;
            input::key(Some(target.id), code, true);
            input::key(Some(target.id), code, false);
        }
        ActivationStep::TypeText => {
            let text = text.ok_or_else(|| "l'étape type_text requiert un paramètre 'text'".to_string())?;
            crate::mcp::type_text(Some(target.id), text);
        }
    }
    tokio::time::sleep(Duration::from_millis(60)).await;
    Ok(())
}

/// Séquences à tenter, dans l'ordre. Une séquence explicite sur la zone prime ;
/// sinon `Auto` déplie les replis classiques (le scénario « le clic ne fait que
/// sélectionner, Entrée valide » est couvert par la 2e tentative).
fn planned_attempts(zone: &LayoutZone, strategy: Strategy) -> Vec<(String, Vec<ActivationStep>)> {
    if let Some(steps) = &zone.activation {
        return vec![("configured".into(), steps.clone())];
    }
    match strategy {
        Strategy::Auto => vec![
            ("click".into(), Strategy::Click.sequence()),
            ("click_then_enter".into(), Strategy::ClickThenEnter.sequence()),
            ("double_click".into(), Strategy::DoubleClick.sequence()),
        ],
        Strategy::Click => vec![("click".into(), Strategy::Click.sequence())],
        Strategy::ClickThenEnter => vec![("click_then_enter".into(), Strategy::ClickThenEnter.sequence())],
        Strategy::DoubleClick => vec![("double_click".into(), Strategy::DoubleClick.sequence())],
    }
}

/// Attend (≤ ~2,5 s) que l'état attendu soit détecté sur les frames de la
/// sonde. Renvoie (atteint, vue du meilleur état pour le rapport).
async fn wait_expected_state(
    probe: &mut Probe,
    states: &[UiStateFile],
    expected: &str,
) -> Result<(bool, Value), String> {
    let mut last_view = Value::Null;
    for _ in 0..8 {
        let frame = probe.frame(Duration::from_millis(800)).await?;
        let (best, others) = crate::detect::detect(&frame, states)?;
        if let Some(best) = &best {
            if best.name == expected {
                return Ok((true, crate::detect::score_view(best)));
            }
        }
        last_view = best
            .as_ref()
            .map(crate::detect::score_view)
            .or_else(|| others.first().map(crate::detect::score_view))
            .unwrap_or(Value::Null);
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    Ok((false, last_view))
}

/// Active un élément de layout sur la fenêtre cible et rapporte chaque
/// tentative. `Ok` dès qu'une tentative produit un changement d'écran — et,
/// si la zone déclare un `expected_state` connu de `states`, que cet état est
/// détecté ensuite. `Err` avec le rapport complet sinon.
pub async fn activate_element(
    mut target: AttachedWindow,
    layouts: &[LayoutFile],
    states: &[UiStateFile],
    layout: Option<&str>,
    element: &str,
    strategy: Strategy,
    verify: bool,
    text: Option<&str>,
) -> Result<Value, String> {
    let (file, zone) = resolve_zone(layouts, layout, element)?;
    let (width, height) = input::client_size(target.id)
        .ok_or_else(|| format!("fenêtre cible {} introuvable", target.id))?;
    target.width = width;
    target.height = height;
    let (cx, cy) = effective_center(file, zone);
    let px = (cx * (width.max(1) - 1) as f64).round() as i32;
    let py = (cy * (height.max(1) - 1) as f64).round() as i32;
    let (zx, zy) = effective_origin(file, zone, 0);
    let zone_bounds = crate::protocol::NormalizedBounds {
        x: zx.clamp(0.0, 1.0),
        y: zy.clamp(0.0, 1.0),
        width: zone.bounds.width,
        height: zone.bounds.height,
    };

    // L'injection MCP est toujours en avant-plan : focus d'abord, comme le
    // moteur de workflow, pour que les touches atteignent la bonne fenêtre.
    input::focus_window(target.id);
    tokio::time::sleep(Duration::from_millis(180)).await;
    let foreground = input::is_foreground_window(target.id);

    let mut probe = if verify { Some(Probe::start(target.id)?) } else { None };
    let before = match &mut probe {
        Some(probe) => Some(frame_signature(&probe.frame(Duration::from_millis(1500)).await?, &zone_bounds)?),
        None => None,
    };

    let attempts = planned_attempts(zone, strategy);
    let mut reports: Vec<Value> = Vec::new();
    let mut verified = false;

    for (name, steps) in &attempts {
        for step in steps {
            run_step(&target, step, px, py, text).await?;
        }
        let Some(probe_ref) = &mut probe else {
            reports.push(json!({ "strategy": name, "steps": steps.iter().map(step_label).collect::<Vec<_>>() }));
            break; // sans vérification, une seule tentative a un sens
        };
        // Laisse l'application réagir avant de relever la frame.
        tokio::time::sleep(Duration::from_millis(600)).await;
        let after = frame_signature(&probe_ref.frame(Duration::from_millis(1000)).await?, &zone_bounds)?;
        let diff = frame_diff(before.as_ref().unwrap(), &after);
        let changed = diff.changed();
        reports.push(json!({
            "strategy": name,
            "steps": steps.iter().map(step_label).collect::<Vec<_>>(),
            "changed": changed,
            "hammingDistance": diff.hamming,
            "meanLumaDiff": (diff.mean * 100.0).round() / 100.0,
            "zoneHammingDistance": diff.zone_hamming,
            "zoneMeanLumaDiff": (diff.zone_mean * 100.0).round() / 100.0,
        }));
        if changed {
            // Si un état attendu est déclaré ET enregistré, l'écran doit non
            // seulement changer mais atterrir sur cet état ; sinon on passe à
            // la stratégie suivante.
            let expected_known = zone
                .expected_state
                .as_deref()
                .filter(|expected| states.iter().any(|s| s.name == *expected));
            match expected_known {
                None => {
                    verified = true;
                    break;
                }
                Some(expected) => {
                    let (reached, view) = wait_expected_state(probe_ref, states, expected).await?;
                    if let Some(last) = reports.last_mut() {
                        last["stateMatched"] = json!(reached);
                        last["stateAfter"] = view;
                    }
                    if reached {
                        verified = true;
                        break;
                    }
                }
            }
        }
    }
    if let Some(probe) = probe {
        probe.stop();
    }

    let report = json!({
        "element": zone.name,
        "zoneId": zone.id,
        "layout": file.name,
        "context": file.context,
        "clickPoint": { "x": px, "y": py, "normalized": { "x": cx, "y": cy } },
        "windowFocused": foreground,
        "attempts": reports,
        "verified": if verify { json!(verified) } else { json!("skipped") },
        "expectedState": zone.expected_state,
        "note": match zone.expected_state.as_deref() {
            Some(expected) if states.iter().any(|s| s.name == expected) =>
                "vérification par changement d'écran puis détection de l'état attendu",
            Some(_) =>
                "expectedState déclaré mais aucun état enregistré sous ce nom pour ce contexte; vérification par changement d'écran uniquement (capturez l'état via le panneau Layout)",
            None =>
                "vérification par changement d'écran (dHash + luminance, frame entière + crop de la zone); les popups (menus contextuels) s'ouvrent hors de la fenêtre capturée et sont invisibles à cette vérification",
        },
    });

    if verify && !verified {
        return Err(format!(
            "aucune stratégie n'a produit de changement d'écran; rapport: {}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        ));
    }
    Ok(report)
}

/// Clique sur une zone (focus du champ) puis y tape `text`. Si la zone définit
/// une séquence d'activation contenant `type_text`, cette séquence est rejouée
/// telle quelle avec le texte fourni.
pub async fn type_into_element(
    mut target: AttachedWindow,
    layouts: &[LayoutFile],
    layout: Option<&str>,
    element: &str,
    text: &str,
) -> Result<Value, String> {
    let (file, zone) = resolve_zone(layouts, layout, element)?;
    let (width, height) = input::client_size(target.id)
        .ok_or_else(|| format!("fenêtre cible {} introuvable", target.id))?;
    target.width = width;
    target.height = height;
    let (cx, cy) = effective_center(file, zone);
    let px = (cx * (width.max(1) - 1) as f64).round() as i32;
    let py = (cy * (height.max(1) - 1) as f64).round() as i32;

    input::focus_window(target.id);
    tokio::time::sleep(Duration::from_millis(180)).await;

    let steps: Vec<ActivationStep> = match &zone.activation {
        Some(steps) if steps.iter().any(|s| *s == ActivationStep::TypeText) => steps.clone(),
        _ => vec![ActivationStep::ClickCenter, ActivationStep::TypeText],
    };
    for step in &steps {
        run_step(&target, step, px, py, Some(text)).await?;
    }
    Ok(json!({
        "element": zone.name,
        "zoneId": zone.id,
        "layout": file.name,
        "context": file.context,
        "clickPoint": { "x": px, "y": py },
        "steps": steps.iter().map(step_label).collect::<Vec<_>>(),
        "typedChars": text.chars().count(),
    }))
}
