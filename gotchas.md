# Gotchas

Lessons learned during development. Read at session start.

## Protocol Implementation

### 2026-07-15 — Implement known protocol edge cases BEFORE live testing

**Mistake:** The `{type:"event", event:{...}}` wrapper form was documented in the wire protocol research (lib-5) but marked `todo!()` in code "for later". The very first live test against real Minecraft Bedrock failed with `unknown variant 'event'` because MC wraps most incoming messages this way (Stopped/Print/Stat/etc. — only ProtocolEvent and debuggee-response come direct).

**Rule:** When upstream protocols have documented edge cases (wrappers, framing quirks, version-specific shapes, alternate encodings), implement them BEFORE live integration testing. Do not mark them `todo!()` and hope they don't trigger — they will. The research already told us this case exists; treating it as optional was the mistake.

**Apply to:** Any protocol port work. If the source analysis mentions a code path that handles a specific shape, that shape exists in production traffic.

### 2026-07-15 — Test helpers that read from sockets must share codec+buffer state

**Mistake:** The `do_handshake` test helper created a fresh `MessageCodec` + `BytesMut` locally, read the ProtocolResponse, then dropped both on return. When the client immediately sent another frame (Request) right after ProtocolResponse, TCP coalesced both frames into one segment. The helper's `recv_frame` consumed both into `buf`, decoded the first, and the second frame's bytes were still sitting in `buf` when it was dropped. The next `recv_frame` (with a fresh buffer) then blocked forever waiting for bytes that were already consumed. Deadlock → test timeout.

**Rule:** Test helpers that do framed socket reads must take `codec: &mut MessageCodec` and `buf: &mut BytesMut` as parameters (or otherwise share state across all reads in the test). Never drop the buffer between reads — TCP coalesces adjacent frames on localhost, and the codec's state machine can leave residual bytes in the buffer that the next read depends on.

**Apply to:** Any test against a real (or mock) socket using a stateful codec. If the helper "simplifies" by hiding codec/buffer creation inside itself, that's the bug pattern.

### 2026-07-15 — Some MC debugger commands are fire-and-forget (no debuggee-response)

**Mistake:** Assumed every `DebuggerEvent::Request` would get a matching `DebuggeeResponse`. Implemented pause/step/continue using the request/response pattern. Live testing against real MC Bedrock: clicking Pause hung the UI forever on a spinner because MC never sent a debuggee-response — it just paused and emitted StoppedEvent.

**Rule:** Control-flow commands (pause, continue, step_next, step_in, step_out) are fire-and-forget on MC's side. The "response" is the asynchronous StoppedEvent (or for continue, simply no event until the next break). Only commands that need to return data — evaluate, stackTrace, scopes, variables, threads — actually wait for a debuggee-response. When porting a debugger protocol, classify each command as "needs result" vs "control only" before wiring up the request/response infrastructure.

**Apply to:** Any request/response-style protocol where some commands are semantically ack-only. Don't assume uniform response behavior across all command types.

### 2026-07-15 — Verify the exact response discriminator for each request type

**Mistake:** Read the protocol-events.ts TypeScript definitions and assumed the `DebuggeeResponse` ("debuggee-response") was the response to all `Request` ("request") messages. Lived with mysterious hangs for two iterations (pause first, then evaluate) before investigating the upstream session.ts dispatch code.

**Reality:** The MC protocol has TWO completely separate response systems with different discriminators:
1. **Basic Request → Response:** `{type:"response", request_seq, body}` — for all DAP commands (evaluate, stackTrace, scopes, variables, etc.)
2. **DebuggerRequest → DebuggeeResponse:** `{type:"debuggee-response", request_seq, args}` — only for the v7+ webview UI's typed request system

Our code was listening for `"debuggee-response"` (System 2) when awaiting replies to basic Request messages (System 1). MC was correctly sending `"response"` which we treated as Unknown, causing request() to loop forever.

**Rule:** When a protocol has multiple request types, each may have its own response discriminator. Don't trust type-name symmetry (request vs response); verify by reading the actual dispatch/routing code in the reference implementation. The TypeScript interface names were misleading — `DebuggeeResponse` sounds like "the response from the debuggee" but it's actually "the response to DebuggerRequest specifically."

**Apply to:** Any port of a multi-purpose protocol. The "sounds-right" naming is a trap; the dispatch code is the truth.
