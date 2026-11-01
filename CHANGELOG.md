# Changelog

All notable changes to this project are documented here. Format: [Keep a Changelog](https://keepachangelog.com/); versions follow [SemVer](https://semver.org/).

## [Unreleased]

## [0.2.1]
### Fixed
- Release packages crashed with "illegal instruction" (the speech model never loaded) on CPUs that differ from the CI machine: the whisper.cpp / llama.cpp servers were built with the CI runner's CPU extensions. They are now built for a portable AVX2 baseline on x86-64.
- Linux and macOS packages: the servers could not start because the versioned library links (`libwhisper.so.1` and so on) were missing from the archives.
- Linux: EchoSub opened the microphone instead of the speakers. It now captures the monitor source of the default output through PulseAudio or PipeWire, and does not fall back to the microphone.
- Windows: the speech server failed when the program folder had non-ASCII characters (for example a Turkish user name).
- `cargo run` from `app/` could not find the servers or the models, because the relative `runtime` path broke once a server started in its own folder.
- With the VAD model missing (for example after cancelling a download) every transcription failed silently.
- Final captions could lose their last words when the newest partial result had been dropped from the queue.
- A late partial translation could overwrite a final caption or bring back one that had been removed.
- Turning on vocal separation now offers to download its model if it is missing.
- An empty number field in Settings no longer stops settings from being saved.
- CPU-only machines showed no captions at all: requests timed out after 30 s while the server kept working on them. The timeout is now 180 s, partial captions are skipped when recognition is slow, and at most three sentences wait in line.
- `build-sidecars.sh` started an unlimited number of compilers on Linux (`-j` without a number). It now uses the CPU count, or `JOBS=n`.

### Added
- A crashing speech or translation server is reported right away, also while the app is running, instead of after a two-minute timeout or not at all.
- Only one copy runs at a time, and on Windows the servers now always stop with the app, even if it is killed. A port that is already in use is reported.
- Release checks: each package must start both servers and contain no AVX-512 code in its CPU libraries before it is published.

### Changed
- Windows packages include the Visual C++ runtime, so it no longer has to be installed.
- whisper.cpp and llama.cpp are built from fixed, tested commits instead of whatever is newest on the day of the build.
- What the app hears is no longer written to `app.log` by default (`ECHOSUB_LOG_CAPTIONS=1` turns it on). The log is also rotated at 5 MB.

## [0.2.0]
### Added
- First-run model downloader: EchoSub detects missing models, shows what will be downloaded and asks before starting. Progress, cancel, resume after interruption, and automatic model loading when the download finishes. Reopen it from *Settings → Download / check models*; switching the quality preset opens it again if the new models are missing.
- GPU support documentation for AMD, NVIDIA and Intel (Vulkan), with an optional CUDA build (`-Backend cuda` / `BACKEND=cuda`) and a CPU-only build option.
- Release workflow: portable Windows (zip), Linux and macOS (tar.gz) packages built by CI on every `v*` tag.

### Fixed
- Windows release package: `llama-server` failed to start because it was linked against OpenSSL DLLs that were not shipped (OpenSSL is now disabled in the sidecar builds).
- Vocal separation model that was downloaded after start-up is now picked up without restarting.
- CI: Ubuntu link errors (needs 24.04), macOS CoreML link error (`clang_rt.osx`), Windows `link.exe` shadowed by Git's.

## [0.1.0] - first public preview
### Added
- Transparent, borderless, always-on-top subtitle overlay (Tauri 2): drag from the top strip, resize from the edges, text scales with the window.
- Speaker loopback capture (WASAPI on Windows, PipeWire/PulseAudio monitor on Linux, Core Audio tap on macOS 14.6+).
- Fully local pipeline: whisper.cpp (language auto-detect + ASR), TranslateGemma via llama.cpp (translation), Silero VAD.
- Partial captions while speaking; final captions ~0.6 s after the speaker stops.
- Vocal-separation fallback (UVR MDX-Net, ONNX Runtime) for music and singing.
- Click-through caption area, with a fallback when the cursor position cannot be read (Wayland).
- Settings: languages, quality/speed presets, fonts, colors, outline, background, line count, hide delay.

### Fixed
- Caption text could be clipped at the top when "show original" was on and the text was long.

### Known limitations
- Linux and macOS builds are compiled in CI but not yet validated on real hardware.
- No installer yet (portable archives only).
