// Idempotent DOM writes for everything the 120 ms poll repaints.
//
// Assigning `textContent` replaces the text node even when the string
// is unchanged, and `replaceChildren` replaces every child. WebKit
// dispatches no click when the node a press landed on is gone by the
// release (Chromium falls back to the common ancestor, which still
// loses a rebuilt button's own listener) — so a control repainted on
// every poll only works when the press fits between two polls, or lands
// on padding that owns no text node. Poll-driven code writes through
// these, and the DOM under the pointer only changes when the organ did.

/// Sets the element's text only when it differs.
export function setText(el, text) {
  if (el.textContent !== text) el.textContent = text;
}

/// Runs `render()` — which rebuilds `el`'s children — only when
/// `signature` differs from the one `el` was last built for. The
/// signature rides on the element itself, so a popover closed and
/// reopened onto unchanged state is not rebuilt either.
export function renderIfChanged(el, signature, render) {
  if (el.dataset.renderedFor === signature) return;
  el.dataset.renderedFor = signature;
  render();
}

/// Forgets `el`'s last-rendered signature, so the next `renderIfChanged`
/// on it rebuilds unconditionally — for popovers that force a fresh
/// paint on open even when the state underneath happens not to have
/// moved since they last closed.
export function resetRender(el) {
  delete el.dataset.renderedFor;
}
