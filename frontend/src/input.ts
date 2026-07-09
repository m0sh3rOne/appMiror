// Capture des événements souris/clavier sur le canvas et TRANSLATION des
// coordonnées du repère HTML (taille affichée du canvas) vers le repère natif
// de la fenêtre (résolution renvoyée par le backend via "selected").

import type { ClientMessage, MouseButtonName } from "./protocol";

type Sender = (msg: ClientMessage) => void;

const BUTTON_MAP: Record<number, MouseButtonName> = {
  0: "left",
  1: "middle",
  2: "right",
};

export class InputController {
  /** Résolution native de la fenêtre cible (repère de destination). */
  private nativeW = 0;
  private nativeH = 0;

  /** false = injection suspendue (un outil d'annotation est actif). */
  private enabled = true;

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly send: Sender,
  ) {}

  setNativeSize(w: number, h: number) {
    this.nativeW = w;
    this.nativeH = h;
  }

  setEnabled(v: boolean) {
    this.enabled = v;
  }

  /**
   * Cœur de la translation : (clientX,clientY) écran -> pixel natif.
   *
   * getBoundingClientRect() donne la taille RÉELLEMENT affichée du canvas
   * (ex: 800x600 après contraintes CSS). On projette linéairement sur la
   * résolution native (ex: 1920x1080).
   */
  private toNative(e: MouseEvent): { x: number; y: number } {
    const rect = this.canvas.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return { x: 0, y: 0 };
    const nx = ((e.clientX - rect.left) / rect.width) * this.nativeW;
    const ny = ((e.clientY - rect.top) / rect.height) * this.nativeH;
    return {
      x: Math.round(Math.max(0, Math.min(this.nativeW - 1, nx))),
      y: Math.round(Math.max(0, Math.min(this.nativeH - 1, ny))),
    };
  }

  attach() {
    const c = this.canvas;

    c.addEventListener("mousemove", (e) => {
      if (!this.enabled) return;
      const { x, y } = this.toNative(e);
      this.send({ type: "mouse_move", x, y });
    });

    c.addEventListener("mousedown", (e) => {
      if (!this.enabled) return;
      c.focus();
      const { x, y } = this.toNative(e);
      const button = BUTTON_MAP[e.button];
      if (button) this.send({ type: "mouse_button", button, pressed: true, x, y });
    });

    c.addEventListener("mouseup", (e) => {
      if (!this.enabled) return;
      const { x, y } = this.toNative(e);
      const button = BUTTON_MAP[e.button];
      if (button) this.send({ type: "mouse_button", button, pressed: false, x, y });
    });

    // Empêche le menu contextuel natif pour transmettre le clic droit.
    c.addEventListener("contextmenu", (e) => {
      if (this.enabled) e.preventDefault();
    });

    c.addEventListener(
      "wheel",
      (e) => {
        if (!this.enabled) return;
        e.preventDefault();
        const { x, y } = this.toNative(e);
        // deltaY > 0 = scroll vers le bas ; on inverse pour coller à WHEEL_DELTA.
        const delta = e.deltaY > 0 ? -1 : 1;
        this.send({ type: "wheel", delta, x, y });
      },
      { passive: false },
    );

    // Clavier : capté quand le canvas a le focus.
    c.addEventListener("keydown", (e) => {
      if (!this.enabled) return;
      e.preventDefault();
      if (e.repeat) return;
      this.send({ type: "key", code: e.code, pressed: true });
    });
    c.addEventListener("keyup", (e) => {
      if (!this.enabled) return;
      e.preventDefault();
      this.send({ type: "key", code: e.code, pressed: false });
    });
  }
}
