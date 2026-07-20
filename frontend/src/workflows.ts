import type {
  ClientMessage, MacroSummary, WorkflowFile, WorkflowGraph, WorkflowNodeData,
  WorkflowHandle, WorkflowNode, WorkflowNodeType, WorkflowSummary, WindowInfo,
} from "./protocol";

type Sender = (message: ClientMessage) => void;
const OUTPUTS: Record<WorkflowNodeType, Array<{ handle: WorkflowHandle; label: string }>> = {
  start: [{ handle: "next", label: "next" }],
  macro: [{ handle: "on_success", label: "success" }, { handle: "on_error", label: "error" }],
  loop: [{ handle: "loop_cycle", label: "cycle" }, { handle: "out", label: "out" }],
  condition: [{ handle: "true", label: "true" }, { handle: "false", label: "false" }],
  wait: [{ handle: "wait_done", label: "done" }],
};
const TITLES: Record<WorkflowNodeType, string> = { start: "START", macro: "MACRO", loop: "LOOP", condition: "IF CONDITION", wait: "WAIT" };

type Point = { x: number; y: number };
type Rect = { nodeId: string; left: number; top: number; right: number; bottom: number };

export class WorkflowEditor {
  private graph: WorkflowGraph = { nodes: {}, edges: [] };
  private macros: MacroSummary[] = [];
  private windows: WindowInfo[] = [];
  private scale = 1;
  private panX = 100;
  private panY = 80;
  private pending: { nodeId: string; handle: WorkflowHandle } | null = null;
  private selectedNode: string | null = null;
  private sequence = 1;

  private readonly viewport: HTMLElement;
  private readonly world: HTMLElement;
  private readonly edgeSvg: SVGSVGElement;
  private readonly nodesLayer: HTMLElement;
  private readonly minimap: SVGSVGElement;

  constructor(
    private readonly root: HTMLElement,
    private readonly send: Sender,
    private readonly onVisibility: (open: boolean) => void,
    private readonly onStatus: (message: string) => void,
  ) {
    this.viewport = root.querySelector("#workflowViewport") as HTMLElement;
    this.world = root.querySelector("#workflowWorld") as HTMLElement;
    this.edgeSvg = root.querySelector("#workflowEdges") as SVGSVGElement;
    this.nodesLayer = root.querySelector("#workflowNodes") as HTMLElement;
    this.minimap = root.querySelector("#workflowMinimap") as SVGSVGElement;
    this.bind();
    this.newGraph();
  }

  toggle() { this.root.hidden ? this.open() : this.close(); }
  open() {
    this.root.hidden = false;
    document.getElementById("toggleWorkflow")?.classList.add("active");
    this.onVisibility(true);
    this.send({ type: "list_macros" });
    this.send({ type: "list_workflows" });
    this.render();
  }
  close() {
    this.root.hidden = true;
    document.getElementById("toggleWorkflow")?.classList.remove("active");
    this.onVisibility(false);
  }

  setMacros(items: MacroSummary[]) { this.macros = items; if (!this.root.hidden) this.render(); }
  setWindows(items: WindowInfo[]) { this.windows = items; if (!this.root.hidden) this.render(); }
  showLibrary(items: WorkflowSummary[]) {
    const select = this.root.querySelector<HTMLSelectElement>("#workflowLibrary")!;
    select.replaceChildren(new Option(items.length ? `— ${items.length} workflow(s) —` : "Aucun workflow", ""));
    for (const item of items) select.add(new Option(`${item.name} (${item.nodes} nodes / ${item.edges} edges)`, item.name));
  }
  load(file: WorkflowFile) {
    this.graph = file.graph;
    this.root.querySelector<HTMLInputElement>("#workflowName")!.value = file.name;
    this.selectedNode = null;
    this.pending = null;
    this.fitView();
    this.render();
    this.onStatus(`workflow ${file.name} chargé`);
  }
  nodeStarted(id: string) {
    this.nodesLayer.querySelectorAll(".wf-node.running").forEach((n) => n.classList.remove("running"));
    this.nodesLayer.querySelector<HTMLElement>(`[data-node-id="${CSS.escape(id)}"]`)?.classList.add("running");
  }
  executionFinished(message: string) {
    this.nodesLayer.querySelectorAll(".wf-node.running").forEach((n) => n.classList.remove("running"));
    this.onStatus(message);
  }

  private bind() {
    this.root.querySelector<HTMLButtonElement>("#workflowClose")!.onclick = () => this.close();
    this.root.querySelector<HTMLButtonElement>("#workflowNew")!.onclick = () => this.newGraph();
    this.root.querySelector<HTMLButtonElement>("#workflowSave")!.onclick = () => {
      const name = this.name(); if (!name) return this.onStatus("Saisissez un nom de workflow");
      this.send({ type: "save_workflow", name, graph: this.graph });
    };
    this.root.querySelector<HTMLButtonElement>("#workflowLoad")!.onclick = () => this.send({ type: "list_workflows" });
    this.root.querySelector<HTMLSelectElement>("#workflowLibrary")!.onchange = (e) => {
      const name = (e.currentTarget as HTMLSelectElement).value; if (name) this.send({ type: "load_workflow", name });
    };
    this.root.querySelector<HTMLButtonElement>("#workflowRun")!.onclick = () => this.send({ type: "test_workflow", graph: this.graph });
    this.root.querySelector<HTMLButtonElement>("#workflowFit")!.onclick = () => this.fitView();
    this.root.querySelector<HTMLInputElement>("#workflowSearch")!.oninput = (e) => {
      const q = (e.currentTarget as HTMLInputElement).value.toLowerCase();
      this.root.querySelectorAll<HTMLElement>("[data-add-node]").forEach((b) => b.hidden = !b.textContent!.toLowerCase().includes(q));
    };
    this.root.querySelectorAll<HTMLButtonElement>("[data-add-node]").forEach((button) => {
      button.onclick = () => this.addNode(button.dataset.addNode as WorkflowNodeType);
    });
    this.bindPanZoom();
    document.addEventListener("keydown", (event) => {
      if (this.root.hidden || !this.selectedNode || !["Delete", "Backspace"].includes(event.key)) return;
      const active = document.activeElement?.tagName; if (active === "INPUT" || active === "SELECT") return;
      this.deleteNode(this.selectedNode);
    });
  }

  private bindPanZoom() {
    let panning = false, sx = 0, sy = 0, ox = 0, oy = 0;
    this.viewport.addEventListener("pointerdown", (e) => {
      if ((e.target as HTMLElement).closest(".wf-node,.wf-handle")) return;
      panning = true; sx = e.clientX; sy = e.clientY; ox = this.panX; oy = this.panY;
      this.viewport.setPointerCapture(e.pointerId);
    });
    this.viewport.addEventListener("pointermove", (e) => {
      if (!panning) return; this.panX = ox + e.clientX - sx; this.panY = oy + e.clientY - sy; this.applyTransform();
    });
    this.viewport.addEventListener("pointerup", () => panning = false);
    this.viewport.addEventListener("wheel", (e) => {
      e.preventDefault();
      const rect = this.viewport.getBoundingClientRect();
      const mx = e.clientX - rect.left, my = e.clientY - rect.top;
      const wx = (mx - this.panX) / this.scale, wy = (my - this.panY) / this.scale;
      this.scale = Math.max(.25, Math.min(2, this.scale * (e.deltaY < 0 ? 1.1 : .9)));
      this.panX = mx - wx * this.scale; this.panY = my - wy * this.scale; this.applyTransform();
    }, { passive: false });
  }

  private name() { return this.root.querySelector<HTMLInputElement>("#workflowName")!.value.trim(); }
  private newGraph() {
    this.graph = { nodes: {}, edges: [] }; this.sequence = 1;
    const node = this.makeNode("start", 120, 220); this.graph.nodes[node.id] = node;
    this.root.querySelector<HTMLInputElement>("#workflowName")!.value = "workflow_1";
    this.scale = 1; this.panX = 100; this.panY = 80; this.render();
  }
  private makeNode(type: WorkflowNodeType, x: number, y: number): WorkflowNode {
    const id = `${type}-${Date.now().toString(36)}-${this.sequence++}`;
    let data: WorkflowNodeData;
    if (type === "macro") data = { macroName: this.macros[0]?.name ?? "", macroContext: this.macros[0]?.context ?? null, injectionMode: "background" };
    else if (type === "loop") data = { repetitions: 2 };
    else if (type === "condition") data = { expression: "last_result == \"success\"" };
    else if (type === "wait") data = { durationMs: 500 };
    else data = { executionMode: "main_desktop", focus: false };
    return { id, type, position: { x, y }, data };
  }
  private addNode(type: WorkflowNodeType) {
    if (type === "start" && Object.values(this.graph.nodes).some((n) => n.type === "start")) return this.onStatus("Un seul nœud START est autorisé");
    const rect = this.viewport.getBoundingClientRect();
    const node = this.makeNode(type, (rect.width / 2 - this.panX) / this.scale, (rect.height / 2 - this.panY) / this.scale);
    this.graph.nodes[node.id] = node; this.render();
  }
  private deleteNode(id: string) {
    delete this.graph.nodes[id]; this.graph.edges = this.graph.edges.filter((e) => e.sourceNodeId !== id && e.targetNodeId !== id); this.selectedNode = null; this.render();
  }

  private render() {
    this.nodesLayer.replaceChildren();
    for (const node of Object.values(this.graph.nodes)) this.nodesLayer.append(this.nodeElement(node));
    this.renderEdges(); this.renderMinimap(); this.applyTransform();
  }

  private nodeElement(node: WorkflowNode): HTMLElement {
    const el = document.createElement("article"); el.className = `wf-node wf-${node.type}`; el.dataset.nodeId = node.id;
    // La position persistée est exprimée dans le repère monde. Elle doit être
    // appliquée dès la création du DOM ; sinon toutes les cartes restent à
    // (0,0) alors que les arêtes et la mini-carte utilisent node.position.
    el.style.left = `${node.position.x}px`;
    el.style.top = `${node.position.y}px`;
    if (this.selectedNode === node.id) el.classList.add("selected");
    const head = document.createElement("header"); head.className = "wf-node-head"; head.innerHTML = `<strong>${TITLES[node.type]}</strong><button title="Supprimer">×</button>`;
    head.querySelector("button")!.onclick = (e) => { e.stopPropagation(); this.deleteNode(node.id); };
    const body = document.createElement("div"); body.className = "wf-node-body"; this.nodeFields(node, body);
    if (node.type !== "start") body.prepend(this.handle(node.id, "in", "input", "in"));
    const outputs = document.createElement("div"); outputs.className = "wf-outputs";
    for (const out of OUTPUTS[node.type]) outputs.append(this.handle(node.id, out.handle, "output", out.label));
    el.append(head, body, outputs);
    el.onclick = (event) => {
      if ((event.target as HTMLElement).closest("input,select,button")) return;
      this.selectedNode = node.id; this.render();
    };
    this.bindNodeDrag(el, node, head); return el;
  }

  private nodeFields(node: WorkflowNode, body: HTMLElement) {
    if (node.type === "start") {
      const data = node.data as { appContext?: string | null; appTitle?: string | null; command?: string | null; executionMode?: "main_desktop" | "virtual_desktop"; focus?: boolean };
      const app = document.createElement("select");
      app.append(new Option("— application active / commande —", ""));
      for (const win of this.windows) {
        const option = new Option(`${win.is_virtual ? "[V] " : ""}${win.title} — ${win.context}`, String(win.id));
        option.dataset.context = win.context; option.dataset.title = win.title; app.add(option);
        if (data.appContext === win.context && data.appTitle === win.title) app.value = String(win.id);
      }
      const command = document.createElement("input"); command.type = "text"; command.placeholder = "e.g. notepad.exe or C:\\path\\app.exe"; command.value = data.command ?? "";
      app.onchange = () => {
        const option = app.selectedOptions[0];
        data.appContext = option.dataset.context ?? null;
        data.appTitle = option.dataset.title ?? null;
        if (!command.value && data.appContext && data.appContext !== "unknown") { command.value = data.appContext; data.command = data.appContext; }
      };
      command.oninput = () => data.command = command.value || null;
      const mode = document.createElement("select"); mode.append(new Option("Main desktop", "main_desktop"), new Option("Virtual desktop", "virtual_desktop")); mode.value = data.executionMode ?? "main_desktop"; mode.onchange = () => data.executionMode = mode.value as "main_desktop" | "virtual_desktop";
      const focus = document.createElement("input"); focus.type = "checkbox"; focus.checked = data.focus ?? false; focus.onchange = () => data.focus = focus.checked;
      const focusLabel = document.createElement("label"); focusLabel.className = "wf-check"; focusLabel.append(focus, " Focus application at workflow start");
      body.append(this.field("Target application", app), this.field("Launch command", command), this.field("Execution mode", mode), focusLabel);
    } else if (node.type === "macro") {
      const data = node.data as { macroName: string; macroContext?: string | null; injectionMode: "foreground" | "background" };
      const macro = document.createElement("select");
      macro.append(new Option("— choisir une macro —", ""));
      for (const item of this.macros) { const o = new Option(`[${item.context}] ${item.name}`, `${item.context}::${item.name}`); o.dataset.context = item.context; o.dataset.name = item.name; macro.add(o); }
      macro.value = data.macroName ? `${data.macroContext ?? ""}::${data.macroName}` : "";
      macro.onchange = () => { const o = macro.selectedOptions[0]; data.macroName = o.dataset.name ?? ""; data.macroContext = o.dataset.context ?? null; };
      const mode = document.createElement("select");
      mode.append(new Option("Background", "background"), new Option("Foreground", "foreground"));
      mode.value = data.injectionMode; mode.onchange = () => data.injectionMode = mode.value as "foreground" | "background";
      body.append(this.field("Macro", macro), this.field("Injection", mode));
    } else if (node.type === "loop") {
      const data = node.data as { repetitions: number }; const input = document.createElement("input"); input.type = "number"; input.min = "1"; input.max = "10000"; input.value = String(data.repetitions); input.oninput = () => data.repetitions = Number(input.value); body.append(this.field("Repetitions", input));
    } else if (node.type === "condition") {
      const data = node.data as { expression: string }; const input = document.createElement("input"); input.type = "text"; input.value = data.expression; input.oninput = () => data.expression = input.value; body.append(this.field("Expression", input));
    } else if (node.type === "wait") {
      const data = node.data as { durationMs: number }; const input = document.createElement("input"); input.type = "number"; input.min = "0"; input.max = "3600000"; input.value = String(data.durationMs); input.oninput = () => data.durationMs = Number(input.value); body.append(this.field("Duration ms", input));
    }
  }
  private field(label: string, control: HTMLElement) { const wrap = document.createElement("label"); wrap.className = "wf-field"; wrap.append(label, control); return wrap; }
  private handle(nodeId: string, handle: WorkflowHandle | "in", kind: "input" | "output", label: string) {
    const button = document.createElement("button"); button.className = `wf-handle ${kind}`; button.dataset.handle = handle; button.textContent = `${kind === "input" ? "●" : label + "  ●"}`;
    button.onclick = (e) => {
      e.stopPropagation();
      if (kind === "output") { this.pending = { nodeId, handle: handle as WorkflowHandle }; this.onStatus(`Connexion ${nodeId}.${handle} : choisissez une entrée`); this.render(); }
      else if (this.pending && this.pending.nodeId !== nodeId) {
        this.graph.edges = this.graph.edges.filter((edge) => !(edge.sourceNodeId === this.pending!.nodeId && edge.sourceHandle === this.pending!.handle));
        this.graph.edges.push({ id: `edge-${Date.now().toString(36)}-${this.sequence++}`, sourceNodeId: this.pending.nodeId, sourceHandle: this.pending.handle, targetNodeId: nodeId, targetHandle: "in" });
        this.pending = null; this.render();
      }
    };
    if (kind === "output" && this.pending?.nodeId === nodeId && this.pending.handle === handle) button.classList.add("pending");
    return button;
  }

  private bindNodeDrag(el: HTMLElement, node: WorkflowNode, handle: HTMLElement) {
    let dragging = false, grabX = 0, grabY = 0;
    const toWorld = (clientX: number, clientY: number) => {
      const rect = this.viewport.getBoundingClientRect();
      return {
        x: (clientX - rect.left - this.panX) / this.scale,
        y: (clientY - rect.top - this.panY) / this.scale,
      };
    };
    handle.onpointerdown = (e) => {
      if ((e.target as HTMLElement).tagName === "BUTTON") return;
      e.preventDefault();
      const point = toWorld(e.clientX, e.clientY);
      grabX = point.x - node.position.x;
      grabY = point.y - node.position.y;
      dragging = true;
      el.classList.add("dragging");
      handle.setPointerCapture(e.pointerId);
    };
    handle.onpointermove = (e) => {
      if (!dragging) return;
      const point = toWorld(e.clientX, e.clientY);
      node.position.x = point.x - grabX;
      node.position.y = point.y - grabY;
      el.style.left = `${node.position.x}px`;
      el.style.top = `${node.position.y}px`;
      this.renderEdges();
      this.renderMinimap();
    };
    const stop = (e: PointerEvent) => {
      if (!dragging) return;
      dragging = false;
      el.classList.remove("dragging");
      if (handle.hasPointerCapture(e.pointerId)) handle.releasePointerCapture(e.pointerId);
    };
    handle.onpointerup = stop;
    handle.onpointercancel = stop;
  }

  private renderEdges() {
    this.edgeSvg.replaceChildren();
    const defs = document.createElementNS("http://www.w3.org/2000/svg", "defs");
    for (const type of Object.keys(TITLES) as WorkflowNodeType[]) {
      const marker = document.createElementNS("http://www.w3.org/2000/svg", "marker");
      marker.setAttribute("id", `wfArrow-${type}`); marker.setAttribute("viewBox", "0 0 10 10"); marker.setAttribute("refX", "9"); marker.setAttribute("refY", "5"); marker.setAttribute("markerWidth", "7"); marker.setAttribute("markerHeight", "7"); marker.setAttribute("orient", "auto-start-reverse");
      const arrow = document.createElementNS("http://www.w3.org/2000/svg", "path"); arrow.setAttribute("d", "M 0 0 L 10 5 L 0 10 z"); arrow.setAttribute("class", `wf-arrow wf-arrow-${type}`); marker.append(arrow); defs.append(marker);
    }
    this.edgeSvg.append(defs);
    const routed: Point[][] = [];
    const edges = [...this.graph.edges].sort((a, b) => a.id.localeCompare(b.id));
    for (let edgeIndex = 0; edgeIndex < edges.length; edgeIndex++) {
      const edge = edges[edgeIndex];
      const source = this.graph.nodes[edge.sourceNodeId], target = this.graph.nodes[edge.targetNodeId]; if (!source || !target) continue;
      const start = this.portPoint(edge.sourceNodeId, edge.sourceHandle, "output");
      const end = this.portPoint(edge.targetNodeId, "in", "input");
      if (!start || !end) continue;
      const points = this.routeEdge(start, end, edge.sourceNodeId, edge.targetNodeId, routed, edgeIndex);
      routed.push(points);
      const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
      path.setAttribute("d", points.map((p, index) => `${index ? "L" : "M"} ${p.x} ${p.y}`).join(" "));
      path.classList.add("wf-edge", `wf-edge-${source.type}`);
      path.setAttribute("marker-end", `url(#wfArrow-${source.type})`);
      path.onclick = (e) => { e.stopPropagation(); this.graph.edges = this.graph.edges.filter((v) => v.id !== edge.id); this.render(); };
      this.edgeSvg.append(path);
    }
  }

  private portPoint(nodeId: string, handle: WorkflowHandle | "in", kind: "input" | "output"): Point | null {
    const node = this.graph.nodes[nodeId];
    const card = this.nodesLayer.querySelector<HTMLElement>(`[data-node-id="${CSS.escape(nodeId)}"]`);
    const port = card?.querySelector<HTMLElement>(`.wf-handle.${kind}[data-handle="${handle}"]`);
    if (!node || !card || !port) return null;
    let x = port.offsetLeft + port.offsetWidth / 2, y = port.offsetTop + port.offsetHeight / 2;
    let parent = port.offsetParent as HTMLElement | null;
    while (parent && parent !== card) { x += parent.offsetLeft; y += parent.offsetTop; parent = parent.offsetParent as HTMLElement | null; }
    return { x: node.position.x + x, y: node.position.y + y };
  }

  private routeEdge(start: Point, end: Point, sourceId: string, targetId: string, routed: Point[][], edgeIndex: number): Point[] {
    const MARGIN = 20;
    const obstacles: Rect[] = Object.values(this.graph.nodes)
      .map((node) => {
        const card = this.nodesLayer.querySelector<HTMLElement>(`[data-node-id="${CSS.escape(node.id)}"]`);
        return { nodeId: node.id, left: node.position.x - MARGIN, top: node.position.y - MARGIN, right: node.position.x + (card?.offsetWidth ?? 230) + MARGIN, bottom: node.position.y + (card?.offsetHeight ?? 160) + MARGIN };
      });
    const xs = new Set<number>([(start.x + end.x) / 2]);
    const ys = new Set<number>([(start.y + end.y) / 2]);
    // Une voie placée 1 px au-delà de l'enveloppe garantit strictement plus de
    // 20 px entre le lien et le contour visuel du nœud.
    for (const rect of obstacles) { xs.add(rect.left - 1); xs.add(rect.right + 1); ys.add(rect.top - 1); ys.add(rect.bottom + 1); }
    const allNodes = Object.values(this.graph.nodes);
    xs.add(Math.min(start.x, end.x, ...allNodes.map((n) => n.position.x)) - 70 - edgeIndex * 12);
    xs.add(Math.max(start.x, end.x, ...allNodes.map((n) => n.position.x + 260)) + 70 + edgeIndex * 12);
    ys.add(Math.min(start.y, end.y, ...allNodes.map((n) => n.position.y)) - 70 - edgeIndex * 12);
    ys.add(Math.max(start.y, end.y, ...allNodes.map((n) => n.position.y + 220)) + 70 + edgeIndex * 12);

    const candidates: Point[][] = [];
    if (start.x === end.x || start.y === end.y) candidates.push([start, end]);
    candidates.push([start, { x: end.x, y: start.y }, end], [start, { x: start.x, y: end.y }, end]);
    for (const x of xs) candidates.push([start, { x, y: start.y }, { x, y: end.y }, end]);
    for (const y of ys) candidates.push([start, { x: start.x, y }, { x: end.x, y }, end]);
    for (const x of xs) for (const y of ys) {
      candidates.push(
        [start, { x, y: start.y }, { x, y }, { x: end.x, y }, end],
        [start, { x: start.x, y }, { x, y }, { x, y: end.y }, end],
      );
    }

    // Retour droite -> gauche : deux couloirs verticaux sont nécessaires pour
    // quitter le handle droit et rejoindre le handle gauche sans toucher les
    // enveloppes source/cible. C'est le seul cas autorisé à utiliser 6 points.
    const rightLanes = [...xs].filter((x) => x > start.x + MARGIN);
    const leftLanes = [...xs].filter((x) => x < end.x - MARGIN);
    for (const outX of rightLanes) for (const inX of leftLanes) for (const y of ys) {
      candidates.push([start, { x: outX, y: start.y }, { x: outX, y }, { x: inX, y }, { x: inX, y: end.y }, end]);
    }

    const normalized = candidates.map((points) => this.compactRoute(points)).filter((points) => points.length >= 2 && points.length <= 6);
    normalized.sort((a, b) => this.routeScore(a, obstacles, routed, sourceId, targetId) - this.routeScore(b, obstacles, routed, sourceId, targetId));
    return normalized[0] ?? [start, { x: end.x, y: start.y }, end];
  }

  private compactRoute(points: Point[]): Point[] {
    const result: Point[] = [];
    for (const point of points) {
      const last = result[result.length - 1]; if (last && last.x === point.x && last.y === point.y) continue;
      if (result.length >= 2) {
        const previous = result[result.length - 2];
        if ((previous.x === last.x && last.x === point.x) || (previous.y === last.y && last.y === point.y)) { result[result.length - 1] = point; continue; }
      }
      result.push(point);
    }
    return result;
  }

  private routeScore(points: Point[], obstacles: Rect[], routed: Point[][], sourceId: string, targetId: string): number {
    let length = 0, obstacleHits = 0, crossings = 0, overlaps = 0;
    for (let i = 1; i < points.length; i++) {
      const a = points[i - 1], b = points[i]; length += Math.abs(a.x - b.x) + Math.abs(a.y - b.y);
      for (const rect of obstacles) {
        // L'exception ne couvre que la traversée normale de l'enveloppe depuis
        // le handle droit et vers le handle gauche. Un segment vertical le long
        // d'une carte reste donc une collision.
        const permittedExit = rect.nodeId === sourceId && i === 1
          && a.x === points[0].x && a.y === points[0].y && a.y === b.y && b.x >= a.x;
        const permittedEntry = rect.nodeId === targetId && i === points.length - 1
          && b.x === points[points.length - 1].x && b.y === points[points.length - 1].y && a.y === b.y && a.x <= b.x;
        if (!permittedExit && !permittedEntry && this.segmentHitsRect(a, b, rect)) obstacleHits++;
      }
      for (const route of routed) for (let j = 1; j < route.length; j++) {
        const relation = this.segmentRelation(a, b, route[j - 1], route[j]);
        if (relation === "cross") crossings++; else if (relation === "overlap") overlaps++;
      }
    }
    return obstacleHits * 1_000_000 + crossings * 20_000 + overlaps * 30_000 + (points.length - 2) * 35 + length;
  }

  private segmentHitsRect(a: Point, b: Point, rect: Rect): boolean {
    if (a.x === b.x) return a.x >= rect.left && a.x <= rect.right && Math.max(a.y, b.y) >= rect.top && Math.min(a.y, b.y) <= rect.bottom;
    return a.y >= rect.top && a.y <= rect.bottom && Math.max(a.x, b.x) >= rect.left && Math.min(a.x, b.x) <= rect.right;
  }

  private segmentRelation(a: Point, b: Point, c: Point, d: Point): "none" | "cross" | "overlap" {
    const aVertical = a.x === b.x, cVertical = c.x === d.x;
    if (aVertical === cVertical) {
      if (aVertical && a.x === c.x && Math.max(Math.min(a.y, b.y), Math.min(c.y, d.y)) < Math.min(Math.max(a.y, b.y), Math.max(c.y, d.y))) return "overlap";
      if (!aVertical && a.y === c.y && Math.max(Math.min(a.x, b.x), Math.min(c.x, d.x)) < Math.min(Math.max(a.x, b.x), Math.max(c.x, d.x))) return "overlap";
      return "none";
    }
    const v1 = aVertical ? a : c, v2 = aVertical ? b : d, h1 = aVertical ? c : a, h2 = aVertical ? d : b;
    const crosses = v1.x > Math.min(h1.x, h2.x) && v1.x < Math.max(h1.x, h2.x) && h1.y > Math.min(v1.y, v2.y) && h1.y < Math.max(v1.y, v2.y);
    return crosses ? "cross" : "none";
  }
  private renderMinimap() {
    this.minimap.replaceChildren(); const nodes = Object.values(this.graph.nodes); if (!nodes.length) return;
    const minX = Math.min(...nodes.map((n) => n.position.x)), minY = Math.min(...nodes.map((n) => n.position.y));
    const maxX = Math.max(...nodes.map((n) => n.position.x + 230)), maxY = Math.max(...nodes.map((n) => n.position.y + 160));
    const sx = 170 / Math.max(300, maxX - minX), sy = 105 / Math.max(200, maxY - minY), s = Math.min(sx, sy);
    for (const node of nodes) { const r = document.createElementNS("http://www.w3.org/2000/svg", "rect"); r.setAttribute("x", String(5 + (node.position.x - minX) * s)); r.setAttribute("y", String(5 + (node.position.y - minY) * s)); r.setAttribute("width", String(230 * s)); r.setAttribute("height", String(130 * s)); r.setAttribute("class", `mini-${node.type}`); this.minimap.append(r); }
    const view = document.createElementNS("http://www.w3.org/2000/svg", "rect");
    const rect = this.viewport.getBoundingClientRect();
    view.setAttribute("x", String(5 + ((-this.panX / this.scale) - minX) * s));
    view.setAttribute("y", String(5 + ((-this.panY / this.scale) - minY) * s));
    view.setAttribute("width", String((rect.width / this.scale) * s));
    view.setAttribute("height", String((rect.height / this.scale) * s));
    view.setAttribute("class", "mini-viewport"); this.minimap.append(view);
  }
  private applyTransform() { this.world.style.transform = `translate(${this.panX}px, ${this.panY}px) scale(${this.scale})`; this.root.querySelector("#workflowZoom")!.textContent = `${Math.round(this.scale * 100)}%`; this.renderMinimap(); }
  private fitView() {
    const nodes = Object.values(this.graph.nodes); if (!nodes.length) return;
    const minX = Math.min(...nodes.map((n) => n.position.x)), minY = Math.min(...nodes.map((n) => n.position.y));
    const maxX = Math.max(...nodes.map((n) => n.position.x + 250)), maxY = Math.max(...nodes.map((n) => n.position.y + 180));
    const rect = this.viewport.getBoundingClientRect(); this.scale = Math.max(.25, Math.min(1.2, Math.min((rect.width - 120) / (maxX - minX), (rect.height - 120) / (maxY - minY))));
    this.panX = 60 - minX * this.scale; this.panY = 60 - minY * this.scale; this.applyTransform();
  }
}
