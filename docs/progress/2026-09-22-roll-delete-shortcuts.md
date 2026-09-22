# Piano-roll deletion shortcuts

Right-click deletes the event under the pointer in all four Build studies.
Delete prioritizes the currently hovered event, falling back to the selected
one. Both call the inspector's shared undo-backed removal action. Removing a
held continuation deletes its single event, so both timeline halves disappear
and Undo restores both.

Empty-space right-click leaves the selection untouched. Keyboard deletion is
inactive while Build is hidden, while editing input/contenteditable fields, or
while another sheet/popover owns input. Key repeats and Ctrl/Command/Alt-modified Delete presses
do not cascade through the remaining notes. The event inspector still permits
Delete when its focus is outside a field. Touch retains the inspector Delete
button, using the same action.

Validation: production frontend build; piano-roll browser regression suite with
all four layout right-click cases, hovered/selected keyboard targets, typing and
hidden-panel protection, continuation deletion and undo. Screenshot reviewed
following deletion/undo; the only visible UI change is the concise shortcut hint.
The existing touch drawing/pan check remains part of the regression suite.
These studies stay silent and do not issue engine requests or persist changes.
