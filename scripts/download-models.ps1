# Modelleri runtime\models altına indirir. Kullanım: .\download-models.ps1 [-Quality fast|balanced|quality]
param([ValidateSet('fast', 'balanced', 'quality')][string]$Quality = 'balanced')
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$root = (Resolve-Path "$PSScriptRoot\..").Path
$home_ = if ($env:ECHOSUB_HOME) { $env:ECHOSUB_HOME } elseif (Test-Path "$root\app") { "$root\app\runtime" } else { "$root\runtime" }
$dir = "$home_\models"; New-Item -ItemType Directory -Force $dir | Out-Null
function Get-Model([string]$url, [string]$name) {
  if ((Test-Path "$dir\$name") -and (Get-Item "$dir\$name").Length -gt 0) { Write-Host "var: $name"; return }
  Write-Host "indiriliyor: $name"
  Invoke-WebRequest -Uri $url -OutFile "$dir\$name.part" -UseBasicParsing
  Move-Item "$dir\$name.part" "$dir\$name" -Force
}
$hf = 'https://huggingface.co'
Get-Model "$hf/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin" 'ggml-silero-v5.1.2.bin'
Get-Model 'https://github.com/TRvlvr/model_repo/releases/download/all_public_uvr_models/UVR-MDX-NET-Voc_FT.onnx' 'UVR-MDX-NET-Voc_FT.onnx'
if ($Quality -eq 'fast') { Get-Model "$hf/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin" 'ggml-small-q5_1.bin' }
else { Get-Model "$hf/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin" 'ggml-large-v3-turbo-q5_0.bin' }
if ($Quality -eq 'quality') { Get-Model "$hf/bullerwins/translategemma-12b-it-GGUF/resolve/main/translategemma-12b-it-Q4_K_M.gguf" 'translategemma-12b-it-Q4_K_M.gguf' }
else { Get-Model "$hf/bullerwins/translategemma-4b-it-GGUF/resolve/main/translategemma-4b-it-Q4_K_M.gguf" 'translategemma-4b-it-Q4_K_M.gguf' }
Write-Host "tamam: $dir"
