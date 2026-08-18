# 2026-08-18 — extension prefers a rank's real pipes over repitching

A correction to the repitching rule (2026-08-17): the *pipe that
stands in* for a widened key was being chosen from the wrong set.

## The case

Unit organs draw one rank at several pitches: a Bourdon rank speaking
at 16' on the Pedal is the same pipes speaking at 8' on the Swell.
In the model that is one `Rank` with two stops holding different
`RankRange` windows into it — the 16' the bottom 32 pipes, the 8'
starting twelve pipes up and running to the top.

Widen the pedalboard past the set's compass and the extended keys
want pipes past the 16' window. Those pipes *exist* — they are the
8' stop's treble — but `pipe_for` clamped its search to the stop's
own window, so the window's edge pipe was stretched up to imitate a
neighbour that was sitting right there, recorded.

## The rule, sharpened

Coverage and stand-in selection are different questions with
different owners:

- **Which keys a stop answers** is the range's business, settled in
  `range_covers` before any pipe is chosen. Nothing there moved: a
  short stop is still a musical decision and is not extended, keys
  outside the player's compass are still silent, couplers still may
  not repitch unless opted in.
- **Which pipe answers** is the rank's business. Once a key is
  covered, the search for the pipe spans the whole rank, not the
  stop's window. A real pipe at the wanted ladder position beats any
  repitched neighbour; repitching begins only past the rank's true
  ends, or across a hole where a sample failed to load — and a hole
  now fills from the genuinely nearest pipe, wherever the window
  boundary falls.

This was already what `pipe_for`'s own comment promised ("the
nearest pipe the rank does have"); the implementation now keeps the
promise. Downward works symmetrically: an 8' drawn mid-rank from a
16' unit rank finds the real bottom pipes when the keyboard widens
down.

## Out of scope

Sets that model an extension stop as its own short rank of per-pipe
`REF:` borrows still repitch past that rank's end. The model cannot
know what a hypothetical pipe past the end would have borrowed;
extrapolating a contiguous borrow run is possible but would be
inventing structure, so it waits for a set that actually needs it.

## Verified

- `extended_keys_use_the_real_pipes_of_a_shared_rank`: a 73-pipe
  unit rank under a Pedal 16' and Swell 8'; extended pedal keys
  speak real pipes at ratio 1, extension below the 8' finds the 16'
  bottom, and past the rank's real end the old stretch rule holds.
- Full `aristide-server` suite green; every 2026-08-17 rule keeps
  its test.
- The code landed inside `f1c933b` (feat(compose)) — a concurrent
  session's `git add -A` swept the working tree; this note is the
  attribution.
