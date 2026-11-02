# Test penceresi: üstüne gelen tıklamaları dosyaya yazar. Overlay'in ALTINA yerleştirilir.
param([int]$X, [int]$Y, [int]$W, [int]$H, [string]$Log)
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -Name D -Namespace W -MemberDefinition '[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool SetProcessDPIAware();'
[W.D]::SetProcessDPIAware() | Out-Null   # fiziksel piksel koordinatları (overlay ile aynı)
$f = New-Object System.Windows.Forms.Form
$f.FormBorderStyle = 'None'
$f.StartPosition = 'Manual'
$f.Location = New-Object System.Drawing.Point $X, $Y
$f.Size = New-Object System.Drawing.Size $W, $H
$f.BackColor = [System.Drawing.Color]::FromArgb(40, 90, 40)
$f.TopMost = $false
$l = New-Object System.Windows.Forms.Label
$l.Text = 'TIKLAMA HEDEFI (overlay altinda)'
$l.ForeColor = 'White'; $l.AutoSize = $true; $l.Location = New-Object System.Drawing.Point 20, 20
$f.Controls.Add($l)
$f.Add_MouseDown({ param($s, $e) Add-Content -Path $Log -Value ("click {0},{1}" -f $e.X, $e.Y) })
$l.Add_MouseDown({ param($s, $e) Add-Content -Path $Log -Value ("click-label") })
$t = New-Object System.Windows.Forms.Timer
$t.Interval = 60000
$t.Add_Tick({ $f.Close() })
$t.Start()
[System.Windows.Forms.Application]::Run($f)
