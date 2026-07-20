//! Activation sémantique des zones de layout : séquences temporisées,
//! stratégies de repli et vérification du résultat.
//!
//! C'est le cœur de `activate_ui_element` (MCP). **La vérification est une
//! hiérarchie stricte, pas un OU** : dès qu'une zone déclare un `expectedState`,
//! seule la détection de cet état vaut succès — un simple changement d'écran
//! (surlignage d'un bouton survolé, animation de fond) ne doit jamais suffire,
//! sous peine de faux positifs. Le mode `screen_change` reste disponible pour
//! les zones sans état déclaré, mais il est explicitement signalé comme faible.

use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::watch;

use crate::app::AttachedWindow;
use crate::protocol::{ActivationStep, AppProfile, LayoutFile, LayoutZone, MouseButton, UiStateFile, Verification};
use crate::{capture, input};

/// Délais par défaut quand ni l'étape ni le profil applicatif ne les fixent.
const DEFAULT_POST_CLICK_DELAY_MS: u64 = 60;
const DEFAULT_KEY_HOLD_MS: u64 = 0;
const DEFAULT_POST_ACTION_DELAY_MS: u64 = 600;

/// Délais effectifs d'une activation : cascade étape → profil → constante.
#[derive(Debug, Clone, Copy)]
struct Timings {
    post_click_ms: u64,
    key_hold_ms: u64,
    post_action_ms: u64,
}

impl Timings {
    fn from_profile(profile: &AppProfile) -> Self {
        Self {
            post_click_ms: profile.default_post_click_delay_ms.unwrap_or(DEFAULT_POST_CLICK_DELAY_MS),
            key_hold_ms: profile.default_key_hold_ms.unwrap_or(DEFAULT_KEY_HOLD_MS),
            post_action_ms: profile.default_post_action_delay_ms.unwrap_or(DEFAULT_POST_ACTION_DELAY_MS),
        }
    }
}

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
            Strategy::Auto | Strategy::Click => vec![ActivationStep::ClickCenter { delay_after_ms: None }],
            // Le clic ne fait souvent que déplacer la sélection : le délai
            // avant Entrée laisse l'application enregistrer ce déplacement.
            Strategy::ClickThenEnter => vec![
                ActivationStep::ClickCenter { delay_after_ms: Some(250) },
                ActivationStep::Key { code: "Enter".into(), hold_ms: Some(120), delay_after_ms: None },
            ],
            Strategy::DoubleClick => vec![ActivationStep::DoubleClick { delay_after_ms: None }],
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
    let matched: Vec<(&LayoutFile, &LayoutZone)> = candidates
        .iter()
        .flat_map(|file| file.zones.iter().filter(|z| matches_zone(z)).map(move |z| (*file, z)))
        .collect();
    if matched.is_empty() {
        let available: Vec<String> = candidates
            .iter()
            .flat_map(|file| file.zones.iter().map(|z| format!("{}/{}", file.name, z.name)))
            .collect();
        return Err(format!("élément '{element}' introuvable; zones connues: {}", available.join(", ")));
    }
    // Ambiguïté entre layouts (une application a souvent plusieurs variantes du
    // même écran) : préférer la zone qui déclare un expectedState, sinon la
    // vérification retomberait silencieusement en mode faible sur un homonyme
    // moins bien décrit. C'est cette bascule qui produisait des faux positifs.
    let best = matched
        .iter()
        .find(|(_, zone)| zone.expected_state.is_some())
        .copied()
        .unwrap_or(matched[0]);
    if matched.len() > 1 && layout.is_none() {
        tracing::warn!(
            "élément '{element}' présent dans {} layouts; retenu: {} (expectedState={:?})",
            matched.len(),
            best.0.name,
            best.1.expected_state
        );
    }
    Ok(best)
}

/// Layouts contenant un homonyme de `element`, hors celui retenu — remonté
/// dans le rapport pour que l'agent puisse lever l'ambiguïté explicitement.
fn ambiguous_layouts(layouts: &[LayoutFile], element: &str, chosen: &LayoutFile) -> Vec<String> {
    let wanted = element.trim().to_lowercase();
    layouts
        .iter()
        .filter(|file| file.name != chosen.name)
        .filter(|file| {
            file.zones.iter().any(|z| {
                z.id.to_lowercase() == wanted
                    || z.name.to_lowercase() == wanted
                    || z.label.as_deref().is_some_and(|l| l.to_lowercase() == wanted)
            })
        })
        .map(|file| file.name.clone())
        .collect()
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
    /// `borrow_and_update` marque la valeur comme vue : un `fresh_frame`
    /// ultérieur attendra donc bien une frame *postérieure*.
    async fn frame(&mut self, timeout: Duration) -> Result<Vec<u8>, String> {
        let current = self.rx.borrow_and_update().clone();
        if !current.is_empty() {
            return Ok(current);
        }
        tokio::select! {
            res = self.rx.changed() => {
                res.map_err(|_| "capture de vérification interrompue".to_string())?;
                Ok(self.rx.borrow_and_update().clone())
            }
            _ = tokio::time::sleep(timeout) => Err("délai dépassé pour la frame de vérification".into()),
        }
    }

    /// Frame strictement postérieure au dernier relevé. Indispensable après une
    /// action : sans cela on peut comparer la frame d'avant le clic avec
    /// elle-même et conclure à tort que « rien n'a changé ».
    async fn fresh_frame(&mut self, timeout: Duration) -> Result<Vec<u8>, String> {
        tokio::select! {
            res = self.rx.changed() => {
                res.map_err(|_| "capture de vérification interrompue".to_string())?;
                Ok(self.rx.borrow_and_update().clone())
            }
            // Pas de nouvelle frame : l'écran est probablement figé. On rend la
            // dernière connue plutôt que d'échouer.
            _ = tokio::time::sleep(timeout) => Ok(self.rx.borrow().clone()),
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
        ActivationStep::ClickCenter { .. } => "click_center".into(),
        ActivationStep::DoubleClick { .. } => "double_click".into(),
        ActivationStep::Key { code, hold_ms, .. } => match hold_ms {
            Some(hold) => format!("key {code} (hold {hold}ms)"),
            None => format!("key {code}"),
        },
        ActivationStep::FocusThenKey { code, .. } => format!("focus_then_key {code}"),
        ActivationStep::TypeText { .. } => "type_text".into(),
        ActivationStep::WaitForState { state, timeout_ms } => {
            format!("wait_for_state {state} (≤{}ms)", timeout_ms.unwrap_or(3_000))
        }
        ActivationStep::WaitMs { duration_ms } => format!("wait {duration_ms}ms"),
    }
}

/// Appui maintenu puis relâche. `hold` à 0 conserve l'ancien comportement
/// (down+up immédiats), suffisant pour la plupart des applications de bureau.
async fn press_key(window: isize, code: &str, hold_ms: u64) {
    input::key(Some(window), code, true);
    if hold_ms > 0 {
        tokio::time::sleep(Duration::from_millis(hold_ms.min(5_000))).await;
    }
    input::key(Some(window), code, false);
}

async fn run_step(
    target: &AttachedWindow,
    step: &ActivationStep,
    px: i32,
    py: i32,
    text: Option<&str>,
    timings: Timings,
    states: &[UiStateFile],
) -> Result<(), String> {
    match step {
        ActivationStep::ClickCenter { .. } => {
            input::mouse_move(target.id, px, py);
            input::mouse_button(target.id, MouseButton::Left, true, px, py);
            input::mouse_button(target.id, MouseButton::Left, false, px, py);
        }
        ActivationStep::DoubleClick { .. } => {
            input::mouse_move(target.id, px, py);
            for _ in 0..2 {
                input::mouse_button(target.id, MouseButton::Left, true, px, py);
                input::mouse_button(target.id, MouseButton::Left, false, px, py);
            }
        }
        ActivationStep::Key { code, hold_ms, .. } => {
            press_key(target.id, code, hold_ms.unwrap_or(timings.key_hold_ms)).await;
        }
        ActivationStep::FocusThenKey { code, hold_ms, .. } => {
            input::focus_window(target.id);
            tokio::time::sleep(Duration::from_millis(150)).await;
            press_key(target.id, code, hold_ms.unwrap_or(timings.key_hold_ms)).await;
        }
        ActivationStep::TypeText { .. } => {
            let text = text.ok_or_else(|| "l'étape type_text requiert un paramètre 'text'".to_string())?;
            crate::mcp::type_text(Some(target.id), text);
        }
        ActivationStep::WaitForState { state, timeout_ms } => {
            crate::detect::wait_for_state_on_window(target.id, states, state, timeout_ms.unwrap_or(3_000))
                .await
                .map(|_| ())?;
        }
        ActivationStep::WaitMs { duration_ms } => {
            tokio::time::sleep(Duration::from_millis((*duration_ms).min(300_000))).await;
        }
    }
    // Délai post-étape : valeur de l'étape, sinon défaut du profil.
    let delay = step.delay_after_ms().unwrap_or(timings.post_click_ms);
    if delay > 0 {
        tokio::time::sleep(Duration::from_millis(delay.min(300_000))).await;
    }
    Ok(())
}

/// Séquences à tenter, dans l'ordre. Une séquence explicite sur la zone prime ;
/// sinon `Auto` déplie les replis classiques (le scénario « le clic ne fait que
/// sélectionner, Entrée valide » est couvert par la 2e tentative).
fn planned_attempts(zone: &LayoutZone, strategy: Strategy, preferred: Option<&str>) -> Vec<(String, Vec<ActivationStep>)> {
    if let Some(steps) = &zone.activation {
        return vec![("configured".into(), steps.clone())];
    }
    match strategy {
        Strategy::Auto => {
            if preferred.is_some_and(|p| p.eq_ignore_ascii_case("keyboard")) {
                vec![
                    ("click_then_enter".into(), Strategy::ClickThenEnter.sequence()),
                    ("click".into(), Strategy::Click.sequence()),
                    ("double_click".into(), Strategy::DoubleClick.sequence()),
                ]
            } else {
                vec![
                    ("click".into(), Strategy::Click.sequence()),
                    ("click_then_enter".into(), Strategy::ClickThenEnter.sequence()),
                    ("double_click".into(), Strategy::DoubleClick.sequence()),
                ]
            }
        }
        Strategy::Click => vec![("click".into(), Strategy::Click.sequence())],
        Strategy::ClickThenEnter => vec![("click_then_enter".into(), Strategy::ClickThenEnter.sequence())],
        Strategy::DoubleClick => vec![("double_click".into(), Strategy::DoubleClick.sequence())],
    }
}

/// Attend que l'état attendu soit détecté sur les frames de la sonde.
/// Renvoie (atteint, vue du meilleur état pour le rapport).
async fn wait_expected_state(
    probe: &mut Probe,
    states: &[UiStateFile],
    expected: &str,
    timeout_ms: u64,
) -> Result<(bool, Value), String> {
    let mut last_view = Value::Null;
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms.clamp(250, 60_000));
    while std::time::Instant::now() < deadline {
        let frame = probe.fresh_frame(Duration::from_millis(800)).await?;
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

/// Détermine l'autorité de validation : demande explicite de l'agent, sinon
/// `State` dès qu'un `expectedState` est déclaré, sinon `ScreenChange`.
fn resolve_verification(zone: &LayoutZone, requested: Option<Verification>, verify: bool) -> Verification {
    if !verify {
        return Verification::None;
    }
    match requested {
        Some(mode) => mode,
        None if zone.expected_state.is_some() => Verification::State,
        None => Verification::ScreenChange,
    }
}

/// Active un élément de layout sur la fenêtre cible et rapporte chaque
/// tentative.
///
/// En mode `State` (défaut dès qu'un `expectedState` est déclaré), **seule** la
/// détection de cet état vaut succès : le changement d'écran est rapporté mais
/// n'est jamais un critère. Si l'état n'a pas encore été capturé, l'appel
/// échoue explicitement plutôt que de retomber en silence sur une vérification
/// faible — c'est la source du faux positif « clic qui ne fait que surligner ».
pub async fn activate_element(
    mut target: AttachedWindow,
    layouts: &[LayoutFile],
    states: &[UiStateFile],
    layout: Option<&str>,
    element: &str,
    strategy: Strategy,
    verify: bool,
    text: Option<&str>,
    requested_verification: Option<Verification>,
) -> Result<Value, String> {
    let (file, zone) = resolve_zone(layouts, layout, element)?;
    let profile = crate::profile_store::load_or_default(&file.context).await;
    let timings = Timings::from_profile(&profile);
    let verification = resolve_verification(zone, requested_verification, verify);

    // Mode State : l'état attendu doit être déclaré *et* enregistré. Sinon on
    // ne peut rien affirmer — mieux vaut une erreur lisible qu'un faux succès.
    if verification == Verification::State {
        let Some(expected) = zone.expected_state.as_deref() else {
            return Err(format!(
                "vérification 'state' demandée mais la zone '{}' ne déclare aucun expectedState; \
                 ajoutez-le au layout ou utilisez verification='screen_change'",
                zone.name
            ));
        };
        if !states.iter().any(|s| s.name == expected) {
            let known: Vec<&str> = states.iter().map(|s| s.name.as_str()).collect();
            return Err(format!(
                "l'état attendu '{expected}' n'est pas enregistré pour '{}': impossible de vérifier \
                 l'activation. Capturez-le (capture_ui_state) puis réessayez. États connus: [{}]",
                file.context,
                known.join(", ")
            ));
        }
    }
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

    // L'injection MCP est toujours en avant-plan. Une seule tentative ne suffit
    // pas : les fenêtres de jeu (et les WinUI) tardent à accepter le focus, et
    // sans focus les frappes partent dans une autre application — l'action
    // semble alors « sans effet » pour une raison invisible dans le rapport.
    let mut foreground = false;
    for _ in 0..6 {
        input::focus_window(target.id);
        tokio::time::sleep(Duration::from_millis(120)).await;
        if input::is_foreground_window(target.id) {
            foreground = true;
            break;
        }
    }
    if !foreground && profile.foreground_required {
        return Err(format!(
            "impossible de mettre la fenêtre {} au premier plan alors que le profil de '{}' \
             déclare foregroundRequired: les entrées seraient injectées dans une autre application",
            target.id, file.context
        ));
    }

    let mut probe = if verification == Verification::None { None } else { Some(Probe::start(target.id)?) };
    let before_frame = match &mut probe {
        Some(probe) => Some(probe.frame(Duration::from_millis(1500)).await?),
        None => None,
    };
    let before = match &before_frame {
        Some(frame) => Some(frame_signature(frame, &zone_bounds)?),
        None => None,
    };

    // Précondition : l'écran courant doit être celui auquel appartient la zone.
    // Sans ce garde-fou, on clique les coordonnées d'un layout alors que
    // l'application est sur un tout autre écran — l'action tombe dans le vide
    // et le diagnostic devient incompréhensible. On ne refuse que si l'écran
    // est reconnu *avec certitude* comme un autre écran : un état simplement
    // non reconnu (fond animé, variante non capturée) laisse passer avec un
    // avertissement, sinon la détection imparfaite bloquerait tout.
    let precondition = match &before_frame {
        Some(frame) if !states.is_empty() => {
            let (best, _) = crate::detect::detect(frame, states)?;
            match &best {
                Some(current) => {
                    let expected_here = current
                        .layout
                        .as_deref()
                        .is_none_or(|layout| layout.eq_ignore_ascii_case(&file.name));
                    if !expected_here {
                        return Err(format!(
                            "écran courant reconnu comme '{}' (layout '{}'), mais la zone '{}' appartient au layout '{}'. \
                             Naviguez d'abord vers le bon écran, ou utilisez detect_layout pour connaître les éléments \
                             réellement disponibles ici.",
                            current.name,
                            current.layout.as_deref().unwrap_or("?"),
                            zone.name,
                            file.name
                        ));
                    }
                    json!({ "state": current.name, "match": true })
                }
                None => json!({
                    "state": null,
                    "match": null,
                    "warning": "écran courant non reconnu: impossible de garantir que la zone est visible à l'écran. \
                                L'action va être tentée à l'aveugle; capturez cet écran (capture_ui_state) pour fiabiliser."
                }),
            }
        }
        _ => json!({ "state": null, "match": null, "warning": "aucun état enregistré pour ce contexte" }),
    };

    let attempts = planned_attempts(zone, strategy, Some(&profile.preferred_activation));
    let mut reports: Vec<Value> = Vec::new();
    let mut verified = false;

    for (name, steps) in &attempts {
        for step in steps {
            run_step(&target, step, px, py, text, timings, states).await?;
        }
        let Some(probe_ref) = &mut probe else {
            reports.push(json!({ "strategy": name, "steps": steps.iter().map(step_label).collect::<Vec<_>>() }));
            break; // sans vérification, une seule tentative a un sens
        };
        // Laisse l'application réagir avant de relever la frame.
        tokio::time::sleep(Duration::from_millis(timings.post_action_ms.min(30_000))).await;
        let after = frame_signature(&probe_ref.fresh_frame(Duration::from_millis(1000)).await?, &zone_bounds)?;
        let diff = frame_diff(before.as_ref().unwrap(), &after);
        let changed = diff.changed();
        reports.push(json!({
            "strategy": name,
            "steps": steps.iter().map(step_label).collect::<Vec<_>>(),
            "screenChanged": changed,
            "hammingDistance": diff.hamming,
            "meanLumaDiff": (diff.mean * 100.0).round() / 100.0,
            "zoneHammingDistance": diff.zone_hamming,
            "zoneMeanLumaDiff": (diff.zone_mean * 100.0).round() / 100.0,
        }));

        match verification {
            // Seul l'état fait foi. On l'attend systématiquement : un écran qui
            // n'a pas « assez » changé peut tout de même avoir navigué, et un
            // écran qui a changé (surlignage) peut n'avoir rien validé.
            Verification::State => {
                let expected = zone.expected_state.as_deref().unwrap_or_default();
                let (reached, view) = wait_expected_state(probe_ref, states, expected, 3_000).await?;
                if let Some(last) = reports.last_mut() {
                    last["stateMatched"] = json!(reached);
                    last["stateAfter"] = view;
                }
                if reached {
                    verified = true;
                    break;
                }
            }
            Verification::ScreenChange => {
                if changed {
                    verified = true;
                    break;
                }
            }
            Verification::None => break,
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
        "focusWarning": if foreground { Value::Null } else {
            json!("la fenêtre n'a pas pu être mise au premier plan: les frappes clavier ont pu être reçues par une autre application. Si cette application exige le focus, renseignez foregroundRequired dans son profil (set_app_profile)")
        },
        "alsoDefinedIn": ambiguous_layouts(layouts, element, file),
        "precondition": precondition,
        "attempts": reports,
        "verification": verification,
        "verified": if verification == Verification::None { json!("skipped") } else { json!(verified) },
        "confidence": match verification {
            Verification::State => "strong",
            Verification::ScreenChange => "weak",
            Verification::None => "none",
        },
        "expectedState": zone.expected_state,
        "timings": {
            "postClickMs": timings.post_click_ms,
            "keyHoldMs": timings.key_hold_ms,
            "postActionMs": timings.post_action_ms,
        },
        "note": match verification {
            Verification::State =>
                "succès conditionné à la détection de expectedState; le changement d'écran est informatif seulement",
            Verification::ScreenChange =>
                "vérification FAIBLE: un simple changement visuel (surlignage au survol, animation de fond) suffit et peut produire un faux positif. Déclarez un expectedState sur la zone et capturez cet état pour une vérification fiable",
            Verification::None => "aucune vérification demandée",
        },
    });

    if verification != Verification::None && !verified {
        return Err(format!(
            "activation non vérifiée ({}); rapport: {}",
            match verification {
                Verification::State => format!(
                    "l'état attendu '{}' n'a pas été atteint",
                    zone.expected_state.as_deref().unwrap_or("?")
                ),
                _ => "aucune stratégie n'a produit de changement d'écran".to_string(),
            },
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

    let profile = crate::profile_store::load_or_default(&file.context).await;
    let timings = Timings::from_profile(&profile);
    let steps: Vec<ActivationStep> = match &zone.activation {
        Some(steps) if steps.iter().any(|s| matches!(s, ActivationStep::TypeText { .. })) => steps.clone(),
        _ => vec![
            ActivationStep::ClickCenter { delay_after_ms: None },
            ActivationStep::TypeText { delay_after_ms: None },
        ],
    };
    for step in &steps {
        run_step(&target, step, px, py, Some(text), timings, &[]).await?;
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
