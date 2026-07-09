// Types partagés avec le backend (voir backend/src/protocol.rs).

export type MouseButtonName = "left" | "right" | "middle";

export type ClientMessage =
  | { type: "list_windows" }
  | { type: "select_window"; id: number; pin?: string | null; quality?: number | null; scale?: number | null; show_cursor?: boolean | null }
  | { type: "set_input_mode"; background: boolean }
  | { type: "mouse_move"; x: number; y: number }
  | { type: "mouse_button"; button: MouseButtonName; pressed: boolean; x: number; y: number }
  | { type: "wheel"; delta: number; x: number; y: number }
  | { type: "key"; code: string; pressed: boolean }
  | { type: "screenshot"; dir?: string | null }
  | { type: "start_recording"; dir?: string | null; fps?: number | null }
  | { type: "stop_recording" };

export interface WindowInfo {
  id: number;
  title: string;
  locked: boolean;
}

export type ServerMessage =
  | { type: "windows"; items: WindowInfo[] }
  | { type: "selected"; width: number; height: number }
  | { type: "screenshot_saved"; path: string }
  | { type: "recording_started"; dir: string; fps: number }
  | { type: "recording_stopped"; dir: string; frames: number }
  | { type: "notice"; message: string }
  | { type: "error"; message: string };
