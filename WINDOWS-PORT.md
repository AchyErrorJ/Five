# Five — Windows (Legion Go) Port Plan

> **Status: DONE and live.** See "Field notes" at the bottom for what the plan
> didn't predict.

## Goal
Build and run five-daemon natively on Windows 11 (Legion Go), keeping the Linux build untouched and working. HANDOFF.md's "ALSA directly, no cpal" decision stays in force **on Linux** — the Windows backend is an addition, not a replacement.

## Linux-specific surface area (found by reading every source file)

| File | Linux-only code | Port needed |
|------|-----------------|-------------|
| `src/audio.rs` | ALSA capture (`alsa::pcm`), `Format::from_str` | WASAPI capture backend |
| `src/voice.rs` | ALSA playback in `play()` | WASAPI playback backend |
| `src/captions.rs` | shells out to `gdbus` / `notify-send` | Windows toast or disable |
| `src/config.rs` | `alsa_card`, `alsa_device`, `format` fields | add device-name fields, make ALSA fields optional |
| `Cargo.toml` | `alsa` dep; `nix` dep (**unused in code** — delete) | target-gated deps |

Everything else is already cross-platform: `transcribe.rs` (whisper-rs builds on Windows via cmake+MSVC), `wakeword.rs` (rustpotter = pure Rust), `openclaw.rs` (reqwest+rustls), `main.rs`, and kokoro-en (pure Rust + ONNX Runtime; its `directml` feature even gives GPU acceleration on the Go's AMD APU).

## Approach: `cfg`-gated backends behind the existing APIs

The existing public APIs stay exactly as they are — `AudioCapture::start/recv/receiver`, `voice::play`, `write_wav`, `record_to_file` — so `main.rs` and the listen loop need **zero changes**.

### 1. `Cargo.toml`
- Move `alsa` under `[target.'cfg(target_os = "linux")'.dependencies]`
- Add `cpal` under `[target.'cfg(target_os = "windows")'.dependencies]` (WASAPI backend)
- Delete `nix` (declared but never used)
- Keep `rubato`, `hound`, everything else shared

### 2. `src/audio/` — split into module with two backends
- `src/audio/mod.rs` — shared API surface: `AudioCapture` (mpsc receiver of 16 kHz mono f32 chunks), `write_wav`, `record_to_file`, the rubato resample helper, and the `to_mono_f32`-style conversion utilities. Re-exports whichever backend.
- `src/audio/alsa.rs` (`#[cfg(target_os = "linux")]`) — current `audio.rs` code moved verbatim.
- `src/audio/wasapi.rs` (`#[cfg(target_os = "windows")]`) — cpal capture:
  - Open input device (config `input_device` name, else system default — the Go's mic array)
  - cpal is callback-driven, so the callback pushes converted mono f32 into a buffer; a small adapter thread (or the callback itself) slices it into `chunk_ms` chunks, resamples with the same `FftFixedIn` path, and feeds the same `sync_channel(64)` — downstream code can't tell the difference
  - Handle cpal's negotiated format (f32 or i16, device rate/channels) instead of ALSA's S24_3LE
  - Drop semantics preserved: dropping the receiver stops the stream

### 3. `src/voice.rs` — `play()` cfg-split
- Linux: current ALSA code, untouched
- Windows: cpal output stream to default device (the Go's speakers / whatever's connected), S16 or f32 interleaved, block until drained — same signature, same blocking behavior

### 4. `src/captions.rs` — Windows stub
- `#[cfg(windows)]`: log "captions not supported on Windows yet" and continue (text is already printed to the terminal). Toast notifications via WinRT can come later — not worth blocking the port on.

### 5. `src/config.rs`
- Add `input_device: Option<String>` / `output_device: Option<String>` (Windows device names; `None` = system default)
- Give `alsa_card`, `alsa_device`, `format` serde defaults so a Windows config file needn't carry them

### 6. New `config.windows.yaml`
Legion Go defaults: default input/output devices, 48 kHz → 16 kHz resample, relative `models/` paths, `captions.enabled: false`.

### 7. Models (not in git — must be fetched/copied)
- `models/ggml-base.en.bin` — download from Hugging Face (whisper.cpp repo)
- Kokoro ONNX model + voices dir — download (kokoro-en docs)
- `models/five.rpw` — copy from the Linux machine (or retrain via `train-wakeword` on the Go's mic — probably worth it, different mic/acoustics)

## Build verification (this machine)
1. `cargo build` — expect friction only from whisper.cpp's cmake build; fix forward
2. `five-daemon record` → WAV of the Go's mic (verify capture + resample)
3. `five-daemon transcribe recording.wav` (verify whisper)
4. `five-daemon speak "hello"` (verify kokoro + WASAPI playback)
5. `five-daemon listen --bridge bridge.txt` end-to-end: wake word → command → file → reply spoken

## Not in scope
- systemd → Windows Service / Task Scheduler (run it in a terminal for now)
- Ambient recording, file logging (unimplemented on Linux too)
- Windows toast captions

## Field notes (from first live session, 2026-08-24)
- **Build release-only.** Debug builds crash with a MSVC CRT debug assert
  (`_osfile(fh) & FOPEN`, read.cpp) inside whisper's native code. Release is
  unaffected. `cargo build --release` is the supported Windows build.
- **Rustpotter wakeword abandoned** on this machine: the .rpw matched room
  tone, not the word. Always-listening text trigger (whisper hears "five" in
  every utterance) is the default; `wakeword.enabled: false`.
- **tiny.en over base.en**: always-listening transcribes every utterance;
  base.en cost ~9s per 10s block on the Z1 Extreme CPU, tiny.en ~4s.
- **whisper-rs `vulkan` feature doesn't build on Windows** — ggml's nested
  vulkan-shaders-gen ExternalProject exceeds the 260-char path limit even
  with a short CARGO_TARGET_DIR. CPU STT for now.
- **Kokoro must run on CPU** (`voice.provider: "cpu"`): DirectML on the AMD
  iGPU intermittently fails the ConvTranspose node and returns silence.
- **Pin `audio.output_device`**: Windows' default output was a BT adapter,
  so replies were inaudible. `devices` subcommand lists output names.
- **Playback tail-clip**: WASAPI drops the last buffer on stream close —
  play() pads 1s of silence.
