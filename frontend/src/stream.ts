// Connexion WebSocket : reçoit les messages de contrôle (JSON) et les frames
// vidéo (soit binaire = JPEG en fallback, soit via WebRTC/H.264 peint sur le canvas).

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
  private pc: RTCPeerConnection | null = null;
  private video: HTMLVideoElement | null = null;
  private isStreaming = false;

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly handlers: StreamHandlers,
  ) {
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("2D context indisponible");
    this.ctx = ctx;
  }

  connect() {
    this.cleanupWebrtc();

    const proto = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${proto}://${location.host}/ws`);
    ws.binaryType = "arraybuffer";
    this.ws = ws;

    ws.onopen = () => {
      this.handlers.onStatus("connected");
      this.handlers.onOpen?.();
    };
    
    ws.onclose = () => {
      this.handlers.onStatus("disconnected");
      this.cleanupWebrtc();
    };

    ws.onerror = () => {
      this.handlers.onStatus("WebSocket error");
      this.cleanupWebrtc();
    };

    ws.onmessage = async (ev) => {
      if (typeof ev.data === "string") {
        const msg = JSON.parse(ev.data) as ServerMessage;
        
        if (msg.type === "selected") {
          // Une fenêtre a été sélectionnée, on initie la négociation WebRTC si active.
          const webrtcCheckbox = document.getElementById("webrtcToggle") as HTMLInputElement | null;
          if (!webrtcCheckbox || webrtcCheckbox.checked) {
            this.initWebrtc();
          } else {
            this.cleanupWebrtc();
          }
        } else if (msg.type === "webrtc_offer") {
          await this.handleWebrtcOffer(msg.sdp);
        } else if (msg.type === "webrtc_ice_candidate") {
          await this.handleRemoteCandidate(msg.candidate, msg.sdp_mid, msg.sdp_mline_index);
        }

        this.handlers.onServerMessage(msg);
      } else {
        // Fallback : dessin direct si on reçoit des frames JPEG binaires
        await this.drawFrame(ev.data as ArrayBuffer);
      }
    };
  }

  send(msg: ClientMessage) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }

  private async initWebrtc() {
    this.cleanupWebrtc();
    this.send({ type: "webrtc_init" });
  }

  private cleanupWebrtc() {
    this.isStreaming = false;
    if (this.pc) {
      this.pc.close();
      this.pc = null;
    }
    if (this.video) {
      this.video.pause();
      this.video.srcObject = null;
      this.video = null;
    }
  }

  private async handleWebrtcOffer(sdp: string) {
    this.pc = new RTCPeerConnection({
      iceServers: [{ urls: "stun:stun.l.google.com:19302" }],
    });

    this.pc.onicecandidate = (event) => {
      if (event.candidate) {
        this.send({
          type: "ice_candidate",
          candidate: event.candidate.candidate,
          sdp_mid: event.candidate.sdpMid,
          sdp_mline_index: event.candidate.sdpMLineIndex,
        });
      }
    };

    this.pc.ontrack = (event) => {
      if (event.streams && event.streams[0]) {
        this.startVideoPlayback(event.streams[0]);
      }
    };

    await this.pc.setRemoteDescription(new RTCSessionDescription({ type: "offer", sdp }));
    const answer = await this.pc.createAnswer();
    await this.pc.setLocalDescription(answer);

    this.send({
      type: "webrtc_answer",
      sdp: answer.sdp || "",
    });
  }

  private async handleRemoteCandidate(
    candidate: string,
    sdpMid?: string | null,
    sdpMLineIndex?: number | null,
  ) {
    if (this.pc) {
      await this.pc.addIceCandidate(
        new RTCIceCandidate({
          candidate,
          sdpMid: sdpMid || undefined,
          sdpMLineIndex: sdpMLineIndex !== null && sdpMLineIndex !== undefined ? sdpMLineIndex : undefined,
        })
      ).catch((e) => {
        console.warn("Échec d'ajout du candidat ICE distant :", e);
      });
    }
  }

  private startVideoPlayback(stream: MediaStream) {
    this.video = document.createElement("video");
    this.video.autoplay = true;
    this.video.muted = true;
    this.video.playsInline = true;
    this.video.srcObject = stream;

    this.video.onloadedmetadata = () => {
      if (this.video) {
        this.video.play()
          .then(() => {
            this.isStreaming = true;
            this.renderVideoLoop();
          })
          .catch((e) => {
            console.warn("Échec play() vidéo :", e);
          });
      }
    };
  }

  private renderVideoLoop() {
    if (!this.isStreaming || !this.video) return;

    const w = this.video.videoWidth;
    const h = this.video.videoHeight;

    if (w > 0 && h > 0) {
      if (this.canvas.width !== w || this.canvas.height !== h) {
        this.canvas.width = w;
        this.canvas.height = h;
        this.handlers.onFrameSize?.(w, h);
      }
      this.ctx.drawImage(this.video, 0, 0);
    }

    requestAnimationFrame(() => this.renderVideoLoop());
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

