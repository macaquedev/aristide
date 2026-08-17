# 2026-08-17 — MIDI assignment, read manual-first

Yesterday's routing was device-first: a list of the machine's MIDI
inputs, each with a dropdown saying where it plays (`none` / `channels`
/ a manual), plus a separate 16-channel map for the devices that said
`channels`. It worked, and it was the wrong way round. Hauptwerk asks
the question the other way — here are *this organ's* keyboards, tell
each one what to listen to — and that is the question an organist
actually has ("what plays the Récit?"). The channel map was the tell:
an extra table that existed only to make the device-first shape able to
express a console whose manuals speak on separate channels.

## The shape now

One binding is `(manual ← device, channel)`. A manual holds a **list**
of them:

```toml
[[organs."GrandOrgue Demo".manuals."Second Manual"]]
device = "Johannus DIN IN"
channel = 2
```

That covers both real rigs without a second concept: a DIN console is
three bindings on one device with different channels, and a plain USB
keyboard is one binding with no channel at all (any channel). It also
subsumes the channel map, which is gone — from the config file, the
API, the snapshot, and `Console` (`note_on(channel, …)`,
`manual_index`, `set_channel*`, `channel_names`, `default_channel_map`
all deleted; the manual-addressed methods were already there, since the
on-screen keyboard and pinned devices used them).

The list is deliberate, and so is keeping the *target* end open. The
long-term aim is Orgelpark-style routing — two keyboards sharing one
division, a keyboard acting as a coupling manual, a single note made
responsible for something — so nothing in the stored shape may assume
one keyboard per manual. Two manuals may name the same device and
channel; both sound. Each row has room to grow a key range, a
transposition, a velocity rule, without disturbing the others.

Resolution stays where it was: manual names are matched with the
sidecar's fuzzy matcher, and the resolved `(channel, manual)` pairs are
cached on each connected port, so the MIDI callback scans a handful of
pairs and never touches a name. A binding whose device is unplugged is
kept and reported as `connected: false` rather than dropped.

## Auto-detect

Hauptwerk's one genuinely delightful bit of that dialog: press
**Listen**, play a key, and the binding takes the port and channel from
the note itself. `POST /api/midi/learn?manual=&slot=` arms it, the
snapshot reports `learning` so the row can show it, the note that
teaches it is swallowed (a division blurting out mid-assignment reads
as a fault), and the wait gives up after 20 s or when the dialog
closes.

## Kept, changed meaning

The sidecar's `[midi] channels` (manual names in channel order) is no
longer a route. It is read backwards into a per-manual *suggestion*, so
hand-assigning a device to the Récit pre-fills the channel the set says
the Récit speaks on. Nothing sounds until a binding exists.

`midi.toml` gains a new table shape; the old `devices` / `channels`
keys are ignored, so yesterday's assignments are re-made once. One
day's data, and the dialog that re-makes them is the point of the
change.

## Verified

- `cargo test -p aristide-server` green (routing by channel, any-channel
  binding, unassigned silence, learn-and-swallow, per-organ resolution,
  bind/unbind/learn over the API, port names through the query string).
- The dialog rendered headlessly against a stub server: empty manual,
  two-input manual, unplugged device, and the listening row.
- Audible check is the user's, on their desktop rig.
