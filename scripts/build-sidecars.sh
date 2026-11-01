#!/usr/bin/env bash
# whisper.cpp ve llama.cpp sunucularını derler ve app/runtime/{whisper,llama} altına koyar.
# Linux: Vulkan (AMD/NVIDIA/Intel; vulkan-sdk + glslc gerekir). macOS: Metal.
# Gerekenler: git, cmake, ninja (ya da make), C++ derleyici.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="${SRC:-$ROOT/build-src}"; if [ -n "${ECHOSUB_HOME:-}" ]; then OUT="$ECHOSUB_HOME"; elif [ -d "$ROOT/app" ]; then OUT="$ROOT/app/runtime"; else OUT="$ROOT/runtime"; fi
# BACKEND=vulkan (varsayılan, Linux), cuda (yalnızca NVIDIA, CUDA Toolkit gerekir) ya da cpu. macOS her zaman Metal.
BACKEND="${BACKEND:-vulkan}"
case "$(uname -s)" in
  Linux)
    case "$BACKEND" in
      vulkan) GPU="-DGGML_VULKAN=1" ;;
      cuda)   GPU="-DGGML_CUDA=1" ;;
      cpu)    GPU="-DGGML_VULKAN=0" ;;
      *) echo "BACKEND: vulkan | cuda | cpu"; exit 1 ;;
    esac ;;
  Darwin) GPU="-DGGML_METAL=1" ;;
  *) echo "Bu betik Linux/macOS içindir"; exit 1 ;;
esac
# Taşınabilir paket: derleyen makinenin CPU'suna özel kod üretme (CI'daki AVX-512 başka CPU'larda çöker). x86-64 için AVX2 tabanı.
CPU=""
if [ "$(uname -m)" = x86_64 ]; then CPU="-DGGML_NATIVE=OFF -DGGML_AVX2=ON -DGGML_FMA=ON -DGGML_F16C=ON -DGGML_BMI2=ON"; fi
build() { # ad repo hedef
  [ -d "$SRC/$1" ] || git clone --depth 1 "$2" "$SRC/$1"
  cmake -S "$SRC/$1" -B "$SRC/$1/build" -DCMAKE_BUILD_TYPE=Release $GPU $CPU \
        -DBUILD_SHARED_LIBS=ON -DCMAKE_INSTALL_RPATH='$ORIGIN' -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON \
        -DLLAMA_CURL=OFF -DLLAMA_OPENSSL=OFF -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF -DLLAMA_BUILD_SERVER=ON \
        -DWHISPER_BUILD_TESTS=OFF -DWHISPER_BUILD_EXAMPLES=ON
  cmake --build "$SRC/$1/build" -j --target "$3"
  mkdir -p "$OUT/$4"
  cp "$(find "$SRC/$1/build" -name "$3" -type f | head -1)" "$OUT/$4/"
  find "$SRC/$1/build" \( -name '*.so*' -o -name '*.dylib' \) -type f -exec cp -P {} "$OUT/$4/" \;
}
build whisper.cpp https://github.com/ggml-org/whisper.cpp whisper-server whisper
build llama.cpp   https://github.com/ggml-org/llama.cpp   llama-server   llama
echo "tamam: $OUT"
