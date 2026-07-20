// Câblage de l'UI : sélection de fenêtre, connexion au flux, injection input,
// outils d'annotation vectoriels (overlay SVG aligné sur le canvas).

import { InputController, type KeyboardLayout } from "./input";
import { StreamClient } from "./stream";
import { Annotator, type Tool } from "./annotate";
import { Rulers, RULER_THICKNESS } from "./rulers";
import { MacroController } from "./macros";
import { WorkflowEditor } from "./workflows";
import { LayoutController } from "./layouts";
import type { ServerMessage } from "./protocol";

const canvas = document.getElementById("screen") as HTMLCanvasElement;
const select = document.getElementById("windowSelect") as HTMLSelectElement;
const refreshBtn = document.getElementById("refresh") as HTMLButtonElement;
const connectBtn = document.getElementById("connect") as HTMLButtonElement;
const statusEl = document.getElementById("status") as HTMLSpanElement;
const saveDirInput = document.getElementById("saveDir") as HTMLInputElement;
const shotBtn = document.getElementById("shot") as HTMLButtonElement;
const recBtn = document.getElementById("rec") as HTMLButtonElement;

// PIN modal elements
const pinModal = document.getElementById("pinModal") as HTMLDivElement;
const pinInput = document.getElementById("pinInput") as HTMLInputElement;
const pinSubmit = document.getElementById("pinSubmit") as HTMLButtonElement;
const pinCancel = document.getElementById("pinCancel") as HTMLButtonElement;
const pinError = document.getElementById("pinError") as HTMLDivElement;
const qualitySelect = document.getElementById("qualitySelect") as HTMLSelectElement;
const qualitySlider = document.getElementById("qualitySlider") as HTMLInputElement;
const qualitySliderVal = document.getElementById("qualitySliderVal") as HTMLSpanElement;
const scaleSlider = document.getElementById("scaleSlider") as HTMLInputElement;
const scaleSliderVal = document.getElementById("scaleSliderVal") as HTMLSpanElement;
const showCursorInput = document.getElementById("showCursor") as HTMLInputElement;
const bgInput = document.getElementById("bgInput") as HTMLInputElement;
const keyboardLayoutSelect = document.getElementById("keyboardLayout") as HTMLSelectElement;
const webrtcToggle = document.getElementById("webrtcToggle") as HTMLInputElement;
const recFpsInput = document.getElementById("recFps") as HTMLInputElement;
const virtualCursor = document.getElementById("virtualCursor") as HTMLDivElement;
const focusWindowBtn = document.getElementById("focusWindow") as HTMLButtonElement;
const launchCmdInput = document.getElementById("launchCmd") as HTMLInputElement;
const btnLaunchVirtual = document.getElementById("btnLaunchVirtual") as HTMLButtonElement;
const btnSendActiveToVirtual = document.getElementById("btnSendActiveToVirtual") as HTMLButtonElement;

const confirmCloseModal = document.getElementById("confirmCloseModal") as HTMLDivElement;
const confirmCloseText = document.getElementById("confirmCloseText") as HTMLParagraphElement;
const confirmCloseCancel = document.getElementById("confirmCloseCancel") as HTMLButtonElement;
const confirmCloseSubmit = document.getElementById("confirmCloseSubmit") as HTMLButtonElement;

let pendingWindowId: number | null = null;
let pendingCloseWindowId: number | null = null;
// Dernière sélection (pour re-streamer quand une option de capture change).
let lastSelectedId: number | null = null;
let lastPin: string | null = null;
const windowContexts = new Map<number, string>();

function showPinModal(windowId: number) {
  pendingWindowId = windowId;
  pinModal.style.display = "flex";
  pinInput.value = "";
  pinInput.focus();
  pinError.textContent = "";
}

function hidePinModal() {
  pendingWindowId = null;
  pinModal.style.display = "none";
}

// Préréglages : { qualité JPEG %, résolution % }. Les sliders sont la source de
// vérité ; le dropdown ne fait que les positionner (et passe en "custom" dès
// qu'on ajuste finement).
const QUALITY_PRESETS: Record<string, { q: number; s: number }> = {
  high: { q: 75, s: 100 },
  medium: { q: 60, s: 70 },
  low: { q: 45, s: 50 },
};

function getQualitySettings(): { quality: number; scale: number } {
  return {
    quality: Number(qualitySlider.value),
    scale: Number(scaleSlider.value) / 100,
  };
}

function updateQualityLabels() {
  qualitySliderVal.textContent = `${qualitySlider.value}%`;
  scaleSliderVal.textContent = `${scaleSlider.value}%`;
}

/** Aligne le dropdown de préréglage sur les valeurs actuelles des sliders. */
function syncPresetFromSliders() {
  const q = Number(qualitySlider.value);
  const s = Number(scaleSlider.value);
  const match = Object.entries(QUALITY_PRESETS).find(([, p]) => p.q === q && p.s === s);
  qualitySelect.value = match ? match[0] : "custom";
}

/** Re-streame la fenêtre courante pour appliquer les nouveaux réglages à chaud. */
function reStreamIfActive() {
  if (lastSelectedId !== null) sendSelect(lastSelectedId, lastPin);
}

// Restaure les réglages persistés.
{
  const savedQ = localStorage.getItem("appmirror.quality");
  const savedS = localStorage.getItem("appmirror.scale");
  if (savedQ) qualitySlider.value = savedQ;
  if (savedS) scaleSlider.value = savedS;
  updateQualityLabels();
  syncPresetFromSliders();
}

qualitySelect.addEventListener("change", () => {
  const p = QUALITY_PRESETS[qualitySelect.value];
  if (!p) return; // "custom" : on garde les sliders tels quels
  qualitySlider.value = String(p.q);
  scaleSlider.value = String(p.s);
  updateQualityLabels();
  persistQuality();
  reStreamIfActive();
});

function persistQuality() {
  localStorage.setItem("appmirror.quality", qualitySlider.value);
  localStorage.setItem("appmirror.scale", scaleSlider.value);
}

for (const slider of [qualitySlider, scaleSlider]) {
  // input = pendant le glissement : label live + bascule en "custom".
  slider.addEventListener("input", () => {
    updateQualityLabels();
    syncPresetFromSliders();
  });
  // change = au relâchement : on persiste et on ré-applique au flux.
  slider.addEventListener("change", () => {
    persistQuality();
    reStreamIfActive();
  });
}

/** Envoi centralisé de select_window : porte qualité/échelle/curseur + PIN. */
function sendSelect(id: number, pin: string | null) {
  const { quality, scale } = getQualitySettings();
  lastSelectedId = id;
  lastPin = pin;
  // Le panneau Macro peut démarrer dès la sélection utilisateur ; il ne
  // dépend plus du délai de redémarrage de la capture côté serveur.
  macros.selectTarget(id, windowContexts.get(id) ?? "unknown");
  layoutEditor.selectTarget(id, windowContexts.get(id) ?? "unknown");
  stream.send({
    type: "select_window",
    id,
    pin,
    quality,
    scale,
    show_cursor: showCursorInput.checked,
  });
}

function selectWindowAndStream(id: number, isLocked: boolean) {
  select.value = String(id);
  if (isLocked) {
    showPinModal(id);
  } else {
    sendSelect(id, null);
  }
  settingsModal.style.display = "none";
}

function submitPin() {
  const pin = pinInput.value;
  if (!pin) {
    pinError.textContent = "Please enter the PIN code";
    return;
  }
  if (pendingWindowId !== null) {
    sendSelect(pendingWindowId, pin);
  }
}

pinSubmit.addEventListener("click", submitPin);
pinCancel.addEventListener("click", hidePinModal);
pinInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    submitPin();
  } else if (e.key === "Escape") {
    hidePinModal();
  }
});

const viewport = document.querySelector("main") as HTMLElement;
const stage = document.getElementById("stage") as HTMLDivElement;
const overlay = document.getElementById("overlay") as unknown as SVGSVGElement;
const rulerTop = document.getElementById("rulerTop") as HTMLCanvasElement;
const rulerLeft = document.getElementById("rulerLeft") as HTMLCanvasElement;
const coordChip = document.getElementById("coordChip") as HTMLDivElement;

// Le dossier de capture est persisté localement (vide = défaut backend).
saveDirInput.value = localStorage.getItem("appmirror.saveDir") ?? "";
saveDirInput.addEventListener("change", () =>
  localStorage.setItem("appmirror.saveDir", saveDirInput.value),
);

let recording = false;

function setRecording(on: boolean) {
  recording = on;
  recBtn.textContent = on ? "⏹ Stop" : "⏺ REC";
  recBtn.classList.toggle("recording", on);
}

/** Dossier saisi, ou null pour laisser le backend choisir le défaut. */
function saveDir(): string | null {
  const v = saveDirInput.value.trim();
  return v.length > 0 ? v : null;
}

const input = new InputController(canvas, (msg) => stream.send(msg));
const savedKeyboardLayout = localStorage.getItem("appmirror.keyboardLayout") as KeyboardLayout | null;
const defaultKeyboardLayout: KeyboardLayout = navigator.language.toLowerCase().startsWith("fr") ? "azerty_fr" : "qwerty_us";
keyboardLayoutSelect.value = savedKeyboardLayout === "azerty_fr" || savedKeyboardLayout === "qwerty_us"
  ? savedKeyboardLayout
  : defaultKeyboardLayout;
input.setKeyboardLayout(keyboardLayoutSelect.value as KeyboardLayout);
keyboardLayoutSelect.addEventListener("change", () => {
  const layout = keyboardLayoutSelect.value as KeyboardLayout;
  localStorage.setItem("appmirror.keyboardLayout", layout);
  input.setKeyboardLayout(layout);
  statusEl.textContent = `keyboard input: ${layout === "azerty_fr" ? "Azerty (FR)" : "Qwerty (US)"}`;
  canvas.focus();
});

const rulers = new Rulers(rulerTop, rulerLeft, coordChip);
const annot = new Annotator({
  stage,
  overlay,
  rulerTop,
  rulerLeft,
  rulers,
  onStatus: (s) => (statusEl.textContent = s),
});

// Mode d'affichage du mirroir. "fit" = plus grand possible en gardant tout
// visible (ratio conservé) ; "fill" = remplit la largeur de la fenêtre (ratio
// conservé, défilement vertical au besoin) ; "actual" = résolution native 1:1.
type FitMode = "fit" | "fill" | "actual";
const fitModeSelect = document.getElementById("fitMode") as HTMLSelectElement;
let fitMode: FitMode =
  (localStorage.getItem("appmirror.fitMode") as FitMode) || "fit";
fitModeSelect.value = fitMode;

/** Dimensionne #stage selon le mode d'affichage courant, en conservant le ratio. */
function layout() {
  // On dimensionne d'après le buffer du canvas = la frame réellement décodée.
  const fw = canvas.width || 1;
  const fh = canvas.height || 1;
  // -26 = padding du <main> (2×12px) + bordure de #stage (2×1px). Sans réserver
  // la bordure, le mode "Fit" déborde de 2px et fait apparaître une barre de
  // défilement fantôme (le <main> est en overflow: auto).
  // Quand l'outil règle est actif, #stage porte une marge (gouttière) en haut et
  // à gauche pour loger les règles À L'EXTÉRIEUR de la vidéo : on la déduit de
  // l'espace disponible pour que l'ensemble tienne sans débordement.
  const rulerGutter = stage.dataset.tool === "ruler" ? RULER_THICKNESS : 0;
  const availW = Math.max(50, viewport.clientWidth - 26 - rulerGutter);
  const availH = Math.max(50, viewport.clientHeight - 26 - rulerGutter);
  let scale: number;
  switch (fitMode) {
    case "fill":
      // Remplit la largeur disponible (le ratio impose la hauteur ; scroll si besoin).
      scale = availW / fw;
      break;
    case "actual":
      // 1:1 pixel natif (scroll si la frame dépasse la zone visible).
      scale = 1;
      break;
    default:
      // "fit" : plus grande échelle qui garde toute la frame visible.
      scale = Math.min(availW / fw, availH / fh);
  }
  const cssW = Math.max(1, Math.round(fw * scale));
  const cssH = Math.max(1, Math.round(fh * scale));
  stage.style.width = `${cssW}px`;
  stage.style.height = `${cssH}px`;

  // Le repère de l'overlay = dimensions de la frame (cf. onFrameSize), et les
  // deux dimensions sont mises à l'échelle par le MÊME facteur : l'échelle reste
  // uniforme en x et y (px CSS / px de frame), donc le trait n'est pas déformé.
  annot.layoutChanged(cssW / fw);
}
new ResizeObserver(layout).observe(viewport);

fitModeSelect.addEventListener("change", () => {
  fitMode = fitModeSelect.value as FitMode;
  localStorage.setItem("appmirror.fitMode", fitMode);
  layout();
});

const stream = new StreamClient(canvas, {
  onStatus: (s) => (statusEl.textContent = s),
  // Charge la liste des fenêtres dès la connexion établie (et à chaque
  // reconnexion) : la liste n'est jamais vide à l'ouverture de la page.
  onOpen: () => stream.send({ type: "list_windows" }),
  // La taille des frames fait foi (la fenêtre distante peut être
  // redimensionnée en cours de stream, sans nouveau message "selected").
  // L'overlay d'annotation est calé sur ces dimensions réelles — et non sur la
  // taille client renvoyée par "selected", qui peut différer (fenêtre réduite,
  // DPI, downscale qualité) et déformerait alors le trait.
  onFrameSize: (w, h) => {
    annot.setNativeSize(w, h);
    macros.setNativeSize(w, h);
    layoutEditor.setNativeSize(w, h);
    layout();
  },
  onServerMessage: (msg: ServerMessage) => {
    switch (msg.type) {
      case "windows": {
        select.length = 1; // garde le placeholder
        windowContexts.clear();
        workflow.setWindows(msg.items);
        
        const mainContainer = document.getElementById("mainWindowsContainer") as HTMLDivElement;
        const virtualContainer = document.getElementById("virtualWindowsContainer") as HTMLDivElement;
        
        if (mainContainer && virtualContainer) {
          mainContainer.innerHTML = "";
          virtualContainer.innerHTML = "";
          
          let mainCount = 0;
          let virtualCount = 0;
          
          for (const w of msg.items) {
            windowContexts.set(w.id, w.context);
            // Select dropdown classique
            const opt = document.createElement("option");
            opt.value = String(w.id);
            opt.textContent = (w.locked ? "🔒 " : "") + (w.is_desktop ? "🖥 " : (w.is_virtual ? "[V] " : "")) + w.title;
            opt.dataset.locked = w.locked ? "true" : "false";
            select.appendChild(opt);
            
            // Conteneur de ligne
            const rowDiv = document.createElement("div");
            rowDiv.style.display = "flex";
            rowDiv.style.alignItems = "center";
            rowDiv.style.gap = "0.4rem";
            rowDiv.style.width = "100%";
            
            // Ligne de switch dans les settings
            const btn = document.createElement("button");
            btn.type = "button";
            btn.style.flex = "1";
            btn.style.textAlign = "left";
            btn.style.padding = "0.4rem 0.55rem";
            btn.style.fontSize = "0.82rem";
            btn.style.background = "#1d2026";
            btn.style.border = "1px solid #3a3f49";
            btn.style.borderRadius = "4px";
            btn.style.color = "#e6e6e6";
            btn.style.cursor = "pointer";
            btn.style.whiteSpace = "nowrap";
            btn.style.overflow = "hidden";
            btn.style.textOverflow = "ellipsis";
            btn.style.transition = "background 0.12s, border-color 0.12s";
            btn.style.display = "flex";
            btn.style.alignItems = "center";
            btn.style.gap = "0.35rem";
            
            btn.addEventListener("mouseover", () => {
              btn.style.background = "#2a2f38";
              btn.style.borderColor = "#4a505c";
            });
            btn.addEventListener("mouseout", () => {
              btn.style.background = "#1d2026";
              btn.style.borderColor = "#3a3f49";
            });
            
            btn.innerHTML = `<span>${w.locked ? "🔒 " : (w.is_desktop ? "🖥 " : "")}</span><span style="flex: 1; overflow: hidden; text-overflow: ellipsis;">${w.title}</span><span style="font-size: 0.7rem; opacity: 0.5;">Stream ➜</span>`;
            
            btn.addEventListener("click", () => {
              selectWindowAndStream(w.id, w.locked);
            });
            
            // Bouton de fermeture
            const closeBtn = document.createElement("button");
            closeBtn.type = "button";
            closeBtn.style.padding = "0.4rem 0.6rem";
            closeBtn.style.fontSize = "0.82rem";
            closeBtn.style.background = "#2a1e1e";
            closeBtn.style.border = "1px solid #5a2e2e";
            closeBtn.style.borderRadius = "4px";
            closeBtn.style.color = "#ff8888";
            closeBtn.style.cursor = "pointer";
            closeBtn.style.transition = "background 0.12s, border-color 0.12s";
            closeBtn.style.fontWeight = "bold";
            closeBtn.title = "Fermer cette application";
            closeBtn.hidden = w.is_desktop;
            
            closeBtn.addEventListener("mouseover", () => {
              closeBtn.style.background = "#4a2424";
              closeBtn.style.borderColor = "#7a3434";
            });
            closeBtn.addEventListener("mouseout", () => {
              closeBtn.style.background = "#2a1e1e";
              closeBtn.style.borderColor = "#5a2e2e";
            });
            
            closeBtn.innerHTML = "❌";
            closeBtn.addEventListener("click", (e) => {
              e.stopPropagation();
              pendingCloseWindowId = w.id;
              confirmCloseText.textContent = `Voulez-vous vraiment fermer la fenêtre "${w.title}" ?`;
              confirmCloseModal.style.display = "flex";
            });
            
            rowDiv.appendChild(btn);
            rowDiv.appendChild(closeBtn);
            
            if (w.is_virtual) {
              virtualContainer.appendChild(rowDiv);
              virtualCount++;
            } else {
              mainContainer.appendChild(rowDiv);
              mainCount++;
            }
          }
          
          if (mainCount === 0) {
            mainContainer.innerHTML = `<div style="font-size: 0.8rem; opacity: 0.5; font-style: italic; padding: 0.2rem;">No windows found</div>`;
          }
          if (virtualCount === 0) {
            virtualContainer.innerHTML = `<div style="font-size: 0.8rem; opacity: 0.5; font-style: italic; padding: 0.2rem;">No windows found</div>`;
          }
        } else {
          // Fallback au select classique si les containers ne sont pas chargés
          for (const w of msg.items) {
            windowContexts.set(w.id, w.context);
            const opt = document.createElement("option");
            opt.value = String(w.id);
            opt.textContent = (w.locked ? "🔒 " : "") + w.title;
            opt.dataset.locked = w.locked ? "true" : "false";
            select.appendChild(opt);
          }
        }
        break;
      }
      case "selected":
        hidePinModal();
        bgInput.checked = msg.background;
        updateVirtualCursor();
        // L'injection d'input cible la fenêtre native → coords client natives.
        // L'overlay d'annotation, lui, est calé sur la frame (voir onFrameSize).
        input.setNativeSize(msg.width, msg.height);
        if (lastSelectedId !== null) {
          const ctx = windowContexts.get(lastSelectedId) ?? "unknown";
          macros.setTarget(lastSelectedId, ctx, msg.width, msg.height);
          stream.send({ type: "get_profile", context: ctx });
        }
        statusEl.textContent = `streaming ${msg.width}×${msg.height}`;
        setRecording(false); // toute nouvelle sélection arrête l'enregistrement
        if (annot.activeTool() === "cursor") canvas.focus();
        break;
      case "screenshot_saved":
        statusEl.textContent = `capture: ${msg.path}`;
        break;
      case "recording_started":
        setRecording(true);
        statusEl.textContent = `recording (${msg.fps} fps) → ${msg.dir}`;
        break;
      case "recording_stopped":
        setRecording(false);
        statusEl.textContent = `sequence: ${msg.frames} frames → ${msg.dir}`;
        break;
      case "macro_saved":
        statusEl.textContent = `macro ${msg.name} sauvegardée → ${msg.path}`;
        break;
      case "macro_list":
        macros.showLibrary(msg.items);
        workflow.setMacros(msg.items);
        statusEl.textContent = `${msg.items.length} macro(s) disponible(s)`;
        break;
      case "macro_loaded":
        macros.load(msg.macro_file);
        break;
      case "macro_played":
        statusEl.textContent = `test terminé : ${msg.steps} étapes exécutées`;
        break;
      case "workflow_saved":
        statusEl.textContent = `workflow ${msg.name} sauvegardé → ${msg.path}`;
        stream.send({ type: "list_workflows" });
        break;
      case "workflow_list":
        workflow.showLibrary(msg.items);
        break;
      case "workflow_loaded":
        workflow.load(msg.workflow);
        break;
      case "workflow_started":
        statusEl.textContent = `workflow ${msg.name} démarré`;
        break;
      case "workflow_node_started":
        workflow.nodeStarted(msg.node_id);
        statusEl.textContent = `workflow : ${msg.node_id}`;
        break;
      case "workflow_node_finished":
        break;
      case "workflow_finished":
        workflow.executionFinished(`workflow ${msg.name} terminé : ${msg.nodes_executed} nœuds`);
        break;
      case "workflow_failed":
        workflow.executionFinished(`workflow échoué${msg.node_id ? ` sur ${msg.node_id}` : ""} : ${msg.message}`);
        break;
      case "layout_saved":
        statusEl.textContent = `layout ${msg.name} sauvegardé pour ${msg.context} → ${msg.path}`;
        layoutEditor.saved(msg.context, msg.name);
        stream.send({ type: "list_layouts", context: msg.context });
        break;
      case "layout_list":
        layoutEditor.showLibrary(msg.items);
        break;
      case "layout_loaded":
        layoutEditor.load(msg.layout);
        break;
      case "state_saved":
        statusEl.textContent = `state ${msg.name} captured for ${msg.context} (${msg.signatures} signature(s))`;
        layoutEditor.refreshStates();
        break;
      case "state_list":
        layoutEditor.showStates(msg.items);
        break;
      case "state_deleted":
        statusEl.textContent = `state ${msg.name} deleted`;
        layoutEditor.refreshStates();
        break;
      case "profile_saved":
        statusEl.textContent = `profil d'application sauvegardé pour ${msg.context}`;
        break;
      case "profile_loaded": {
        const p = msg.profile;
        const lbl = document.getElementById("profileContextLabel");
        if (lbl) lbl.textContent = p.context;
        const fg = document.getElementById("profileForegroundRequired") as HTMLInputElement | null;
        if (fg) fg.checked = p.foregroundRequired;
        const act = document.getElementById("profilePreferredActivation") as HTMLSelectElement | null;
        if (act) act.value = p.preferredActivation;
        const bm = document.getElementById("profileBackgroundMouse") as HTMLSelectElement | null;
        if (bm) bm.value = p.backgroundMouse;
        const bk = document.getElementById("profileBackgroundKeyboard") as HTMLSelectElement | null;
        if (bk) bk.value = p.backgroundKeyboard;
        break;
      }
      case "notice":
        statusEl.textContent = `⚙️ ${msg.message}`;
        break;
      case "error":
        if (msg.message === "PIN_REQUIRED") {
          if (pendingWindowId !== null) {
            pinError.textContent = "Incorrect PIN code. Try again.";
            pinInput.value = "";
            pinInput.focus();
          } else {
            const id = Number(select.value);
            if (id) {
              showPinModal(id);
              pinError.textContent = "Security PIN is required.";
            }
          }
        } else {
          statusEl.textContent = `error: ${msg.message}`;
        }
        break;
    }
  },
});

const macros = new MacroController(
  document.getElementById("macroPanel") as HTMLElement,
  (message) => stream.send(message),
  layout,
  (message) => (statusEl.textContent = message),
  (isRecording) => {
    if (!isRecording) return;
    // Une macro enregistre des interactions distantes : repasse toujours en
    // mode contrôle, même si un outil d’annotation était encore sélectionné.
    setTool("cursor");
    input.setEnabled(true);
    canvas.focus();
  },
);
const layoutEditor = new LayoutController(
  document.getElementById("layoutPanel") as HTMLElement,
  annot,
  (message) => stream.send(message),
  (tool) => setTool(tool),
  layout,
  (message) => (statusEl.textContent = message),
);
const workflow = new WorkflowEditor(
  document.getElementById("workflowEditor") as HTMLElement,
  (message) => stream.send(message),
  (open) => {
    const main = document.querySelector("#workspace > main") as HTMLElement;
    main.hidden = open;
    if (open) {
      macros.close();
      layoutEditor.close();
      toolbarEl.hidden = true;
    } else {
      applyToolbarVisible();
      layout();
    }
  },
  (message) => (statusEl.textContent = message),
);
input.setObserver((message, text) => macros.observe(message, text));
(document.getElementById("toggleMacros") as HTMLButtonElement).addEventListener("click", () => {
  layoutEditor.close();
  macros.togglePanel();
});
(document.getElementById("toggleLayout") as HTMLButtonElement).addEventListener("click", () => {
  macros.close();
  workflow.close();
  layoutEditor.toggle();
});
(document.getElementById("toggleWorkflow") as HTMLButtonElement).addEventListener("click", () => workflow.toggle());

input.attach();
stream.connect();
layout();

// ── Barre d'outils d'annotation ──────────────────────────────────────────────

// Doit être initialisé avant applyToolbarVisible(): si la toolbar est mémorisée
// comme masquée, setTool("cursor") appelle immédiatement updateVirtualCursor().
let lastMouseEvent: MouseEvent | null = null;

const toolButtons = [...document.querySelectorAll<HTMLButtonElement>("#toolbar [data-tool]")];

function setTool(t: Tool) {
  annot.setTool(t);
  // En mode annotation, l'injection distante est suspendue et l'overlay
  // intercepte le pointeur (pointer-events géré par l'Annotator).
  input.setEnabled(t === "cursor");
  stage.dataset.tool = t;
  for (const b of toolButtons) b.classList.toggle("active", b.dataset.tool === t);
  // La gouttière des règles change l'espace dispo : on recalcule la mise en page.
  layout();
  if (t === "cursor") canvas.focus();
  else canvas.blur();
  updateVirtualCursor();
}

for (const b of toolButtons) {
  b.addEventListener("click", () => setTool(b.dataset.tool as Tool));
}

// ── Bascule de la barre d'outils d'annotation ────────────────────────────────
const toolbarEl = document.getElementById("toolbar") as HTMLDivElement;
const toggleToolbarBtn = document.getElementById("toggleToolbar") as HTMLButtonElement;
let toolbarVisible = localStorage.getItem("appmirror.toolbar") !== "hidden";
function applyToolbarVisible() {
  toolbarEl.hidden = !toolbarVisible;
  toggleToolbarBtn.classList.toggle("active", toolbarVisible);
  // En masquant la barre, on revient au contrôle distant (pas d'outil orphelin).
  if (!toolbarVisible) setTool("cursor");
}
toggleToolbarBtn.addEventListener("click", () => {
  toolbarVisible = !toolbarVisible;
  localStorage.setItem("appmirror.toolbar", toolbarVisible ? "shown" : "hidden");
  applyToolbarVisible();
});
applyToolbarVisible();

// ── Fenêtre modale des réglages ──────────────────────────────────────────────
const settingsModal = document.getElementById("settingsModal") as HTMLDivElement;
(document.getElementById("openSettings") as HTMLButtonElement).addEventListener("click", () => {
  settingsModal.style.display = "flex";
});

confirmCloseCancel.addEventListener("click", () => {
  pendingCloseWindowId = null;
  confirmCloseModal.style.display = "none";
});

confirmCloseSubmit.addEventListener("click", () => {
  if (pendingCloseWindowId !== null) {
    stream.send({ type: "close_window", id: pendingCloseWindowId });
    pendingCloseWindowId = null;
    confirmCloseModal.style.display = "none";
  }
});

(document.getElementById("settingsClose") as HTMLButtonElement).addEventListener("click", () => {
  settingsModal.style.display = "none";
});
settingsModal.addEventListener("click", (e) => {
  if (e.target === settingsModal) settingsModal.style.display = "none";
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && settingsModal.style.display === "flex") {
    settingsModal.style.display = "none";
  }
});

const colorInput = document.getElementById("annColor") as HTMLInputElement;
const widthInput = document.getElementById("annWidth") as HTMLInputElement;
const widthVal = document.getElementById("annWidthVal") as HTMLSpanElement;
const fillInput = document.getElementById("annFill") as HTMLInputElement;
const fillOpacityInput = document.getElementById("annFillOpacity") as HTMLInputElement;
const fillOpacityVal = document.getElementById("annFillOpacityVal") as HTMLSpanElement;

colorInput.addEventListener("input", () => annot.setColor(colorInput.value));
widthInput.addEventListener("input", () => {
  widthVal.textContent = widthInput.value;
  annot.setStrokeWidth(Number(widthInput.value));
});

// Le curseur d'opacité ne concerne que le remplissage : on le grise tant que
// "Fill" est décoché pour montrer la dépendance.
function syncFillOpacityEnabled() {
  fillOpacityInput.disabled = !fillInput.checked;
}
fillInput.addEventListener("change", () => {
  annot.setFill(fillInput.checked);
  syncFillOpacityEnabled();
});
fillOpacityInput.addEventListener("input", () => {
  const pct = Number(fillOpacityInput.value);
  fillOpacityVal.textContent = `${pct}%`;
  annot.setFillOpacity(pct / 100);
});
// État initial : slider aligné sur la valeur par défaut de l'Annotator (30%).
annot.setFillOpacity(Number(fillOpacityInput.value) / 100);
syncFillOpacityEnabled();

(document.getElementById("undoBtn") as HTMLButtonElement).addEventListener("click", () =>
  annot.undo(),
);
(document.getElementById("redoBtn") as HTMLButtonElement).addEventListener("click", () =>
  annot.redo(),
);
(document.getElementById("clearBtn") as HTMLButtonElement).addEventListener("click", () =>
  annot.clearAll(),
);
(document.getElementById("exportBtn") as HTMLButtonElement).addEventListener("click", () => {
  void annot.exportPng(canvas).catch((e) => (statusEl.textContent = `export: ${e}`));
});
// ── Actions existantes ───────────────────────────────────────────────────────

refreshBtn.addEventListener("click", () => stream.send({ type: "list_windows" }));

connectBtn.addEventListener("click", () => {
  const id = Number(select.value);
  if (!id) {
    statusEl.textContent = "select a window first";
    return;
  }
  const selectedOpt = select.options[select.selectedIndex];
  const isLocked = selectedOpt.dataset.locked === "true";

  if (isLocked) {
    showPinModal(id);
  } else {
    sendSelect(id, null);
  }
});

function updateVirtualCursor(e?: MouseEvent) {
  if (e) {
    lastMouseEvent = e;
  }
  const isCursorHidden = !showCursorInput.checked;
  const isBgInput = bgInput.checked;
  const isCursorTool = annot.activeTool() === "cursor";
  const shouldShow = (isCursorHidden || isBgInput) && isCursorTool;

  if (shouldShow && lastMouseEvent) {
    // On se cale sur le rect du CANVAS (= boîte de contenu, à l'intérieur de la
    // bordure 1px de #stage), pas sur #stage : left/top:0 du curseur virtuel sont
    // relatifs à cette même boîte, donc la pointe colle exactement au pointeur
    // (et à la cible du clic, qui utilise aussi le rect du canvas).
    const rect = canvas.getBoundingClientRect();
    const x = lastMouseEvent.clientX - rect.left;
    const y = lastMouseEvent.clientY - rect.top;
    virtualCursor.style.left = `${x}px`;
    virtualCursor.style.top = `${y}px`;
    virtualCursor.style.display = "block";
  } else {
    virtualCursor.style.display = "none";
  }
}

stage.addEventListener("mousemove", (e) => updateVirtualCursor(e));
stage.addEventListener("mouseenter", (e) => updateVirtualCursor(e));
stage.addEventListener("mouseleave", () => {
  virtualCursor.style.display = "none";
});

// Curseur distant : bascule à chaud (l'option WGC est fixée au démarrage de la
// capture, donc on relance le stream de la fenêtre courante avec le nouveau réglage).
showCursorInput.addEventListener("change", () => {
  if (lastSelectedId !== null) {
    sendSelect(lastSelectedId, lastPin);
  }
  updateVirtualCursor();
});

// Contrôle en arrière-plan : injection via PostMessage sans voler le focus hôte.
bgInput.addEventListener("change", () => {
  stream.send({ type: "set_input_mode", background: bgInput.checked });
  statusEl.textContent = bgInput.checked
    ? "background injection (host focus preserved)"
    : "foreground injection (SendInput)";
  updateVirtualCursor();
});

// Toggle WebRTC : relance la capture pour basculer de flux
webrtcToggle.addEventListener("change", () => {
  if (lastSelectedId !== null) {
    sendSelect(lastSelectedId, lastPin);
  }
});

shotBtn.addEventListener("click", () =>
  stream.send({ type: "screenshot", dir: saveDir() }),
);

focusWindowBtn.addEventListener("click", () => {
  stream.send({ type: "focus_window" });
});

recBtn.addEventListener("click", () => {
  if (recording) {
    stream.send({ type: "stop_recording" });
  } else {
    const fps = recFpsInput ? parseInt(recFpsInput.value, 10) : 10;
    stream.send({ type: "start_recording", dir: saveDir(), fps });
  }
});

btnLaunchVirtual.addEventListener("click", () => {
  const command = launchCmdInput.value.trim();
  if (command) {
    stream.send({ type: "launch_app", command });
  }
});

btnSendActiveToVirtual.addEventListener("click", () => {
  stream.send({ type: "launch_active_on_virtual" });
});

document.getElementById("profileSaveBtn")?.addEventListener("click", () => {
  const context = document.getElementById("profileContextLabel")?.textContent ?? "";
  if (!context || context === "none") return;
  const profile = {
    context,
    foregroundRequired: (document.getElementById("profileForegroundRequired") as HTMLInputElement).checked,
    preferredActivation: (document.getElementById("profilePreferredActivation") as HTMLSelectElement).value,
    backgroundMouse: (document.getElementById("profileBackgroundMouse") as HTMLSelectElement).value,
    backgroundKeyboard: (document.getElementById("profileBackgroundKeyboard") as HTMLSelectElement).value,
    pauseOnFocusLoss: false,
  };
  stream.send({ type: "save_profile", profile });
});
