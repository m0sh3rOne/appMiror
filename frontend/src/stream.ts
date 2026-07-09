// Connexion WebSocket : reçoit les messages de contrôle (JSON) et les frames
// vidéo (binaire = JPEG), qu'elle décode et peint sur le canvas.

import type { ClientMessage, ServerMessage } from "./protocol";

export interface StreamHandlers {
  onServerMessage: (msg: ServerMessage) => void;
  onStatus: (status: string) => void;
  /** Appelé quand la résolution native de la frame change (redimensionne le canvas). */
  onFrameSize?: (width: number, height: number) => void;
  /** Appelé dès que la connexion WebSocket est ouverte (ou rétablie). */
  onOpen?: () => void;
}

export class StreamClient {
  private ws: WebSocket | null = null;
  private ctx: CanvasRenderingContext2D;

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly handlers: StreamHandlers,
  ) {
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("2D context indisponible");
    this.ctx = ctx;
  }

  connect() {
    const proto = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${proto}://${location.host}/ws`);
    ws.binaryType = "arraybuffer";
    this.ws = ws;

    ws.onopen = () => {
      this.handlers.onStatus("connected");
      this.handlers.onOpen?.();
    };
    ws.onclose = () => this.handlers.onStatus("disconnected");
    ws.onerror = () => this.handlers.onStatus("WebSocket error");

    ws.onmessage = async (ev) => {
      if (typeof ev.data === "string") {
        this.handlers.onServerMessage(JSON.parse(ev.data) as ServerMessage);
      } else {
        await this.drawFrame(ev.data as ArrayBuffer);
      }
    };
  }

  send(msg: ClientMessage) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }

  private async drawFrame(buffer: ArrayBuffer) {
    // createImageBitmap décode le JPEG hors du thread principal (rapide).
    const bitmap = await createImageBitmap(new Blob([buffer], { type: "image/jpeg" }));
    if (this.canvas.width !== bitmap.width || this.canvas.height !== bitmap.height) {
      // Le buffer interne du canvas suit la résolution native de la source.
      this.canvas.width = bitmap.width;
      this.canvas.height = bitmap.height;
      this.handlers.onFrameSize?.(bitmap.width, bitmap.height);
    }
    this.ctx.drawImage(bitmap, 0, 0);
    bitmap.close();
  }
}
