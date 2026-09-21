# Establish the new UI specification

Alex supplied a new UI and flow spec after the UI cleanup and requested that it
be saved and read consistently before work continues. The complete specification
is in `docs/design/ui-flow-spec.md`, with Markdown headings, tables, lists and its
Mermaid flowchart. Its product requirements have not been reduced to a summary.

`CLAUDE.md` and `AGENTS.md` now require reading the full spec at the start of every
task/session and after a context reset. The spec governs new product behaviour,
interaction, visuals and the target instrument model. The deleted UI must not be
restored or used as a template. The old design-rules file is a redirect, and
`DESIGN.md` distinguishes current backend behaviour from the new target, including
separate combination sets, automatic layers and restoring the last instrument.

Implementation is to proceed in small increments. Build and Tuning require the
specified four-option clickable mock process and Alex's choice; other panels
follow their specified conventions. UI increments require screenshots and review
against the control-flow and visual rules, plus touch/mouse and audio-continuity
checks. Unsupported capabilities must be recorded, not simulated as working.

This step saves the design foundation only. No UI, backend change, framework or
component-library selection is included. The next implementation step can start
from the new shell and Play requirements while accounting for backend gaps.

Validation: reviewed the saved spec against all supplied sections, tables, seven
control-flow rules and five AI-workflow steps; checked local documentation links,
authority references and `git diff --check`. Documentation only, so no audio build
or runtime test was needed.
