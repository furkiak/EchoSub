# Changelog

All notable changes to this project are documented here. Format: [Keep a Changelog](https://keepachangelog.com/); versions follow [SemVer](https://semver.org/).

## [Unreleased]
### Added
- GPU support documentation for AMD, NVIDIA and Intel (Vulkan), with an optional CUDA build (`-Backend cuda` / `BACKEND=cuda`) and a CPU-only build option.

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
- No installer and no first-run model downloader yet (use `scripts/download-models`).
