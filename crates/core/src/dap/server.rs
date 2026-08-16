//! Blocking DAP server loop, mirroring `lsp::server` for DAP's own
//! message shape and lifecycle: a background reader thread owns stdin
//! decode (via the shared `framing` codec) onto an `mpsc` channel; the
//! main loop owns dispatch, stdout, and every `seq` number the session
//! ever emits. Unlike LSP's synchronous request/response dance, a DAP
//! session can be *running* — advancing the debuggee on its own between
//! client requests — so the main loop alternates a non-blocking drain of
//! the channel with a bounded `tick()` call while running, and falls
//! back to a blocking wait once there is nothing left to advance.
//!
//! Per-command semantics (launch, breakpoints, stepping, …) live entirely
//! in the [`DebugAdapter`] implementor; this module knows only the DAP
//! envelope, the `initialize`/`disconnect` lifecycle bookends every DAP
//! session shares, and how to turn an [`AdapterEvent`] into a protocol
//! event. It carries zero knowledge of any particular machine
//! architecture.

use std::sync::mpsc;
use std::thread;

use serde_json::Value;

use super::protocol::{MessageKind, ProtocolMessage};
use crate::framing;

/// One thing an adapter wants the client to know about, pushed from
/// [`DebugAdapter::handle`] or [`DebugAdapter::tick`] and translated by
/// the server loop into the matching DAP event.
#[derive(Debug, Clone, PartialEq)]
pub enum AdapterEvent {
    /// The debuggee paused. `reason` is a DAP `stopped` reason string
    /// (`"breakpoint"`, `"step"`, `"pause"`, `"entry"`, `"exception"`,
    /// …); `description` is the optional human-readable elaboration
    /// (e.g. naming the source-authored `debugger` statement a `Brk`
    /// pause hit, or a trap's kind).
    Stopped {
        reason: &'static str,
        description: Option<String>,
    },
    /// A line of adapter-produced output. `category` is a DAP output
    /// category (`"stderr"`, `"console"`, …).
    Output {
        category: &'static str,
        output: String,
    },
    /// The debuggee's run has ended; no further stepping is possible.
    Terminated,
    /// The debuggee's process-equivalent exit code is known.
    Exited { code: i32 },
}

/// Whether the adapter's session is presently advancing on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// Paused: waiting on the next client request.
    Stopped,
    /// Advancing: the loop alternates draining pending requests with
    /// `tick()` calls until this changes.
    Running,
    /// The session has concluded (naturally, or via `disconnect`); no
    /// further run is possible, but requests may still be answered.
    Done,
}

/// The per-arch (or, for this crate's own tests, fake) half of a DAP
/// session. The server loop owns framing, `seq` bookkeeping, and the
/// `initialize`/`disconnect` lifecycle guard; everything about what a
/// command *means* — launch, breakpoints, stack/scopes/variables,
/// stepping — lives here.
pub trait DebugAdapter {
    /// Handle one request; push events via `out`. Returns the response
    /// body or a DAP error message. Called by the loop for every request
    /// that passes the lifecycle guard (`initialize` first; nothing once
    /// disconnected).
    fn handle(
        &mut self,
        command: &str,
        arguments: &Value,
        out: &mut Vec<AdapterEvent>,
    ) -> Result<Value, String>;

    /// Called by the loop when no request is pending and `run_state()` is
    /// `Running`: advance one bounded slice (the adapter calls
    /// `run_steps_tapes(BUDGET)` internally), pushing events on pause or
    /// completion.
    fn tick(&mut self, out: &mut Vec<AdapterEvent>) -> RunState;

    /// The adapter's current run state, consulted by the loop before
    /// every poll to decide whether to drain non-blockingly (`Running`)
    /// or wait for the next request (`Stopped`/`Done`).
    fn run_state(&self) -> RunState;
}

/// The DAP-conformant failure text for a command outside an adapter's
/// supported set. Shared here so wording stays identical across every
/// adapter this crate's own tests or a later arch crate ships, rather
/// than each implementor inventing its own phrasing.
pub fn unsupported_command(command: &str) -> String {
    format!("unrecognized request: '{command}'")
}

/// Blocking DAP server loop. Spawns a reader thread that owns `reader`,
/// decoding `Content-Length`-framed messages via [`crate::framing`] onto
/// an `mpsc` channel; the calling thread owns `writer`, every `seq`
/// number the session emits, and dispatch against `adapter`.
///
/// While `adapter.run_state()` is `Running`, the loop alternates a
/// non-blocking channel drain (`try_recv`) with a bounded `adapter.tick()`
/// call, so a queued client request (`pause`, `disconnect`, …) is noticed
/// promptly instead of waiting for the run to finish on its own. Once the
/// adapter is not running, the loop blocks on the channel instead of
/// spinning.
///
/// Returns 0 after a clean `disconnect` (the response was sent and the
/// transport later closed); 1 if the reader thread's channel disconnects
/// (client EOF or a malformed frame — see `crate::framing`) before a
/// `disconnect` was ever answered. Every request received after
/// `disconnect` has been answered is itself answered with a rejection —
/// the session stays alive just long enough to tell a straggling request
/// no, then winds down once the transport ends.
///
/// The reader thread is deliberately never joined here: a thread blocked
/// in a stdin read cannot be interrupted portably, and in the real
/// `pmt dap`/`tmt dap` deployment the client closes the pipe (or kills
/// the process) shortly after receiving the `disconnect` response, which
/// is what lets the OS reap the detached thread. Callers driving `run`
/// from finite in-memory input (tests) see the reader hit clean EOF and
/// exit on its own; nothing leaks.
pub fn run(
    reader: impl std::io::Read + Send + 'static,
    writer: &mut dyn std::io::Write,
    adapter: &mut dyn DebugAdapter,
) -> i32 {
    let (tx, rx) = mpsc::channel::<String>();

    // The reader thread's only job: decode frames and forward their raw
    // payload. A clean EOF or a malformed frame both end this thread —
    // the malformed-frame case mirrors the LSP transport's precedent of
    // ending the session rather than trying to resynchronize on a
    // corrupt stream. Dropping `tx` on return is what lets the main
    // loop's `recv`/`try_recv` observe the reader going away.
    thread::spawn(move || {
        let mut buffered = std::io::BufReader::new(reader);
        loop {
            match framing::read_message(&mut buffered) {
                Ok(Some(payload)) => {
                    if tx.send(payload).is_err() {
                        return;
                    }
                }
                Ok(None) | Err(_) => return,
            }
        }
    });

    let mut state = ServerState::new();

    loop {
        let next = if adapter.run_state() == RunState::Running {
            // While running, a disconnected channel is treated the same
            // as "nothing pending right now" (`Empty`), not as a reason
            // to stop: the debuggee may still have useful ticking left
            // to do even after the client's input has ended. Only the
            // blocking `recv` below — reached once the adapter is no
            // longer running — treats a disconnected channel as the
            // session actually ending.
            rx.try_recv().ok()
        } else {
            match rx.recv() {
                Ok(payload) => Some(payload),
                Err(_) => break,
            }
        };

        match next {
            Some(payload) => dispatch(&mut state, writer, adapter, &payload),
            None => {
                let mut events = Vec::new();
                adapter.tick(&mut events);
                flush_events(&mut state, writer, events);
            }
        }
    }

    if state.disconnected { 0 } else { 1 }
}

/// Session state the loop threads through every dispatched message:
/// whether `initialize` has been answered, whether `disconnect` has, and
/// the one running `seq` counter shared by every response and event this
/// side of the session ever emits.
struct ServerState {
    initialized: bool,
    disconnected: bool,
    next_seq: u64,
}

impl ServerState {
    fn new() -> Self {
        ServerState {
            initialized: false,
            disconnected: false,
            next_seq: 1,
        }
    }
}

/// Decodes one raw payload and routes it. Payloads that fail to parse as
/// a [`ProtocolMessage`] request, or that decode as a response/event
/// (DAP v1 has no reverse requests, so the client never sends either),
/// are dropped silently — there is no `request_seq` to answer against.
fn dispatch(
    state: &mut ServerState,
    writer: &mut dyn std::io::Write,
    adapter: &mut dyn DebugAdapter,
    payload: &str,
) {
    let Ok(message) = serde_json::from_str::<ProtocolMessage>(payload) else {
        return;
    };
    let MessageKind::Request { command, arguments } = message.kind else {
        return;
    };
    let request_seq = message.seq;

    // Lifecycle guard: once disconnected, everything is rejected — the
    // session stays alive just long enough to say no to a straggler,
    // never to do more work. Otherwise, everything but `initialize`
    // itself requires `initialize` to have already succeeded.
    if state.disconnected {
        respond(
            state,
            writer,
            request_seq,
            &command,
            Err("session has already disconnected".to_string()),
        );
        return;
    }
    if !state.initialized && command != "initialize" {
        respond(
            state,
            writer,
            request_seq,
            &command,
            Err("adapter has not been initialized".to_string()),
        );
        return;
    }

    let mut events = Vec::new();
    let result = adapter.handle(&command, &arguments, &mut events);
    let succeeded = result.is_ok();
    respond(state, writer, request_seq, &command, result);
    flush_events(state, writer, events);

    if succeeded {
        match command.as_str() {
            "initialize" => state.initialized = true,
            "disconnect" => state.disconnected = true,
            _ => {}
        }
    }
}

/// Writes queued [`AdapterEvent`]s as protocol events, in push order,
/// each numbered with the next `seq`.
fn flush_events(
    state: &mut ServerState,
    writer: &mut dyn std::io::Write,
    events: Vec<AdapterEvent>,
) {
    for event in events {
        let (name, body) = to_protocol_event(event);
        send_event(state, writer, name, body);
    }
}

/// Translates one [`AdapterEvent`] to its DAP event name and body,
/// carrying exactly the fields the variant names — no invented fields
/// (e.g. no assumed `threadId`) belong at this layer.
fn to_protocol_event(event: AdapterEvent) -> (&'static str, Value) {
    match event {
        AdapterEvent::Stopped {
            reason,
            description,
        } => {
            let mut body = serde_json::json!({ "reason": reason });
            if let Some(description) = description {
                body["description"] = Value::String(description);
            }
            ("stopped", body)
        }
        AdapterEvent::Output { category, output } => (
            "output",
            serde_json::json!({ "category": category, "output": output }),
        ),
        AdapterEvent::Terminated => ("terminated", Value::Null),
        AdapterEvent::Exited { code } => ("exited", serde_json::json!({ "exitCode": code })),
    }
}

fn respond(
    state: &mut ServerState,
    writer: &mut dyn std::io::Write,
    request_seq: u64,
    command: &str,
    result: Result<Value, String>,
) {
    let seq = state.next_seq;
    state.next_seq += 1;
    write_protocol_message(
        writer,
        &ProtocolMessage::response_to(request_seq, seq, command, result),
    );
}

fn send_event(state: &mut ServerState, writer: &mut dyn std::io::Write, name: &str, body: Value) {
    let seq = state.next_seq;
    state.next_seq += 1;
    write_protocol_message(writer, &ProtocolMessage::event(seq, name, body));
}

/// Serializes and frames one message, best-effort: a write failure (a
/// closed pipe, most likely) has nowhere to go — the loop's next read
/// will observe the transport is gone and end the session.
fn write_protocol_message(writer: &mut dyn std::io::Write, message: &ProtocolMessage) {
    if let Ok(payload) = serde_json::to_string(message) {
        let _ = framing::write_message(writer, &payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    // ---- shared test plumbing -------------------------------------

    fn request(seq: u64, command: &str, arguments: Value) -> Value {
        json!({"seq": seq, "type": "request", "command": command, "arguments": arguments})
    }

    fn framed_bytes(messages: &[Value]) -> Vec<u8> {
        let mut buf = Vec::new();
        for msg in messages {
            framing::write_message(&mut buf, &msg.to_string())
                .expect("write_message into a Vec cannot fail");
        }
        buf
    }

    fn decode_output_frames(buf: &[u8]) -> Vec<Value> {
        let mut reader = buf;
        let mut outputs = Vec::new();
        while let Some(payload) =
            framing::read_message(&mut reader).expect("recorded output must be correctly framed")
        {
            outputs.push(
                serde_json::from_str(&payload).expect("recorded output payload must be valid json"),
            );
        }
        outputs
    }

    fn run_session(messages: &[Value], adapter: &mut dyn DebugAdapter) -> (Vec<Value>, i32) {
        let input = std::io::Cursor::new(framed_bytes(messages));
        let mut output = Vec::new();
        let exit_code = run(input, &mut output, adapter);
        (decode_output_frames(&output), exit_code)
    }

    fn is_success(response: &Value) -> bool {
        response["success"]
            .as_bool()
            .expect("response must carry a success flag")
    }

    // ---- FakeAdapter: initialize/disconnect + everything else fails -

    /// A minimal adapter for the tests that only care about lifecycle,
    /// seq bookkeeping, and unknown-command handling — it never runs.
    struct FakeAdapter;

    impl DebugAdapter for FakeAdapter {
        fn handle(
            &mut self,
            command: &str,
            _arguments: &Value,
            _out: &mut Vec<AdapterEvent>,
        ) -> Result<Value, String> {
            match command {
                "initialize" => Ok(json!({"supportsConfigurationDoneRequest": true})),
                "disconnect" => Ok(Value::Null),
                _ => Err(unsupported_command(command)),
            }
        }

        fn tick(&mut self, _out: &mut Vec<AdapterEvent>) -> RunState {
            RunState::Stopped
        }

        fn run_state(&self) -> RunState {
            RunState::Stopped
        }
    }

    #[test]
    fn rejects_requests_before_initialize_then_completes_a_clean_session() {
        let mut adapter = FakeAdapter;
        let (outputs, exit_code) = run_session(
            &[
                request(1, "pause", Value::Null),
                request(2, "initialize", Value::Null),
                request(3, "disconnect", Value::Null),
            ],
            &mut adapter,
        );

        assert_eq!(outputs.len(), 3);
        assert!(!is_success(&outputs[0]), "pause before initialize");
        assert_eq!(outputs[0]["command"], json!("pause"));
        assert!(is_success(&outputs[1]), "initialize");
        assert!(is_success(&outputs[2]), "disconnect");
        assert_eq!(exit_code, 0);
    }

    #[test]
    fn seq_is_server_owned_and_monotonic_while_request_seq_echoes_the_client_value() {
        let mut adapter = FakeAdapter;
        let (outputs, exit_code) = run_session(
            &[
                request(10, "initialize", Value::Null),
                request(3, "disconnect", Value::Null),
            ],
            &mut adapter,
        );

        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0]["seq"], json!(1));
        assert_eq!(outputs[0]["request_seq"], json!(10));
        assert_eq!(outputs[1]["seq"], json!(2));
        assert_eq!(outputs[1]["request_seq"], json!(3));
        assert_eq!(exit_code, 0);
    }

    #[test]
    fn unknown_command_answers_the_uniform_unsupported_command_error() {
        let mut adapter = FakeAdapter;
        let (outputs, _exit_code) = run_session(
            &[
                request(1, "initialize", Value::Null),
                request(2, "doSomethingUnknown", Value::Null),
                request(3, "disconnect", Value::Null),
            ],
            &mut adapter,
        );

        assert_eq!(outputs.len(), 3);
        assert!(!is_success(&outputs[1]));
        assert_eq!(outputs[1]["command"], json!("doSomethingUnknown"));
        assert_eq!(
            outputs[1]["message"],
            json!(unsupported_command("doSomethingUnknown"))
        );
    }

    #[test]
    fn rejects_any_request_received_after_the_disconnect_response() {
        let mut adapter = FakeAdapter;
        let (outputs, exit_code) = run_session(
            &[
                request(1, "initialize", Value::Null),
                request(2, "disconnect", Value::Null),
                request(3, "pause", Value::Null),
            ],
            &mut adapter,
        );

        assert_eq!(outputs.len(), 3);
        assert!(is_success(&outputs[0]));
        assert!(is_success(&outputs[1]));
        assert!(!is_success(&outputs[2]), "post-disconnect request");
        assert_eq!(exit_code, 0);
    }

    // ---- ScriptedRunAdapter: proves the try_recv/tick alternation ---

    /// An adapter that goes `Running` on `launch` and refuses to stop
    /// ticking until it has actually seen a `pause` request — removing
    /// any timing race between the gated reader delivering that request
    /// and an independent tick counter reaching some threshold. `ticks`
    /// is shared with the test's `GatedReader` so the reader can prove,
    /// structurally, that the `pause` frame cannot reach the channel
    /// before a minimum number of `tick()` calls have completed.
    struct ScriptedRunAdapter {
        ticks: Arc<AtomicUsize>,
        running: bool,
        pause_seen: bool,
        calls: Vec<String>,
    }

    impl ScriptedRunAdapter {
        fn new(ticks: Arc<AtomicUsize>) -> Self {
            ScriptedRunAdapter {
                ticks,
                running: false,
                pause_seen: false,
                calls: Vec::new(),
            }
        }
    }

    impl DebugAdapter for ScriptedRunAdapter {
        fn handle(
            &mut self,
            command: &str,
            _arguments: &Value,
            _out: &mut Vec<AdapterEvent>,
        ) -> Result<Value, String> {
            self.calls.push(format!("handle:{command}"));
            match command {
                "initialize" => Ok(Value::Null),
                "launch" => {
                    self.running = true;
                    Ok(Value::Null)
                }
                "pause" => {
                    self.pause_seen = true;
                    Ok(Value::Null)
                }
                _ => Err(unsupported_command(command)),
            }
        }

        fn tick(&mut self, out: &mut Vec<AdapterEvent>) -> RunState {
            let n = self.ticks.fetch_add(1, Ordering::SeqCst) + 1;
            self.calls.push(format!("tick:{n}"));
            // Never allowed to conclude the run before the scripted
            // pause was actually dispatched — see the struct doc.
            if self.pause_seen {
                self.running = false;
                out.push(AdapterEvent::Stopped {
                    reason: "step",
                    description: None,
                });
                RunState::Stopped
            } else {
                RunState::Running
            }
        }

        fn run_state(&self) -> RunState {
            if self.running {
                RunState::Running
            } else {
                RunState::Stopped
            }
        }
    }

    /// A `Read` whose bytes are grouped into chunks, each gated behind a
    /// minimum value of a shared tick counter: `read()` blocks (short
    /// polling sleep, bounded by a hard timeout so a broken test fails
    /// fast instead of hanging) until the counter reaches the chunk's
    /// threshold, then serves that chunk's bytes before moving to the
    /// next. EOF once every chunk has been served. Because the
    /// underlying `read()` call itself does not return early, the gated
    /// bytes are structurally unable to reach the reader-thread's mpsc
    /// channel before the threshold is met — this is what makes the
    /// mid-run pause test's ordering proof deterministic rather than a
    /// timing race.
    struct GatedReader {
        chunks: std::collections::VecDeque<(usize, Vec<u8>)>,
        ticks: Arc<AtomicUsize>,
        pending: Vec<u8>,
        pos: usize,
    }

    impl std::io::Read for GatedReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.pending.len() {
                let Some((threshold, bytes)) = self.chunks.pop_front() else {
                    return Ok(0); // EOF: every chunk has been served.
                };
                let deadline = Instant::now() + Duration::from_secs(5);
                while self.ticks.load(Ordering::SeqCst) < threshold {
                    if Instant::now() > deadline {
                        return Err(std::io::Error::other(
                            "GatedReader: tick threshold never reached",
                        ));
                    }
                    thread::sleep(Duration::from_micros(100));
                }
                self.pending = bytes;
                self.pos = 0;
            }
            let n = std::cmp::min(buf.len(), self.pending.len() - self.pos);
            buf[..n].copy_from_slice(&self.pending[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn pump_alternates_try_recv_and_tick_processing_a_pause_queued_mid_run() {
        let ticks = Arc::new(AtomicUsize::new(0));
        let mut adapter = ScriptedRunAdapter::new(Arc::clone(&ticks));

        let mut chunks = std::collections::VecDeque::new();
        chunks.push_back((
            0,
            framed_bytes(&[
                request(1, "initialize", Value::Null),
                request(2, "launch", Value::Null),
            ]),
        ));
        // The pause request's bytes cannot enter the channel until at
        // least 2 tick() calls have completed.
        chunks.push_back((2, framed_bytes(&[request(3, "pause", Value::Null)])));

        let reader = GatedReader {
            chunks,
            ticks: Arc::clone(&ticks),
            pending: Vec::new(),
            pos: 0,
        };

        let mut output = Vec::new();
        let exit_code = run(reader, &mut output, &mut adapter);
        let outputs = decode_output_frames(&output);

        // initialize, launch, pause responses, then the stopped event.
        assert_eq!(outputs.len(), 4);
        assert_eq!(outputs[0]["command"], json!("initialize"));
        assert_eq!(outputs[1]["command"], json!("launch"));
        assert_eq!(outputs[2]["command"], json!("pause"));
        assert!(is_success(&outputs[2]));
        assert_eq!(outputs[3]["type"], json!("event"));
        assert_eq!(outputs[3]["event"], json!("stopped"));

        let pause_at = adapter
            .calls
            .iter()
            .position(|c| c == "handle:pause")
            .expect("pause must have been dispatched");
        let ticks_before_pause = adapter.calls[..pause_at]
            .iter()
            .filter(|c| c.starts_with("tick:"))
            .count();
        assert!(
            ticks_before_pause >= 2,
            "expected at least 2 tick() calls before pause was dispatched, got {ticks_before_pause}: {:?}",
            adapter.calls
        );

        // No disconnect was ever sent; the loop ends via reader EOF.
        assert_eq!(exit_code, 1);
    }
}
