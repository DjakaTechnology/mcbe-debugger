# Gotchas

Lessons learned during development. Read at session start.

## Protocol Implementation

### 2026-07-15 — Implement known protocol edge cases BEFORE live testing

**Mistake:** The `{type:"event", event:{...}}` wrapper form was documented in the wire protocol research (lib-5) but marked `todo!()` in code "for later". The very first live test against real Minecraft Bedrock failed with `unknown variant 'event'` because MC wraps most incoming messages this way (Stopped/Print/Stat/etc. — only ProtocolEvent and debuggee-response come direct).

**Rule:** When upstream protocols have documented edge cases (wrappers, framing quirks, version-specific shapes, alternate encodings), implement them BEFORE live integration testing. Do not mark them `todo!()` and hope they don't trigger — they will. The research already told us this case exists; treating it as optional was the mistake.

**Apply to:** Any protocol port work. If the source analysis mentions a code path that handles a specific shape, that shape exists in production traffic.
