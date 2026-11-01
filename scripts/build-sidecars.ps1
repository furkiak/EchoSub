# whisper.cpp ve llama.cpp sunucularını Vulkan ile derleyip runtime\{whisper,llama} altına koyar.
# Gerekenler: Visual Studio 2022 (C++), CMake, Ninja, Vulkan SDK (VULKAN_SDK ortam değişkeni), git.
# "x64 Native Tools Command Prompt" ya da MSVC ortamı yüklü bir PowerShell'de çalıştırın.
# -Backend vulkan (varsayılan; AMD, NVIDIA, Intel), cuda (yalnızca NVIDIA, CUDA Toolkit gerekir) ya da cpu.
param([string]$Src = '', [ValidateSet('vulkan', 'cuda', 'cpu')][string]$Backend = 'vulkan')
$ErrorActionPreference = 'Stop'
$gpu = switch ($Backend) { 'vulkan' { '-DGGML_VULKAN=1' } 'cuda' { '-DGGML_CUDA=1' } default { '-DGGML_VULKAN=0' } }
# Paketler başka bilgisayarlarda çalışacağı için derleyen makinenin CPU'suna özel (-march=native) kod üretilmez;
# aksi halde (ör. CI'daki AVX-512) daha eski/farklı CPU'larda "illegal instruction" ile çöker. AVX2 tabanı (Haswell 2013+ / Zen 1+).
$cpu = @('-DGGML_NATIVE=OFF', '-DGGML_AVX2=ON', '-DGGML_FMA=ON', '-DGGML_F16C=ON', '-DGGML_BMI2=ON')
$root = (Resolve-Path "$PSScriptRoot\..").Path
if (-not $Src) { $Src = "$root\build-src" }
$out = if ($env:ECHOSUB_HOME) { $env:ECHOSUB_HOME } elseif (Test-Path "$root\app") { "$root\app\runtime" } else { "$root\runtime" }
if (-not (Get-Command cl -ErrorAction SilentlyContinue)) { throw 'MSVC (cl.exe) ortamı yok: "x64 Native Tools" kabuğundan çalıştırın.' }
if ($Backend -eq 'vulkan' -and -not $env:VULKAN_SDK) { throw 'VULKAN_SDK tanımlı değil (Vulkan SDK kurun).' }
function Build([string]$name, [string]$repo, [string]$target, [string]$sub, [string[]]$extra) {
  if (-not (Test-Path "$Src\$name")) { git clone --depth 1 $repo "$Src\$name" }
  $b = "$Src\$name\build-echosub"
  cmake -S "$Src\$name" -B $b -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_C_COMPILER=cl -DCMAKE_CXX_COMPILER=cl $gpu @cpu @extra
  if ($LASTEXITCODE) { throw "$name yapılandırılamadı" }
  cmake --build $b --target $target
  if ($LASTEXITCODE) { throw "$name derlenemedi" }
  New-Item -ItemType Directory -Force "$out\$sub" | Out-Null
  Get-ChildItem $b -Recurse -Include "$target.exe", '*.dll' | Where-Object { $_.FullName -notmatch 'CMakeFiles' } |
    ForEach-Object { Copy-Item $_.FullName "$out\$sub" -Force }
}
New-Item -ItemType Directory -Force $Src | Out-Null
Build 'whisper.cpp' 'https://github.com/ggml-org/whisper.cpp' 'whisper-server' 'whisper' @('-DWHISPER_BUILD_TESTS=OFF', '-DWHISPER_BUILD_EXAMPLES=ON')
Build 'llama.cpp' 'https://github.com/ggml-org/llama.cpp' 'llama-server' 'llama' @('-DLLAMA_CURL=OFF', '-DLLAMA_OPENSSL=OFF', '-DLLAMA_BUILD_TESTS=OFF', '-DLLAMA_BUILD_EXAMPLES=OFF', '-DLLAMA_BUILD_SERVER=ON')
Write-Host "tamam: $out"
