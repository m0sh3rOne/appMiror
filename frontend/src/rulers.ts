// Règles latérales (haut + gauche) superposées au bord du canvas vidéo,
// graduées en PIXELS NATIFS de la fenêtre capturée. Un marqueur suit le
// pointeur et une pastille affiche la coordonnée native courante.
//
// Les règles sont des canvas 2D redessinés à chaque changement de layout
// (échelle d'affichage) et à chaque mouvement de pointeur — le coût est
// négligeable (quelques centaines de traits).

/** Épaisseur des règles en pixels CSS. */
export const RULER_THICKNESS = 22;

/** Pas de graduation candidats (en px natifs). */
const STEPS = [1, 2, 5, 10, 25, 50, 100, 250, 500, 1000, 2500];

export class Rulers {
  private scale = 1;
  private nativeW = 0;
  private nativeH = 0;
  private pointer: { x: number; y: number } | null = null;
  private visible = false;

  constructor(
    private readonly top: HTMLCanvasElement,
    private readonly left: HTMLCanvasElement,
    private readonly coordChip: HTMLElement,
  ) {}

  setVisible(v: boolean) {
    this.visible = v;
    const d = v ? "block" : "none";
    this.top.style.display = d;
    this.left.style.display = d;
    this.coordChip.style.display = v && this.pointer ? "block" : "none";
    if (v) this.redraw();
  }

  isVisible(): boolean {
    return this.visible;
  }

  /** À appeler quand l'échelle d'affichage ou la résolution native change. */
  layout(scale: number, nativeW: number, nativeH: number) {
    this.scale = scale;
    this.nativeW = nativeW;
    this.nativeH = nativeH;
    if (this.visible) this.redraw();
  }

  /** Position du pointeur en px natifs (null = pointeur hors du canvas). */
  setPointer(p: { x: number; y: number } | null) {
    this.pointer = p;
    if (!this.visible) return;
    if (p) {
      this.coordChip.textContent = `${Math.round(p.x)} , ${Math.round(p.y)} px`;
      this.coordChip.style.display = "block";
    } else {
      this.coordChip.style.display = "none";
    }
    this.redraw();
  }

  private redraw() {
    this.drawRuler(this.top, "x");
    this.drawRuler(this.left, "y");
  }

  /** Choisit le pas de graduation labellisé : ≥ ~55 px CSS entre deux labels. */
  private labelStep(): number {
    for (const s of STEPS) if (s * this.scale >= 55) return s;
    return STEPS[STEPS.length - 1];
  }

  private drawRuler(canvas: HTMLCanvasElement, axis: "x" | "y") {
    const dpr = window.devicePixelRatio || 1;
    const cssW = canvas.clientWidth;
    const cssH = canvas.clientHeight;
    if (cssW === 0 || cssH === 0) return;
    const bw = Math.round(cssW * dpr);
    const bh = Math.round(cssH * dpr);
    if (canvas.width !== bw || canvas.height !== bh) {
      canvas.width = bw;
      canvas.height = bh;
    }
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, cssW, cssH);

    ctx.fillStyle = "rgba(17, 19, 24, 0.82)";
    ctx.fillRect(0, 0, cssW, cssH);

    const horizontal = axis === "x";
    const nativeLen = horizontal ? this.nativeW : this.nativeH;
    const step = this.labelStep();
    const minor = step / 5;
    const showMinor = minor * this.scale >= 5;

    ctx.strokeStyle = "rgba(230, 230, 230, 0.55)";
    ctx.fillStyle = "rgba(230, 230, 230, 0.75)";
    ctx.lineWidth = 1;
    ctx.font = "9px system-ui, sans-serif";
    ctx.textBaseline = "top";

    ctx.beginPath();
    const inc = showMinor ? minor : step;
    for (let v = 0; v <= nativeLen; v += inc) {
      const pos = Math.round(v * this.scale) + 0.5;
      const major = v % step === 0;
      const len = major ? RULER_THICKNESS : 6;
      if (horizontal) {
        ctx.moveTo(pos, RULER_THICKNESS - len);
        ctx.lineTo(pos, RULER_THICKNESS);
      } else {
        ctx.moveTo(RULER_THICKNESS - len, pos);
        ctx.lineTo(RULER_THICKNESS, pos);
      }
      if (major && v > 0) {
        if (horizontal) {
          ctx.fillText(String(v), pos + 3, 2);
        } else {
          // Texte vertical : un caractère par ligne serait illisible ;
          // on tourne le contexte de -90°.
          ctx.save();
          ctx.translate(2, pos + 3);
          ctx.rotate(Math.PI / 2);
          ctx.fillText(String(v), 0, -9);
          ctx.restore();
        }
      }
    }
    ctx.stroke();

    // Marqueur de position du pointeur.
    if (this.pointer) {
      const pv = horizontal ? this.pointer.x : this.pointer.y;
      const pos = pv * this.scale + 0.5;
      ctx.strokeStyle = "#4dd2ff";
      ctx.beginPath();
      if (horizontal) {
        ctx.moveTo(pos, 0);
        ctx.lineTo(pos, RULER_THICKNESS);
      } else {
        ctx.moveTo(0, pos);
        ctx.lineTo(RULER_THICKNESS, pos);
      }
      ctx.stroke();
    }

    // Liseré côté image.
    ctx.strokeStyle = "rgba(120, 130, 145, 0.6)";
    ctx.beginPath();
    if (horizontal) {
      ctx.moveTo(0, RULER_THICKNESS - 0.5);
      ctx.lineTo(cssW, RULER_THICKNESS - 0.5);
    } else {
      ctx.moveTo(RULER_THICKNESS - 0.5, 0);
      ctx.lineTo(RULER_THICKNESS - 0.5, cssH);
    }
    ctx.stroke();
  }
}
