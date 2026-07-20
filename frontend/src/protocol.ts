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
  | { type: "save_workflow"; name: string; graph: WorkflowGraph }
  | { type: "list_workflows" }
  | { type: "load_workflow"; name: string }
  | { type: "execute_workflow"; name: string }
  | { type: "test_workflow"; graph: WorkflowGraph }
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

export type WorkflowNodeType = "start" | "macro" | "loop" | "condition" | "wait";
export type InjectionMode = "foreground" | "background";
export type WorkflowHandle = "next" | "on_success" | "on_error" | "loop_cycle" | "out" | "true" | "false" | "wait_done";

export interface WorkflowPosition { x: number; y: number }
export type AppExecutionMode = "main_desktop" | "virtual_desktop";
export type WorkflowNodeData =
  | { appContext?: string | null; appTitle?: string | null; command?: string | null; executionMode?: AppExecutionMode; focus?: boolean }
  | { macroName: string; macroContext?: string | null; injectionMode: InjectionMode }
  | { repetitions: number }
  | { expression: string }
  | { durationMs: number };
export interface WorkflowNode { id: string; type: WorkflowNodeType; position: WorkflowPosition; data: WorkflowNodeData }
export interface WorkflowEdge { id: string; sourceNodeId: string; sourceHandle: WorkflowHandle; targetNodeId: string; targetHandle: "in" }
export interface WorkflowGraph { nodes: Record<string, WorkflowNode>; edges: WorkflowEdge[] }
export interface WorkflowFile { version: number; name: string; graph: WorkflowGraph }
export interface WorkflowSummary { name: string; nodes: number; edges: number }

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
  | { type: "workflow_saved"; name: string; path: string }
  | { type: "workflow_list"; items: WorkflowSummary[] }
  | { type: "workflow_loaded"; workflow: WorkflowFile }
  | { type: "workflow_started"; execution_id: string; name: string }
  | { type: "workflow_node_started"; execution_id: string; node_id: string }
  | { type: "workflow_node_finished"; execution_id: string; node_id: string; output: WorkflowHandle }
  | { type: "workflow_finished"; execution_id: string; name: string; nodes_executed: number }
  | { type: "workflow_failed"; execution_id: string; node_id?: string | null; message: string }
  | { type: "notice"; message: string }
  | { type: "error"; message: string }
  | { type: "webrtc_offer"; sdp: string }
  | { type: "webrtc_ice_candidate"; candidate: string; sdp_mid?: string | null; sdp_mline_index?: number | null };
