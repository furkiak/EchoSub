# Dokümantasyon görsellerini üretir (docs/images). Gerekli: Windows + Microsoft Edge.
# Kullanım: tools\screenshots\make-screenshots.ps1
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path "$PSScriptRoot\..\..").Path
$here = $PSScriptRoot
$out = "$root\docs\images"
$edge = @("${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe", "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe") |
  Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $edge) { throw 'Microsoft Edge bulunamadı' }

function Shot([string]$url, [string]$png, [int]$w, [int]$h, [switch]$transparent) {
  $a = @('--headless=new', '--disable-gpu', '--hide-scrollbars', '--force-device-scale-factor=2',
         "--window-size=$w,$h", "--screenshot=$png", '--allow-file-access-from-files',
         '--virtual-time-budget=2500', "--user-data-dir=$env:TEMP\echosub-shot-profile")
  if ($transparent) { $a += '--default-background-color=00000000' }
  Start-Process -FilePath $edge -ArgumentList ($a + $url) -Wait -NoNewWindow
  if (-not (Test-Path $png)) { throw "ekran görüntüsü alınamadı: $png" }
}
function FileUrl([string]$p) { 'file:///' + ($p -replace '\\', '/') }

# --- Gerçek arayüz sayfalarının sahte-Tauri'li kopyaları (git'e girmez) ---
$gen = "$here\_gen"; New-Item -ItemType Directory -Force $gen | Out-Null
foreach ($pair in @(@('index.html', 'overlay.js'), @('settings.html', 'settings.js'))) {
  $html = Get-Content "$root\app\ui\$($pair[0])" -Raw -Encoding UTF8
  $html = $html.Replace("<script src=`"$($pair[1])`"></script>",
    "<script src=`"../mock-tauri.js`"></script><script src=`"../../../app/ui/$($pair[1])`"></script>")
  Set-Content "$gen\$($pair[0])" $html -Encoding UTF8
}

# 1) Logo PNG + uygulama simgeleri
Shot (FileUrl "$root\docs\images\logo.svg") "$out\logo.png" 512 512 -transparent

# 2) Altyazı katmanı sahneleri (video karesi üstünde)
Shot (FileUrl "$here\scene-ja.html") "$out\overlay-ja.png" 1280 720
Shot (FileUrl "$here\scene-ru.html") "$out\overlay-ru.png" 1280 720
Shot (FileUrl "$here\scene-hover.html") "$out\overlay-hover.png" 1280 720

# 3) Ayarlar penceresi (gerçek settings.html)
Shot ((FileUrl "$gen\settings.html") + '?settings=%7B%22hide_after_secs%22%3A6%7D') "$out\settings.png" 520 1030

# 4) Banner ve mimari şema
Shot (FileUrl "$here\banner.html") "$out\banner.png" 1280 640
Shot (FileUrl "$here\architecture.html") "$out\architecture.png" 1280 560

Write-Host 'Görseller hazır:' (Get-ChildItem $out | Select-Object -ExpandProperty Name) -Separator ', '
