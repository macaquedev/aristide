import {test, expect} from 'bun:test';
import {trackPointer} from './pointer-gesture.js';

test('a second finger cannot move or finish a layout gesture', () => {
  const host = new EventTarget(), moves=[], ends=[];
  const emit=(type,id)=>{const e=new Event(type); e.pointerId=id;host.dispatchEvent(e);};
  trackPointer({pointerId:1}, e=>moves.push(e.pointerId), (e,cancelled)=>ends.push(cancelled), host);
  emit('pointermove',2); emit('pointerup',2);
  expect(moves).toEqual([]); expect(ends).toEqual([]);
  emit('pointermove',1); emit('pointerup',1); emit('pointermove',1);
  expect(moves).toEqual([1]); expect(ends).toEqual([false]);
});

test('pointer cancellation and window blur terminate without committing', () => {
  for (const type of ['pointercancel','blur']) {
    const host=new EventTarget(), ends=[];
    trackPointer({pointerId:1}, ()=>{}, (_,cancelled)=>ends.push(cancelled), host);
    const event=new Event(type); event.pointerId=1; host.dispatchEvent(event);
    const up=new Event('pointerup');up.pointerId=1;host.dispatchEvent(up);
    expect(ends).toEqual([true]);
  }
});
