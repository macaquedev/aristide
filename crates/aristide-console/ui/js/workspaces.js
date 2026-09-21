import { commands } from './api.js';

// Workspace navigation never clones a live field: the existing editor remains
// the single owner of its subject, commit-on-change handlers and file writes.
export class Workspaces {
  constructor({ root, editor, organPrefs, prefs, picker, view, keys }) {
    Object.assign(this, { root, editor, organPrefs, prefs, picker, view, keys });
    this.mode = 'library';
    picker.shouldAutoOpen = () => this.mode === 'library';
    this.changing = false;
    this.nav = root.getElementById('workspace-nav');
    this.palette = root.getElementById('command-palette');
    this.search = root.getElementById('command-search');
    // The lock is the only configuration entry on the playing surface.
    root.getElementById('menubar').append(root.getElementById('editor-lock-dock'));
    root.getElementById('organ-name').after(root.getElementById('offline'));
    editor.onModeChange = unlocked => {
      if (!this.changing) this.navigate(unlocked ? 'build' : 'play');
    };
    organPrefs.onOpen = () => {
      if (this.changing) return;
      this.changing = true;
      this.setMode(this.mode === 'voice' ? 'voice' : 'build');
      editor.unlock();
      this.changing = false;
    };
    organPrefs.onCategory = category => {
      const mode = ['general', 'stops', 'sources'].includes(category) && this.mode === 'voice' ? 'voice' : 'build';
      this.setMode(mode);
    };
    this.nav.addEventListener('click', event => {
      const button = event.target.closest('[data-workspace]');
      if (button) this.navigate(button.dataset.workspace);
    });
    root.getElementById('instrument-settings').addEventListener('click', () => this.navigate('build'));
    for (const button of root.querySelectorAll('#organ-prefs [data-organ-prefs-close], #prefs [data-close], #picker-close')) {
      button.addEventListener('click', () => {
        if (!this.picker.pickPending && this.snapshot?.organ) this.navigate('play');
      });
    }
    root.getElementById('command-open').addEventListener('click', () => this.openCommands());
    root.getElementById('command-close').addEventListener('click', () => this.closeCommands());
    this.search.addEventListener('input', () => this.renderCommands());
    this.search.addEventListener('keydown', event => {
      if (event.key === 'ArrowDown') { event.preventDefault(); root.querySelector('#command-results button')?.focus(); }
      if (event.key === 'Enter') { event.preventDefault(); root.querySelector('#command-results button')?.click(); }
    });
    window.addEventListener('keydown', event => {
      if (event.key === 'Escape' && !this.palette.classList.contains('hidden')) {
        event.preventDefault(); this.closeCommands();
      }
      if (this.mode === 'play' || !(event.ctrlKey || event.metaKey)) return;
      if (event.key.toLowerCase() === 'k') { event.preventDefault(); this.openCommands(); }
      const modes = { '1': 'library', '2': 'build', '3': 'voice', '4': 'setup' };
      if (modes[event.key]) { event.preventDefault(); this.navigate(modes[event.key]); }
    });
    // Native titles are tooltips too. Keep their accessible names without
    // allowing them to cover Play, including editor-generated title strings.
    root.addEventListener('pointerover', event => {
      if (this.mode !== 'play') return;
      for (let el = event.target; el?.removeAttribute; el = el.parentElement) {
        if (el.title) { if (!el.hasAttribute('aria-label')) el.setAttribute('aria-label', el.title); el.removeAttribute('title'); }
      }
    }, true);
    for (const name of ['contextmenu', 'dblclick']) root.getElementById('console').addEventListener(name, event => {
      if (this.mode === 'play') { event.preventDefault(); event.stopImmediatePropagation(); }
    }, true);
    // Dialogs are exclusively desk surfaces, including a refused edit or a
    // MIDI binding conflict raised asynchronously by the server.
    new MutationObserver(() => {
      if (this.changing) return;
      const dialog = root.querySelector('.modal:not(.hidden)');
      if (dialog && this.mode === 'play') this.navigate('build');
      if (picker.isOpen && this.mode !== 'library') this.setMode('library');
    }).observe(root.body, { subtree: true, attributes: true, attributeFilter: ['class'] });
    this.addLayoutAction();
    for (const surface of [organPrefs, prefs, picker]) {
      const close = surface.close.bind(surface);
      surface.close = (...args) => {
        close(...args);
        if (!this.changing && !surface.isOpen && this.snapshot?.organ && !this.snapshot.loading) this.navigate('play');
      };
    }
    this.addSetupNavigation();
  }

  addLayoutAction() {
    const button = document.createElement('button');
    button.id = 'build-layout'; button.textContent = 'Console layout';
    button.addEventListener('click', () => this.navigate('layout'));
    this.root.getElementById('organ-prefs-nav').append(button);
  }

  addSetupNavigation() {
    const card = this.root.querySelector('#prefs .modal-card');
    const body = card.querySelector('.modal-body');
    const layout = document.createElement('div'); layout.className = 'setup-layout';
    const nav = document.createElement('nav'); nav.className = 'setup-nav'; nav.setAttribute('aria-label', 'Setup sections');
    for (const [id, label] of [['appearance', 'Screen'], ['memory', 'Memory & loading']]) {
      const button = document.createElement('button'); button.textContent = label;
      button.addEventListener('click', () => {
        for (const pane of body.querySelectorAll('[data-pane]')) pane.hidden = pane.dataset.pane !== id;
        for (const other of nav.children) other.setAttribute('aria-current', String(other === button));
      }); nav.append(button);
    }
    layout.append(nav, body); card.append(layout); nav.firstElementChild.click();
  }

  setMode(mode) {
    this.keys.releaseAll();
    this.view.notes.releaseAll();
    this.mode = mode;
    this.root.body.dataset.workspace = mode;
    const voice = mode === 'voice';
    this.organPrefs.allowedCategories = voice ? ['general', 'stops', 'sources'] : ['general', 'keyboards', 'stops', 'couplers', 'sources', 'bindings'];
    this.root.getElementById('organ-prefs-title').textContent = voice ? 'Voice' : 'Build';
    for (const button of this.nav.querySelectorAll('button')) {
      button.setAttribute('aria-current', String(button.dataset.workspace === (mode === 'layout' ? 'build' : mode)));
    }
    this.root.getElementById('build-layout').hidden = voice;
    this.root.getElementById('console').inert = mode !== 'play' && mode !== 'layout';
    this.view.playLayout?.restore();
    if (this.view.snapshot) this.view.layoutPanels(this.view.snapshot);
  }

  navigate(mode) {
    if (this.changing) return;
    if (mode === 'play' && (!this.snapshot?.organ || this.snapshot.loading)) return;
    if (['build', 'voice', 'layout'].includes(mode) && !this.snapshot?.organ) mode = 'library';
    this.changing = true;
    if (this.organPrefs.isOpen) {
      this.organPrefs.close();
      if (this.organPrefs.isOpen) { this.changing = false; return; }
    }
    this.prefs.close();
    if (mode !== 'library' && this.picker.isOpen) {
      if ((!this.picker.closable && mode !== 'setup') || this.picker.pickPending) { this.changing = false; return; }
      this.picker.close(mode === 'setup');
    }
    this.closeCommands();
    this.setMode(mode);
    if (mode === 'play') {
      this.editor.lock();
      this.keys.close?.();
      this.root.getElementById('keys-legend').classList.add('hidden');
      this.root.body.classList.remove('modal-open');
    } else {
      this.editor.unlock();
      if (mode === 'library') this.picker.open();
      if (mode === 'setup') this.prefs.open();
      if (mode === 'build' || mode === 'voice') {
        this.organPrefs.open();
        this.organPrefs.select(mode === 'voice' ? 'general' : 'keyboards');
      }
    }
    this.changing = false;
    this.root.getElementById('editor-lock-label').textContent = mode === 'play' ? 'Edit' : 'Play';
    this.root.getElementById('editor-lock').disabled = !this.snapshot?.organ || !!this.snapshot.loading;
  }

  update(snapshot) {
    const previous = this.snapshot;
    this.snapshot = snapshot;
    this.root.body.dataset.ready = 'true';
    const ready = !!snapshot.organ && !snapshot.loading;
    for (const button of this.nav.querySelectorAll('[data-workspace="build"], [data-workspace="voice"]')) button.disabled = !ready;
    this.root.getElementById('editor-lock').disabled = !ready;
    this.root.getElementById('console').inert = !ready || !['play', 'layout'].includes(this.mode);
    if (snapshot.loading && this.mode === 'play') {
      this.navigate('library');
    } else if (ready && !this.picker.isOpen && this.mode === 'library' &&
      (!previous?.organ || previous.loading || previous.organ !== snapshot.organ)) {
      this.navigate(snapshot.manuals.length ? 'play' : 'build');
    }
  }

  commandList() {
    const list = [
      ['Open Library', () => this.navigate('library'), 'Ctrl 1'],
      ['Build instrument', () => this.navigate('build'), 'Ctrl 2'],
      ['Voice instrument', () => this.navigate('voice'), 'Ctrl 3'],
      ['Open Setup', () => this.navigate('setup'), 'Ctrl 4'],
      ['Return to Play', () => this.navigate('play'), 'Ctrl E'],
      ['Load organ or sample set', () => { this.navigate('library'); this.picker.newFromSet(); }],
      ['Create organ', () => { this.navigate('library'); this.picker.newBlank(); }],
      ['Silence', () => this.editor.send(commands.panic())],
      ['Toggle full screen', () => document.fullscreenElement ? document.exitFullscreen() : document.documentElement.requestFullscreen?.()],
      ['About Aristide', () => { this.navigate('setup'); this.prefs.openAbout(); }],
    ];
    if (this.snapshot?.organ) list.push(
      ['Save a copy', () => { this.navigate('build'); this.editor.openSaveAsForm(); }],
      ['Arrange console', () => this.navigate('layout')],
      ['Show computer keyboard map', () => { this.navigate('layout'); this.keys.toggle(); }],
      ['Edit tuning', () => { this.navigate('voice'); this.editor.openTuningForm('organ', 0, 0); }],
      ['Edit room and noises', () => { this.navigate('voice'); this.editor.openRoomForm(0, 0); }],
      ['Edit MIDI assignments', () => { this.navigate('build'); this.organPrefs.select('keyboards'); }],
      ['Edit buttons and shortcuts', () => { this.navigate('build'); this.editor.openBindingsForm(0, 0); }],
      ...this.snapshot.stops.map(stop => [`Voice ${stop.manual} · ${stop.name}`, () => { this.navigate('voice'); this.editor.openStopForm(stop.id, 0, 0); }]),
      ...this.snapshot.manuals.map(manual => [`Edit ${manual.name}`, () => { this.navigate('build'); this.organPrefs.openKeyboard(manual.idx); }]),
    );
    return list;
  }

  openCommands() {
    if (this.mode === 'play') return;
    this.palette.classList.remove('hidden'); this.search.value = ''; this.renderCommands(); this.search.focus();
  }
  closeCommands() { this.palette.classList.add('hidden'); }
  renderCommands() {
    const list = this.root.getElementById('command-results'); list.replaceChildren();
    for (const [label, run, shortcut] of this.commandList().filter(([label]) => label.toLocaleLowerCase().includes(this.search.value.toLocaleLowerCase()))) {
      const button = document.createElement('button'); button.textContent = label;
      if (shortcut) { const kbd = document.createElement('kbd'); kbd.textContent = shortcut; button.append(kbd); }
      button.addEventListener('click', () => { this.closeCommands(); run(); }); list.append(button);
    }
    if (!list.childElementCount) list.textContent = 'No matching commands. Try a stop or workspace name.';
  }
}
