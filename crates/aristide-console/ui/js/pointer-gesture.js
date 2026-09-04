// Follow only the pointer that started a layout gesture. An OS interruption
// cancels it instead of leaving a panel attached to the next finger or mouse.
export function trackPointer(start, move, finish, host = window) {
  const onMove = (event) => { if (event.pointerId === start.pointerId) move(event); };
  const end = (event) => {
    if (event.type !== 'blur' && event.pointerId !== start.pointerId) return;
    host.removeEventListener('pointermove', onMove);
    host.removeEventListener('pointerup', end);
    host.removeEventListener('pointercancel', end);
    host.removeEventListener('blur', end);
    finish(event, event.type !== 'pointerup');
  };
  host.addEventListener('pointermove', onMove);
  host.addEventListener('pointerup', end);
  host.addEventListener('pointercancel', end);
  host.addEventListener('blur', end);
}
