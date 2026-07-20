import type { ActivationStep, ClientMessage, LayoutFile, LayoutSummary, LayoutZone, LayoutZoneKind, UiStateSummary } from "./protocol";
import type { Annotator } from "./annotate";

type ActivationKind = ActivationStep["type"];

const ACTIVATION_KINDS: Array<{ value: ActivationKind; label: string; needsCode: boolean }> = [
  { value: "click_center", label: "Click center", needsCode: false },
  { value: "double_click", label: "Double click", needsCode: false },
  { value: "key", label: "Key press", needsCode: true },
  { value: "focus_then_key", label: "Focus + key", needsCode: true },
  { value: "type_text", label: "Type text (runtime)", needsCode: false },
];

function activationLabel(step: ActivationStep): string {
  switch (step.type) {
    case "click_center": return "click center";
    case "double_click": return "double click";
    case "key": return `key ${step.code}`;
    case "focus_then_key": return `focus + ${step.code}`;
    case "type_text": return "type text";
  }
}

const ZONE_KINDS: Array<{ value: LayoutZoneKind; label: string }> = [
  { value: "button", label: "Button / clickable" },
  { value: "input", label: "Input field" },
  { value: "textarea", label: "Text area" },
  { value: "dropdown", label: "Dropdown" },
  { value: "menu", label: "Menu" },
  { value: "checkbox", label: "Checkbox" },
  { value: "radio", label: "Radio button" },
  { value: "link", label: "Link" },
  { value: "text", label: "Text / label" },
  { value: "region", label: "Generic region" },
];

export class LayoutController {
  private context = "unknown";
  private windowId: number | null = null;
  private sourceWidth = 1280;
  private sourceHeight = 720;
  private isOpen = false;
  private pendingSavedName: string | null = null;

  private readonly contextEl: HTMLElement;
  private readonly nameInput: HTMLInputElement;
  private readonly library: HTMLSelectElement;
  private readonly zonesEl: HTMLElement;
  private readonly drawBtn: HTMLButtonElement;
  private readonly toggleBtn: HTMLButtonElement;
  private readonly jsonModal: HTMLElement;
  private readonly jsonArea: HTMLTextAreaElement;
  private readonly jsonError: HTMLElement;
  private readonly stateNameInput: HTMLInputElement;
  private readonly stateListEl: HTMLElement;

  constructor(
    private readonly panel: HTMLElement,
    private readonly annotator: Annotator,
    private readonly send: (message: ClientMessage) => void,
    private readonly selectTool: (tool: "cursor" | "slice") => void,
    private readonly relayout: () => void,
    private readonly status: (message: string) => void,
  ) {
    this.contextEl = panel.querySelector("#layoutContext") as HTMLElement;
    this.nameInput = panel.querySelector("#layoutName") as HTMLInputElement;
    this.library = panel.querySelector("#layoutLibrary") as HTMLSelectElement;
    this.zonesEl = panel.querySelector("#layoutZones") as HTMLElement;
    this.drawBtn = panel.querySelector("#layoutDraw") as HTMLButtonElement;
    this.toggleBtn = document.getElementById("toggleLayout") as HTMLButtonElement;
    this.jsonModal = document.getElementById("zonesModal") as HTMLElement;
    this.jsonArea = document.getElementById("zonesJsonArea") as HTMLTextAreaElement;
    this.jsonError = document.getElementById("zonesError") as HTMLElement;
    this.stateNameInput = panel.querySelector("#stateName") as HTMLInputElement;
    this.stateListEl = panel.querySelector("#stateList") as HTMLElement;
    panel.querySelector("#stateCapture")?.addEventListener("click", () => this.captureState());

    panel.querySelector("#layoutClose")?.addEventListener("click", () => this.close());
    panel.querySelector("#layoutNew")?.addEventListener("click", () => this.createNew());
    panel.querySelector("#layoutLoad")?.addEventListener("click", () => this.loadSelected());
    panel.querySelector("#layoutSave")?.addEventListener("click", () => this.save());
    panel.querySelector("#layoutJson")?.addEventListener("click", () => this.openJson());
    this.drawBtn.addEventListener("click", () => {
      this.selectTool("slice");
      this.drawBtn.classList.add("active");
      this.status("Layout: tracez une zone sur l'application, puis renseignez son type.");
    });
    this.library.addEventListener("change", () => {
      const selected = this.library.selectedOptions[0];
      if (selected?.dataset.name) this.nameInput.value = selected.dataset.name;
    });

    document.getElementById("zonesClose")?.addEventListener("click", () => this.closeJson());
    document.getElementById("zonesCopy")?.addEventListener("click", () => void this.copyJson());
    document.getElementById("zonesImport")?.addEventListener("click", () => this.importJson());
    this.jsonModal.addEventListener("keydown", (event) => {
      if (event.key === "Escape") this.closeJson();
    });

    this.annotator.setZonesChangedHandler(() => this.renderZones());
    this.renderZones();
  }

  toggle() { this.isOpen ? this.close() : this.open(); }

  open() {
    this.isOpen = true;
    this.panel.hidden = false;
    this.toggleBtn.classList.add("active");
    this.annotator.setLayoutVisible(true);
    this.selectTool("slice");
    this.send({ type: "list_layouts", context: this.context });
    this.send({ type: "list_states", context: this.context });
    this.relayout();
  }

  close() {
    this.isOpen = false;
    this.panel.hidden = true;
    this.toggleBtn.classList.remove("active");
    this.drawBtn.classList.remove("active");
    this.annotator.setLayoutVisible(false);
    this.selectTool("cursor");
    this.relayout();
  }

  selectTarget(windowId: number, context: string) {
    const changed = this.windowId !== windowId || this.context !== context;
    this.windowId = windowId;
    this.context = context || "unknown";
    this.contextEl.textContent = this.context;
    if (changed) {
      this.nameInput.value = "layout_1";
      this.annotator.clearLayoutZones();
      this.send({ type: "list_layouts", context: this.context });
      this.send({ type: "list_states", context: this.context });
    }
  }

  setNativeSize(width: number, height: number) {
    this.sourceWidth = Math.max(1, width);
    this.sourceHeight = Math.max(1, height);
  }

  showLibrary(items: LayoutSummary[]) {
    const contextKey = this.context.trim().toLocaleLowerCase();
    const matching = items.filter((item) => item.context.trim().toLocaleLowerCase() === contextKey);
    this.library.replaceChildren();
    const placeholder = document.createElement("option");
    placeholder.value = "";
    placeholder.textContent = matching.length ? `— ${matching.length} layout(s) —` : "— aucun layout —";
    this.library.appendChild(placeholder);
    for (const item of matching) {
      const option = document.createElement("option");
      option.value = `${item.context}\u0000${item.name}`;
      option.dataset.context = item.context;
      option.dataset.name = item.name;
      option.textContent = `${item.name} (${item.zones} zones)`;
      this.library.appendChild(option);
    }
    if (this.pendingSavedName) {
      const wanted = this.storeKey(this.pendingSavedName);
      const savedOption = [...this.library.options].find((option) =>
        option.dataset.name && this.storeKey(option.dataset.name) === wanted
      );
      if (savedOption) {
        this.library.value = savedOption.value;
        this.nameInput.value = savedOption.dataset.name ?? this.pendingSavedName;
      }
      this.pendingSavedName = null;
    }
  }

  saved(context: string, name: string) {
    if (context.trim().toLocaleLowerCase() !== this.context.trim().toLocaleLowerCase()) return;
    this.pendingSavedName = name;
  }

  /** Captures the current frame's zone hashes as a named, recognizable state. */
  private captureState() {
    const name = this.stateNameInput.value.trim();
    if (!this.windowId || this.context === "unknown") {
      this.status("Select a target application first.");
      return;
    }
    if (!name) {
      this.status("State name is required.");
      this.stateNameInput.focus();
      return;
    }
    const zones = this.annotator.layoutZones();
    if (zones.length === 0) {
      this.status("Draw or load at least one zone: state signatures are hashed from zones.");
      return;
    }
    this.send({
      type: "capture_state",
      context: this.context,
      name,
      layout: this.nameInput.value.trim() || null,
      zones,
    });
  }

  showStates(items: UiStateSummary[]) {
    const contextKey = this.context.trim().toLocaleLowerCase();
    const matching = items.filter((item) => item.context.trim().toLocaleLowerCase() === contextKey);
    this.stateListEl.replaceChildren();
    if (matching.length === 0) {
      const empty = document.createElement("small");
      empty.textContent = "No states yet. Bring the app to the wanted screen, then Capture state.";
      empty.style.opacity = "0.55";
      this.stateListEl.appendChild(empty);
      return;
    }
    for (const item of matching) {
      const row = document.createElement("div");
      row.className = "layout-state";
      const name = document.createElement("span");
      name.className = "state-name";
      name.textContent = item.name;
      const info = document.createElement("small");
      info.textContent = `${item.layout ? `layout ${item.layout} · ` : ""}${item.signatures} signature(s)`;
      const remove = document.createElement("button");
      remove.textContent = "×";
      remove.title = "Delete this state";
      remove.addEventListener("click", () => this.send({ type: "delete_state", context: item.context, name: item.name }));
      row.append(name, info, remove);
      this.stateListEl.appendChild(row);
    }
  }

  refreshStates() {
    this.send({ type: "list_states", context: this.context });
  }

  load(file: LayoutFile) {
    this.context = file.context;
    this.contextEl.textContent = file.context;
    this.nameInput.value = file.name;
    this.annotator.setLayoutZones(file.zones);
    this.status(`layout ${file.name} chargé : ${file.zones.length} zone(s)`);
  }

  private createNew() {
    this.nameInput.value = "layout_1";
    this.annotator.clearLayoutZones();
    this.selectTool("slice");
    this.status("Nouveau layout : dessinez les zones UI sur le stream.");
  }

  private loadSelected() {
    const option = this.library.selectedOptions[0];
    const context = option?.dataset.context;
    const name = option?.dataset.name;
    if (!context || !name) {
      this.status("Sélectionnez un layout à charger.");
      return;
    }
    this.send({ type: "load_layout", context, name });
  }

  private save() {
    const name = this.nameInput.value.trim();
    if (!this.windowId || this.context === "unknown") {
      this.status("Sélectionnez d'abord une application cible.");
      return;
    }
    if (!name) {
      this.status("Le nom du layout est obligatoire.");
      this.nameInput.focus();
      return;
    }
    const zones = this.annotator.layoutZones();
    if (zones.length === 0) {
      this.status("Le layout doit contenir au moins une zone UI.");
      return;
    }
    this.send({
      type: "save_layout",
      context: this.context,
      name,
      source_width: this.sourceWidth,
      source_height: this.sourceHeight,
      zones,
    });
  }

  private renderZones() {
    const zones = this.annotator.layoutZones();
    this.zonesEl.replaceChildren();
    if (zones.length === 0) {
      const empty = document.createElement("p");
      empty.className = "layout-empty";
      empty.textContent = "Aucune zone. Cliquez sur Draw zone puis tracez un élément UI sur le stream.";
      this.zonesEl.appendChild(empty);
      return;
    }
    zones.forEach((zone, index) => this.zonesEl.appendChild(this.zoneRow(zone, index)));
  }

  private zoneRow(zone: LayoutZone, index: number): HTMLElement {
    const row = document.createElement("article");
    row.className = "layout-zone";
    const heading = document.createElement("div");
    heading.className = "layout-zone-head";
    const number = document.createElement("strong");
    number.textContent = `${index + 1}.`;
    const kind = document.createElement("select");
    kind.title = "Type d'élément exposé à l'agent MCP";
    for (const item of ZONE_KINDS) kind.add(new Option(item.label, item.value));
    kind.value = zone.kind;
    kind.addEventListener("change", () => this.annotator.updateLayoutZone(zone.id, { kind: kind.value as LayoutZoneKind }));
    const remove = document.createElement("button");
    remove.textContent = "×";
    remove.title = "Supprimer la zone";
    remove.addEventListener("click", () => this.annotator.deleteLayoutZone(zone.id));
    heading.append(number, kind, remove);

    const name = document.createElement("input");
    name.type = "text";
    name.value = zone.name;
    name.placeholder = "Nom lisible (ex. bouton Connexion)";
    name.maxLength = 120;
    name.addEventListener("change", () => this.annotator.updateLayoutZone(zone.id, { name: name.value.trim() }));
    const description = document.createElement("input");
    description.type = "text";
    description.value = zone.description ?? "";
    description.placeholder = "Description / usage pour l'agent";
    description.maxLength = 300;
    description.addEventListener("change", () => this.annotator.updateLayoutZone(zone.id, { description: description.value.trim() }));
    const label = document.createElement("input");
    label.type = "text";
    label.value = zone.label ?? "";
    label.placeholder = "Visible text on screen (e.g. New Game)";
    label.maxLength = 120;
    label.addEventListener("change", () => this.annotator.updateLayoutZone(zone.id, { label: label.value.trim() }));
    const expected = document.createElement("input");
    expected.type = "text";
    expected.value = zone.expectedState ?? "";
    expected.placeholder = "Expected state after activation (e.g. gameplay)";
    expected.maxLength = 120;
    expected.addEventListener("change", () => this.annotator.updateLayoutZone(zone.id, { expectedState: expected.value.trim() }));
    const b = zone.bounds;
    const coords = document.createElement("small");
    coords.textContent = `x ${b.x.toFixed(3)} · y ${b.y.toFixed(3)} · ${b.width.toFixed(3)} × ${b.height.toFixed(3)}`;
    row.append(heading, name, label, description, expected, this.activationEditor(zone), coords);
    return row;
  }

  /** Ordered activation sequence editor: chips + add controls. */
  private activationEditor(zone: LayoutZone): HTMLElement {
    const steps: ActivationStep[] = [...(zone.activation ?? [])];
    const box = document.createElement("div");
    box.className = "zone-activation";

    const chips = document.createElement("div");
    chips.className = "zone-activation-steps";
    if (steps.length === 0) {
      const hint = document.createElement("small");
      hint.textContent = "Activation: auto (click, then click + Enter)";
      chips.appendChild(hint);
    }
    steps.forEach((step, index) => {
      const chip = document.createElement("span");
      chip.className = "zone-activation-step";
      chip.textContent = `${index + 1}. ${activationLabel(step)}`;
      const remove = document.createElement("button");
      remove.textContent = "×";
      remove.title = "Remove step";
      remove.addEventListener("click", () => {
        const next = steps.filter((_, i) => i !== index);
        this.annotator.updateLayoutZone(zone.id, { activation: next.length ? next : null });
      });
      chip.appendChild(remove);
      chips.appendChild(chip);
    });

    const controls = document.createElement("div");
    controls.className = "zone-activation-add";
    const kindSelect = document.createElement("select");
    kindSelect.title = "Activation step to append";
    for (const item of ACTIVATION_KINDS) kindSelect.add(new Option(item.label, item.value));
    const code = document.createElement("input");
    code.type = "text";
    code.placeholder = "Key code (e.g. Enter)";
    code.maxLength = 40;
    const syncCode = () => {
      const needsCode = ACTIVATION_KINDS.find((item) => item.value === kindSelect.value)?.needsCode ?? false;
      code.hidden = !needsCode;
    };
    kindSelect.addEventListener("change", syncCode);
    syncCode();
    const add = document.createElement("button");
    add.textContent = "+ Step";
    add.title = "Append this step to the activation sequence";
    add.addEventListener("click", () => {
      const kind = kindSelect.value as ActivationKind;
      const needsCode = ACTIVATION_KINDS.find((item) => item.value === kind)?.needsCode ?? false;
      const keyCode = code.value.trim();
      if (needsCode && !keyCode) {
        this.status("A key code is required for this activation step.");
        code.focus();
        return;
      }
      const step: ActivationStep = needsCode
        ? { type: kind as "key" | "focus_then_key", code: keyCode }
        : { type: kind as "click_center" | "double_click" | "type_text" };
      this.annotator.updateLayoutZone(zone.id, { activation: [...steps, step] });
    });
    controls.append(kindSelect, code, add);

    box.append(chips, controls);
    return box;
  }

  private currentFile(): LayoutFile {
    return {
      version: 1,
      context: this.context,
      name: this.nameInput.value.trim() || "layout_1",
      sourceWidth: this.sourceWidth,
      sourceHeight: this.sourceHeight,
      zones: this.annotator.layoutZones(),
    };
  }

  private openJson() {
    this.jsonArea.value = JSON.stringify(this.currentFile(), null, 2);
    this.jsonError.textContent = "";
    this.jsonModal.style.display = "flex";
    this.jsonArea.focus();
  }

  private closeJson() { this.jsonModal.style.display = "none"; }

  private async copyJson() {
    try {
      await navigator.clipboard.writeText(this.jsonArea.value);
      this.status("Layout JSON copié dans le presse-papiers.");
    } catch {
      this.jsonError.textContent = "Impossible de copier le JSON.";
    }
  }

  private importJson() {
    try {
      const parsed = JSON.parse(this.jsonArea.value) as Partial<LayoutFile> | unknown[];
      if (Array.isArray(parsed)) {
        // Compatibilité avec les anciens exports Slice en pixels natifs.
        this.annotator.importSlicesJson(JSON.stringify(parsed));
      } else if (parsed && Array.isArray(parsed.zones)) {
        this.validateZones(parsed.zones);
        if (typeof parsed.name === "string") this.nameInput.value = parsed.name;
        this.annotator.setLayoutZones(parsed.zones);
      } else {
        throw new Error("Le JSON doit contenir un tableau 'zones'.");
      }
      this.closeJson();
      this.status("Layout JSON importé.");
    } catch (error) {
      this.jsonError.textContent = error instanceof Error ? error.message : String(error);
    }
  }

  private validateZones(zones: LayoutZone[]) {
    for (const [index, zone] of zones.entries()) {
      if (!zone || typeof zone.id !== "string" || typeof zone.name !== "string" || !zone.bounds) {
        throw new Error(`Zone ${index + 1} invalide.`);
      }
      if (!ZONE_KINDS.some((kind) => kind.value === zone.kind)) {
        throw new Error(`Type inconnu pour la zone ${index + 1}.`);
      }
      const b = zone.bounds;
      if (![b.x, b.y, b.width, b.height].every((value) => typeof value === "number" && value >= 0 && value <= 1)) {
        throw new Error(`Coordonnées invalides pour la zone ${index + 1}.`);
      }
      if (b.width <= 0 || b.height <= 0 || b.x + b.width > 1 || b.y + b.height > 1) {
        throw new Error(`La zone ${index + 1} dépasse la surface normalisée.`);
      }
    }
  }

  private storeKey(value: string): string {
    return value.trim().replace(/[^A-Za-z0-9._-]/g, "_").replace(/^_+|_+$/g, "").toLocaleLowerCase();
  }
}
