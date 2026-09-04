import { commands } from "./api.js";

const CATEGORIES = [
  ["general", "Organ & sound"], ["keyboards", "Keyboards & inputs"],
  ["stops", "Stops & pipes"], ["couplers", "Couplers"],
  ["sources", "Sample sets"], ["appearance", "Appearance & loading"],
];
const node = (tag, text, cls) => Object.assign(document.createElement(tag), {textContent:text, className:cls ?? ""});

// Reuse the live editors rather than maintaining a second set of controls.
// While this window is open they live inside its focus boundary; on close
// each returns to its exact original location for console shortcuts.
export class OrganPreferences {
  constructor(root, editor) {
    this.root = root; this.editor = editor; this.category = "general";
    this.modal = root.getElementById("organ-prefs");
    this.index = root.getElementById("organ-prefs-index");
    this.host = root.getElementById("organ-prefs-editors");
    this.back = root.getElementById("organ-prefs-back");
    this.moved = [];
    this.panels = [...root.querySelectorAll(".editor-add"), editor.el.removeConfirm, editor.el.linkConfirm];
    this.appPanes = [...root.querySelectorAll("#prefs .modal-body > *")];
    for (const [id, label] of CATEGORIES) {
      const button = this.button(label, () => this.select(id));
      button.dataset.category = id;
      root.getElementById("organ-prefs-nav").append(button);
    }
    for (const button of this.modal.querySelectorAll("[data-organ-prefs-close]")) button.addEventListener("click", () => this.close());
    this.back.addEventListener("click", () => this.select(this.category));
    this.observer = new MutationObserver(() => this.syncPanels());
    for (const panel of this.panels) this.observer.observe(panel, {attributes:true,attributeFilter:["class"]});
    // Own Escape before the canvas's popover dismissal; a nested modal
    // (Save a copy or an input conflict) retains its own Escape behavior.
    window.addEventListener("keydown", event => {
      if (event.key !== "Escape" || !this.isOpen || this.modal.inert) return;
      event.preventDefault(); event.stopImmediatePropagation();
      this.close();
    }, true);
  }
  get isOpen() { return !this.modal.classList.contains("hidden"); }
  move(element, parent) {
    const marker = document.createComment("settings home");
    element.before(marker); parent.append(element); element.inert = false;
    this.moved.push([element,marker]);
  }
  open() {
    if (this.isOpen || !this.editor.lastSnapshot?.organ) return;
    this.editor.setInspect(false); this.editor.closeAllPopovers(); this.editor.closeDrawer();
    for (const panel of this.panels) this.move(panel,this.host);
    this.move(this.editor.el.error, this.root.getElementById("organ-prefs-errors"));
    this.appearance = node("div", "", "hidden");
    this.host.append(this.appearance);
    for (const pane of this.appPanes) this.move(pane,this.appearance);
    this.modal.classList.remove("hidden");
    this.select(this.category);
  }
  close() {
    if (!this.isOpen) return;
    this.editor.closeAllPopovers(); this.editor.hideRemoveConfirm(); this.editor.pendingLink = null; this.editor.el.linkConfirm.classList.add("hidden");
    this.modal.classList.add("hidden");
    for (const [element,marker] of this.moved) marker.replaceWith(element);
    this.moved = []; this.appearance.remove();
  }
  button(label, run, note) {
    const button = node("button", "", "organ-pref-action"); button.type = "button";
    button.append(node("span",label));
    if (note) button.append(node("small",note));
    button.addEventListener("click",run); return button;
  }
  action(label, run, note, target=this.index) {
    target.append(this.button(label, () => { run(); this.syncPanels(); }, note));
  }
  edit(run) { this.editor.closeAllPopovers(); run(); this.syncPanels(); }
  add(kind) {
    this.edit(() => {
      this.editor.pendingPlace = null;
      this.editor.openAddMenu(20,80);
      if (kind) this.root.getElementById(`editor-add-${kind}`).click();
    });
  }
  select(category) {
    this.category = category; this.editor.closeAllPopovers(); this.editor.hideRemoveConfirm(); this.editor.pendingLink = null; this.editor.el.linkConfirm.classList.add("hidden");
    this.appearance?.classList.toggle("hidden", category !== "appearance");
    for (const button of this.modal.querySelectorAll("[data-category]")) button.setAttribute("aria-current", String(button.dataset.category === category));
    this.render(); this.syncPanels();
  }
  syncPanels() {
    if (!this.isOpen) return;
    const editing = this.panels.some(panel => !panel.classList.contains("hidden"));
    this.index.classList.toggle("hidden",editing || this.category === "appearance");
    this.back.classList.toggle("hidden",!editing);
  }
  update(snapshot) {
    this.snapshot = snapshot;
    if (!this.isOpen) return;
    if (!snapshot.organ) {this.close();return;}
    const context = this.root.getElementById("organ-prefs-context");
    const text = `${snapshot.organ} · Changes are saved automatically. Structural changes to a sample set's own organ require a copy.`;
    if (context.textContent !== text) context.textContent = text;
    const signature = JSON.stringify([snapshot.organ, snapshot.manuals?.map(m=>[m.idx,m.name,m.kind]), snapshot.stops?.map(s=>[s.id,s.name,s.midx]), snapshot.couplers?.map(c=>[c.idx,c.name])]);
    if (signature !== this.signature) {this.signature=signature; if (!this.index.classList.contains("hidden")) this.render();}
  }
  heading(text, target=this.index) {target.append(node("h2",text));}
  render() {
    const s = this.editor.lastSnapshot; if (!s) return;
    const e = this.editor;
    this.index.replaceChildren();
    this.root.getElementById("organ-prefs-context").textContent = `${s.organ} · Changes are saved automatically.`;
    const title = CATEGORIES.find(([id])=>id===this.category)?.[1];
    this.heading(title);
    if (this.category === "general") {
      this.rename("Organ name",s.organ,name=>e.send(commands.organRename(name)));
      this.action("Tuning & pitch",()=>this.edit(()=>e.openTuningForm("organ",20,80)),"Temperament, reference pitch and transposition");
      this.action("Room & noises",()=>this.edit(()=>e.openRoomForm(20,80)),"Reverb and mechanical sounds");
      this.action("Buttons & shortcuts",()=>this.edit(()=>e.openBindingsForm(20,80)),"Assign MIDI controls and computer keys");
      if (s.trems?.some(t=>!t.wave)) this.action("Tremulant",()=>this.edit(()=>e.openTremForm(20,80)),"Speed, depth and response");
      const boxes = node("details","","organ-pref-group"); boxes.append(node("summary","Swell boxes")); this.index.append(boxes);
      this.action("+ Add swell box",()=>this.add("enc"),null,boxes);
      for (const box of s.enclosures ?? []) this.action(`Remove ${box.name}…`,()=>e.showRemoveConfirm("enclosure",{name:box.name,stopCount:s.stops.filter(stop=>(stop.enc??[]).includes(box.idx)).length}),"Stops remain in the organ",boxes);
      this.action("Save a copy…",()=>e.openSaveAsForm(),"Keep this organ and continue editing a named copy");
      this.action("Add to this organ…",()=>this.add(),"Keyboards, couplers, swell boxes and sample sets");
      this.action("Choose a control on the console",()=>{this.close(); if(!e.unlocked)e.unlock();e.setInspect(true);},"Optional shortcut: select a stop, key or coupler to edit");
    } else if (this.category === "keyboards") {
      this.action("+ Add keyboard",()=>this.add("manual"));
      for (const m of s.manuals ?? []) {
        const group = node("details","","organ-pref-group");group.append(node("summary",m.name));this.index.append(group);
        this.rename("Keyboard name",m.name,name=>e.organCommand(commands.organManualRename(m.idx,name)),group);
        this.action("Connect keyboard / MIDI input",()=>this.edit(()=>e.openMidiForm(m.idx,20,80)),null,group);
        this.action("Key range",()=>this.edit(()=>e.openCompassForm(m.idx,20,80)),null,group);
        this.action("Division tuning",()=>this.edit(()=>e.openTuningForm({kind:"division",idx:m.idx},20,80)),null,group);
        const select = node("select","");select.setAttribute("aria-label",`${m.name} keyboard type`);
        for(const [value,label] of [["manual","Hand keyboard"],["pedal","Pedalboard"],["microtonal","Microtonal keyboard"]]){const option=node("option",label);option.value=value;select.append(option);}
        select.value=m.kind??(m.pedal?"pedal":"manual");select.addEventListener("change",()=>e.organCommand(commands.organManualKind(m.idx,select.value)));group.append(select);
        if(m.kind==="microtonal")this.action("Hex layout",()=>this.edit(()=>e.openHexForm(m.idx,20,80)),null,group);
        this.action("Add stops to this keyboard",()=>this.edit(()=>{e.openDivisionMenu(m.idx,this.back);e.showDivisionStops(e.el.divisionMenu,m);}),null,group);
        this.action("Remove keyboard…",()=>e.showRemoveConfirm("manual",{idx:m.idx,name:m.name,stopCount:s.stops.filter(stop=>stop.midx===m.idx).length}),null,group);
      }
    } else if (this.category === "stops") {
      const search = node("input","");search.type="search";search.placeholder="Find a stop";search.setAttribute("aria-label","Find a stop");this.index.append(search);
      const list=node("div","");this.index.append(list);
      const render=()=>{list.replaceChildren();for(const stop of s.stops??[]){if(!`${stop.name} ${stop.manual}`.toLocaleLowerCase().includes(search.value.toLocaleLowerCase()))continue;
        this.action(stop.name,()=>this.edit(()=>e.openStopForm(stop.id,20,80)),stop.manual,list);
      }};search.addEventListener("input",render);render();
      // Pipe selection without having to reach a tiny on-screen key.
      if (!s.stops?.length) { this.index.append(node("p","No stops yet. Add a sample set, then add its stops to a keyboard.","pane-note")); return; }
      const pipes=node("details","","organ-pref-group");pipes.append(node("summary","Individual pipes"));this.index.append(pipes);
      const stopSelect=node("select","");stopSelect.setAttribute("aria-label","Stop for pipe voicing");
      for(const stop of s.stops??[]){const o=node("option",`${stop.manual} · ${stop.name}`);o.value=stop.id;stopSelect.append(o);}pipes.append(stopSelect);
      const low=node("input","");low.type="number";low.required=true;low.min="0";low.max="65535";low.setAttribute("aria-label","First pipe key");
      const high=node("input","");high.type="number";high.required=true;high.min="0";high.max="65535";high.setAttribute("aria-label","Last pipe key");
      const reset=()=>{const stop=s.stops.find(v=>v.id===Number(stopSelect.value));const m=s.manuals.find(m=>m.idx===stop?.midx);low.value=high.value=String(m?.first_key??0);};stopSelect.addEventListener("change",reset);reset();
      for(const [label,input]of [["First key number",low],["Last key number",high]]){const field=node("label",label);field.append(input);pipes.append(field);}
      this.action("Edit selected pipes",()=>{if(!low.reportValidity()||!high.reportValidity())return;this.edit(()=>{const id=Number(stopSelect.value);e.openStopForm(id,20,80);e.openKeyVoicing(id,[Math.min(+low.value,+high.value),Math.max(+low.value,+high.value)],20,80);});},null,pipes);
    } else if(this.category === "couplers") {
      this.action("+ Add coupler",()=>this.add("coupler"));
      for(const c of s.couplers??[])this.action(c.name,()=>this.edit(()=>e.openCouplerForm(c.idx,20,80)),"Routes, key ranges and behavior");
      this.action("Coupler display options",()=>this.edit(()=>e.openCouplersMenu(20,80)));
    } else if(this.category === "sources") {
      this.action("+ Add sample set",()=>this.add("source"));
      const list=node("div","");this.index.append(list);list.append(node("p","Reading sample sets…","pane-note"));
      e.fetchOfferings(false).then(()=>{if(!list.isConnected)return;list.replaceChildren();for(const source of e.offerings??[]){this.heading(source.name??source.alias,list);list.append(node("p",source.path,"pane-note"));this.action("Sample set tuning",()=>this.edit(()=>e.openTuningForm({kind:"source",alias:source.alias},20,80)),null,list);}if(!e.offerings?.length)list.append(node("p","No sample sets added yet.","pane-note"));});
    }
  }
  rename(label,value,save,target=this.index) {
    const form=node("form","","organ-pref-rename");const input=node("input","");input.value=value;input.required=true;input.setAttribute("aria-label",label);
    const button=node("button","Rename","ghost");button.type="submit";form.append(input,button);form.addEventListener("submit",event=>{event.preventDefault();const name=input.value.trim();if(name&&name!==value)save(name);});target.append(form);
  }
}
