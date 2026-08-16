# 2026-08-09 — sidecar v0 + GrandOrgue critique

- First real sidecar: `<set>.aristide.toml` with `[registration] default`
  and `[midi] channels`. Pattern matching exact-first-then-shortest so
  "plein jeu" can't draw its drawstop noise. New generic channel default:
  keyboards first, pedal last (channel 0 = the Great). Demo sidecar sets
  a plein jeu (Bourdon 16', Montre 8', Prestant 4', Plein jeu III).
  53 tests green incl. a sidecar-driven end-to-end.
- `docs/go-critique.md`: cited critique of GO's renderer from a source
  read (`reference/grandorgue/`, gitignored). Key findings: condvar
  waits + mutexes inside the audio callback; 8-tap Lanczos resampler
  with rate frozen at voice start; 2-sample amplitude/slope release
  alignment; no wind model; 16-bit amplitude-only synth trem. Plus a
  what-they-get-right list (load cache, ODF leniency) to steal from.
