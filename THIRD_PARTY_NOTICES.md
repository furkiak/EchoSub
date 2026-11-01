# Third-party components

EchoSub itself is MIT-licensed. It runs the following components locally; their own licenses apply.
Model files are **not** distributed with this repository or the release packages: the app (or `scripts/download-models.*`) downloads them from their original sources.
Windows packages include the Microsoft Visual C++ runtime DLLs (`msvcp140*`, `vcruntime140*`, `vcomp140`), redistributed under the Visual Studio license terms.

| Component | Use | License |
|---|---|---|
| [whisper.cpp](https://github.com/ggml-org/whisper.cpp) | Speech recognition server | MIT |
| [Whisper models](https://huggingface.co/ggerganov/whisper.cpp) (OpenAI) | `large-v3-turbo` q5_0, `small` q5_1 | MIT |
| [llama.cpp](https://github.com/ggml-org/llama.cpp) | Translation model server | MIT |
| [TranslateGemma](https://huggingface.co/google/translategemma-4b-it) (Google), GGUF by bullerwins | Translation 4B / 12B | Gemma Terms of Use; review before redistribution or commercial use |
| [Silero VAD](https://github.com/snakers4/silero-vad), ggml port `ggml-org/whisper-vad` | Voice activity filter | MIT |
| [UVR MDX-Net `Voc_FT`](https://github.com/TRvlvr/model_repo) | Vocal separation fallback | See the model repository's terms |
| [Tauri](https://tauri.app), [cpal](https://github.com/RustAudio/cpal), [ONNX Runtime](https://onnxruntime.ai) (`ort`), [rustfft](https://github.com/ejmahler/RustFFT), [ureq](https://github.com/algesten/ureq) | Application framework and libraries | MIT / Apache-2.0 |

Licenses can change; please verify each upstream project before redistributing.
