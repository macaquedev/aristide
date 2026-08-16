# 2026-08-09 — stop noises done right (user report)

Noise "stops" were showing as drawable stops. Investigating the actual
noise wavs revealed the GO-set convention: each is structured like a
pipe — pull-thump attack → **near-silent sustain loop** → push-in thump
as the release tail (Motor.wav likewise: blower start → running drone →
wind-down). So the right mechanism was already built: the engine's note
lifecycle. Drawing a stop note-ons its noise voice (thump, then holds
the silent loop); retiring note-offs it (splices to the push-in thump).
Same for coupler clacks and the tremulant. Level matcher gained a guard:
a near-silent loop means the tail is *meant* to be louder — play it as
recorded (the matcher would have crushed thumps to 5 %). Noise voices
draw no wind and skip the brightness filter. New `KillVoice` command
silences open noise voices when noises are disabled mid-flight.
Classification by name ("noise"), fuzzy-mapped to their control
(normalized-prefix/token scoring handles "Fl Harm 8' stop noise" →
"Flute Harm. 8'"). Hidden from all UIs; global enable + volume in
sidecar `[noises]`, `/api/noises`, web console, native GUI. E2E test on
the demo set covers the whole lifecycle. 83 tests green.
