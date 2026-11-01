# Changelog

All notable changes to this project are documented here. Format: [Keep a Changelog](https://keepachangelog.com/); versions follow [SemVer](https://semver.org/).

## [Unreleased]

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
