# Durable run prototype

This branch is a design reference for NAK-439. Do not merge it or enable it in production.

`server::runs::RunService` owns run records, capabilities, persistence, and restart reconciliation.
It accepts runtime facts through `RunHost`. It does not depend on the TUI.
The current combined event loop holds the service. Its `App` adapter resolves live identities and terminal access.
The standalone service test exercises submission, observation, persistence, and restart without an `App`.

A new submission requires an idle agent with the exact requested binding.
Working, blocked, and unknown agents reject new submissions before prompt delivery.
A matching idempotency key still returns its existing record when the agent becomes busy.

Cancellation returns the exact bytes when it prevents the delayed Enter.
If the interrupt write fails, the service persists the previous active state and reschedules those Enter bytes.
A matching submission retry returns the existing run while delivery remains pending.
The binding stays reserved, so another run cannot append text to the staged prompt.
If Enter already arrived, an interrupt failure restores the previous state without scheduling another Enter.
Both paths preserve the consumed capability sequence. Failed compensation disables run operations and does not reschedule Enter.

New capabilities expose `issued_at_unix_ms` and `expires_at_unix_ms`.
Authorization and expiry use these exact millisecond timestamps.
The legacy `issued_at_unix` field rounds down. The legacy `expires_at_unix` field rounds up.
Existing records without millisecond fields retain their recorded whole-second expiry.
Registry validation rejects incomplete or inconsistent timestamp pairs.

Windows compile checks do not establish Windows runtime behavior.
