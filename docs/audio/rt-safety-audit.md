# Real-time safety audit (E03-T08)

Scope: `drumpad-engine` and the render loop in `drumpad-audio`. The rule is
simple: once the stream is running, the audio thread may not allocate, lock,
log, touch the filesystem, sleep, or call into the OS on the normal path.
Error paths must also stay allocation-free.

## Callback entry points

| Thread | Entry point | RT-safe |
| --- | --- | --- |
| WASAPI event thread (owned by `drumpad-audio`) | `Running::run_loop` → `AudioCallback::process` → `Engine::process` | Yes, normal and error paths |
| Fake backend test thread | same callback shape | Yes |
| Manual fake stream | `ManualStream::render_block` | Test-only, still kept allocation-free |
| UI/input threads | `EngineHandle::note_on/note_off/send_input/send` | Not real-time; lock-free queues |

## Vocabulary

The engine uses the shared contract types, not private copies:

- `drumpad_samples::SampleBuffer` for immutable audio data.
- `drumpad_input::trigger::{InputEvent, TriggerEvent, ReleaseEvent}` as the
  trigger/release queue payload. The input producer is single-owner; the app's
  input aggregator funnels keyboard, MIDI, and UI-preview events through one
  `EngineHandle`.
- `drumpad_model::ids::{PadId, Velocity}` for pad and velocity parameters.
- `drumpad_engine::voice::SampleBank` for the published slot table (engine
  concern; buffers inside are canonical `SampleBuffer` values).

## Checklist

| Requirement | Implementation | Evidence |
| --- | --- | --- |
| No heap allocation | Engine allocates only in `Engine::new`; voices, event buffers and scratch are preallocated. `Vec` length never changes in `process`. | `tests/rt_safety.rs`: counting global allocator, zero allocations across 1,000 blocks. |
| No locking | Triggers/releases use the input crate's SPSC queue; commands use `rtrb`; meters use atomics. No mutex is touched from the stream thread. `StreamDiagnostics` error paths publish an `AtomicU8` code; the detailed `String` is only written before the stream starts or by the UI. | Source scan; `mark_error`/`mark_device_lost` are allocation-free. |
| No logging | No `println!`, `eprintln!`, `tracing`, or `log` in `mixer`, `voice`, `limiter`, or `meter`. | Source scan test. |
| No filesystem or OS calls | `Engine::process` performs arithmetic, slice indexing, and atomics only. `std::fs` appears only in `settings.rs` (off-thread). | Source scan test. |
| No `Arc` clone/drop | Voices store a slot index, not an `Arc`. A bank swap moves an `Arc` through the command queue and retires the old bank through a return queue; the UI drops it. If the return path is full, the swap is deferred (commands are peeked, not popped) or the displaced bank is parked in a fixed overflow slot. `mem::forget` is the last-resort invariant guard, never a drop. | `rapid_bank_swaps_do_not_free_on_the_audio_thread`: 64 swaps in one block, zero allocations and zero deallocations on the audio thread. |
| No panics on the normal path | All indexing is bounded by preallocated lengths; no `unwrap`/`expect`/`panic!` in the render path. `debug_assert!` is debug-only. The playhead uses saturating arithmetic. | Source scan for `unwrap(`/`expect(`/`panic!` in RT modules. |
| No unbounded work | At most 64 trigger events and 256 commands per block; voice loop is the preallocated maximum (256). | `PENDING_MAX`, `COMMAND_BUDGET`, `MAX_VOICES`. |
| Deterministic event order | Same-timestamp events are compacted stably and insertion-sorted by offset, so FIFO is preserved. | `equal_timestamp_triggers_keep_fifo_order`. |
| Deterministic stealing | Oldest by a fresh global `started_at` order, or quietest by envelope estimate; ties resolve to the lowest voice index. The stolen voice claims its new order immediately so the next hit cannot re-steal it. | `oldest_stealing_advances_through_the_pool`. |
| Click-free transitions | Start/end de-click fades, 2 ms steal fade, 5-6 ms choke/release fades, 5 ms panic, 5 ms bank swap. | Unit tests in `voice`/`mixer`. |
| Bounded queue drops | Queue-full drops the newest item and increments an atomic counter. | `queue` tests and `EngineHandle::dropped_triggers`. |
| Zero-latency limiter | Peak limiter plus final clamp; no lookahead. | `limiter` tests, `latency_frames() == 0`. |
| Real-time thread priority | The audio thread registers with MMCSS "Pro Audio" and `AVRT_PRIORITY_CRITICAL` before opening/rendering. | `drumpad_audio::priority`; measurements in `backend-spike.md`. |

## Block budget

`Engine::process` performs, per output frame:

1. Update 16 pad gain/pan smoothers.
2. Render every active voice: one interpolation read (linear or 4-point cubic)
   plus envelopes.
3. Sum into the stereo bus, apply the limiter, write two samples, meter.

At 48 kHz with 64 voices that is ~3.1 M voice-samples per second. The measured
silent-render WASAPI load was ~1 %; the real engine load is dominated by active
voices and is tracked by the callback-load diagnostic.

## Why voices do not own samples

Holding an `Arc<SampleBuffer>` in a voice would clone a refcount on every
trigger and drop one when the voice ends. A drop can be the last reference and
free memory on the audio thread. Instead:

- `SampleBank` owns `SampleBuffer` values (each already internally shared).
- A voice stores a `u16` slot index and reads through the bank.
- A bank swap (`Command::SwapSampleBank`) fades all voices, then moves the old
  `Arc<SampleBank>` into a retire queue. The UI drains and drops retired banks
  in `EngineHandle::collect_retired_banks`.
- If the retire queue and overflow slots are full, `apply_commands` peeks at
  the next swap and stops processing commands, leaving the `Arc` in the queue.
  `try_apply_pending_bank` also defers installation until a retire slot exists.
  No path drops the last reference in `process`.

## CI guardrails

The workspace CI (foundation epic E01-T02) runs `cargo test`, so these
integration tests are the guardrail:

- `crates/drumpad-engine/tests/rt_safety.rs` — counting allocator: zero
  allocations **and zero deallocations** inside `Engine::process`, including
  a 64-bank rapid-swap burst.
- The source scan in the same test — fails if `println!`, `eprintln!`,
  `Mutex`, `std::fs::`, `thread::sleep`, `log::`, or `tracing::` appears in the
  real-time modules, and if `unwrap`/`expect`/`panic!` appears in their
  non-test code.

If a future change needs one of those, it must move to the non-real-time side
or be added to the explicit allowlist with a comment explaining why it cannot
run in the callback.

## Known exceptions

- `StreamDiagnostics::set_error` / `set_device_lost` take a mutex and allocate.
  They are called from open/startup paths, before the render loop begins. The
  running loop uses `mark_error` / `mark_device_lost`, which are atomics only.
- `EngineHandle::collect_retired_banks` drops `Arc`s on the caller's thread.
  Call it from the UI thread, not the audio thread.
- The fake backend thread allocates its scratch buffer once at thread start,
  before entering its loop. The real WASAPI loop allocates all scratch buffers
  during stream setup.

## Verification (2026-11-07, Windows 11 x64)

Run from the repository root:

```powershell
cargo test -p drumpad-engine                 # 51 unit/integration tests
cargo test -p drumpad-engine --test rt_safety # allocation + rapid swap + source scan
cargo test -p drumpad-audio                  # 33 unit tests
cargo test -p drumpad-audio --test hardware -- --ignored --nocapture --test-threads=1
cargo clippy -p drumpad-engine -p drumpad-audio --all-targets -- -D warnings
cargo fmt -p drumpad-engine -p drumpad-audio -- --check
```

The hardware suite passed all three ignored tests on the Realtek endpoint:
shared mode at 48 kHz with 0 xruns, exclusive mode at 256 frames with 0 xruns,
and 100 real open/stop cycles without leaking threads or handles. The 120 s
MMCSS soak in `measurements/` reports 11,252 shared-mode callbacks and 22,503
exclusive-mode callbacks with zero late callbacks.

## Residual risks

- The analog round trip (PC 3.5 mm output to Nitro Max Aux In) needs a loopback
  cable and is owned by QA (E11); it is not claimed as measured here.
- This Realtek endpoint cannot run 128 frames: shared mode is fixed at 512
  frames and exclusive mode bottoms out at 256 frames. The backend reports the
  negotiated period instead of pretending the request succeeded.
- Physical unplug/replug is simulated in the `AudioSystem` test; the real device
  watcher and stream reopen path still need one manual session on stage hardware.
- `retire_bank`'s `mem::forget` is a last-resort invariant guard for a logic
  error, not a normal path. If it ever runs, memory is retained rather than
  freed on the audio thread; the rapid-swap test exists to keep that path from
  firing.
- `StreamDiagnostics::set_error` / `set_device_lost` still use a mutex and
  allocate, but only before the render loop starts; the running loop uses the
  atomic `mark_error` / `mark_device_lost` path.

## Sign-off

- [x] Reviewed against the E02/E03 final tree.
- [x] Counting allocator reports zero allocations and zero deallocations for
      silent blocks, active voices, stealing/choke, single swaps, and a 64-bank
      rapid-swap burst.
- [x] Source scan passes for `mixer`, `voice`, `limiter`, `meter`, and `queue`.
- [x] Same-timestamp FIFO order and stealing-order regressions pass.
- [x] Hardware smoke tests pass on the reference Realtek endpoint.
- [x] Clippy with `-D warnings` and rustfmt check pass for both crates.
