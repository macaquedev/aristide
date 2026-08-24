# 2026-08-24 — M6 contemporary layer: pitch moves, sound routes

One session, the rest of the milestone: sounding pipes learned to move
in pitch, and voices learned to leave by different doors.

## Ramped SetVoiceRate (the seam everything rides on)

`Command::SetVoiceRate { handle, rate, glide_ms }`: a Held voice slews
geometrically — constant cents per frame, so a glide reads as linear
pitch — toward the target, one step per block; `glide_ms: 0` snaps.
The sinc kernel bucket, picked once at StartVoice, is re-picked while
a glide is in motion (a bend can cross the quarter-octave boundary
tremulant wobble never does). Release tails freeze a glide in flight:
a tail is room decay, the same reason shutter moves never touch it.

## Live tuning drift

`Speaking` voices now remember where they were priced (sounding manual
+ post-transpose key) and the exact deviation. `/api/tuning?…&glide=ms`
(default 150) re-prices exactly that coordinate and glides each moved
voice by the exact cent delta **on the pipe it already plays** — a pipe
mid-speech glides; it is not re-recorded. Transpose changes still only
reroute future presses (a re-walk would have dragged held notes with
the transposer — caught by test). Tens of seconds of `glide` is a
performed drift, the contemporary-music feature by name.

## MPE per-note pitch

An input can declare a pitch-bend range (`bend = 48` in the organ
file's `[midi]` wiring, `bend=` on `/api/midi/bind`, a Bend control in
the console's MIDI panel). `0xE0` on a channel then bends the notes
that channel holds — per-note, since an MPE member channel carries one
note — through `Console::bend_key` (absolute cents on top of the
tuning, surviving drifts) into short-glide SetVoiceRate. A note
arriving on an already-bent channel starts bent, as MPE controllers
expect. Unconfigured inputs ignore the wheel, as organ consoles do.
MIDI 2.0 per-note pitch will reuse the same cents seam when the device
layer speaks UMP (midir is MIDI 1.0 byte streams; DESIGN's "MIDI 2.0
later").

## Lumatone input maps

`aristide-model::lumatone` parses the Lumatone Editor's `.ltn` files
(verified against real community mappings and the editor's own
serializer): five boards × 56 keys of note/channel/colour/type,
line-tolerant, first-wins on duplicate addresses. An input names a map
(`map = "layout.ltn"`, resolved against the organ file's dir,
warn-and-skip on failure) and the map replaces channel/compass routing:
only mapped (channel, note) pairs play, each landing in extended-note
numbering — the map's Nth used channel owns keys N×128 up — so a
280-key layout is one contiguous ladder for the Scala layer. Manual
keys widened to u16 through the whole control plane to make room
(MIDI wire stays u8; behaviour over the old range bit-identical).
Key colours parse and wait for the hex-field UI to use them.

## Output buses, routing, delays (effects graph goes public)

Every voice renders onto one of 8 stereo buses; each bus runs a delay
insert (mix/feedback/dry; the read head slews ~100 ms, so live time
changes bend tape-style instead of clicking) and lands on a chosen
interface channel pair. Sidecar:

```toml
[[routing.bus]]
stops = ["Trompette en chamade*"]
output = [3, 4]            # 1-based interface channels
[routing.bus.delay]
ms = 120
dry = 0.0                  # the division itself arrives late

[[voicing.delay]]
stops = ["Montre*"]        # per-pipe speaking delay
ms = 12.5
```

Resolution uses the coupler name-pattern rules; routing travels with
the STOP (borrowed pipes included). The stream widens to the channels
routing asks for when the device has an f32 layout at the same rate;
otherwise routed buses fold to the main pair with a warning — wrong,
never silent. `StartVoice` grew `bus` + `delay_frames`: an onset delay
holds the pipe silent and windless; released early, it never speaks.
`POST /api/bus` is the live performance knob. Default path (bus 0, no
delay, channels 0/1) is bit-identical to the pre-bus engine — every
exact-value engine test passes unchanged.

## Deferred, named

- MIDI 2.0 UMP parsing (device layer; the cents seam is ready).
- Full node graph (arbitrary effect nodes/edges) and per-single-pipe
  addressing — `[[voicing.delay]]` is per-stop patterns today. Both
  overlap M4's engine pass.
- Multi-device audio output; a full bus→channel dB matrix (GO-style).
- Hex-field key colours from `.ltn`; a console routing editor.
