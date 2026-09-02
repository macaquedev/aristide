#!/usr/bin/env python3
"""Write tools/ab/passage.py's event list as a Standard MIDI File (format
0, one track) for GrandOrgue's MIDI Player (Audio/Midi -> Load MIDI,
Ctrl+P; play from the Recorder/Player panel). No external MIDI library
needed -- SMF is simple enough to emit by hand.

Usage: python3 tools/ab/make_midi.py OUT.mid
"""

import struct
import sys

from passage import GO_CHANNEL, build_passage

PPQN = 480  # ticks per quarter note
US_PER_QUARTER = 500_000  # 120 bpm tempo map -- only used to convert our
# absolute seconds into ticks; the notes' actual timing is what matters,
# not the nominal tempo.


def seconds_to_ticks(t: float) -> int:
    return round(t * PPQN * 1_000_000 / US_PER_QUARTER)


def write_varlen(value: int) -> bytes:
    buf = [value & 0x7F]
    value >>= 7
    while value:
        buf.append((value & 0x7F) | 0x80)
        value >>= 7
    return bytes(reversed(buf))


def build_midi_bytes() -> bytes:
    events = build_passage()
    # Absolute tick, MIDI status/data bytes.
    midi_events: list[tuple[int, bytes]] = []
    for e in events:
        channel = GO_CHANNEL[e.manual]
        status = (0x90 if e.on else 0x80) | channel
        velocity = e.velocity if e.on else 0
        midi_events.append((seconds_to_ticks(e.t), bytes([status, e.key, velocity])))
    midi_events.sort(key=lambda ev: ev[0])

    track = bytearray()
    # Tempo meta event at t=0.
    track += write_varlen(0)
    track += bytes([0xFF, 0x51, 0x03]) + US_PER_QUARTER.to_bytes(3, "big")

    last_tick = 0
    for tick, data in midi_events:
        delta = tick - last_tick
        last_tick = tick
        track += write_varlen(delta)
        track += data
    # End of track.
    track += write_varlen(0)
    track += bytes([0xFF, 0x2F, 0x00])

    header = b"MThd" + struct.pack(">IHHH", 6, 0, 1, PPQN)
    track_chunk = b"MTrk" + struct.pack(">I", len(track)) + bytes(track)
    return header + track_chunk


def main() -> None:
    if len(sys.argv) != 2:
        print("usage: make_midi.py OUT.mid", file=sys.stderr)
        raise SystemExit(2)
    data = build_midi_bytes()
    with open(sys.argv[1], "wb") as f:
        f.write(data)
    print(f"wrote {sys.argv[1]} ({len(data)} bytes)")


if __name__ == "__main__":
    main()
