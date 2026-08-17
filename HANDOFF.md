# Five Daemon — Agent Handoff Document

**Project:** Five Voice Assistant Daemon
**Repository:** `~/Desktop/Software/Five/five-daemon/` (git, branch `main`)
**Language:** Rust (Edition 2021) — **all-Rust stack, no Python, no subprocesses**
**Last Updated:** 2026-08-16
**Status:** 🟡 Scaffolded — compiles clean (`cargo check` ✅), config + CLI entry defined, subsystems pending

---

## 1. What Is This?

Five is a **voice assistant daemon** designed to integrate with OpenClaw. It runs as a background service that:

1. Listens for a wake word via ALSA audio capture
2. Records the subsequent command
3. Transcribes speech to text in-process via whisper.cpp (through `whisper-rs`)
4. POSTs the transcribed command to an OpenClaw HTTP endpoint
5. (Optionally) Records ambient audio on a schedule for context/logging

The name "Five" is a working title — a voice assistant that gives you a hand.

---

## 2. Stack Decision: All-Rust (2026-08-16)

The original plan used a **Python ONNX bridge** (wake word over TCP) and a **whisper.cpp subprocess**. Both are replaced by in-process Rust:

| Role | Old plan | Current stack | Why |
|------|----------|---------------|-----|
| Wake word | Python + ONNX Runtime, TCP bridge on :8765 | **`rustpotter`** | Pure Rust, no C deps, no Python runtime, no bridge process to supervise |
| Speech-to-text | whisper.cpp subprocess | **`whisper-rs`** (FFI to whisper.cpp) | Same ggml models and accuracy, no process spawn, streaming API |
| HTTP TLS | reqwest + native OpenSSL | reqwest + **rustls** | No system OpenSSL dependency; static, reproducible builds |
| Audio | cpal *or* alsa (undecided) | **alsa** directly | Linux-only daemon, hardware-specific config (Blue Yeti Nano); cpal abstraction paid for nothing |
| Config | `config` crate + serde_yaml (both half-present) | **serde_yaml** only | One loader, no drift |

**Result:** one binary, one language, no Python, no subprocess supervision, no TCP bridge. The config schema shrank accordingly (`bridge_python`, `bridge_script`, `bridge_port`, `whisper_path` removed).

**Accepted trade-offs:**
- `rustpotter` has a smaller ecosystem than openWakeWord/Porcupine. Pre-trained wake words are limited — expect to train your own `.rpw` model (one-time, ~1 hour with their tooling).
- **rustpotter 3.0.2 pins candle-core 0.2.2**, which only compiles with `half = "=2.4.1"` (half ≥ 2.5 retargeted its `rand_distr` impls from rand 0.8 to rand 0.9, breaking candle-core). The pin is in `Cargo.toml` with a comment — remove it when rustpotter upgrades its candle dependency.
- `whisper-rs` is in-process FFI: a whisper.cpp crash takes the daemon down. Mitigate with systemd `Restart=on-failure`.
- Build-time requirements: `cmake`, a C++ compiler (whisper.cpp), `libclang-dev` (bindgen), `libasound2-dev` (ALSA). Runtime needs none of these.

**If rustpotter proves inadequate:** fallback is openWakeWord models via the `ort` crate (still Rust, but requires the ONNX Runtime shared lib at runtime). Decide after real-world false-trigger testing.

---

## 3. Current State

| Component | Status | Notes |
|-----------|--------|-------|
| Project scaffold | ✅ Done | Flattened layout, git initialized, `.gitignore` in place |
| Configuration schema | ✅ Done | `src/config.rs` — YAML config, all sections, updated for all-Rust stack |
| CLI + entry point | ✅ Done | `src/main.rs` — clap `--config` flag, config load, tracing init |
| Audio capture (ALSA) | ❌ Not started | Dependency declared (`alsa`) |
| Resampling | ❌ Not started | Dependency declared (`rubato`) |
| Wake word detection | ❌ Not started | Dependency declared (`rustpotter`); **model file needed** |
| Speech-to-text | ❌ Not started | Dependency declared (`whisper-rs`); **ggml model needed** |
| HTTP client (OpenClaw) | ❌ Not started | `reqwest` declared; **blocked on API contract (§6)** |
| Ambient recording | ❌ Not started | Config defined, no logic |
| File logging/rotation | ❌ Not started | `tracing-appender` declared; console logging works now |
| VAD (end-of-speech) | ❌ Not started | Not yet scoped — see §6.2 |

**Compiles:** ✅ `cargo check` clean as of 2026-08-16 (requires `libasound2-dev`, `libclang-dev`, `cmake`, `g++`)

---

## 4. Architecture

```
five-daemon/
├── Cargo.toml          # All-Rust dependency stack
├── HANDOFF.md          # This document
├── .gitignore          # /target, logs, config.local.yaml
├── src/
│   ├── main.rs         # ✅ Entry point: CLI, config load, tracing
│   ├── config.rs       # ✅ YAML config structs + loader
│   ├── audio.rs        # ❌ ALSA capture + rubato resampling
│   ├── wakeword.rs     # ❌ rustpotter integration
│   ├── transcribe.rs   # ❌ whisper-rs wrapper
│   ├── openclaw.rs     # ❌ HTTP client to POST commands
│   ├── ambient.rs      # ❌ Scheduled ambient recording
│   └── logging.rs      # ❌ tracing-appender file rotation
└── scripts/            # (empty — for systemd unit, setup scripts)
```

### Planned Data Flow

```
[ALSA Capture] → [rubato: resample to 16kHz mono] → [rustpotter wake word]
                                                          │
               [Ambient Recorder] ←────────────────┘      │ (trigger)
                                                          ▼
                                            [Record Command Audio]
                                            (until VAD silence or timeout)
                                                          │
                                                          ▼
                                            [whisper-rs transcription]
                                                          │
                                                          ▼
                                            [POST → OpenClaw endpoint]
```

---

## 5. Configuration

Configuration is YAML-based, loaded at startup via `AppConfig::from_file()`. Path is set with `--config` (default: `config.yaml`).

### Config Sections

| Section | Purpose |
|---------|---------|
| `audio` | ALSA card/device, sample rates, channels, chunk size |
| `wakeword` | rustpotter model path, detection threshold |
| `transcription` | whisper ggml model path, recording duration, sample rate |
| `openclaw` | HTTP endpoint URL, timeout |
| `ambient` | Enable/disable, interval, duration, log directory, retention |
| `logging` | Log level, directory, rotation settings |

### Example `config.yaml` (matches current `config.rs` exactly)

```yaml
audio:
  alsa_card: 2
  alsa_device: 0
  sample_rate: 48000
  channels: 2
  format: "S24_3LE"
  target_rate: 16000
  target_channels: 1
  chunk_ms: 100

wakeword:
  model_path: "/opt/five/models/five.rpw"
  threshold: 0.5

transcription:
  model_path: "/opt/five/models/ggml-base.en.bin"
  command_duration_sec: 10
  sample_rate: 16000

openclaw:
  endpoint: "http://localhost:8080/api/v1/command"
  timeout_sec: 30

ambient:
  enabled: true
  interval_min: 15
  duration_sec: 30
  log_dir: "/var/log/five/ambient"
  max_files: 100

logging:
  level: "info"
  log_dir: "/var/log/five"
  max_size_mb: 10
  max_files: 5
```

**Known config weaknesses** (deliberate for now, revisit before deployment):
- `threshold` documented as 0.0–1.0 but not validated (bare `f32`)
- `audio.format` is a free-form string; an enum would catch typos at load time
- No serde defaults — every field is mandatory

---

## 6. Open Questions / Decisions Needed

1. **OpenClaw API contract — THE blocker for `openclaw.rs`.**
   - Request body schema? (`{"text": "..."}`? metadata? session IDs?)
   - Authentication? (API key, JWT, none on localhost?)
   - Response format — does OpenClaw return something Five should act on or log?
   - Which errors are retryable?
   *Cannot write the HTTP client until this is answered.*

2. **End-of-speech detection.** Current config records a fixed `command_duration_sec: 10` window. Better: record until the user stops talking. Options: energy-threshold VAD (simple, no deps) or Silero VAD via `vad-rs`/`ort` (robust, adds ONNX Runtime). Decide during `audio.rs` work.

3. **Wake word model.** Train a custom "Five" `.rpw` with rustpotter's tooling, or use a generic pre-trained wake word for the prototype? Prototype can also start with a keyboard/timer trigger stub (see §7).

4. **Whisper model size.** `ggml-tiny.en` (fastest, least accurate) vs `base.en` (balanced) vs `small.en` (best, slowest). Depends on target hardware — decide with a latency measurement once `transcribe.rs` exists.

5. **Ambient recording: why?**
   - Context awareness ("what was happening before the command")?
   - Or logging/debugging?
   - **Privacy implications must be documented before this ships enabled.**

6. **Deployment.** systemd user service vs. system service? Affects `/var/log/five` ownership and whether the daemon can hold the ALSA device exclusively. Docker is unlikely (ALSA hardware access) but not ruled out.

---

## 7. Next Steps (Suggested Priority)

1. **Resolve the OpenClaw API contract** (§6.1) — everything else can proceed in parallel, but this unblocks the end-to-end path
2. **Implement `audio.rs`** — ALSA capture + rubato resampling to 16 kHz mono; verify with a WAV dump via `hound`
3. **Build a wake word stub** — keyboard/timer trigger for pipeline testing before the `.rpw` model exists
4. **Implement `transcribe.rs`** — whisper-rs on a recorded WAV; measure latency for `tiny` vs `base` models
5. **Implement `wakeword.rs`** — rustpotter on the live audio stream
6. **Implement `openclaw.rs`** — POST with JSON body (once §6.1 is answered)
7. **Add VAD-based end-of-speech** — replace fixed `command_duration_sec` window (§6.2)
8. **Implement `ambient.rs`** — timer-based recording with file rotation
9. **Implement `logging.rs`** — `tracing-appender` file rotation per `logging` config
10. **Write systemd unit + real `config.yaml`** — for deployment; document ambient-recording privacy posture

---

## 8. Related Projects

- **OpenClaw** — The receiving endpoint. Its API contract is the key external dependency (§6.1).
- **whisper.cpp** — Used via `whisper-rs` FFI. Models are the standard ggml files; download from the whisper.cpp/Hugging Face repos.
- **rustpotter** — https://github.com/GiviMAD/rustpotter — model tooling for training the wake word.
- **Almanach / SemOS** — User's other active projects. Five may eventually integrate with these.

---

## 9. Files in This Repo

```
five-daemon/
├── Cargo.toml          # Package manifest (all-Rust stack)
├── HANDOFF.md          # This document
├── .gitignore
├── src/
│   ├── main.rs         # Entry point: clap, config load, tracing
│   └── config.rs       # Configuration structs + YAML loader
└── scripts/            # (empty — for systemd unit, setup scripts)
```

---

## 10. Contact / Context

- **Author:** Agent working for user (professor/educator, Toronto timezone)
- **User's note:** This is a side project. Almanach and SemOS are higher priority.
- **Philosophy:** The user values craft over speed. Do it right, not fast.

---

*Generated: 2026-08-16 (rewritten for all-Rust stack)*
*Status: Ready for handoff — scaffold compiles, stack decided, implementation awaits.*
