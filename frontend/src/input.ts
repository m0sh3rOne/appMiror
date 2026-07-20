// Capture des événements souris/clavier sur le canvas et TRANSLATION des
// coordonnées du repère HTML (taille affichée du canvas) vers le repère natif
// de la fenêtre (résolution renvoyée par le backend via "selected").

import type { ClientMessage, MouseButtonName } from "./protocol";

type Sender = (msg: ClientMessage) => void;
export type InputObserver = (msg: ClientMessage, text?: string) => void;
export type KeyboardLayout = "azerty_fr" | "qwerty_us";

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
  private observer: InputObserver | null = null;
  private keyboardLayout: KeyboardLayout = "qwerty_us";

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

  setObserver(observer: InputObserver) {
    this.observer = observer;
  }

  setKeyboardLayout(layout: KeyboardLayout) {
    this.keyboardLayout = layout;
  }

  private shouldSendSemanticText(e: KeyboardEvent): boolean {
    // En AZERTY, KeyboardEvent.code décrit toujours la position physique
    // QWERTY (la touche « A » remonte KeyQ). `key` contient en revanche le
    // caractère réellement voulu, y compris les accents et la rangée chiffrée.
    // Ctrl/Alt/Meta restent physiques afin de préserver les raccourcis.
    return this.keyboardLayout === "azerty_fr"
      && e.key.length === 1
      && !e.ctrlKey
      && !e.altKey
      && !e.metaKey;
  }

  private emit(msg: ClientMessage, text?: string) {
    this.observer?.(msg, text);
    this.send(msg);
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
      this.emit({ type: "mouse_move", x, y });
    });

    c.addEventListener("mousedown", (e) => {
      if (!this.enabled) return;
      c.focus();
      const { x, y } = this.toNative(e);
      const button = BUTTON_MAP[e.button];
      if (button) this.emit({ type: "mouse_button", button, pressed: true, x, y });
    });

    c.addEventListener("mouseup", (e) => {
      if (!this.enabled) return;
      const { x, y } = this.toNative(e);
      const button = BUTTON_MAP[e.button];
      if (button) this.emit({ type: "mouse_button", button, pressed: false, x, y });
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
        this.emit({ type: "wheel", delta, x, y });
      },
      { passive: false },
    );

    // Clavier : capté quand le canvas a le focus.
    c.addEventListener("keydown", (e) => {
      if (!this.enabled) return;
      e.preventDefault();
      if (e.repeat) return;
      if (this.shouldSendSemanticText(e)) {
        this.emit({ type: "text_input", text: e.key }, e.key);
        return;
      }
      this.emit({ type: "key", code: e.code, pressed: true }, e.key.length === 1 ? e.key : undefined);
    });
    c.addEventListener("keyup", (e) => {
      if (!this.enabled) return;
      e.preventDefault();
      if (this.shouldSendSemanticText(e)) return;
      this.emit({ type: "key", code: e.code, pressed: false }, e.key.length === 1 ? e.key : undefined);
    });
  }
}
