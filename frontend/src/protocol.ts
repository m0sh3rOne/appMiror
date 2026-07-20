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
  | { type: "text_input"; text: string }
  | { type: "save_macro"; context: string; name: string; window_id: number; params?: MacroParam[]; steps: MacroStep[] }
  | { type: "list_macros" }
  | { type: "load_macro"; context: string; name: string }
  | { type: "test_macro"; steps: MacroStep[] }
  | { type: "save_workflow"; name: string; graph: WorkflowGraph }
  | { type: "list_workflows" }
  | { type: "load_workflow"; name: string }
  | { type: "execute_workflow"; name: string }
  | { type: "test_workflow"; graph: WorkflowGraph }
  | { type: "save_layout"; context: string; name: string; source_width: number; source_height: number; zones: LayoutZone[] }
  | { type: "list_layouts"; context?: string | null }
  | { type: "load_layout"; context: string; name: string }
  | { type: "capture_state"; context: string; name: string; layout?: string | null; zones: LayoutZone[] }
  | { type: "list_states"; context?: string | null }
  | { type: "delete_state"; context: string; name: string }
  | { type: "save_profile"; profile: AppProfile }
  | { type: "get_profile"; context: string }
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
  is_desktop: boolean;
  context: string;
}

export type MacroStep =
  | { type: "mouse_move"; x: number; y: number }
  | { type: "mouse_button"; button: MouseButtonName; pressed: boolean; x: number; y: number }
  | { type: "wheel"; delta: number; x: number; y: number }
  | { type: "key"; code: string; pressed: boolean }
  | { type: "text"; text: string }
  | { type: "activate_element"; element: string; layout?: string | null; strategy?: string | null; verify?: boolean | null }
  | { type: "type_into_element"; element: string; text: string; layout?: string | null }
  | { type: "wait_for_state"; state: string; timeout_ms?: number | null }
  | { type: "wait_ms"; duration_ms: number }
  | { type: "call_macro"; name: string; context?: string | null };

export type MacroParamKind = "text" | "number" | "boolean" | "secret" | "choice" | "zone_ref";
export interface MacroParam {
  name: string;
  kind: MacroParamKind;
  default?: string | null;
  description?: string | null;
  choices?: string[];
}

export interface MacroFile {
  version: number;
  context: string;
  name: string;
  window_id: number;
  params?: MacroParam[];
  steps: MacroStep[];
}

export interface MacroSummary { context: string; name: string; steps: number }

export type LayoutZoneKind = "button" | "input" | "textarea" | "dropdown" | "menu" | "checkbox" | "radio" | "link" | "text" | "region";
export interface NormalizedBounds { x: number; y: number; width: number; height: number }
export type ActivationStep =
  | { type: "click_center" }
  | { type: "double_click" }
  | { type: "key"; code: string }
  | { type: "focus_then_key"; code: string }
  | { type: "type_text" };
export interface ZoneAnchor { relativeTo: string; dx: number; dy: number }
export interface LayoutZone {
  id: string;
  name: string;
  kind: LayoutZoneKind;
  description?: string | null;
  bounds: NormalizedBounds;
  label?: string | null;
  activation?: ActivationStep[] | null;
  expectedState?: string | null;
  anchor?: ZoneAnchor | null;
}
export interface LayoutFile {
  version: number;
  context: string;
  name: string;
  sourceWidth: number;
  sourceHeight: number;
  zones: LayoutZone[];
}
export interface LayoutSummary { context: string; name: string; zones: number }
export interface UiStateSummary { context: string; name: string; layout?: string | null; signatures: number }

export type WorkflowNodeType = "start" | "macro" | "loop" | "condition" | "wait" | "activate" | "wait_for_state";
export type InjectionMode = "foreground" | "background";
export type WorkflowHandle = "next" | "on_success" | "on_error" | "loop_cycle" | "out" | "true" | "false" | "wait_done";

export interface WorkflowPosition { x: number; y: number }
export type AppExecutionMode = "main_desktop" | "virtual_desktop";
export type StartTargetKind = "window" | "command" | "main_desktop" | "virtual_desktop";
export type WorkflowNodeData =
  | { targetKind?: StartTargetKind | null; appContext?: string | null; appTitle?: string | null; command?: string | null; executionMode?: AppExecutionMode; focus?: boolean }
  | { macroName: string; macroContext?: string | null; injectionMode: InjectionMode; parameters?: Record<string, string>; verify?: boolean }
  | { repetitions: number }
  | { expression: string }
  | { durationMs: number }
  | { element: string; layout?: string | null; strategy?: string | null; expectedState?: string | null; timeoutMs?: number; retries?: number }
  | { state: string; timeoutMs?: number };
export interface WorkflowNode { id: string; type: WorkflowNodeType; position: WorkflowPosition; data: WorkflowNodeData }
export interface WorkflowEdge { id: string; sourceNodeId: string; sourceHandle: WorkflowHandle; targetNodeId: string; targetHandle: "in" }
export interface WorkflowGraph { nodes: Record<string, WorkflowNode>; edges: WorkflowEdge[] }
export interface WorkflowFile { version: number; name: string; graph: WorkflowGraph }
export interface WorkflowSummary { name: string; nodes: number; edges: number }

export type ExecutionStatus = "running" | "finished" | "failed" | "cancelled";

export interface NodeExecutionLog {
  nodeId: string;
  nodeType: string;
  startedAtMs: number;
  finishedAtMs?: number | null;
  durationMs?: number | null;
  outputHandle?: string | null;
  actionAttempted?: string | null;
  stateBefore?: string | null;
  stateAfter?: string | null;
  attemptsCount?: number | null;
  selectionReason?: string | null;
  errorMessage?: string | null;
  screenshotBase64?: string | null;
}

export interface ExecutionRecord {
  executionId: string;
  workflowName: string;
  status: ExecutionStatus;
  currentNodeId?: string | null;
  startedAtMs: number;
  finishedAtMs?: number | null;
  nodesExecuted: number;
  history: NodeExecutionLog[];
  errorMessage?: string | null;
}

export interface AppProfile {
  context: string;
  foregroundRequired: boolean;
  preferredActivation: string;
  backgroundMouse: string;
  backgroundKeyboard: string;
  pauseOnFocusLoss: boolean;
}

export type ServerMessage =
  | { type: "windows"; items: WindowInfo[] }
  | { type: "selected"; width: number; height: number; background: boolean }
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
  | { type: "layout_saved"; context: string; name: string; path: string }
  | { type: "layout_list"; items: LayoutSummary[] }
  | { type: "layout_loaded"; layout: LayoutFile }
  | { type: "state_saved"; context: string; name: string; signatures: number }
  | { type: "state_list"; items: UiStateSummary[] }
  | { type: "state_deleted"; context: string; name: string }
  | { type: "profile_saved"; context: string }
  | { type: "profile_loaded"; profile: AppProfile }
  | { type: "notice"; message: string }
  | { type: "error"; message: string }
  | { type: "webrtc_offer"; sdp: string }
  | { type: "webrtc_ice_candidate"; candidate: string; sdp_mid?: string | null; sdp_mline_index?: number | null };
