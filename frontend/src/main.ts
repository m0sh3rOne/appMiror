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

let pendingWindowId: number | null = null;

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

function submitPin() {
  const pin = pinInput.value;
  if (!pin) {
    pinError.textContent = "Please enter the PIN code";
    return;
  }
  if (pendingWindowId !== null) {
    stream.send({ type: "select_window", id: pendingWindowId, pin });
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

/** Ajuste #stage (letterbox) à la résolution native du canvas, sans upscale. */
function layout() {
  const nw = canvas.width || 1;
  const nh = canvas.height || 1;
  const availW = Math.max(50, viewport.clientWidth - 24);
  const availH = Math.max(50, viewport.clientHeight - 24);
  const scale = Math.min(availW / nw, availH / nh, 1);
  stage.style.width = `${Math.max(1, Math.round(nw * scale))}px`;
  stage.style.height = `${Math.max(1, Math.round(nh * scale))}px`;
  annot.layoutChanged(scale);
}
new ResizeObserver(layout).observe(viewport);

const stream = new StreamClient(canvas, {
  onStatus: (s) => (statusEl.textContent = s),
  // La taille des frames fait foi (la fenêtre distante peut être
  // redimensionnée en cours de stream, sans nouveau message "selected").
  onFrameSize: (w, h) => {
    input.setNativeSize(w, h);
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
        input.setNativeSize(msg.width, msg.height);
        annot.setNativeSize(msg.width, msg.height);
        statusEl.textContent = `streaming ${msg.width}×${msg.height}`;
        setRecording(false); // toute nouvelle sélection arrête l'enregistrement
        if (annot.activeTool() === "cursor") canvas.focus();
        break;
      case "screenshot_saved":
        statusEl.textContent = `capture : ${msg.path}`;
        break;
      case "recording_started":
        setRecording(true);
        statusEl.textContent = `enregistrement (${msg.fps} fps) → ${msg.dir}`;
        break;
      case "recording_stopped":
        setRecording(false);
        statusEl.textContent = `séquence : ${msg.frames} images → ${msg.dir}`;
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
          statusEl.textContent = `erreur: ${msg.message}`;
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
  void annot.exportPng(canvas).catch((e) => (statusEl.textContent = `export : ${e}`));
});
slicesJsonBtn.addEventListener("click", async () => {
  if (annot.sliceCount() === 0) {
    statusEl.textContent = "aucune zone de découpe";
    return;
  }
  try {
    await navigator.clipboard.writeText(annot.slicesJson());
    statusEl.textContent = `${annot.sliceCount()} zone(s) copiée(s) en JSON (px natifs)`;
  } catch {
    statusEl.textContent = "copie refusée par le navigateur";
  }
});

// ── Actions existantes ───────────────────────────────────────────────────────

refreshBtn.addEventListener("click", () => stream.send({ type: "list_windows" }));

connectBtn.addEventListener("click", () => {
  const id = Number(select.value);
  if (!id) {
    statusEl.textContent = "sélectionnez une fenêtre d'abord";
    return;
  }
  const selectedOpt = select.options[select.selectedIndex];
  const isLocked = selectedOpt.dataset.locked === "true";
  
  if (isLocked) {
    showPinModal(id);
  } else {
    stream.send({ type: "select_window", id });
  }
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

// Charge la liste dès que le WS est prêt.
setTimeout(() => stream.send({ type: "list_windows" }), 300);
