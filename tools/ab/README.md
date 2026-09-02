# GrandOrgue / Aristide A/B rig (M4)

Renders the same ~40s passage on the same sample set
(`testsets/grandorgue-demo/demo.organ`, GrandOrgue's bundled Friesach
demo) through both engines, so the two recordings can be compared with
`analyze.py`. See `docs/progress/2026-09-02-ab-grandorgue.md` for the
run this rig produced and its results.

## The passage

`passage.py` is the single source of truth — a note-event list plus the
registration, shared by both halves so the two engines play the
identical performance:

- **Registration** (drawn once, before any note sounds): First Manual
  Montre 8' + Prestant 4' + Plein jeu III; Second Manual Trompette 8'
  alone; Pedal Contre- basse 16' + Sous- basse 16' (verbatim ODF names,
  hyphen-space and all).
- **Section A** (0.0s): a plein jeu chord progression on First Manual —
  chorus + mixture.
- **Section B** (13.5s): Second Manual Trompette 8' alone, an ascending
  then descending scale, staccato then legato — the release-splice
  probe (staccato forces a full release-and-reattack every note; legato
  forces fast voice overlap).
- **Section C** (24.0s): a 12-second held chord, First Manual plein jeu
  + Pedal 16' pair together — the wind-sag probe (a big simultaneous
  wind draw, then a released chord).

`python3 make_midi.py OUT.mid` writes it as a Standard MIDI file (channel
= GO's `MIDIInputNumber` for this set: Pedal=1, First Manual=2, Second
Manual=3 — verified empirically, see below) for GrandOrgue's MIDI Player.
`python3 drive_aristide.py [http://host:port]` plays it live against a
running `aristide-server` over the HTTP console API, sleeping to each
event's real time.

## Recording GrandOrgue

GrandOrgue 3.13.1 (`apt install grandorgue`) has no batch/record CLI —
everything below is GUI, driven headlessly with Xvfb + xdotool on this
box, or normally on a desktop with real audio.

**Audio device**: GO needs *paced* audio. An ALSA `null` PCM
(`pcm.!default { type null }` in `~/.asoundrc`) accepts writes with no
timing at all — GO's callback then free-runs at whatever speed the CPU
allows, and the Audio Recorder captures that: a 64-second run against
`null` produced a **2.7 GB / ~128-minute** WAV (see the finding below).
Use JACK's `dummy` backend instead, which paces itself with a real
sleep between periods:

```
sudo apt install jackd2      # jackd2's postinst asks a debconf question
                              # about realtime priority; answer No if it
                              # prompts, or preseed:
                              #   echo "jackd2 jackd2/tweak_rt_limits.conf boolean false" | sudo debconf-set-selections
jackd -d dummy -r 44100 -p 1024 &
```

Then in GO: **Audio/Midi → Organ settings... → Audio tab → Mapping
output → select "Device: PortAudio: ALSA: default" → Change → "Jack:
Native Output" → OK → OK**. Confirm via Audio/Midi → Sound Output State
("Jack: Native Output: NN ms").

**Registration**: click Montre 8', Prestant 4', Plein jeu III (First
Manual), Trompette 8' (Second Manual), Contre- basse 16', Sous- basse
16' (Pedal) — the six drawstops `passage.py`'s `REGISTRATION` list
names, at whatever their console positions are for this demo set.

**Play + record**: Panel → Recorder / Player. Audio Recorder REC (plain
REC, not "REC File" — REC auto-names the file with a timestamp under
`~/GrandOrgue/Audio recordings/`, no save dialog to script around), then
Audio/Midi → Load MIDI (Ctrl+P, or the location-bar `Ctrl+L` trick under
a GTK file chooser to type a path directly) to load `passage.mid`, then
MIDI Player PLAY. Wait for the passage to finish (the Recorder panel's
MIDI Player timer / the PLAY button unlit), then Audio Recorder STOP.

**Headless mechanics** (Xvfb, no window manager needed once you avoid
GO's own Program Settings dialog freezing — see the finding below):
`Xvfb :N -screen 0 1280x900x24 -auth <file>`, `DISPLAY=:N XAUTHORITY=<file>
GrandOrgue &`, then `xdotool` mouse clicks by absolute screen coordinate
(`xdotool mousemove X Y click 1`) and `import -window root out.png`
(ImageMagick) to see what you clicked. GO's console panel and its main
frame (menu bar) start stacked at (0,0); `xdotool windowmove <panel-id>
0 200` before the first screenshot so the menu bar isn't hidden behind
it (an unmapped/obscured window's backing store can come back blank from
`import`). A GTK file chooser's location bar opens with `Ctrl+L`, not by
typing a path directly into the list.

## Recording Aristide

```
aristide-server --record OUT.wav testsets/grandorgue-demo/demo.organ &
sleep 3   # let it finish loading before driving it
python3 tools/ab/drive_aristide.py http://127.0.0.1:9669
kill -INT %1   # SIGINT, not SIGTERM -- see the finding below
```

`--record` taps the engine's output from the moment the audio stream
opens, so start it before drawing stops or playing notes.

**Audio device pacing** (this box has no audio hardware): the same
`null`-PCM problem hits `aristide-server` even harder — a 5-second test
run produced a **2.6 GB / ~245-minute** file (cpal's ALSA backend
free-runs against `null` just like GO's PortAudio backend does).
`aristide-server` doesn't have a JACK backend built in (cpal's `jack`
Cargo feature isn't enabled), so route its plain ALSA output through
JACK via the ALSA↔JACK bridge plugin instead of switching engines:

```
sudo apt install jackd2 libasound2-plugins
jackd -d dummy -r 44100 -p 1024 &
cat > ~/.asoundrc <<'EOF'
pcm.!default {
    type jack
    playback_ports { 0 system:playback_1  1 system:playback_2 }
}
ctl.!default { type jack }
EOF
```

This measured ~10-20% *faster* than real time over a 10s probe (a
usleep-paced dummy driver without realtime scheduling privileges drifts
a bit fast, cumulatively) — close enough that `analyze.py`'s metrics
(computed per-file, not diffed sample-for-sample against GO's take) are
unaffected, but note it if you re-run this and get a duration that
doesn't match the passage's nominal 40.5s (registration-draw round trip
+ 36.0s of events + 2s tail) within ~15%.

**Clean shutdown matters**: `aristide-server` finalizes the WAV header
(and flushes the recorder thread) on `SIGINT` (`main.rs`'s handler); a
plain `kill` sends `SIGTERM`, which does neither — the file's `data`
chunk size is left at 0 even though the audio bytes are on disk.
`analyze.py`'s reader tolerates this (falls back to EOF when the
declared chunk size is 0 or exceeds the file), but `kill -INT` is one
line and avoids relying on that fallback.

**ALSA loopback (`snd-aloop`) alternative**: tried first, before the
ALSA↔JACK bridge. It paces correctly too (a real ring-buffer device, not
a discard sink) but occasionally threw `alsa::poll() returned POLLERR`
mid-recording, killing the stream a few seconds in with no recovery —
flaky enough on this box that the JACK-bridge route above is the one to
use. `sudo modprobe snd-aloop` if you want to try it again (needs the
`linux-modules-extra-<uname -r>` package on Ubuntu; not loaded by
default).

## Analysis

```
uv run tools/ab/analyze.py go-take.wav aristide-take.wav
```

Needs `numpy`/`scipy`; the `uv run` inline-script header pulls them into
an ephemeral env, no project venv needed. Loudness-matches each file to
a common broadband RMS (over non-silent 50 ms frames only, so lead-in/
tail silence doesn't skew the reference level) before comparing, then
reports peak/RMS, a spectral-centroid distribution (STFT, 2048/512),
a release-splice discontinuity metric (second-difference spikes more
than 10x the local second-difference RMS, per channel — a curvature
measure so a fast-but-smooth attack doesn't false-positive the way a
plain first-difference/max-step check would), and the noise floor (the
quietest 200ms window's RMS). Works on any two WAVs of a shared passage,
not just this one — point it at a desktop re-recording to sanity-check
the headless numbers.

## Files

- `passage.py` — the note-event list + registration (edit this to
  change the passage; both `make_midi.py` and `drive_aristide.py` read
  it).
- `make_midi.py` — passage.py → Standard MIDI file for GO.
- `drive_aristide.py` — passage.py → live HTTP calls against
  `aristide-server`.
- `analyze.py` — the comparison (`uv run`, needs no separate install).
