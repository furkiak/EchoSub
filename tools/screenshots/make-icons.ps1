# docs/images/logo.png -> app/icons/icon.png + icon.ico (PNG katmanlı çok boyutlu ICO)
Add-Type -AssemblyName System.Drawing
$root = (Resolve-Path "$PSScriptRoot\..\..").Path
$src = [System.Drawing.Image]::FromFile("$root\docs\images\logo.png")
function Resized([int]$n) {
  $bmp = New-Object System.Drawing.Bitmap $n, $n
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.InterpolationMode = 'HighQualityBicubic'; $g.SmoothingMode = 'HighQuality'; $g.PixelOffsetMode = 'HighQuality'
  $g.DrawImage($src, 0, 0, $n, $n); $g.Dispose(); $bmp
}
$512 = Resized 512; $512.Save("$root\app\icons\icon.png", [System.Drawing.Imaging.ImageFormat]::Png)
$sizes = 16, 32, 48, 64, 128, 256
$pngs = foreach ($s in $sizes) { $ms = New-Object IO.MemoryStream; (Resized $s).Save($ms, [System.Drawing.Imaging.ImageFormat]::Png); ,$ms.ToArray() }
$out = New-Object IO.MemoryStream; $w = New-Object IO.BinaryWriter $out
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$off = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
  $s = $sizes[$i]; $b = if ($s -ge 256) { 0 } else { $s }
  $w.Write([byte]$b); $w.Write([byte]$b); $w.Write([byte]0); $w.Write([byte]0)
  $w.Write([uint16]1); $w.Write([uint16]32); $w.Write([uint32]$pngs[$i].Length); $w.Write([uint32]$off)
  $off += $pngs[$i].Length
}
foreach ($p in $pngs) { $w.Write($p) }
[IO.File]::WriteAllBytes("$root\app\icons\icon.ico", $out.ToArray())
$src.Dispose(); Write-Host 'ikonlar yazıldı'
