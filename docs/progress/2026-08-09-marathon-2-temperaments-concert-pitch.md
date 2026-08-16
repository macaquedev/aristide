# 2026-08-09 — marathon 2/N: temperaments, concert pitch, transposer

First slice of the M6 contemporary/tuning layer, entirely control-side
(per-note rate multiplier folded into StartVoice — the RT engine's
"pitch travels as Hz" design paying off). Temperaments: Equal,
Werckmeister III, Kirnberger III, ¼-comma meantone, Pythagorean —
a-referenced precise cent tables cross-checked against Carey Beebe's
reference (hpschd.nu/tech/tun/cents.html; every entry matches their
rounded values). Concert pitch a′ = 300–500 Hz; transposer shifts key
routing (selects different pipes, like the console gadget). Sidecar
`[tuning]` + `/api/tuning` endpoint. Tests: table-vs-CBH, a′ invariance
across temperaments, meantone retune factor, transpose routing/compass.
76 tests green.
