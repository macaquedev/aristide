// Screen-local pagination. Never writes panel coordinates or organ structure.
// Every DOM control keeps its original listener, state key and server action.
export function pagePlan(width, height, counts) {
  const gap = 16;
  const perScreen = Math.max(1, Math.min(counts.length || 1, Math.floor((width + gap) / 208)));
  const columnWidth = (width - gap * (perScreen - 1)) / perScreen;
  const stopColumns = Math.max(1, Math.floor((columnWidth + 8) / 128));
  const maxCount = Math.max(0, ...counts);
  const stopHeight = [96, 80, 64].find(size => Math.floor((height - 128) / (size + 8)) * stopColumns >= maxCount) ?? 64;
  const rows = Math.max(1, Math.floor((height - 128) / (stopHeight + 8)));
  const capacity = rows * stopColumns;
  const pages = [];
  for (let start = 0; start < counts.length; start += perScreen) {
    const end = Math.min(counts.length, start + perScreen);
    const slices = Math.max(1, ...counts.slice(start, end).map(n => Math.ceil(n / capacity)));
    for (let slice = 0; slice < slices; slice++) pages.push({ start, end, offset: slice * capacity });
  }
  return { pages, columnWidth, stopColumns, stopHeight, capacity };
}

export class PlayLayout {
  constructor(view) {
    this.view = view;
    this.root = view.root;
    this.canvas = view.el.canvas;
    this.page = 0;
    this.moves = [];
    this.strips = new Map();
    const nav = document.createElement('nav'); nav.className = 'play-pages'; nav.setAttribute('aria-label', 'Stop pages');
    this.label = document.createElement('span'); this.label.setAttribute('aria-live', 'polite');
    this.prev = this.button('Previous stop page', '←', () => { this.page--; this.layout(); });
    this.next = this.button('Next stop page', '→', () => { this.page++; this.layout(); });
    nav.append(this.label, this.prev, this.next); this.canvas.before(nav);
  }
  button(label, text, run) {
    const el = document.createElement('button'); el.textContent = text; el.setAttribute('aria-label', label); el.addEventListener('click', run); return el;
  }
  move(el, parent) {
    if (!el || !parent) return;
    const marker = document.createComment('desk location'); el.before(marker); parent.append(el); this.moves.push([el, marker]);
  }
  restore() {
    for (const [el, marker] of this.moves) if (marker.isConnected) marker.replaceWith(el);
    this.moves = [];
    for (const el of this.canvas.querySelectorAll('[data-page-hidden]')) el.removeAttribute('data-page-hidden');
    for (const el of this.canvas.querySelectorAll('.strip-page')) el.remove();
    this.strips.clear();
    for (const panel of this.view.panels.values()) { panel.style.bottom = ''; panel.style.height = ''; }
    this.prepared = null;
  }
  prepare() {
    const first = this.view.panels.values().next().value;
    if (this.prepared === first) return;
    this.restore(); this.prepared = first; this.page = 0;
    const rail = this.canvas.querySelector('.coupler-rail');
    for (const manual of this.view.snapshot.manuals) {
      const jamb = this.view.panels.get(`jamb:${manual.name}`);
      const divisionals = this.view.panels.get(`keyboard:${manual.name}`)?.querySelector('.divisional-rail');
      this.move(divisionals, jamb?.querySelector('.division'));
      for (const coupler of jamb?.querySelectorAll('.coupler') ?? []) this.move(coupler, rail);
    }
    // Native titles must not create tooltips over the instrument.
    for (const el of this.canvas.querySelectorAll('[title]')) {
      if (!el.hasAttribute('aria-label')) el.setAttribute('aria-label', el.title);
      el.removeAttribute('title');
    }
  }
  paginateStrip(el, capacity, name) {
    if (!el) return;
    let state = this.strips.get(el);
    if (!state) {
      const items = [...el.children].filter(item => !item.classList.contains("division-add"));
      state = { items, page: 0 };
      const step = delta => { state.page += delta; this.layout(); };
      state.prev = this.button(`Previous ${name}`, '←', () => step(-1));
      state.next = this.button(`Next ${name}`, '→', () => step(1));
      state.prev.className = state.next.className = 'strip-page';
      el.prepend(state.prev); el.append(state.next); this.strips.set(el, state);
    }
    const paged = state.items.length > capacity;
    const limit = Math.max(1, paged ? capacity - 2 : capacity);
    const pages = Math.max(1, Math.ceil(state.items.length / limit));
    state.page = Math.max(0, Math.min(state.page, pages - 1));
    for (const [i, item] of state.items.entries()) item.toggleAttribute('data-page-hidden', i < state.page * limit || i >= (state.page + 1) * limit);
    state.prev.hidden = state.next.hidden = !paged;
    state.prev.disabled = state.page === 0; state.next.disabled = state.page === pages - 1;
  }
  layout() {
    if (this.root.body.dataset.workspace !== 'play' || !this.view.snapshot) return;
    this.prepare();
    const width = this.canvas.clientWidth, height = this.canvas.clientHeight;
    if (!width || !height) return;
    const panels = this.view.panels;
    const couplers = panels.get('couplers');
    const pistons = panels.get('pistons');
    const shoes = panels.get('shoes');
    const expressionWidth = shoes && width >= 700 ? Math.min(328, width / 3) : width;
    const couplerWidth = shoes && width >= 700 ? width - expressionWidth - 16 : width;
    const generalCapacity = width < 360 ? 3 : Math.max(3, Math.floor((width + 8) / 72));
    this.paginateStrip(pistons?.querySelector('.piston-bank'), generalCapacity, 'generals');
    this.paginateStrip(couplers?.querySelector('.coupler-rail'), Math.max(3, Math.floor((couplerWidth + 8) / 152)), 'couplers');
    this.paginateStrip(shoes?.querySelector('.shoes'), Math.max(3, Math.floor((expressionWidth + 8) / 208)), 'swell boxes');
    const place = (panel, left, bottom, panelWidth) => {
      if (!panel) return 0;
      Object.assign(panel.style, { left: `${left}px`, top: 'auto', bottom: `${bottom}px`, width: `${panelWidth}px` });
      return panel.offsetHeight;
    };
    let bottom = place(pistons, 0, 0, width) + 16;
    if (shoes && width >= 700) {
      bottom += Math.max(place(couplers, 0, bottom, couplerWidth), place(shoes, width - expressionWidth, bottom, expressionWidth)) + 16;
    } else {
      for (const panel of [shoes, couplers]) if (panel) bottom += place(panel, 0, bottom, width) + 16;
    }
    const jambs = this.view.snapshot.manuals.map(m => panels.get(`jamb:${m.name}`)).filter(Boolean);
    const plan = pagePlan(width, height - bottom, jambs.map(p => p.querySelector('.division-knobs').children.length));
    this.page = Math.max(0, Math.min(this.page, plan.pages.length - 1));
    const page = plan.pages[this.page];
    this.prev.disabled = this.page === 0; this.next.disabled = this.page >= plan.pages.length - 1;
    const names = page ? this.view.snapshot.manuals.slice(page.start, page.end).map(m => m.name).join(' · ') : '';
    this.label.textContent = plan.pages.length > 1 ? `${this.page + 1} / ${plan.pages.length} · ${names}` : 'Stops';
    for (const [i, panel] of jambs.entries()) {
      const shown = page && i >= page.start && i < page.end;
      panel.toggleAttribute('data-page-hidden', !shown);
      if (!shown) continue;
      Object.assign(panel.style, { left: `${(i - page.start) * (plan.columnWidth + 16)}px`, top: '0px', bottom: 'auto', width: `${plan.columnWidth}px` });
      panel.style.setProperty('--stop-columns', plan.stopColumns);
      panel.style.setProperty('--stop-height', `${plan.stopHeight}px`);
      for (const [n, stop] of [...panel.querySelector('.division-knobs').children].entries()) {
        stop.toggleAttribute('data-page-hidden', n < page.offset || n >= page.offset + plan.capacity);
      }
      this.paginateStrip(panel.querySelector('.divisional-rail'), Math.max(3, Math.floor((plan.columnWidth + 8) / 64)), `${this.view.snapshot.manuals[i].name} divisionals`);
    }
  }
}
