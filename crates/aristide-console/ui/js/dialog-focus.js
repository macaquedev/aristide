// Existing dialogs own their close/commit behavior. This shared layer owns
// focus, so Tab and screen readers cannot reach the instrument behind them.
export function wireDialogFocus(root) {
  const dialogs = [...root.querySelectorAll('.modal[role="dialog"]')];
  const stack = [];
  const previous = new WeakMap();
  const focusable = (dialog) => [...dialog.querySelectorAll(
    'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), summary, [tabindex="0"]'
  )].filter((el) => el.getClientRects().length && !el.closest('[inert]'));
  const focusFirst = (dialog) => {
    dialog.tabIndex = -1;
    (focusable(dialog)[0] ?? dialog).focus({ preventScroll: true });
  };
  const sync = () => {
    let restore;
    for (const dialog of [...stack].reverse()) {
      if (!dialog.classList.contains('hidden')) continue;
      restore = previous.get(dialog);
      stack.splice(stack.indexOf(dialog), 1);
    }
    for (const dialog of dialogs) {
      if (dialog.classList.contains('hidden') || stack.includes(dialog)) continue;
      previous.set(dialog, root.activeElement);
      stack.push(dialog);
    }
    stack.forEach((dialog, index) => { dialog.style.zIndex = String(60 + index); });
    const top = stack.at(-1);
    root.body.classList.toggle('modal-open', !!top);
    for (const element of root.body.children) {
      if (element.tagName === 'SCRIPT') continue;
      element.inert = !!top && element !== top;
    }
    if (top && !top.contains(root.activeElement)) focusFirst(top);
    else if (!top && restore?.isConnected && restore.getClientRects().length) restore.focus({ preventScroll: true });
  };
  const observer = new MutationObserver(sync);
  for (const dialog of dialogs) observer.observe(dialog, { attributes: true, attributeFilter: ['class'] });
  root.addEventListener('keydown', (event) => {
    const top = stack.at(-1);
    if (!top || event.key !== 'Tab') return;
    const items = focusable(top);
    const at = items.indexOf(root.activeElement);
    if (!items.length) { event.preventDefault(); top.focus(); return; }
    if (event.shiftKey && at <= 0) { event.preventDefault(); items.at(-1).focus(); }
    else if (!event.shiftKey && (at === items.length - 1 || at < 0)) { event.preventDefault(); items[0].focus(); }
  }, true);
  sync();
}
