# Writing a connector

A Stintmate connector is the only part of Stintmate that knows a simulator. It reads the
simulator, maps what it sees to Stintmate's neutral data model and hands it to Pitwall, the
telemetry database. This page shows how the AMS2 connector does that, as a map for authors who
bring another simulator. The details of each step are in [ARCHITECTURE.md](ARCHITECTURE.md);
how to study a simulator's data is shown by [FINDINGS.md](FINDINGS.md).

## What the protocols are

- **[The connector protocol](https://github.com/Stintmate-App/connector-kit/blob/main/docs/connector-protocol.md)**, in the connector kit: what a
  connector declares and writes: the live frame, the write messages, the telemetry stream, the
  capture target, announcements. It is language-neutral and is the document to build against.
- **Stintmate IPC v2**, the transport (named pipes, MessagePack, requests, state channels,
  streams, authentication by a key file): the open
  [stintmate-ipc](https://github.com/Stintmate-App/stintmate-ipc) repository, with its
  specification, test vectors and the `stintmate-ipc-check` conformance tool. It has a Rust
  crate (the one this connector uses), a Python binding and a C ABI for other languages.
  Pick the one that suits your simulator's API.

## What to read from the simulator

Everything the connector needs comes from one source, here a shared memory block read
read-only. Write down, for your simulator, what [FINDINGS.md](FINDINGS.md) records for AMS2:

1. How to read a **consistent** frame (a sequence counter, a lock, a snapshot call).
2. **Units, signs and orientation** of every channel (the header is often wrong; verify with a
   known setup or a known corner).
3. **Is the game running?** A mapping or a socket can outlive the game: check the process.
4. **Where the simulator lies about laps**: when the counter increments, what a pit exit or a
   return to the garage does, what a lap time holds just after the line, what an invalid flag
   means.
5. What it **does not expose** (the setup, the weather forecast): say so in the declaration
   texts so the engineer does not ask for it.

## What to declare

A connector connects to Pitwall's endpoint `pitwall` as a client, with a hello that names the
connector (`connector_id`, `connector_name`, `sim`, `sim_name`), and declares what it offers.
This one does so per process:

| Declaration | Purpose |
|---|---|
| `connector.status` | game running, phase, session, lap, the outbox, an error code |
| `connector.live_frame` | the current state on request, converted to neutral units, `null` for "not given" |
| `connector.telemetry` | a stream of small fixed-layout samples while Pitwall subscribes |
| `connector.capture_target` | which window is the game, so Pitwall can grab it |
| `connector.action` | actions the engineer can ask for |
| `pitwall.catalog.declare` | the announcement codes and English texts (Pitwall translates and speaks them) |
| `pitwall.guidance.declare` | notes for the engineer about what this simulator can and cannot tell |

A connector never composes text for the user: it sends **codes** with parameters, and Pitwall
writes them in the user's language. Each connector process declares only what it implements;
Pitwall sees the union and answers `not_implemented_by_connector` for the rest.

## What to write, and the outbox

Sessions, laps (channels as raw float32 arrays), setup snapshots, pit exits and rival rows go
through the `pitwall.write.*` methods. The AMS2 connector never sends them straight from the
sampling loop: it stores each as a file in a disk **outbox** and a sender thread delivers them
in order, with an idempotency key stored in the file. That gives three guarantees an author
should copy: a lap recorded while Pitwall is closed is delivered when it opens; nothing is
stored twice if an answer is lost; and the real-time loop never waits for a pipe or a disk. See
[the outbox](ARCHITECTURE.md#the-outbox).

## The control side

A connector's setup that replaces a running connector should hold it through the hub instead of
asking the person to close Stintmate: see "Updating your connector while Stintmate runs"
in the connector protocol (above), and
[installer.md](installer.md) here for how this connector's setup does it.

To be managed by the Stintmate hub (start, stop, show, quit), a connector also serves a small
control endpoint and, when it is installed, writes a descriptor (`<id>.json`) in the per-user
machine folder `%LOCALAPPDATA%\Stintmate\connectors\` (no workspace needed). The AMS2
connector does both in its control app; see
[the control app](ARCHITECTURE.md#the-control-app-and-its-endpoint). A connector without it
still works when started by hand.

## Checking your work

- Run `stintmate-ipc-check` against your endpoint.
- Replay a recorded or scripted session through your reader into a fake Pitwall and compare the
  stored laps with the simulator's official times (on the first recordings of the AMS2 connector the lap times matched the game's within 18 ms, see [FINDINGS.md](FINDINGS.md#lap-time-accuracy)).
- Measure your loop's cost: a recorder should stay well under 1 percent of a core and never
  allocate or wait on the sampling thread ([performance](ARCHITECTURE.md#performance)).
