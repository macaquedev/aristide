"""The A/B passage: one note-event list shared by the GO `.mid` writer
(make_midi.py) and the Aristide HTTP driver (drive_aristide.py), so both
engines play the identical performance in wall-clock time.

Registration (drawn once, before either engine plays a note — nothing in
this passage changes stops mid-piece, so a plain up-front draw is enough):

  First Manual: Montre 8', Prestant 4', Plein jeu III   (the toml sidecar's
                own example plein jeu, minus Bourdon 16' so the chorus
                stays inside the demo set's un-enclosed principal chorus)
  Second Manual: Trompette 8' alone                      (a solo reed —
                exposes release-splice character cleanly, no chorus to
                mask a bad splice)
  Pedal: Contre- basse 16', Sous- basse 16'               (drawn for
                section C's wind-sag chord; silent through A and B since
                no pedal notes sound until C)

Manual indexing matches both engines' own conventions for this set:
  GO:       Manual000 = Pedal (MIDIInputNumber 1), Manual001 = First Manual
            (MIDIInputNumber 2), Manual002 = Second Manual (MIDIInputNumber
            3) — see testsets/grandorgue-demo/demo.organ. GO's default
            initial MIDI binding is channel = MIDIInputNumber, verified
            empirically with tools/ab/probe_go_channel.mid (see README).
  Aristide: /api/state's `manuals` array is organ.manuals order, i.e.
            0 = Pedal, 1 = First Manual, 2 = Second Manual — verified
            empirically against a running server (see README).
Key numbers are raw MIDI note numbers (36 = C2) in both engines; the demo
set's FirstAccessibleKeyMIDINoteNumber is 36 on every manual (docs/go-odf-notes.md).

Three sections, back to back, silence between them:

  A. 0.0s   -- Grand plein jeu: a slow I-IV-V-vi-IV-V-I chord progression
               on First Manual. Chorus + mixture, so any beating between
               ranks or mixture-break awkwardness shows up here.
  B. 13.5s  -- Second Manual, Trompette 8' alone: an ascending then
               descending C-major scale, staccato, then the same shape
               legato. This is the release-splice probe: staccato forces
               a full release-and-reattack every note; legato forces fast
               voice overlap/steal.
  C. 24.0s  -- Held chord: First Manual plein jeu chord + Pedal 16' pair,
               all attacked together and held 12s. This is the wind-sag
               probe — a big simultaneous wind draw, then a released
               chord to see the whole chorus let go at once.

Total: 37.5s of material (some tail silence added by the renderer).
"""

from dataclasses import dataclass

MANUAL_PEDAL = 0
MANUAL_I = 1
MANUAL_II = 2

# GO channel = MIDIInputNumber for this set (Pedal=1, Manual I=2, Manual
# II=3); MIDI channels here are 0-indexed (0..15) as every MIDI library
# expects, so subtract 1 from the ODF's 1-based MIDIInputNumber.
GO_CHANNEL = {MANUAL_PEDAL: 0, MANUAL_I: 1, MANUAL_II: 2}

# (aristide_stop_id, go_stop_name, manual) -- go_stop_name is exactly the
# ODF's DispLabelText/Name, verbatim (the demo set really does write
# "Contre- basse 16'" and "Sous- basse 16'" with a space after the
# hyphen -- see testsets/grandorgue-demo/demo.organ lines 294/341).
REGISTRATION = [
    (15, "Montre 8'", MANUAL_I),
    (19, "Prestant 4'", MANUAL_I),
    (20, "Plein jeu III", MANUAL_I),
    (36, "Trompette 8'", MANUAL_II),
    (0, "Contre- basse 16'", MANUAL_PEDAL),
    (1, "Sous- basse 16'", MANUAL_PEDAL),
]


@dataclass(frozen=True)
class Event:
    t: float  # seconds from passage start
    manual: int
    key: int
    on: bool
    velocity: int = 100


def _chord(t0: float, dur: float, notes: list[int], manual: int, vel: int = 100) -> list[Event]:
    return [Event(t0, manual, n, True, vel) for n in notes] + [
        Event(t0 + dur, manual, n, False, vel) for n in notes
    ]


def _run(t0: float, notes: list[int], note_dur: float, period: float, manual: int, vel: int) -> list[Event]:
    events = []
    for i, n in enumerate(notes):
        on_t = t0 + i * period
        events.append(Event(on_t, manual, n, True, vel))
        events.append(Event(on_t + note_dur, manual, n, False, vel))
    return events


def build_passage() -> list[Event]:
    events: list[Event] = []

    # -- Section A: plein jeu chord progression (First Manual) --------
    chords = [
        (0.0, 1.4, [60, 64, 67, 72]),  # C
        (1.5, 1.4, [65, 69, 72, 77]),  # F
        (3.0, 1.4, [67, 71, 74, 79]),  # G
        (4.5, 1.4, [60, 64, 67, 72]),  # C
        (6.0, 1.4, [57, 72, 76, 81]),  # Am (open voicing)
        (7.5, 1.4, [65, 69, 72, 77]),  # F
        (9.0, 1.4, [67, 71, 74, 79]),  # G
        (10.5, 2.4, [60, 64, 67, 72]),  # C, held longer
    ]
    for t0, dur, notes in chords:
        events += _chord(t0, dur, notes, MANUAL_I, vel=100)

    # -- Section B: Trompette 8' solo, staccato then legato (Second Manual) --
    b0 = 13.5
    scale_up = [60, 62, 64, 65, 67, 69, 71, 72]
    scale_down = list(reversed(scale_up))

    events += _run(b0, scale_up, note_dur=0.18, period=0.30, manual=MANUAL_II, vel=95)
    b1 = b0 + len(scale_up) * 0.30 + 0.22
    events += _run(b1, scale_down, note_dur=0.18, period=0.30, manual=MANUAL_II, vel=95)
    b2 = b1 + len(scale_down) * 0.30 + 0.22
    # Legato: period 0.28s, note duration 0.32s -- each note starts 0.04s
    # before the previous one releases, forcing a real overlap/voice-steal
    # rather than a clean gap.
    events += _run(b2, scale_up, note_dur=0.32, period=0.28, manual=MANUAL_II, vel=95)
    b3 = b2 + len(scale_up) * 0.28 + 0.22
    events += _run(b3, scale_down, note_dur=0.32, period=0.28, manual=MANUAL_II, vel=95)
    b_end = b3 + len(scale_down) * 0.28

    # -- Section C: held chord, First Manual + Pedal (wind sag) -------
    c0 = 24.0
    c_dur = 12.0
    events += _chord(c0, c_dur, [60, 64, 67, 72], MANUAL_I, vel=110)
    events += _chord(c0, c_dur, [36, 43], MANUAL_PEDAL, vel=110)

    events.sort(key=lambda e: (e.t, not e.on))  # note-offs before note-ons at a shared instant
    return events


TOTAL_SECONDS = 24.0 + 12.0 + 1.5  # section C end + tail silence


if __name__ == "__main__":
    for e in build_passage():
        print(f"{e.t:7.3f}  manual={e.manual} key={e.key:3d} {'on ' if e.on else 'off'} vel={e.velocity}")
