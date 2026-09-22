# Historical temperament presets

The Tuning study's Temperament select now offers Equal, quarter-comma and
sixth-comma meantone, Pythagorean, Werckmeister III, Kirnberger III,
Rameau 1726 (tempérament ordinaire), Vallotti and Young II, plus Custom. Presets
are cent deviations from equal temperament, tabulated on C in
`desktop/src/design/temperaments.ts`, and rotate with the Root note.
Regular temperaments are computed from their tempered fifth (wolf G♯–E♭).
Choosing Custom keeps the current preset's deviations as the starting point
for dragging.

Rameau's 1726 tempérament ordinaire follows Claudi Meneghin's reading
(meantone.org, 2026): quarter-comma fifths B♭–B, pure B–G♯, the remainder
split equally over G♯–E♭–B♭. Klaus Lang's reading (⅙-comma C♯–G♯, used by
Ableton's tuning library) differs; add it as a variant if needed.

The study is still silent and makes no engine requests. Backend gap:
`aristide-server/src/tuning.rs` has Original, Equal, Werckmeister3,
Kirnberger3, Meantone4 and Pythagorean, with no root rotation. Sixth-comma
meantone, Rameau, Vallotti, Young II, a temperament root, and "As recorded"
in this select are required before connection.

Validation: browser test selects meantone, checks E at −13.7 ¢, rotates to
root D (E at −6.8 ¢) and converts to Custom; a unit test checks Rameau's
fifth sizes; screenshots reviewed.
