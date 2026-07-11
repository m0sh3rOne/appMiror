// Couche d'annotation vectorielle superposée au canvas vidéo.
//
// Toutes les géométries sont stockées en PIXELS NATIFS de la fenêtre capturée
// (le viewBox du SVG suit la résolution native) : les annotations restent
// "collées" à l'image quelle que soit l'échelle d'affichage. Les éléments
// d'interface (poignées, pointillés, étiquettes) sont dimensionnés en unités
// natives via u() = px CSS / échelle pour garder une taille constante à l'écran.
//
// Purement frontend : rien ici ne touche au protocole WebSocket. Quand un
// outil d'annotation est actif, l'injection d'input distant est suspendue
// (voir main.ts) et l'overlay intercepte le pointeur.

import { Rulers } from "./rulers";

export type Tool =
  | "cursor" // contrôle distant (comportement historique)
  | "select"
  | "text"
  | "rect"
  | "ellipse"
  | "line"
  | "arrow"
  | "pen"
  | "marker"
  | "ruler"
  | "slice";

interface BaseShape {
  id: number;
  color: string;
  width: number; // épaisseur de trait, en px natifs
}
interface BoxShape extends BaseShape {
  kind: "rect" | "ellipse";
  x: number;
  y: number;
  w: number;
  h: number;
  fill: boolean;
  fillOpacity: number; // opacité du remplissage, 0..1
}
interface LineShape extends BaseShape {
  kind: "line" | "arrow";
  x1: number;
  y1: number;
  x2: number;
  y2: number;
}
interface PathShape extends BaseShape {
  kind: "pen" | "marker";
  pts: number[]; // liste plate [x0,y0, x1,y1, ...]
}
interface TextShape extends BaseShape {
  kind: "text";
  x: number;
  y: number;
  size: number; // taille de fonte en px natifs
  text: string;
}
export type Shape = BoxShape | LineShape | PathShape | TextShape;

export interface Slice {
  id: number;
  x: number;
  y: number;
  w: number;
  h: number;
  name: string; // libellé optionnel de la zone ("" = sans nom)
}
export interface Guide {
  id: number;
  /** "x" = guide vertical (fixe une abscisse), "y" = guide horizontal. */
  axis: "x" | "y";
  pos: number;
}

interface Model {
  shapes: Shape[];
  slices: Slice[];
  guides: Guide[];
}

interface Pt {
  x: number;
  y: number;
}

type DragState =
  | { t: "shape"; ox: number; oy: number; shape: BoxShape | LineShape }
  | { t: "path"; shape: PathShape }
  | { t: "move-shape"; id: number; ox: number; oy: number; orig: Shape }
  | { t: "move-slice"; id: number; ox: number; oy: number; orig: Slice }
  | { t: "resize-slice"; id: number }
  | { t: "new-slice"; ox: number; oy: number; slice: Slice }
  | { t: "move-guide"; id: number }
  | { t: "measure" };

interface EditorMeta {
  pos: Pt;
  size: number;
  color: string;
  orig: TextShape | null; // édition d'un texte existant
  origIndex: number;
}

const SVG_NS = "http://www.w3.org/2000/svg";

function el(name: string, attrs: Record<string, string | number> = {}): SVGElement {
  const n = document.createElementNS(SVG_NS, name);
  for (const [k, v] of Object.entries(attrs)) n.setAttribute(k, String(v));
  return n;
}

const r1 = (n: number) => Math.round(n * 10) / 10;

function pathD(pts: number[]): string {
  if (pts.length < 2) return "";
  let d = `M${r1(pts[0])} ${r1(pts[1])}`;
  for (let i = 2; i + 1 < pts.length; i += 2) d += `L${r1(pts[i])} ${r1(pts[i + 1])}`;
  return d;
}

/** Rectangle normalisé (w/h >= 0) à partir d'un rect potentiellement inversé. */
function norm(b: { x: number; y: number; w: number; h: number }) {
  return {
    x: b.w < 0 ? b.x + b.w : b.x,
    y: b.h < 0 ? b.y + b.h : b.y,
    w: Math.abs(b.w),
    h: Math.abs(b.h),
  };
}

const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

/** Type guard : les kinds multi-littéraux ne se narrowent pas par négation. */
function isLineKind(s: BoxShape | LineShape): s is LineShape {
  return s.kind === "line" || s.kind === "arrow";
}

export interface AnnotatorOptions {
  stage: HTMLElement;
  overlay: SVGSVGElement;
  rulerTop: HTMLCanvasElement;
  rulerLeft: HTMLCanvasElement;
  rulers: Rulers;
  onStatus: (msg: string) => void;
}

export class Annotator {
  private readonly stage: HTMLElement;
  private readonly overlay: SVGSVGElement;
  private readonly rulers: Rulers;
  private readonly onStatus: (msg: string) => void;

  private model: Model = { shapes: [], slices: [], guides: [] };
  private history: string[] = [];
  private future: string[] = [];
  private pendingSnapshot: string | null = null;

  private tool: Tool = "cursor";
  private color = "#ff3355";
  private strokeWidth = 4;
  private fillShapes = false;
  private fillOpacity = 0.3; // opacité du remplissage des formes, 0..1

  private nativeW = 1280;
  private nativeH = 720;
  private scale = 1;
  private nextId = 1;

  private drag: DragState | null = null;
  private sel: { t: "shape" | "slice"; id: number } | null = null;
  private measure: { x1: number; y1: number; x2: number; y2: number } | null = null;
  private guideDraft: Guide | null = null;

  private editor: HTMLTextAreaElement | null = null;
  private editorMeta: EditorMeta | null = null;

  // Éditeur de nom de zone (slice) : un <input> flottant au-dessus de la slice.
  private sliceNameEditor: HTMLInputElement | null = null;
  private sliceNameEditorId: number | null = null;

  // Groupes SVG (reconstruits par render()).
  private gShapes: SVGElement = el("g");
  private gVeil: SVGElement = el("g");
  private gSlices: SVGElement = el("g");
  private gGuides: SVGElement = el("g");
  private gDraft: SVGElement = el("g");
  private gSel: SVGElement = el("g");

  constructor(opts: AnnotatorOptions) {
    this.stage = opts.stage;
    this.overlay = opts.overlay;
    this.rulers = opts.rulers;
    this.onStatus = opts.onStatus;

    const o = this.overlay;
    o.addEventListener("pointerdown", (e) => this.onPointerDown(e));
    o.addEventListener("pointermove", (e) => this.onPointerMove(e));
    o.addEventListener("pointerup", (e) => this.onPointerUp(e));
    o.addEventListener("pointercancel", () => this.cancelDrag());
    o.addEventListener("pointerleave", () => this.rulers.setPointer(null));
    o.addEventListener("dblclick", (e) => this.onDblClick(e));
    o.addEventListener("contextmenu", (e) => {
      if (this.tool !== "cursor") e.preventDefault();
    });

    // Tirer depuis la règle du haut => guide horizontal ; depuis la gauche => vertical.
    this.attachGuideDrag(opts.rulerTop, "y");
    this.attachGuideDrag(opts.rulerLeft, "x");

    document.addEventListener("keydown", (e) => this.onKeyDown(e));
    this.render();
  }

  // ── API publique ──────────────────────────────────────────────────────────

  activeTool(): Tool {
    return this.tool;
  }

  setTool(t: Tool) {
    if (t === this.tool) return;
    this.commitEditor();
    this.commitSliceName();
    this.drag = null;
    this.guideDraft = null;
    this.sel = null;
    if (t !== "ruler") this.measure = null;
    this.tool = t;
    this.rulers.setVisible(t === "ruler");
    this.overlay.style.pointerEvents = t === "cursor" ? "none" : "auto";
    this.render();
    this.onStatus(TOOL_HINTS[t]);
  }

  setColor(c: string) {
    this.color = c;
  }
  setStrokeWidth(w: number) {
    this.strokeWidth = w;
  }
  setFill(f: boolean) {
    this.fillShapes = f;
  }
  /** Opacité du remplissage (0..1) pour les prochaines formes. */
  setFillOpacity(o: number) {
    this.fillOpacity = Math.max(0, Math.min(1, o));
  }

  /** Résolution native de la source. Un changement efface les annotations
   *  (elles étaient calées sur l'ancienne fenêtre) — récupérables via undo. */
  setNativeSize(w: number, h: number) {
    if (w <= 0 || h <= 0 || (w === this.nativeW && h === this.nativeH)) return;
    const empty =
      this.model.shapes.length === 0 &&
      this.model.slices.length === 0 &&
      this.model.guides.length === 0;
    if (!empty) this.pushHistory();
    this.model = { shapes: [], slices: [], guides: [] };
    this.measure = null;
    this.sel = null;
    this.nativeW = w;
    this.nativeH = h;
    this.rulers.layout(this.scale, w, h);
    this.render();
  }

  /** À appeler quand la taille d'affichage change (échelle px CSS / px natif). */
  layoutChanged(scale: number) {
    this.scale = scale;
    this.rulers.layout(scale, this.nativeW, this.nativeH);
    this.positionEditor();
    this.positionSliceNameEditor();
    this.render();
  }

  undo() {
    this.commitEditor();
    const prev = this.history.pop();
    if (prev === undefined) {
      this.onStatus("nothing to undo");
      return;
    }
    this.future.push(JSON.stringify(this.model));
    this.model = JSON.parse(prev) as Model;
    this.sel = null;
    this.render();
  }

  redo() {
    const next = this.future.pop();
    if (next === undefined) {
      this.onStatus("nothing to redo");
      return;
    }
    this.history.push(JSON.stringify(this.model));
    this.model = JSON.parse(next) as Model;
    this.sel = null;
    this.render();
  }

  clearAll() {
    this.commitEditor();
    const m = this.model;
    if (m.shapes.length === 0 && m.slices.length === 0 && m.guides.length === 0) return;
    this.pushHistory();
    this.model = { shapes: [], slices: [], guides: [] };
    this.measure = null;
    this.sel = null;
    this.render();
    this.onStatus("annotations cleared (Ctrl+Z to restore)");
  }

  /** Zones de découpe en px natifs, prêtes à coller (JSON). */
  slicesJson(): string {
    const zones = this.model.slices.map((s, i) => ({
      index: i + 1,
      name: s.name || undefined,
      x: Math.round(s.x),
      y: Math.round(s.y),
      w: Math.round(s.w),
      h: Math.round(s.h),
    }));
    return JSON.stringify(zones, null, 2);
  }

  /** Importe des zones de découpe depuis un JSON et redessine l'overlay. */
  importSlicesJson(jsonStr: string) {
    this.commitEditor();
    try {
      const parsed = JSON.parse(jsonStr);
      if (!Array.isArray(parsed)) {
        throw new Error("JSON must be an array of slice objects");
      }
      
      const newSlices: Slice[] = [];
      let maxId = 0;
      for (const s of this.model.shapes) if (s.id > maxId) maxId = s.id;
      for (const s of this.model.slices) if (s.id > maxId) maxId = s.id;
      for (const g of this.model.guides) if (g.id > maxId) maxId = g.id;

      for (const item of parsed) {
        if (
          typeof item.x !== "number" ||
          typeof item.y !== "number" ||
          typeof item.w !== "number" ||
          typeof item.h !== "number"
        ) {
          throw new Error("Each slice must have numeric x, y, w, and h properties");
        }
        maxId++;
        newSlices.push({
          id: maxId,
          x: item.x,
          y: item.y,
          w: item.w,
          h: item.h,
          name: typeof item.name === "string" ? item.name : "",
        });
      }

      this.pushHistory();
      this.model.slices = newSlices;
      this.render();
      this.onStatus(`imported ${newSlices.length} slice zone(s)`);
    } catch (e: any) {
      throw new Error(`Import failed: ${e.message}`);
    }
  }

  sliceCount(): number {
    return this.model.slices.length;
  }

  /** Compose la frame vidéo + les annotations dans un PNG téléchargé. */
  async exportPng(source: HTMLCanvasElement) {
    const w = source.width;
    const h = source.height;
    if (w === 0 || h === 0) {
      this.onStatus("nothing to export: no stream");
      return;
    }
    const out = document.createElement("canvas");
    out.width = w;
    out.height = h;
    const ctx = out.getContext("2d");
    if (!ctx) return;
    ctx.drawImage(source, 0, 0);

    const cloneSvg = this.overlay.cloneNode(true) as SVGSVGElement;
    // Les couches transitoires (voile, brouillon, sélection) ne s'exportent pas.
    for (const id of ["veil", "draft", "sel", "guides"]) {
      cloneSvg.querySelector(`#${id}`)?.remove();
    }
    cloneSvg.setAttribute("width", String(w));
    cloneSvg.setAttribute("height", String(h));
    const svgText = new XMLSerializer().serializeToString(cloneSvg);
    const img = new Image();
    await new Promise<void>((resolve, reject) => {
      img.onload = () => resolve();
      img.onerror = () => reject(new Error("rasterisation SVG"));
      img.src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svgText);
    });
    ctx.drawImage(img, 0, 0, w, h);

    const a = document.createElement("a");
    const ts = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
    a.download = `appmirror_annot_${ts}.png`;
    a.href = out.toDataURL("image/png");
    a.click();
    this.onStatus("annotated PNG downloaded");
  }

  // ── Conversion de coordonnées ────────────────────────────────────────────

  private toNative(e: { clientX: number; clientY: number }): Pt {
    const r = this.overlay.getBoundingClientRect();
    if (r.width === 0 || r.height === 0) return { x: 0, y: 0 };
    const x = ((e.clientX - r.left) / r.width) * this.nativeW;
    const y = ((e.clientY - r.top) / r.height) * this.nativeH;
    return {
      x: Math.max(0, Math.min(this.nativeW, x)),
      y: Math.max(0, Math.min(this.nativeH, y)),
    };
  }

  /** px CSS -> unités natives (pour garder une taille constante à l'écran). */
  private u(cssPx: number): number {
    return cssPx / Math.max(this.scale, 0.01);
  }

  // ── Historique ───────────────────────────────────────────────────────────

  private pushHistory() {
    this.history.push(JSON.stringify(this.model));
    this.future = [];
  }

  // ── Souris : dessin, sélection, mesure, slices ───────────────────────────

  private onPointerDown(e: PointerEvent) {
    if (this.tool === "cursor" || e.button !== 0) return;
    e.preventDefault();
    this.commitEditor();
    this.overlay.setPointerCapture(e.pointerId);
    const p = this.toNative(e);
    const hit = (sel: string) => (e.target as Element).closest?.(sel);

    switch (this.tool) {
      case "select": {
        const g = hit("[data-shape]");
        if (g) {
          const id = Number(g.getAttribute("data-shape"));
          const shape = this.model.shapes.find((s) => s.id === id);
          if (shape) {
            this.sel = { t: "shape", id };
            this.pendingSnapshot = JSON.stringify(this.model);
            this.drag = { t: "move-shape", id, ox: p.x, oy: p.y, orig: clone(shape) };
          }
        } else {
          this.sel = null;
        }
        this.render();
        break;
      }
      case "text":
        this.openTextEditor(p);
        break;
      case "rect":
      case "ellipse": {
        const shape: BoxShape = {
          kind: this.tool,
          id: this.nextId++,
          color: this.color,
          width: this.strokeWidth,
          x: p.x,
          y: p.y,
          w: 0,
          h: 0,
          fill: this.fillShapes,
          fillOpacity: this.fillOpacity,
        };
        this.drag = { t: "shape", ox: p.x, oy: p.y, shape };
        break;
      }
      case "line":
      case "arrow": {
        const shape: LineShape = {
          kind: this.tool,
          id: this.nextId++,
          color: this.color,
          width: this.strokeWidth,
          x1: p.x,
          y1: p.y,
          x2: p.x,
          y2: p.y,
        };
        this.drag = { t: "shape", ox: p.x, oy: p.y, shape };
        break;
      }
      case "pen":
      case "marker": {
        const shape: PathShape = {
          kind: this.tool,
          id: this.nextId++,
          color: this.color,
          width: this.strokeWidth,
          pts: [p.x, p.y],
        };
        this.drag = { t: "path", shape };
        this.renderDraft();
        break;
      }
      case "ruler": {
        const g = hit("[data-guide]");
        if (g) {
          this.pendingSnapshot = JSON.stringify(this.model);
          this.drag = { t: "move-guide", id: Number(g.getAttribute("data-guide")) };
        } else {
          this.measure = { x1: p.x, y1: p.y, x2: p.x, y2: p.y };
          this.drag = { t: "measure" };
          this.renderDraft();
        }
        break;
      }
      case "slice": {
        const handle = hit("[data-slice-handle]");
        const g = handle ?? hit("[data-slice]");
        if (handle) {
          const id = Number(handle.getAttribute("data-slice-handle"));
          this.sel = { t: "slice", id };
          this.pendingSnapshot = JSON.stringify(this.model);
          this.drag = { t: "resize-slice", id };
        } else if (g) {
          const id = Number(g.getAttribute("data-slice"));
          const slice = this.model.slices.find((s) => s.id === id);
          if (slice) {
            this.sel = { t: "slice", id };
            this.pendingSnapshot = JSON.stringify(this.model);
            this.drag = { t: "move-slice", id, ox: p.x, oy: p.y, orig: clone(slice) };
          }
        } else {
          this.sel = null;
          const slice: Slice = { id: this.nextId++, x: p.x, y: p.y, w: 0, h: 0, name: "" };
          this.drag = { t: "new-slice", ox: p.x, oy: p.y, slice };
        }
        this.render();
        break;
      }
    }
  }

  private onPointerMove(e: PointerEvent) {
    const p = this.toNative(e);
    if (this.tool === "ruler") this.rulers.setPointer(p);
    const d = this.drag;
    if (!d) return;

    switch (d.t) {
      case "shape": {
        const s = d.shape;
        if (isLineKind(s)) {
          s.x2 = p.x;
          s.y2 = p.y;
        } else {
          s.w = p.x - d.ox;
          s.h = p.y - d.oy;
        }
        this.renderDraft();
        break;
      }
      case "path": {
        const pts = d.shape.pts;
        const lx = pts[pts.length - 2];
        const ly = pts[pts.length - 1];
        if (Math.hypot(p.x - lx, p.y - ly) >= 1.2) {
          pts.push(p.x, p.y);
          this.renderDraft();
        }
        break;
      }
      case "move-shape": {
        const idx = this.model.shapes.findIndex((s) => s.id === d.id);
        if (idx >= 0) {
          this.model.shapes[idx] = translated(d.orig, p.x - d.ox, p.y - d.oy);
          this.renderShapes();
          this.renderSelection();
        }
        break;
      }
      case "move-slice": {
        const sl = this.model.slices.find((s) => s.id === d.id);
        if (sl) {
          const o = d.orig;
          sl.x = Math.max(0, Math.min(this.nativeW - o.w, o.x + p.x - d.ox));
          sl.y = Math.max(0, Math.min(this.nativeH - o.h, o.y + p.y - d.oy));
          this.renderSlices();
          this.renderVeil();
          this.renderSelection();
        }
        break;
      }
      case "resize-slice": {
        const sl = this.model.slices.find((s) => s.id === d.id);
        if (sl) {
          sl.w = Math.max(4, p.x - sl.x);
          sl.h = Math.max(4, p.y - sl.y);
          this.renderSlices();
          this.renderVeil();
          this.renderSelection();
        }
        break;
      }
      case "new-slice":
        d.slice.x = Math.min(d.ox, p.x);
        d.slice.y = Math.min(d.oy, p.y);
        d.slice.w = Math.abs(p.x - d.ox);
        d.slice.h = Math.abs(p.y - d.oy);
        this.renderDraft();
        this.renderVeil();
        break;
      case "move-guide": {
        const gd = this.model.guides.find((g) => g.id === d.id);
        if (gd) {
          gd.pos = gd.axis === "x" ? p.x : p.y;
          this.renderGuides();
        }
        break;
      }
      case "measure":
        if (this.measure) {
          this.measure.x2 = p.x;
          this.measure.y2 = p.y;
          this.renderDraft();
        }
        break;
    }
  }

  private onPointerUp(e: PointerEvent) {
    const d = this.drag;
    this.drag = null;
    if (!d) return;
    e.preventDefault();

    switch (d.t) {
      case "shape": {
        const s = d.shape;
        const big = isLineKind(s)
          ? Math.hypot(s.x2 - s.x1, s.y2 - s.y1) >= 2
          : Math.abs(s.w) >= 2 && Math.abs(s.h) >= 2;
        if (big) {
          if (!isLineKind(s)) Object.assign(s, norm(s));
          this.pushHistory();
          this.model.shapes.push(s);
        }
        this.render();
        break;
      }
      case "path":
        if (d.shape.pts.length >= 4) {
          this.pushHistory();
          this.model.shapes.push(d.shape);
        }
        this.render();
        break;
      case "move-shape":
      case "move-slice":
      case "resize-slice":
        this.commitPending();
        this.render();
        break;
      case "move-guide": {
        const gd = this.model.guides.find((g) => g.id === d.id);
        // Relâché au bord = suppression du guide (geste Photoshop).
        if (gd && (gd.pos <= 1 || gd.pos >= (gd.axis === "x" ? this.nativeW : this.nativeH) - 1)) {
          this.model.guides = this.model.guides.filter((g) => g.id !== d.id);
        }
        this.commitPending();
        this.render();
        break;
      }
      case "new-slice": {
        const sl = d.slice;
        if (sl.w >= 8 && sl.h >= 8) {
          this.pushHistory();
          this.model.slices.push(sl);
          this.sel = { t: "slice", id: sl.id };
          this.onStatus(
            `zone ${this.model.slices.length}: ${Math.round(sl.w)}×${Math.round(sl.h)} px`,
          );
        }
        this.render();
        break;
      }
      case "measure":
        if (this.measure) {
          const m = this.measure;
          const dx = Math.round(Math.abs(m.x2 - m.x1));
          const dy = Math.round(Math.abs(m.y2 - m.y1));
          if (dx < 2 && dy < 2) {
            this.measure = null;
          } else {
            this.onStatus(`measure: ${dx} × ${dy} — ${Math.round(Math.hypot(dx, dy))} px`);
          }
        }
        this.renderDraft();
        break;
    }
  }

  /** Pousse pendingSnapshot dans l'historique si le modèle a réellement changé. */
  private commitPending() {
    if (this.pendingSnapshot !== null && this.pendingSnapshot !== JSON.stringify(this.model)) {
      this.history.push(this.pendingSnapshot);
      this.future = [];
    }
    this.pendingSnapshot = null;
  }

  private cancelDrag() {
    if (!this.drag) return;
    // Les déplacements en cours sont annulés en restaurant le snapshot.
    if (this.pendingSnapshot !== null) {
      this.model = JSON.parse(this.pendingSnapshot) as Model;
      this.pendingSnapshot = null;
    }
    this.drag = null;
    this.render();
  }

  private onDblClick(e: MouseEvent) {
    const hit = (sel: string) => (e.target as Element).closest?.(sel);
    if (this.tool === "select") {
      const g = hit("[data-shape]");
      if (g) {
        const shape = this.model.shapes.find((s) => s.id === Number(g.getAttribute("data-shape")));
        if (shape && shape.kind === "text") this.openTextEditor({ x: shape.x, y: shape.y }, shape);
      }
    } else if (this.tool === "ruler") {
      const g = hit("[data-guide]");
      if (g) {
        this.pushHistory();
        const id = Number(g.getAttribute("data-guide"));
        this.model.guides = this.model.guides.filter((gd) => gd.id !== id);
        this.render();
      }
    } else if (this.tool === "slice") {
      // Double-clic = renommer la zone (la suppression se fait via Suppr).
      const g = hit("[data-slice]");
      if (g) {
        const id = Number(g.getAttribute("data-slice"));
        const slice = this.model.slices.find((s) => s.id === id);
        if (slice) this.openSliceNameEditor(slice);
      }
    }
  }

  // ── Guides tirés depuis les règles ───────────────────────────────────────

  private attachGuideDrag(ruler: HTMLCanvasElement, axis: "x" | "y") {
    ruler.addEventListener("pointerdown", (e) => {
      if (this.tool !== "ruler" || e.button !== 0) return;
      e.preventDefault();
      ruler.setPointerCapture(e.pointerId);
      const p0 = this.toNative(e);
      const guide: Guide = { id: this.nextId++, axis, pos: axis === "x" ? p0.x : p0.y };
      this.guideDraft = guide;
      this.renderDraft();

      const move = (ev: PointerEvent) => {
        const p = this.toNative(ev);
        guide.pos = axis === "x" ? p.x : p.y;
        this.rulers.setPointer(p);
        this.renderDraft();
      };
      const up = () => {
        ruler.removeEventListener("pointermove", move);
        ruler.removeEventListener("pointerup", up);
        this.guideDraft = null;
        const max = axis === "x" ? this.nativeW : this.nativeH;
        if (guide.pos > 1 && guide.pos < max - 1) {
          this.pushHistory();
          this.model.guides.push(guide);
          this.onStatus(`${axis === "x" ? "vertical" : "horizontal"} guide at ${Math.round(guide.pos)} px`);
        }
        this.render();
      };
      ruler.addEventListener("pointermove", move);
      ruler.addEventListener("pointerup", up);
    });
  }

  // ── Clavier ──────────────────────────────────────────────────────────────

  private onKeyDown(e: KeyboardEvent) {
    if (this.tool === "cursor") return;
    if (e.target === this.editor) return; // le textarea gère ses propres touches
    const ctrl = e.ctrlKey || e.metaKey;
    const k = e.key.toLowerCase();
    if (ctrl && k === "z" && !e.shiftKey) {
      e.preventDefault();
      this.undo();
    } else if (ctrl && (k === "y" || (k === "z" && e.shiftKey))) {
      e.preventDefault();
      this.redo();
    } else if (e.key === "Delete" || e.key === "Backspace") {
      e.preventDefault();
      this.deleteSelection();
    } else if (e.key === "Escape") {
      this.escape();
    }
  }

  private deleteSelection() {
    if (!this.sel) return;
    this.pushHistory();
    if (this.sel.t === "shape") {
      this.model.shapes = this.model.shapes.filter((s) => s.id !== this.sel!.id);
    } else {
      this.model.slices = this.model.slices.filter((s) => s.id !== this.sel!.id);
    }
    this.sel = null;
    this.render();
  }

  private escape() {
    if (this.editor) {
      this.cancelEditor();
    } else if (this.drag) {
      this.cancelDrag();
    } else if (this.measure) {
      this.measure = null;
      this.renderDraft();
    } else if (this.sel) {
      this.sel = null;
      this.render();
    }
  }

  // ── Éditeur de texte (textarea HTML flottant) ────────────────────────────

  private openTextEditor(p: Pt, existing?: TextShape) {
    this.commitEditor();
    const meta: EditorMeta = existing
      ? {
          pos: { x: existing.x, y: existing.y },
          size: existing.size,
          color: existing.color,
          orig: existing,
          origIndex: this.model.shapes.indexOf(existing),
        }
      : {
          pos: p,
          size: 12 + this.strokeWidth * 3,
          color: this.color,
          orig: null,
          origIndex: -1,
        };
    if (existing) {
      // On retire la forme le temps de l'édition (elle revient au commit/cancel).
      this.pendingSnapshot = JSON.stringify(this.model);
      this.model.shapes.splice(meta.origIndex, 1);
      this.sel = null;
      this.render();
    }
    const ta = document.createElement("textarea");
    ta.className = "text-editor";
    ta.rows = 1;
    ta.value = existing?.text ?? "";
    ta.spellcheck = false;
    this.editor = ta;
    this.editorMeta = meta;
    this.positionEditor();
    this.stage.appendChild(ta);
    ta.focus();

    const autosize = () => {
      ta.style.height = "auto";
      ta.style.height = `${ta.scrollHeight}px`;
      ta.style.width = "auto";
      ta.style.width = `${Math.max(40, ta.scrollWidth + 4)}px`;
    };
    autosize();
    ta.addEventListener("input", autosize);
    ta.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        this.commitEditor();
      } else if (e.key === "Escape") {
        this.cancelEditor();
      }
    });
    ta.addEventListener("blur", () => {
      if (this.editor === ta) this.commitEditor();
    });
  }

  private positionEditor() {
    if (!this.editor || !this.editorMeta) return;
    const m = this.editorMeta;
    const s = this.scale;
    this.editor.style.left = `${m.pos.x * s}px`;
    this.editor.style.top = `${m.pos.y * s}px`;
    this.editor.style.fontSize = `${m.size * s}px`;
    this.editor.style.color = m.color;
  }

  private closeEditorDom() {
    const ta = this.editor;
    this.editor = null;
    this.editorMeta = null;
    ta?.remove();
  }

  private commitEditor() {
    if (!this.editor || !this.editorMeta) return;
    const meta = this.editorMeta;
    const text = this.editor.value.replace(/\s+$/, "");
    this.closeEditorDom();

    if (text.trim().length > 0) {
      if (meta.orig) {
        if (text !== meta.orig.text) this.commitPending();
        else this.pendingSnapshot = null;
        const updated: TextShape = { ...meta.orig, text };
        this.model.shapes.splice(Math.max(0, meta.origIndex), 0, updated);
      } else {
        this.pushHistory();
        this.model.shapes.push({
          kind: "text",
          id: this.nextId++,
          color: meta.color,
          width: this.strokeWidth,
          x: meta.pos.x,
          y: meta.pos.y,
          size: meta.size,
          text,
        });
      }
    } else if (meta.orig) {
      // Texte vidé = suppression (le snapshot contient encore l'original).
      this.commitPending();
    }
    this.render();
  }

  private cancelEditor() {
    if (!this.editor || !this.editorMeta) return;
    const meta = this.editorMeta;
    this.closeEditorDom();
    if (meta.orig) {
      this.model.shapes.splice(Math.max(0, meta.origIndex), 0, meta.orig);
      this.pendingSnapshot = null;
    }
    this.render();
  }

  // ── Éditeur de nom de zone (slice) ───────────────────────────────────────

  private openSliceNameEditor(slice: Slice) {
    this.commitEditor();
    this.closeSliceNameEditor();
    const input = document.createElement("input");
    input.type = "text";
    input.className = "slice-name-editor";
    input.value = slice.name;
    input.placeholder = "zone name";
    input.spellcheck = false;
    input.maxLength = 40;
    this.sliceNameEditor = input;
    this.sliceNameEditorId = slice.id;
    this.positionSliceNameEditor();
    this.stage.appendChild(input);
    input.focus();
    input.select();
    input.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") {
        e.preventDefault();
        this.commitSliceName();
      } else if (e.key === "Escape") {
        this.closeSliceNameEditor();
      }
    });
    input.addEventListener("blur", () => {
      if (this.sliceNameEditor === input) this.commitSliceName();
    });
  }

  private positionSliceNameEditor() {
    if (!this.sliceNameEditor || this.sliceNameEditorId === null) return;
    const sl = this.model.slices.find((s) => s.id === this.sliceNameEditorId);
    if (!sl) return;
    const r = norm(sl);
    const s = this.scale;
    this.sliceNameEditor.style.left = `${r.x * s}px`;
    this.sliceNameEditor.style.top = `${r.y * s}px`;
  }

  private commitSliceName() {
    if (!this.sliceNameEditor || this.sliceNameEditorId === null) return;
    const id = this.sliceNameEditorId;
    const name = this.sliceNameEditor.value.trim();
    this.closeSliceNameEditor();
    const sl = this.model.slices.find((s) => s.id === id);
    if (sl && sl.name !== name) {
      this.pushHistory();
      sl.name = name;
      this.render();
      this.onStatus(name ? `zone renamed “${name}”` : "zone name cleared");
    }
  }

  private closeSliceNameEditor() {
    const inp = this.sliceNameEditor;
    this.sliceNameEditor = null;
    this.sliceNameEditorId = null;
    inp?.remove();
  }

  // ── Rendu SVG ────────────────────────────────────────────────────────────

  private render() {
    const o = this.overlay;
    o.setAttribute("viewBox", `0 0 ${this.nativeW} ${this.nativeH}`);
    o.replaceChildren();
    this.gShapes = el("g", { id: "shapes" });
    this.gVeil = el("g", { id: "veil", "pointer-events": "none" });
    this.gSlices = el("g", { id: "slices" });
    this.gGuides = el("g", { id: "guides" });
    this.gDraft = el("g", { id: "draft", "pointer-events": "none" });
    this.gSel = el("g", { id: "sel", "pointer-events": "none" });
    o.append(this.gShapes, this.gVeil, this.gSlices, this.gGuides, this.gDraft, this.gSel);
    this.renderShapes();
    this.renderVeil();
    this.renderSlices();
    this.renderGuides();
    this.renderDraft();
    this.renderSelection();
  }

  private renderShapes() {
    this.gShapes.setAttribute(
      "pointer-events",
      this.tool === "select" ? "visiblePainted" : "none",
    );
    this.gShapes.replaceChildren(...this.model.shapes.map((s) => this.shapeNode(s)));
  }

  private renderVeil() {
    this.gVeil.replaceChildren();
    if (this.tool !== "slice") return;
    let d = `M0 0H${this.nativeW}V${this.nativeH}H0Z`;
    const holes = [...this.model.slices];
    if (this.drag?.t === "new-slice") holes.push(this.drag.slice);
    for (const sl of holes) {
      const r = norm(sl);
      if (r.w > 0 && r.h > 0) d += `M${r1(r.x)} ${r1(r.y)}h${r1(r.w)}v${r1(r.h)}h${r1(-r.w)}Z`;
    }
    this.gVeil.appendChild(el("path", { d, fill: "rgba(0,0,0,0.42)", "fill-rule": "evenodd" }));
  }

  private renderSlices() {
    const active = this.tool === "slice";
    this.gSlices.setAttribute("pointer-events", active ? "visiblePainted" : "none");
    this.gSlices.setAttribute("opacity", active ? "1" : "0.45");
    this.gSlices.replaceChildren(
      ...this.model.slices.map((sl, i) => this.sliceNode(sl, i, active)),
    );
  }

  private renderGuides() {
    const interactive = this.tool === "ruler";
    this.gGuides.setAttribute("pointer-events", interactive ? "visiblePainted" : "none");
    this.gGuides.replaceChildren(
      ...this.model.guides.map((g) => this.guideNode(g, interactive)),
    );
  }

  private renderDraft() {
    const kids: SVGElement[] = [];
    const d = this.drag;
    if (d?.t === "shape" || d?.t === "path") kids.push(this.shapeNode(d.shape, true));
    if (d?.t === "new-slice") kids.push(this.sliceNode(d.slice, this.model.slices.length, true));
    if (this.guideDraft) kids.push(this.guideNode(this.guideDraft));
    if (this.tool === "ruler" && this.measure) kids.push(this.measureNode());
    this.gDraft.replaceChildren(...kids);
  }

  private renderSelection() {
    this.gSel.replaceChildren();
    if (!this.sel) return;
    const attr = this.sel.t === "shape" ? "data-shape" : "data-slice";
    const node = this.overlay.querySelector(`[${attr}="${this.sel.id}"]`) as SVGGraphicsElement | null;
    if (!node) return;
    const b = node.getBBox();
    const pad = this.u(4);
    this.gSel.appendChild(
      el("rect", {
        x: b.x - pad,
        y: b.y - pad,
        width: b.width + pad * 2,
        height: b.height + pad * 2,
        fill: "none",
        stroke: "#3aa0ff",
        "stroke-width": this.u(1.5),
        "stroke-dasharray": `${this.u(5)} ${this.u(4)}`,
      }),
    );
  }

  private shapeNode(s: Shape, draft = false): SVGElement {
    const g = el("g", draft ? {} : { "data-shape": s.id });
    if (this.tool === "select" && !draft) (g as SVGElement & { style: CSSStyleDeclaration }).style.cursor = "move";
    const stroke = {
      stroke: s.color,
      "stroke-width": s.width,
      fill: "none",
    };
    switch (s.kind) {
      case "rect": {
        const r = norm(s);
        g.appendChild(
          el("rect", {
            ...stroke,
            x: r1(r.x),
            y: r1(r.y),
            width: r1(r.w),
            height: r1(r.h),
            ...(s.fill ? { fill: s.color, "fill-opacity": s.fillOpacity } : {}),
          }),
        );
        break;
      }
      case "ellipse": {
        const r = norm(s);
        g.appendChild(
          el("ellipse", {
            ...stroke,
            cx: r1(r.x + r.w / 2),
            cy: r1(r.y + r.h / 2),
            rx: r1(r.w / 2),
            ry: r1(r.h / 2),
            ...(s.fill ? { fill: s.color, "fill-opacity": s.fillOpacity } : {}),
          }),
        );
        break;
      }
      case "line":
        g.appendChild(
          el("line", {
            ...stroke,
            x1: r1(s.x1),
            y1: r1(s.y1),
            x2: r1(s.x2),
            y2: r1(s.y2),
            "stroke-linecap": "round",
          }),
        );
        break;
      case "arrow": {
        const dx = s.x2 - s.x1;
        const dy = s.y2 - s.y1;
        const len = Math.hypot(dx, dy) || 1;
        const ux = dx / len;
        const uy = dy / len;
        const head = 6 + s.width * 3;
        const bx = s.x2 - ux * head;
        const by = s.y2 - uy * head;
        const wing = head * 0.45;
        g.appendChild(
          el("line", {
            ...stroke,
            x1: r1(s.x1),
            y1: r1(s.y1),
            x2: r1(bx),
            y2: r1(by),
            "stroke-linecap": "round",
          }),
        );
        g.appendChild(
          el("polygon", {
            points: `${r1(s.x2)},${r1(s.y2)} ${r1(bx - uy * wing)},${r1(by + ux * wing)} ${r1(bx + uy * wing)},${r1(by - ux * wing)}`,
            fill: s.color,
          }),
        );
        break;
      }
      case "pen":
        g.appendChild(
          el("path", {
            ...stroke,
            d: pathD(s.pts),
            "stroke-linecap": "round",
            "stroke-linejoin": "round",
          }),
        );
        break;
      case "marker":
        g.appendChild(
          el("path", {
            d: pathD(s.pts),
            fill: "none",
            stroke: s.color,
            "stroke-width": s.width * 3.5,
            "stroke-opacity": 0.4,
            "stroke-linecap": "round",
            "stroke-linejoin": "round",
          }),
        );
        break;
      case "text": {
        const t = el("text", {
          x: r1(s.x),
          y: r1(s.y + s.size * 0.95),
          fill: s.color,
          "font-size": s.size,
          "font-family": "system-ui, sans-serif",
          "paint-order": "stroke",
          stroke: "rgba(0,0,0,0.6)",
          "stroke-width": Math.max(1, s.size / 10),
        });
        const lines = s.text.split("\n");
        lines.forEach((line, i) => {
          const ts = el("tspan", { x: r1(s.x), dy: i === 0 ? "0" : "1.25em" });
          ts.textContent = line.length > 0 ? line : " ";
          t.appendChild(ts);
        });
        g.appendChild(t);
        break;
      }
    }
    return g;
  }

  private sliceNode(sl: Slice, index: number, active: boolean): SVGElement {
    const r = norm(sl);
    const g = el("g", { "data-slice": sl.id });
    const u = (n: number) => this.u(n);
    g.appendChild(
      el("rect", {
        x: r1(r.x),
        y: r1(r.y),
        width: r1(r.w),
        height: r1(r.h),
        fill: "rgba(255,176,32,0.07)",
        stroke: "#ffb020",
        "stroke-width": u(1.5),
        "stroke-dasharray": `${u(6)} ${u(4)}`,
      }),
    );
    // Badge numéroté en haut à gauche.
    const bw = u(20);
    const bh = u(15);
    g.appendChild(el("rect", { x: r1(r.x), y: r1(r.y), width: bw, height: bh, fill: "#ffb020" }));
    const num = el("text", {
      x: r1(r.x + bw / 2),
      y: r1(r.y + bh * 0.78),
      "text-anchor": "middle",
      "font-size": u(11),
      "font-family": "system-ui, sans-serif",
      fill: "#141414",
    });
    num.textContent = String(index + 1);
    g.appendChild(num);
    // Nom de la zone (si défini) à droite du badge.
    if (sl.name) {
      const nameText = el("text", {
        x: r1(r.x + bw + u(5)),
        y: r1(r.y + bh * 0.78),
        "font-size": u(11),
        "font-family": "system-ui, sans-serif",
        fill: "#ffd27a",
        "paint-order": "stroke",
        stroke: "rgba(0,0,0,0.7)",
        "stroke-width": u(2),
      });
      nameText.textContent = sl.name;
      g.appendChild(nameText);
    }
    // Dimensions en bas à droite.
    const dims = el("text", {
      x: r1(r.x + r.w - u(4)),
      y: r1(r.y + r.h - u(5)),
      "text-anchor": "end",
      "font-size": u(10),
      "font-family": "system-ui, sans-serif",
      fill: "#ffd27a",
      "paint-order": "stroke",
      stroke: "rgba(0,0,0,0.7)",
      "stroke-width": u(2),
    });
    dims.textContent = `${Math.round(r.w)}×${Math.round(r.h)}`;
    g.appendChild(dims);
    if (active) {
      const hs = u(9);
      const handle = el("rect", {
        x: r1(r.x + r.w - hs / 2),
        y: r1(r.y + r.h - hs / 2),
        width: hs,
        height: hs,
        fill: "#ffb020",
        stroke: "#141414",
        "stroke-width": u(1),
        "data-slice-handle": sl.id,
      });
      (handle as SVGElement & { style: CSSStyleDeclaration }).style.cursor = "nwse-resize";
      g.appendChild(handle);
      (g as SVGElement & { style: CSSStyleDeclaration }).style.cursor = "move";
    }
    return g;
  }

  private guideNode(gd: Guide, interactive = true): SVGElement {
    const g = el("g", { "data-guide": gd.id });
    const u = (n: number) => this.u(n);
    const vert = gd.axis === "x";
    const line = {
      x1: vert ? r1(gd.pos) : 0,
      y1: vert ? 0 : r1(gd.pos),
      x2: vert ? r1(gd.pos) : this.nativeW,
      y2: vert ? this.nativeH : r1(gd.pos),
    };
    // Bande invisible élargie pour faciliter la saisie du guide. Son
    // `pointer-events: stroke` OUTREPASSE le `none` du groupe parent : on ne
    // l'active donc qu'en mode règle, sinon le guide resterait cliquable (et
    // intercepterait les clics destinés aux slices, qu'il recouvre).
    const hit = el("line", {
      ...line,
      stroke: "rgba(0,0,0,0)",
      "stroke-width": u(9),
      "pointer-events": interactive ? "stroke" : "none",
    });
    (hit as SVGElement & { style: CSSStyleDeclaration }).style.cursor = vert ? "col-resize" : "row-resize";
    g.appendChild(hit);
    g.appendChild(el("line", { ...line, stroke: "#22c8e6", "stroke-width": u(1) }));
    const label = el("text", {
      x: vert ? r1(gd.pos + u(4)) : u(28),
      y: vert ? u(34) : r1(gd.pos - u(4)),
      "font-size": u(10),
      "font-family": "system-ui, sans-serif",
      fill: "#22c8e6",
      "paint-order": "stroke",
      stroke: "rgba(0,0,0,0.7)",
      "stroke-width": u(2),
    });
    label.textContent = `${Math.round(gd.pos)} px`;
    g.appendChild(label);
    return g;
  }

  private measureNode(): SVGElement {
    const m = this.measure!;
    const g = el("g");
    const u = (n: number) => this.u(n);
    g.appendChild(
      el("line", {
        x1: r1(m.x1),
        y1: r1(m.y1),
        x2: r1(m.x2),
        y2: r1(m.y2),
        stroke: "#4dd2ff",
        "stroke-width": u(1.5),
        "stroke-dasharray": `${u(6)} ${u(3)}`,
      }),
    );
    for (const [cx, cy] of [
      [m.x1, m.y1],
      [m.x2, m.y2],
    ]) {
      g.appendChild(el("circle", { cx: r1(cx), cy: r1(cy), r: u(3), fill: "#4dd2ff" }));
    }
    const dx = Math.round(Math.abs(m.x2 - m.x1));
    const dy = Math.round(Math.abs(m.y2 - m.y1));
    const diag = Math.round(Math.hypot(m.x2 - m.x1, m.y2 - m.y1));
    const label = el("text", {
      x: r1((m.x1 + m.x2) / 2 + u(8)),
      y: r1((m.y1 + m.y2) / 2 - u(8)),
      "font-size": u(12),
      "font-family": "system-ui, sans-serif",
      fill: "#4dd2ff",
      "paint-order": "stroke",
      stroke: "rgba(0,0,0,0.75)",
      "stroke-width": u(3),
    });
    label.textContent = `${dx} × ${dy} — ${diag} px`;
    g.appendChild(label);
    return g;
  }
}

/** Clone d'une forme translatée de (dx,dy). */
function translated(s: Shape, dx: number, dy: number): Shape {
  const c = clone(s);
  switch (c.kind) {
    case "rect":
    case "ellipse":
    case "text":
      c.x += dx;
      c.y += dy;
      break;
    case "line":
    case "arrow":
      c.x1 += dx;
      c.y1 += dy;
      c.x2 += dx;
      c.y2 += dy;
      break;
    case "pen":
    case "marker":
      for (let i = 0; i + 1 < c.pts.length; i += 2) {
        c.pts[i] += dx;
        c.pts[i + 1] += dy;
      }
      break;
  }
  return c;
}

const TOOL_HINTS: Record<Tool, string> = {
  cursor: "remote control active",
  select: "click an annotation to select it; drag to move; Del to delete; double-click text to edit",
  text: "click to place text (Enter = confirm, Shift+Enter = new line)",
  rect: "draw a rectangle (remote control paused)",
  ellipse: "draw an ellipse (remote control paused)",
  line: "draw a line (remote control paused)",
  arrow: "draw an arrow (remote control paused)",
  pen: "freehand drawing (remote control paused)",
  marker: "freehand highlight (remote control paused)",
  ruler: "drag to measure; drag from a ruler to add a guide (double-click = remove)",
  slice: "draw slice zones; drag = move, handle = resize, double-click = rename, Del = delete",
};
