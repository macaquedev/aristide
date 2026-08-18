# 2026-08-18 — couplers become routes

A coupler was `{from, to, key_shift}`: one flat shift over the whole
keyboard, engaged or not. Real consoles are already richer than that
(unison off, coupler key ranges), and the couplers this project exists
for are richer still — "a fourths coupler, but only from tenor C", "a
16' that transposes the bottom octave down instead of leaving it on".
None of those are a shift; all of them are *rules about ranges of keys*.

## The model

A coupler is now a named, engageable bundle of **routes**:

    from manual + source key range  →  to manual + key shift
        unison_off   silence the source keys' own division in the range
                     (the note moves instead of doubling)
        repitch      may this route sound pipes the destination hasn't
                     got? (None = follow `[couplers] repitch`)

Everything previously expressible is one full-compass route. GO's
`UnisonOff=Y` couplers — skipped with a warning until now — are a route
with no target; GO's `FirstMIDINoteNumber`/`NumberOfKeys` restriction —
silently ignored until now — becomes the route's range. Bass/Melody
couplers stay skipped (they are stateful picks over held notes, a
different mechanism), and cent-offset routes were considered and
deferred: a copy of a pipe detuned seven cents is the same pipe sounding
twice, which breaks the pipe-speaks-once invariant. That belongs to
per-pipe addressing (M6), not to couplers.

The two headline examples, as sidecar TOML:

    [[couplers.define]]
    name = "Fourths II/I"
    [[couplers.define.route]]
    from = "II"
    to = "I"
    shift = -5
    low = "C3"              # tenor C; a MIDI number works too

    [[couplers.define]]
    name = "16' I"
    [[couplers.define.route]]
    from = "I"
    to = "I"
    shift = -12
    low = "C3"              # the classic doubling above the break
    [[couplers.define.route]]
    from = "I"
    to = "I"
    shift = -12
    high = "B2"             # below it: move, don't double…
    unison_off = true
    repitch = true          # …inventing the pipes the rank hasn't got

Definitions resolve like bindings: manuals by the shared name matcher,
unresolvable definitions reported and ignored, never fatal. Resolved
couplers append to the set's own and appear on the console rail like
any other.

## Repitch escapes the compass — per route, on request

"Couplers never repitch" stands as the default, but it gains a per-route
override, and the override means more than filling holes: a repitching
route also lands *past the destination's compass*. It has to — a 16'
route over an 8' rank's bottom octave wants pitches below any key the
organ has. A route that says `repitch = true` is explicitly asking for
tone the instrument can't otherwise make; bounding it by the compass
would refuse exactly the notes it exists for. Non-repitching couplers
stay compass-bounded as before. DESIGN.md records both halves.

## Coupler changes land on held notes

Engaging a coupler used to affect only the next press; drawing a stop
mid-hold already spoke immediately. That asymmetry is gone: `set_coupler`
re-derives what every held key should sound under the new state and
diffs it against what is sounding — newly demanded pipes start (with the
usual expedite-the-predecessor rule), no-longer-demanded ones release,
and a unison-off coupler audibly moves the held notes. This is what an
electric-action console does, and the diff machinery is also the shape
Bass/Melody couplers will need when they come.

`set_coupler` therefore now returns `(stop handles, voice starts)` like
`set_drawn`, with the clack noise riding along.

## Verified

- `cargo test --workspace` green; `cargo clippy --all-targets` clean
  (including two pre-existing `approx_constant` errors in bank tests,
  fixed in passing).
- New console tests: the fourths-from-tenor-C coupler, the 16'
  bottom-octave transposition (correct repitch ratio, played key
  silent), mid-hold engage/release, pure unison-off moving held notes.
- Sidecar tests: note-name parsing ("C4" = 60), definition resolution
  by manual name, missing-manual definitions reported not fatal.
- Audible check is the user's, on their desktop rig.
