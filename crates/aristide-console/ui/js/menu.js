// The menu bar: desktop-style pull-down menus along the top of the
// console. Each menu's items are produced by a function called at open
// time, so checkmarks and lists that follow the organ (which manual the
// computer keyboard plays) need no separate refresh path.
//
// An item is `{label, run, accel, check, radio, disabled}` — `check` and
// `radio` are booleans describing the item's current state; "-" is a
// separator and a plain string is a section heading.

export class MenuBar {
  /// `menus` = [{title, items: () => [...]}, ...]. A menu may instead
  /// bring its own `{button, list}` elements from the page — how the
  /// organ's name, whose text the console owns, still pulls down like
  /// any other menu.
  constructor(root, host, menus) {
    this.root = root;
    this.menus = menus;
    this.open = null; // the {title, button, list} currently pulled down
    host.replaceChildren();

    for (const menu of menus) {
      let { button, list } = menu;
      if (!button) {
        const holder = document.createElement("div");
        holder.className = "menu";

        button = document.createElement("button");
        button.className = "menu-title";
        button.textContent = menu.title;
        button.setAttribute("aria-haspopup", "true");

        list = document.createElement("div");
        list.className = "menu-list hidden";
        list.setAttribute("role", "menu");

        holder.append(button, list);
        host.append(holder);
      }
      const entry = { menu, button, list };

      button.addEventListener("click", (event) => {
        event.stopPropagation();
        this.open === entry ? this.close() : this.show(entry);
      });
      // Once one menu is down, sliding along the bar opens the next —
      // the behaviour every desktop menu bar has.
      button.addEventListener("pointerenter", () => {
        if (this.open && this.open !== entry) this.show(entry);
      });
    }

    root.addEventListener("click", () => this.close());
    root.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && this.open) {
        event.stopPropagation();
        this.close();
      }
    });
  }

  show(entry) {
    if (this.open) this.hide(this.open);
    this.open = entry;
    entry.button.classList.add("on");
    entry.list.replaceChildren();
    for (const item of entry.menu.items()) {
      entry.list.append(this.render(item));
    }
    entry.list.classList.remove("hidden");
  }

  render(item) {
    if (item === "-") {
      const rule = document.createElement("hr");
      return rule;
    }
    if (typeof item === "string") {
      const heading = document.createElement("span");
      heading.className = "menu-heading";
      heading.textContent = item;
      return heading;
    }
    const button = document.createElement("button");
    button.className = "menu-item";
    button.setAttribute("role", "menuitem");
    button.disabled = Boolean(item.disabled);
    button.classList.toggle("checked", Boolean(item.check || item.radio));
    if (item.radio !== undefined) button.classList.add("radio");
    if (item.check !== undefined) button.classList.add("checkable");

    const label = document.createElement("span");
    label.textContent = item.label;
    button.append(label);
    if (item.accel) {
      const accel = document.createElement("kbd");
      accel.textContent = item.accel;
      button.append(accel);
    }
    button.addEventListener("click", () => {
      this.close();
      item.run?.();
    });
    return button;
  }

  hide(entry) {
    entry.list.classList.add("hidden");
    entry.button.classList.remove("on");
  }

  close() {
    if (!this.open) return;
    this.hide(this.open);
    this.open = null;
  }
}
