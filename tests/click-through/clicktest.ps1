# Overlay tıklama geçirme entegrasyon testi (Windows; önce cargo build --release, runtime/ hazır olmalı). Kullanım: clicktest.ps1 -Json '{"click_through":false}' -Label baseline
param([string]$Json = '', [string]$Label = 'test')
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public class CT {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern void mouse_event(uint f,int x,int y,uint d,UIntPtr e);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  public struct R { public int L,T,Rr,B; }
  public struct POINT { public int X, Y; }
}
"@
[CT]::SetProcessDPIAware() | Out-Null
$app = (Resolve-Path "$PSScriptRoot\..\..\app").Path
$proto = $PSScriptRoot
$cfg = "$env:APPDATA\com.echosub.app"
Get-Process echosub, whisper-server, llama-server -ErrorAction SilentlyContinue | Stop-Process -Force
Get-Process powershell -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowTitle -eq '' -and $_.Id -ne $PID } | Out-Null
New-Item -ItemType Directory -Force $cfg | Out-Null
if ($Json -and (Test-Path $Json)) { Copy-Item $Json "$cfg\settings.json" -Force } elseif ($Json) { $Json | Set-Content "$cfg\settings.json" -Encoding ascii } else { Remove-Item "$cfg\settings.json" -ErrorAction SilentlyContinue }
Remove-Item "$app\runtime\logs" -Recurse -Force -ErrorAction SilentlyContinue
$env:ECHOSUB_HOME = "$app\runtime"
Start-Process "$app\target\release\echosub.exe"
foreach ($i in 1..60) { Start-Sleep 1; if ((Get-Content "$app\runtime\logs\app.log" -Encoding UTF8 -ErrorAction SilentlyContinue | Select-String 'durum ready|durum (warn|error)')) { break } }
Start-Sleep 1
function Rect { $p = Get-Process echosub; $r = New-Object CT+R; [CT]::GetWindowRect($p.MainWindowHandle, [ref]$r) | Out-Null; $r }
function MoveTo($x, $y) { [System.Windows.Forms.Cursor]::Position = New-Object System.Drawing.Point ([int]$x), ([int]$y) }
function Down { [CT]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero) }
function Up { [CT]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero) }
function Click($x, $y) { MoveTo $x $y; Start-Sleep -Milliseconds 350; Down; Start-Sleep -Milliseconds 80; Up; Start-Sleep -Milliseconds 300 }
Add-Type -Name S -Namespace W2 -MemberDefinition '[System.Runtime.InteropServices.DllImport("user32.dll", EntryPoint="GetWindowLongPtrW")] public static extern IntPtr GetWindowLongPtr(IntPtr h, int i);'
function ExStyle { $p = Get-Process echosub; $v = [W2.S]::GetWindowLongPtr($p.MainWindowHandle, -20).ToInt64(); "exstyle=0x{0:x} TRANSPARENT={1} LAYERED={2}" -f $v, [bool]($v -band 0x20), [bool]($v -band 0x80000) }
function Owner($x, $y) { $pt = New-Object CT+POINT; $pt.X = [int]$x; $pt.Y = [int]$y; $h = [CT]::WindowFromPoint($pt); $pid2 = 0; [CT]::GetWindowThreadProcessId($h, [ref]$pid2) | Out-Null; (Get-Process -Id $pid2 -ErrorAction SilentlyContinue).ProcessName }

$r = Rect
$w = $r.Rr - $r.L; $h = $r.B - $r.T
"[$Label] overlay: $($r.L),$($r.T) ${w}x${h}"

# Hedef pencereyi overlay'in tam altına koy.
$log = "$proto\clicks.log"; Remove-Item $log -ErrorAction SilentlyContinue; New-Item $log -ItemType File | Out-Null
# Diğer pencereler hedefi örtmesin: hepsini küçült (test sonunda geri alınır).
$shell = New-Object -ComObject Shell.Application; $shell.MinimizeAll(); Start-Sleep -Milliseconds 800
$t = Start-Process powershell -ArgumentList '-NoProfile', '-WindowStyle', 'Hidden', '-File', "$proto\ClickTarget.ps1", '-X', $r.L, '-Y', $r.T, '-W', $w, '-H', $h, '-Log', $log -PassThru
Start-Sleep 3

$cx = $r.L + $w / 2; $cy = $r.T + $h / 2 + 20          # metin alanı
$barx = $r.L + 200; $bary = $r.T + 20                   # üst şerit (sürükleme bölgesi)
"[$Label] imleç altındaki pencere sahibi: merkez=$(Owner $cx $cy) şerit=$(Owner $barx $bary)"

# T1: metin alanına tıkla -> hedef pencere almalı (click-through açıkken)
$before = (Get-Content $log).Count
# T0: önce şeritte dur (etkileşimli bölge), sonra hemen metin alanına geç ve tıkla.
foreach ($delay in 120, 250, 400) {
  MoveTo $barx $bary; Start-Sleep -Milliseconds 400
  MoveTo $cx $cy; Start-Sleep -Milliseconds $delay
  $b = (Get-Content $log).Count
  Down; Start-Sleep -Milliseconds 60; Up; Start-Sleep -Milliseconds 250
  "[$Label] T1 şeritten metin alanına geçince $delay ms sonra tıklama alttaki pencereye ulaştı: $([bool]((Get-Content $log).Count - $b))"
}
$t1 = (Get-Content $log).Count - $before

# T2: üst şeride tıkla -> hedef pencere ALMAMALI (overlay'in etkileşimli bölgesi)
$before = (Get-Content $log).Count
MoveTo $barx $bary; Start-Sleep -Milliseconds 400   # imleç gelince etkileşimli olması için bekle
Click $barx $bary
$t2 = (Get-Content $log).Count - $before
"[$Label] T2 şeride tıklama alttaki pencereye ulaştı (olmamalı): $([bool]$t2)"

# T3: şeritten sürükle
$r0 = Rect
MoveTo $barx $bary; Start-Sleep -Milliseconds 400; Down; Start-Sleep -Milliseconds 150
foreach ($i in 1..10) { MoveTo ($barx - 20 * $i) ($bary - 15 * $i); Start-Sleep -Milliseconds 30 }; Up; Start-Sleep -Milliseconds 400
$r1 = Rect
"[$Label] T3 şeritten sürükleme: ($($r0.L),$($r0.T)) -> ($($r1.L),$($r1.T))  taşındı: $(($r1.L -ne $r0.L) -or ($r1.T -ne $r0.T))"

# T4: sağ-alt köşeden boyutlandır
$r2 = Rect; $w2 = $r2.Rr - $r2.L; $h2 = $r2.B - $r2.T
MoveTo ($r2.Rr - 4) ($r2.B - 4); Start-Sleep -Milliseconds 400; Down; Start-Sleep -Milliseconds 150
foreach ($i in 1..10) { MoveTo ($r2.Rr - 4 - 25 * $i) ($r2.B - 4 - 8 * $i); Start-Sleep -Milliseconds 30 }; Up; Start-Sleep -Milliseconds 400
$r3 = Rect
"[$Label] T4 köşeden boyutlandırma: ${w2}x${h2} -> $($r3.Rr - $r3.L)x$($r3.B - $r3.T)"

# T5: metin alanında SÜRÜKLEME (click-through açıkken pencereyi taşımamalı)
$r4 = Rect; $cx4 = $r4.L + ($r4.Rr - $r4.L) / 2; $cy4 = $r4.T + ($r4.B - $r4.T) / 2 + 20
MoveTo $cx4 $cy4; Start-Sleep -Milliseconds 400; Down; Start-Sleep -Milliseconds 150
foreach ($i in 1..8) { MoveTo ($cx4 + 20 * $i) ($cy4 + 5 * $i); Start-Sleep -Milliseconds 30 }; Up; Start-Sleep -Milliseconds 400
$r5 = Rect
"[$Label] T5 metin alanından sürükleme: ($($r4.L),$($r4.T)) -> ($($r5.L),$($r5.T))  taşındı: $(($r5.L -ne $r4.L) -or ($r5.T -ne $r4.T))"

Stop-Process -Id $t.Id -Force -ErrorAction SilentlyContinue
$shell.UndoMinimizeALL()
Get-Process echosub, whisper-server, llama-server -ErrorAction SilentlyContinue | Stop-Process -Force
Remove-Item "$cfg\settings.json" -ErrorAction SilentlyContinue
