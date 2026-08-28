// The conflict dialog: the server parks a MIDI bind rather than silently
// giving a device or a message a second job, and reports it as
// `snapshot.conflict` until the player says what to do with it. The
// dialog is visible exactly while that field exists — there is nothing
// to open or close by hand — and every way out of it, backdrop click
// included, sends a choice: the server holds the parked bind, so merely
// hiding the dialog would leave it stuck rather than resolved.

import { commands } from "./api.js";
import { actionLabel, keyGlyph } from "./wiring.js";
import { noteTriggerText } from "./pitch.js";

/// Mirrors wiring.js's own triggerText, applied to a conflict object
/// rather than a control row: a computer key reads as the character it
/// is, a note as its pitch, any other MIDI message as its wire text —
/// each plus device and channel.
function triggerText(conflict) {
  if (conflict.trigger.startsWith("key:")) {
    return `key:${keyGlyph(conflict.trigger.slice(4))}`;
  }
  const channel = conflict.channel ? ` ch${conflict.channel}` : "";
  return `${noteTriggerText(conflict.trigger)} · ${conflict.device}${channel}`;
}

/// An action as prose: the catalogue label for a bare verb, and the
/// label plus the name for a targeted one — "stop:Gamba 8'" reads as
/// Stop Gamba 8', not as its wire text.
function actionText(action) {
  const at = action.indexOf(":");
  if (at === -1) return actionLabel(action);
  const verb = actionLabel(action.slice(0, at + 1)).replace("…", "");
  return `${verb} ${action.slice(at + 1)}`;
}

export class ConflictDialog {
  constructor(root, send) {
    this.root = root;
    this.send = send;
    this.signature = null; // rebuild only when the conflict itself changes
    this.el = {
      modal: root.getElementById("conflict"),
      title: root.getElementById("conflict-title"),
      body: root.getElementById("conflict-body"),
    };
    // The backdrop is another way to say "cancel", not a free dismiss.
    this.el.modal
      .querySelector(".modal-backdrop")
      .addEventListener("click", () => this.choose("cancel"));
  }

  update(snapshot) {
    const conflict = snapshot.conflict ?? null;
    const wasShown = !this.el.modal.classList.contains("hidden");
    this.el.modal.classList.toggle("hidden", !conflict);
    // `modal-open` is shared with preferences and the picker, so it is
    // only touched on this dialog's own transitions — and only released
    // when no other modal is still up underneath (the usual case: the
    // conflict came from a bind made inside preferences).
    if (conflict) {
      this.root.body.classList.add("modal-open");
    } else if (wasShown && !this.root.querySelector(".modal:not(.hidden)")) {
      this.root.body.classList.remove("modal-open");
    }
    if (!conflict) {
      this.signature = null;
      return;
    }
    // The 120ms poll reasserts the same conflict over and over until the
    // player answers; only a genuinely new one should tear out the
    // buttons a click might be mid-flight to.
    const signature = JSON.stringify(conflict);
    if (signature === this.signature) return;
    this.signature = signature;
    this.build(conflict);
  }

  choose(choice) {
    this.send(commands.conflict(choice));
  }

  build(conflict) {
    if (conflict.kind === "input") this.buildInput(conflict);
    else this.buildControl(conflict);
  }

  buildInput(conflict) {
    this.el.title.textContent = "Already assigned";
    const channel = conflict.channel ? ` (channel ${conflict.channel})` : "";
    const manuals = conflict.existing.map((e) => e.manual).join(", ");
    this.el.body.replaceChildren(
      this.paragraph(`${conflict.device}${channel} already plays ${manuals}.`),
      this.paragraph(
        `Assign it to ${conflict.manual} as well, move it there, or leave things as they are?`
      ),
      this.actions()
    );
  }

  buildControl(conflict) {
    this.el.title.textContent = "Already bound";
    const actions = conflict.existing.map((e) => actionText(e.action)).join(", ");
    this.el.body.replaceChildren(
      this.paragraph(`${triggerText(conflict)} is already bound to ${actions}.`),
      this.paragraph(
        `Bind it to ${actionText(conflict.action)} as well, replace that binding, or cancel?`
      ),
      this.actions()
    );
  }

  paragraph(text) {
    const p = document.createElement("p");
    p.className = "pane-note";
    p.textContent = text;
    return p;
  }

  actions() {
    const row = document.createElement("div");
    row.className = "conflict-actions";

    const keep = document.createElement("button");
    keep.className = "conflict-primary";
    keep.textContent = "Keep both";
    keep.addEventListener("click", () => this.choose("keep"));
    row.append(keep);

    const replace = document.createElement("button");
    replace.className = "ghost";
    replace.textContent = "Replace";
    replace.addEventListener("click", () => this.choose("replace"));
    row.append(replace);

    const cancel = document.createElement("button");
    cancel.className = "ghost";
    cancel.textContent = "Cancel";
    cancel.addEventListener("click", () => this.choose("cancel"));
    row.append(cancel);

    return row;
  }
}
