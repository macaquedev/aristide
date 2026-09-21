import { useCallback, useEffect, useRef, useState } from 'react';
import { endpoint, request, status, type Snapshot } from './api';

// One ordered stream of controls for the lifetime of the app. A late poll cannot
// overwrite a newer stop gesture, and note-off cannot overtake its note-on.
export function useEngine() {
  const [snapshot, setSnapshot] = useState<Snapshot>();
  const [error, setError] = useState<string>();
  const [dismissedError, setDismissedError] = useState<string>();
  const [ready, setReady] = useState(false);
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  const mounted = useRef(true);
  const update = useCallback((method: 'GET' | 'POST', url: string) => {
    const task = queue.current.catch(() => {}).then(() => request<Snapshot>(method, url));
    queue.current = task;
    return task.then(value => {
      if (mounted.current) {
        setSnapshot(value); setReady(true);
        setError(previous => previous === 'connection-lost' || previous === 'audio-unavailable' ? undefined : previous);
        setDismissedError(previous => previous === 'connection-lost' || previous === 'audio-unavailable' ? undefined : previous);
      }
      return value;
    });
  }, []);
  const command = useCallback((path: string, values: Record<string, string | number> = {}) => {
    void update('POST', endpoint(path, values)).catch(() => { setDismissedError(undefined); setError('change-failed'); });
  }, [update]);

  useEffect(() => {
    mounted.current = true;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const runtime = await status();
        if (cancelled) return;
        if (runtime.error) { setReady(false); setError(runtime.error); }
        else if (runtime.ready) await update('GET', '/api/state');
      } catch { if (!cancelled) { setReady(false); setError('connection-lost'); } }
      if (!cancelled) timer = setTimeout(poll, 250);
    };
    void poll();
    return () => { cancelled = true; mounted.current = false; clearTimeout(timer); };
  }, [update]);

  useEffect(() => {
    const down = new Set<string>();
    const release = () => {
      for (const code of down) command('key', { code, on: 0 });
      down.clear();
    };
    const keyDown = (event: KeyboardEvent) => {
      if (!ready || event.repeat || event.ctrlKey || event.metaKey || event.altKey ||
        (event.target instanceof Element && event.target.closest('input,textarea,select,[contenteditable="true"],[role="combobox"]'))) return;
      if (!/^(Key[A-Z]|Digit[0-9]|Comma|Period|Slash|Semicolon|BracketLeft|BracketRight|Quote)$/.test(event.code)) return;
      down.add(event.code);
      command('key', { code: event.code, on: 1 });
    };
    const keyUp = (event: KeyboardEvent) => {
      if (down.delete(event.code)) command('key', { code: event.code, on: 0 });
    };
    window.addEventListener('keydown', keyDown);
    window.addEventListener('keyup', keyUp);
    window.addEventListener('blur', release);
    return () => { release(); window.removeEventListener('keydown', keyDown); window.removeEventListener('keyup', keyUp); window.removeEventListener('blur', release); };
  }, [command, ready]);
  return { snapshot, ready, error: error === dismissedError ? undefined : error, dismissError: () => setDismissedError(error), command };
}
