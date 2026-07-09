// Câblage de l'UI : sélection de fenêtre, connexion au flux, injection input,
// outils d'annotation vectoriels (overlay SVG aligné sur le canvas).

import { InputController } from "./input";
import { StreamClient } from "./stream";
import { Annotator, type Tool } from "./annotate";
import { Rulers } from "./rulers";
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
const showCursorInput = document.getElementById("showCursor") as HTMLInputElement;
const bgInput = document.getElementById("bgInput") as HTMLInputElement;

let pendingWindowId: number | null = null;
// Dernière sélection (pour re-streamer quand une option de capture change).
let lastSelectedId: number | null = null;
let lastPin: string | null = null;

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

function getQualitySettings(): { quality: number; scale: number } {
  const val = qualitySelect.value;
  if (val === "high") {
    return { quality: 75, scale: 1.0 };
  } else if (val === "low") {
    return { quality: 45, scale: 0.5 };
  } else {
    return { quality: 60, scale: 0.7 };
  }
}

/** Envoi centralisé de select_window : porte qualité/échelle/curseur + PIN. */
function sendSelect(id: number, pin: string | null) {
  const { quality, scale } = getQualitySettings();
  lastSelectedId = id;
  lastPin = pin;
  stream.send({
    type: "select_window",
    id,
    pin,
    quality,
    scale,
    show_cursor: showCursorInput.checked,
  });
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
  const availW = Math.max(50, viewport.clientWidth - 26);
  const availH = Math.max(50, viewport.clientHeight - 26);
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
    layout();
  },
  onServerMessage: (msg: ServerMessage) => {
    switch (msg.type) {
      case "windows":
        select.length = 1; // garde le placeholder
        for (const w of msg.items) {
          const opt = document.createElement("option");
          opt.value = String(w.id);
          opt.textContent = (w.locked ? "🔒 " : "") + w.title;
          opt.dataset.locked = w.locked ? "true" : "false";
          select.appendChild(opt);
        }
        break;
      case "selected":
        hidePinModal();
        // L'injection d'input cible la fenêtre native → coords client natives.
        // L'overlay d'annotation, lui, est calé sur la frame (voir onFrameSize).
        input.setNativeSize(msg.width, msg.height);
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

input.attach();
stream.connect();
layout();

// ── Barre d'outils d'annotation ──────────────────────────────────────────────

const toolButtons = [...document.querySelectorAll<HTMLButtonElement>("#toolbar [data-tool]")];
const slicesJsonBtn = document.getElementById("slicesJsonBtn") as HTMLButtonElement;

function setTool(t: Tool) {
  annot.setTool(t);
  // En mode annotation, l'injection distante est suspendue et l'overlay
  // intercepte le pointeur (pointer-events géré par l'Annotator).
  input.setEnabled(t === "cursor");
  stage.dataset.tool = t;
  slicesJsonBtn.hidden = t !== "slice";
  for (const b of toolButtons) b.classList.toggle("active", b.dataset.tool === t);
  if (t === "cursor") canvas.focus();
  else canvas.blur();
}

for (const b of toolButtons) {
  b.addEventListener("click", () => setTool(b.dataset.tool as Tool));
}

const colorInput = document.getElementById("annColor") as HTMLInputElement;
const widthInput = document.getElementById("annWidth") as HTMLInputElement;
const widthVal = document.getElementById("annWidthVal") as HTMLSpanElement;
const fillInput = document.getElementById("annFill") as HTMLInputElement;

colorInput.addEventListener("input", () => annot.setColor(colorInput.value));
widthInput.addEventListener("input", () => {
  widthVal.textContent = widthInput.value;
  annot.setStrokeWidth(Number(widthInput.value));
});
fillInput.addEventListener("change", () => annot.setFill(fillInput.checked));

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
slicesJsonBtn.addEventListener("click", async () => {
  if (annot.sliceCount() === 0) {
    statusEl.textContent = "no slice zones";
    return;
  }
  try {
    await navigator.clipboard.writeText(annot.slicesJson());
    statusEl.textContent = `${annot.sliceCount()} zone(s) copied as JSON (native px)`;
  } catch {
    statusEl.textContent = "clipboard blocked by the browser";
  }
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

// Curseur distant : bascule à chaud (l'option WGC est fixée au démarrage de la
// capture, donc on relance le stream de la fenêtre courante avec le nouveau réglage).
showCursorInput.addEventListener("change", () => {
  if (lastSelectedId !== null) {
    sendSelect(lastSelectedId, lastPin);
  }
});

// Contrôle en arrière-plan : injection via PostMessage sans voler le focus hôte.
bgInput.addEventListener("change", () => {
  stream.send({ type: "set_input_mode", background: bgInput.checked });
  statusEl.textContent = bgInput.checked
    ? "background injection (host focus preserved)"
    : "foreground injection (SendInput)";
});

shotBtn.addEventListener("click", () =>
  stream.send({ type: "screenshot", dir: saveDir() }),
);

recBtn.addEventListener("click", () => {
  if (recording) {
    stream.send({ type: "stop_recording" });
  } else {
    stream.send({ type: "start_recording", dir: saveDir(), fps: 10 });
  }
});
