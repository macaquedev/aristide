import { test, expect } from 'bun:test';
import { PointerNotes } from './pointer-notes.js';

function rig() {
  const host = new EventTarget();
  const calls = [];
  const notes = new PointerNotes((...args) => calls.push(args), host);
  const key = (manual, midi) => {
    const element = new EventTarget();
    const classes = new Set();
    element.classList = { add: (...c) => c.forEach(x => classes.add(x)), remove: (...c) => c.forEach(x => classes.delete(x)) };
    notes.bind(element, manual, midi);
    return element;
  };
  const emit = (target, type, pointerId, extra = {}) => {
    const event = new Event(type, { cancelable: true });
    Object.assign(event, { pointerId, pointerType: 'touch', button: 0 }, extra);
    target.dispatchEvent(event);
  };
  return { host, calls, notes, key, emit };
}

test('lifting one finger leaves the other notes of a chord held', () => {
  const { host, calls, key, emit } = rig();
  emit(key(0, 60), 'pointerdown', 1);
  emit(key(0, 64), 'pointerdown', 2);
  emit(host, 'pointerup', 1);
  expect(calls).toEqual([[0,60,true], [0,64,true], [0,60,false]]);
  emit(host, 'pointerup', 2);
  expect(calls.at(-1)).toEqual([0,64,false]);
});

test('duplicate hex notes share a voice until the last finger releases', () => {
  const { host, calls, key, emit } = rig();
  emit(key(1, 60), 'pointerdown', 1);
  emit(key(1, 60), 'pointerdown', 2);
  emit(host, 'pointerup', 1);
  expect(calls).toEqual([[1,60,true]]);
  emit(host, 'pointercancel', 2);
  expect(calls).toEqual([[1,60,true], [1,60,false]]);
});

test('unrelated pointers and secondary buttons cannot release or play notes', () => {
  const { host, calls, key, emit } = rig();
  const k = key(0,60);
  emit(k, 'pointerdown', 1);
  emit(host, 'pointerup', 99);
  emit(k, 'pointerdown', 3, {button:2});
  expect(calls).toEqual([[0,60,true]]);
});

test('blur, capture loss and rebuild cleanup release notes exactly once', () => {
  const { host, calls, notes, key, emit } = rig();
  const k = key(0,60);
  emit(k,'pointerdown',1);
  emit(k,'lostpointercapture',1);
  emit(host,'pointerup',1);
  emit(k,'pointerdown',2);
  host.dispatchEvent(new Event('blur'));
  notes.releaseAll();
  expect(calls).toEqual([[0,60,true],[0,60,false],[0,60,true],[0,60,false]]);
});

test('touch drift holds the key while mouse leave releases it', () => {
  const { calls, key, emit } = rig();
  const k = key(0,60);
  emit(k,'pointerdown',1);
  emit(k,'pointerleave',1);
  expect(calls).toEqual([[0,60,true]]);
  emit(k,'pointerleave',1,{pointerType:'mouse'});
  expect(calls.at(-1)).toEqual([0,60,false]);
});

test('hiding a keyboard releases only that keyboard’s on-screen notes', () => {
  const {calls, notes, key, emit} = rig();
  emit(key(0,60), 'pointerdown', 1);
  emit(key(1,64), 'pointerdown', 2);
  notes.releaseManual(0);
  expect(calls).toEqual([[0,60,true], [1,64,true], [0,60,false]]);
  expect(notes.hasNote(1,64)).toBe(true);
});
