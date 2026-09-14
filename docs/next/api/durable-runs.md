# Durable run prototype

This implementation is a design reference for NAK-439. The owner authorized its merge into the NakamaDevs fork.
It is not Kantoku's production integration. Do not enable it in production without separate authorization and runtime validation.

`server::runs::RunService` owns run records, capabilities, persistence, and restart reconciliation.
It accepts runtime facts through `RunHost`. It does not depend on the TUI.
The current combined event loop holds the service. Its `App` adapter resolves live identities and terminal access.
The standalone service test exercises submission, observation, persistence, and restart without an `App`.

A new submission requires an idle agent with the exact requested binding.
Working, blocked, and unknown agents reject new submissions before prompt delivery.
A matching idempotency key still returns its existing record when the agent becomes busy.
New submissions also reject an earlier pending Enter on the target pane.
Guarded prompt staging counts pending durable-run inputs, so a run's delayed Enter cannot submit newly staged text.

Cancellation returns the exact bytes when it prevents the delayed Enter.
If the interrupt write fails, the service persists the previous active state and reschedules those Enter bytes.
A matching submission retry returns the existing run while delivery remains pending.
The binding stays reserved, so another run cannot append text to the staged prompt.
If Enter already arrived, an interrupt failure restores the previous state without scheduling another Enter.
Both paths preserve the consumed capability sequence. Failed compensation disables run operations and does not reschedule Enter.
If saving the running state fails after prompt delivery, the service disables run operations without sending Enter.
The queued record keeps its binding reserved. An operator must inspect the staged prompt before recovery.

New capabilities expose `issued_at_unix_ms` and `expires_at_unix_ms`.
Authorization and expiry use these exact millisecond timestamps.
The legacy `issued_at_unix` field rounds down. The legacy `expires_at_unix` field rounds up.
Existing records without millisecond fields retain their recorded whole-second expiry.
Registry validation rejects incomplete or inconsistent timestamp pairs.

Windows compile checks do not establish Windows runtime behavior.
