import type { ClientMessage, MacroFile, MacroParam, MacroStep, MacroSummary } from "./protocol";

type Sender = (message: ClientMessage) => void;

/** Templates inserted by the "+ semantic step" control. */
const SEMANTIC_TEMPLATES: Array<{ label: string; step: MacroStep }> = [
  { label: "Activate element", step: { type: "activate_element", element: "zone name or label", strategy: "auto" } },
  { label: "Type into element", step: { type: "type_into_element", element: "zone name or label", text: "{{value}}" } },
  { label: "Wait for state", step: { type: "wait_for_state", state: "state name", timeout_ms: 5000 } },
  { label: "Wait (ms)", step: { type: "wait_ms", duration_ms: 1000 } },
  { label: "Call macro", step: { type: "call_macro", name: "macro name" } },
];

export class MacroController {
  private steps: MacroStep[] = [];
  private params: MacroParam[] = [];
  private recording = false;
  private nativeW = 0;
  private nativeH = 0;
  private windowId: number | null = null;
  private context = "unknown";
  private lastMoveAt = 0;
  private draggedIndex: number | null = null;

  constructor(
    private readonly panel: HTMLElement,
    private readonly send: Sender,
    private readonly onLayout: () => void,
    private readonly onStatus: (message: string) => void,
    private readonly onRecordingChange: (recording: boolean) => void = () => {},
  ) {
    this.bind();
    this.render();
  }

  setTarget(windowId: number, context: string, width: number, height: number) {
    this.selectTarget(windowId, context);
    this.setNativeSize(width, height);
  }

  /**
   * Mémorise immédiatement la fenêtre choisie, sans attendre que le backend
   * ait fini de redémarrer la capture et renvoie le message `selected`.
   */
  selectTarget(windowId: number, context: string) {
    this.windowId = windowId;
    this.context = context || "unknown";
    this.panel.querySelector<HTMLElement>("#macroContext")!.textContent = this.context;
  }

  /** La taille native peut aussi être rafraîchie par les frames vidéo. */
  setNativeSize(width: number, height: number) {
    this.nativeW = width;
    this.nativeH = height;
  }

  togglePanel() {
    this.panel.hidden = !this.panel.hidden;
    document.getElementById("toggleMacros")?.classList.toggle("active", !this.panel.hidden);
    this.onLayout();
  }

  close() {
    this.setRecording(false);
    this.panel.hidden = true;
    document.getElementById("toggleMacros")?.classList.remove("active");
    this.onLayout();
  }

  observe(message: ClientMessage, semanticText?: string) {
    if (!this.recording || this.nativeW <= 0 || this.nativeH <= 0) return;
    let step: MacroStep | null = null;
    const nx = (x: number) => Math.max(0, Math.min(1, x / Math.max(1, this.nativeW - 1)));
    const ny = (y: number) => Math.max(0, Math.min(1, y / Math.max(1, this.nativeH - 1)));
    switch (message.type) {
      case "mouse_move": {
        const now = performance.now();
        if (now - this.lastMoveAt < 100) return;
        this.lastMoveAt = now;
        step = { type: "mouse_move", x: nx(message.x), y: ny(message.y) };
        break;
      }
      case "mouse_button": step = { ...message, x: nx(message.x), y: ny(message.y) }; break;
      case "wheel": step = { ...message, x: nx(message.x), y: ny(message.y) }; break;
      case "key":
        if (semanticText) {
          if (!message.pressed) return;
          const previous = this.steps[this.steps.length - 1];
          if (previous?.type === "text") previous.text += semanticText;
          else this.steps.push({ type: "text", text: semanticText });
          this.render();
          return;
        }
        step = { type: "key", code: message.code, pressed: message.pressed };
        break;
      case "text_input":
        if (!message.text) return;
        {
          const previous = this.steps[this.steps.length - 1];
          if (previous?.type === "text") previous.text += message.text;
          else this.steps.push({ type: "text", text: message.text });
          this.render();
          return;
        }
    }
    if (step) { this.steps.push(step); this.render(); }
  }

  private bind() {
    this.panel.querySelector<HTMLButtonElement>("#macroClose")!.onclick = () => this.close();
    this.panel.querySelector<HTMLButtonElement>("#macroRecord")!.onclick = () => {
      if (this.windowId === null) {
        this.onStatus("Sélectionnez une fenêtre puis cliquez sur Stream avant d’enregistrer une macro");
        return;
      }
      if (!this.recording && (this.nativeW <= 0 || this.nativeH <= 0)) {
        this.onStatus("Le flux de la fenêtre n’est pas encore prêt");
        return;
      }
      this.setRecording(!this.recording);
      this.onStatus(this.recording
        ? `Enregistrement de la macro en cours — ${this.context}`
        : `Enregistrement arrêté — ${this.steps.length} étape(s)`);
    };
    this.panel.querySelector<HTMLButtonElement>("#macroSave")!.onclick = () => this.save();
    this.panel.querySelector<HTMLButtonElement>("#macroLoad")!.onclick = () => this.send({ type: "list_macros" });
    this.panel.querySelector<HTMLButtonElement>("#macroTest")!.onclick = () => {
      if (!this.windowId) return this.onStatus("Sélectionnez une fenêtre avant de tester");
      if (!this.steps.length) return this.onStatus("La macro ne contient aucune étape");
      this.setRecording(false);
      this.send({ type: "test_macro", steps: this.steps });
      this.onStatus("Test de la macro en cours…");
    };
    const addStep = this.panel.querySelector<HTMLSelectElement>("#macroAddStep")!;
    for (const [index, template] of SEMANTIC_TEMPLATES.entries()) {
      addStep.add(new Option(template.label, String(index)));
    }
    addStep.onchange = () => {
      const template = SEMANTIC_TEMPLATES[Number(addStep.value)];
      addStep.value = "";
      if (!template) return;
      this.steps.push(JSON.parse(JSON.stringify(template.step)) as MacroStep);
      this.render();
      this.onStatus("Semantic step added — click ✎ to edit its JSON fields.");
    };
    this.panel.querySelector<HTMLSelectElement>("#macroLibrary")!.onchange = (event) => {
      const option = (event.currentTarget as HTMLSelectElement).selectedOptions[0];
      if (!option?.value) return;
      this.send({ type: "load_macro", context: option.dataset.context ?? "", name: option.dataset.name ?? "" });
    };
  }

  showLibrary(items: MacroSummary[]) {
    const select = this.panel.querySelector<HTMLSelectElement>("#macroLibrary")!;
    select.replaceChildren();
    const placeholder = document.createElement("option");
    placeholder.value = "";
    placeholder.textContent = items.length ? `— ${items.length} macro(s) —` : "Aucune macro sauvegardée";
    select.append(placeholder);
    for (const item of items) {
      const option = document.createElement("option");
      option.value = `${item.context}::${item.name}`;
      option.dataset.context = item.context;
      option.dataset.name = item.name;
      option.textContent = `[${item.context}] ${item.name} (${item.steps} étapes)`;
      select.append(option);
    }
    select.hidden = false;
    select.focus();
  }

  load(file: MacroFile) {
    this.setRecording(false);
    this.steps = file.steps;
    this.params = file.params ?? [];
    this.context = file.context;
    this.panel.querySelector<HTMLElement>("#macroContext")!.textContent = file.context;
    this.panel.querySelector<HTMLInputElement>("#macroName")!.value = file.name;
    this.render();
    this.onStatus(`macro ${file.name} chargée (${file.steps.length} étapes)`);
  }

  private syncRecordButton() {
    const button = this.panel.querySelector<HTMLButtonElement>("#macroRecord")!;
    button.classList.toggle("recording", this.recording);
    button.textContent = this.recording ? "⏹ Arrêter l’enregistrement" : "⏺ Enregistrer une nouvelle action";
  }

  private setRecording(recording: boolean) {
    if (this.recording === recording) {
      this.syncRecordButton();
      return;
    }
    this.recording = recording;
    this.syncRecordButton();
    this.onRecordingChange(recording);
  }

  private save() {
    const name = this.panel.querySelector<HTMLInputElement>("#macroName")!.value.trim();
    if (!this.windowId) return this.onStatus("Aucune fenêtre active");
    if (!name) return this.onStatus("Saisissez un nom de macro");
    if (!this.steps.length) return this.onStatus("La macro ne contient aucune étape");
    this.setRecording(false);
    this.send({ type: "save_macro", context: this.context, name, window_id: this.windowId, params: this.params, steps: this.steps });
  }

  private label(step: MacroStep): string {
    switch (step.type) {
      case "mouse_move": return `🖱 Déplacement — (${step.x.toFixed(3)}, ${step.y.toFixed(3)})`;
      case "mouse_button": return `🖱 ${step.button} ${step.pressed ? "appui" : "relâchement"} — (${step.x.toFixed(3)}, ${step.y.toFixed(3)})`;
      case "wheel": return `↕ Molette ${step.delta} — (${step.x.toFixed(3)}, ${step.y.toFixed(3)})`;
      case "key": return `⌨ ${step.code} — ${step.pressed ? "appui" : "relâchement"}`;
      case "text": return `⌨ Saisie texte — “${step.text}”`;
      case "activate_element": return `🎯 Activate “${step.element}”${step.layout ? ` (${step.layout})` : ""} — ${step.strategy ?? "auto"}`;
      case "type_into_element": return `🎯⌨ Type into “${step.element}” — “${step.text}”`;
      case "wait_for_state": return `⏳ Wait for state “${step.state}” (≤ ${step.timeout_ms ?? 5000} ms)`;
      case "wait_ms": return `⏳ Wait ${step.duration_ms} ms`;
      case "call_macro": return `📦 Call macro “${step.name}”${step.context ? ` [${step.context}]` : ""}`;
    }
  }

  private render() {
    const list = this.panel.querySelector<HTMLElement>("#macroSteps")!;
    list.replaceChildren();
    if (!this.steps.length) {
      const empty = document.createElement("p"); empty.className = "macro-empty"; empty.textContent = "Aucune action enregistrée"; list.append(empty); return;
    }
    this.steps.forEach((step, index) => {
      const card = document.createElement("article");
      card.className = "macro-step";
      card.draggable = true;
      card.title = "Glisser pour déplacer cette étape";
      card.addEventListener("dragstart", (event) => {
        this.draggedIndex = index;
        card.classList.add("dragging");
        event.dataTransfer?.setData("text/plain", String(index));
        if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
      });
      card.addEventListener("dragend", () => {
        this.draggedIndex = null;
        card.classList.remove("dragging");
        list.querySelectorAll(".drag-over").forEach((node) => node.classList.remove("drag-over"));
      });
      card.addEventListener("dragover", (event) => {
        event.preventDefault();
        if (this.draggedIndex !== null && this.draggedIndex !== index) card.classList.add("drag-over");
        if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
      });
      card.addEventListener("dragleave", () => card.classList.remove("drag-over"));
      card.addEventListener("drop", (event) => {
        event.preventDefault();
        card.classList.remove("drag-over");
        const from = this.draggedIndex ?? Number(event.dataTransfer?.getData("text/plain"));
        if (!Number.isInteger(from) || from === index || from < 0 || from >= this.steps.length) return;
        const [moved] = this.steps.splice(from, 1);
        // En descendant, insère après la carte survolée ; en remontant, avant.
        // Cette règle permet aussi d'échanger deux étapes adjacentes.
        const destination = Math.min(index, this.steps.length);
        this.steps.splice(destination, 0, moved);
        this.draggedIndex = null;
        this.render();
      });
      const label = document.createElement("span"); label.className = "macro-step-label"; label.textContent = `☰  ${index + 1}. ${this.label(step)}`;
      const actions = document.createElement("span"); actions.className = "macro-step-actions";
      const edit = document.createElement("button"); edit.textContent = "✎ Éditer"; edit.onclick = () => this.edit(card, index);
      const duplicate = document.createElement("button"); duplicate.textContent = "⧉ Dupliquer"; duplicate.onclick = () => {
        const copy = JSON.parse(JSON.stringify(this.steps[index])) as MacroStep;
        this.steps.splice(index + 1, 0, copy);
        this.render();
      };
      const del = document.createElement("button"); del.textContent = "✕ Suppr"; del.onclick = () => { this.steps.splice(index, 1); this.render(); };
      for (const button of [edit, duplicate, del]) {
        button.draggable = false;
        button.addEventListener("mousedown", (event) => event.stopPropagation());
      }
      actions.append(edit, duplicate, del); card.append(label, actions); list.append(card);
    });
  }

  private edit(card: HTMLElement, index: number) {
    const area = document.createElement("textarea"); area.value = JSON.stringify(this.steps[index], null, 2); area.className = "macro-json-editor";
    const save = document.createElement("button"); save.textContent = "Valider";
    const error = document.createElement("small"); error.className = "macro-edit-error";
    save.onclick = () => {
      try {
        const value = JSON.parse(area.value) as MacroStep;
        if (!value || typeof value.type !== "string") throw new Error("type manquant");
        this.steps[index] = value; this.render();
      } catch (e) { error.textContent = `JSON invalide : ${e}`; }
    };
    card.replaceChildren(area, save, error);
  }
}
