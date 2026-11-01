#!/usr/bin/env bash
# Modelleri runtime/models altına indirir. Kullanım: scripts/download-models.sh [fast|balanced|quality]
# (balanced = varsayılan; fast küçük whisper + 4B çeviri, quality 12B çeviri)
set -euo pipefail
Q="${1:-balanced}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; if [ -n "${ECHOSUB_HOME:-}" ]; then H="$ECHOSUB_HOME"; elif [ -d "$ROOT/app" ]; then H="$ROOT/app/runtime"; else H="$ROOT/runtime"; fi; DIR="$H/models"
mkdir -p "$DIR"
get() { # url dosya
  if [ -s "$DIR/$2" ]; then echo "var: $2"; return; fi
  echo "indiriliyor: $2"; curl -L --fail --retry 3 -C - -o "$DIR/$2.part" "$1" && mv "$DIR/$2.part" "$DIR/$2"
}
HF=https://huggingface.co
get "$HF/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin" ggml-silero-v5.1.2.bin
get "https://github.com/TRvlvr/model_repo/releases/download/all_public_uvr_models/UVR-MDX-NET-Voc_FT.onnx" UVR-MDX-NET-Voc_FT.onnx
case "$Q" in
  fast)     get "$HF/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin" ggml-small-q5_1.bin ;;
  *)        get "$HF/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin" ggml-large-v3-turbo-q5_0.bin ;;
esac
case "$Q" in
  quality)  get "$HF/bullerwins/translategemma-12b-it-GGUF/resolve/main/translategemma-12b-it-Q4_K_M.gguf" translategemma-12b-it-Q4_K_M.gguf ;;
  *)        get "$HF/bullerwins/translategemma-4b-it-GGUF/resolve/main/translategemma-4b-it-Q4_K_M.gguf" translategemma-4b-it-Q4_K_M.gguf ;;
esac
echo "tamam: $DIR"
