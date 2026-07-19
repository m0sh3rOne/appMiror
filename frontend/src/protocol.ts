// Types partagés avec le backend (voir backend/src/protocol.rs).

export type MouseButtonName = "left" | "right" | "middle";

export type ClientMessage =
  | { type: "list_windows" }
  | { type: "focus_window" }
  | { type: "select_window"; id: number; pin?: string | null; quality?: number | null; scale?: number | null; show_cursor?: boolean | null }
  | { type: "set_input_mode"; background: boolean }
  | { type: "mouse_move"; x: number; y: number }
  | { type: "mouse_button"; button: MouseButtonName; pressed: boolean; x: number; y: number }
  | { type: "wheel"; delta: number; x: number; y: number }
  | { type: "key"; code: string; pressed: boolean }
  | { type: "save_macro"; context: string; name: string; window_id: number; steps: MacroStep[] }
  | { type: "list_macros" }
  | { type: "load_macro"; context: string; name: string }
  | { type: "test_macro"; steps: MacroStep[] }
  | { type: "screenshot"; dir?: string | null }
  | { type: "start_recording"; dir?: string | null; fps?: number | null }
  | { type: "stop_recording" }
  | { type: "launch_app"; command: string }
  | { type: "launch_active_on_virtual" }
  | { type: "close_window"; id: number }
  | { type: "webrtc_init" }
  | { type: "webrtc_answer"; sdp: string }
  | { type: "ice_candidate"; candidate: string; sdp_mid?: string | null; sdp_mline_index?: number | null };

export interface WindowInfo {
  id: number;
  title: string;
  locked: boolean;
  is_virtual: boolean;
  context: string;
}

export type MacroStep =
  | { type: "mouse_move"; x: number; y: number }
  | { type: "mouse_button"; button: MouseButtonName; pressed: boolean; x: number; y: number }
  | { type: "wheel"; delta: number; x: number; y: number }
  | { type: "key"; code: string; pressed: boolean }
  | { type: "text"; text: string };

export interface MacroFile {
  version: number;
  context: string;
  name: string;
  window_id: number;
  steps: MacroStep[];
}

export interface MacroSummary { context: string; name: string; steps: number }

export type ServerMessage =
  | { type: "windows"; items: WindowInfo[] }
  | { type: "selected"; width: number; height: number }
  | { type: "screenshot_saved"; path: string }
  | { type: "recording_started"; dir: string; fps: number }
  | { type: "recording_stopped"; dir: string; frames: number }
  | { type: "macro_saved"; path: string; name: string }
  | { type: "macro_list"; items: MacroSummary[] }
  | { type: "macro_loaded"; macro_file: MacroFile }
  | { type: "macro_played"; name: string; steps: number }
  | { type: "notice"; message: string }
  | { type: "error"; message: string }
  | { type: "webrtc_offer"; sdp: string }
  | { type: "webrtc_ice_candidate"; candidate: string; sdp_mid?: string | null; sdp_mline_index?: number | null };
