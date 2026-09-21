# Connection errors and preview access

The browser dev server was running without a server on port 9669. Its proxy
reported connection refused, but the UI called every engine error an unavailable
audio device and told the player to reopen the app. Successful later polls also
left the old error visible, and dismissing it reopened it on the next failed poll.

Connection failures now have their own message. Browser users can enter either
editor study directly from that message or the waiting screen without audio.
Successful state requests clear connection/runtime errors. Dismissal lasts for
the current outage; a new outage after recovery is reported again. Rejected
control actions have a separate message and are not described as device failures.

Validation: Bun production build and 21 browser tests pass, including offline
preview access, automatic recovery and dismissal across repeated polls. Reviewed
the error screenshot after its opening transition. Launched the existing native
release: its audio runtime started and restored `My AVO Solignac extend`; reviewed
the native Play screenshot. This confirms native startup, not physical speaker
output or listening quality. No device settings or real-time engine code changed.
