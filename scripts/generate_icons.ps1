param(
    # Where the icon images go (several folders allowed). Defaults to Pictures\chickenwm-icons.
    [string[]]$OutDir = @(Join-Path ([Environment]::GetFolderPath('MyPictures')) 'chickenwm-icons')
)

Add-Type -AssemblyName System.Drawing

$targets = $OutDir

foreach ($t in $targets) {
    if (-not (Test-Path $t)) {
        New-Item -ItemType Directory -Force -Path $t | Out-Null
    }
}

function Save-IconImages {
    param(
        [string]$Name,
        [string]$SvgContent,
        [scriptblock]$DrawAction
    )

    $bmp = New-Object System.Drawing.Bitmap 512, 512, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)

    & $DrawAction $g

    $g.Dispose()

    foreach ($dir in $targets) {
        $pngPath = Join-Path $dir "$Name.png"
        $svgPath = Join-Path $dir "$Name.svg"
        $bmp.Save($pngPath, [System.Drawing.Imaging.ImageFormat]::Png)
        [System.IO.File]::WriteAllText($svgPath, $SvgContent)
    }

    $bmp.Dispose()
    Write-Host "Generated: $Name (PNG + SVG)"
}

# Color palette
$cWhite = [System.Drawing.Color]::FromArgb(255, 255, 255, 255)
$cOutline = [System.Drawing.Color]::FromArgb(255, 226, 224, 218)
$cRed = [System.Drawing.Color]::FromArgb(255, 230, 46, 61)     # #E62E3D
$cOrange = [System.Drawing.Color]::FromArgb(255, 245, 124, 32)  # #F57C20
$cEye = [System.Drawing.Color]::FromArgb(255, 24, 24, 24)       # #181818

function Draw-BaseChicken {
    param(
        [System.Drawing.Graphics]$g,
        [bool]$IsRooster = $false
    )

    $bWhite = New-Object System.Drawing.SolidBrush $cWhite
    $pOutline = New-Object System.Drawing.Pen $cOutline, 3
    $bRed = New-Object System.Drawing.SolidBrush $cRed
    $bOrange = New-Object System.Drawing.SolidBrush $cOrange
    $bEye = New-Object System.Drawing.SolidBrush $cEye
    $pWhiteEye = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)

    # 1. Comb (behind head/on top)
    if ($IsRooster) {
        # 4-lobed taller rooster comb
        $g.FillEllipse($bRed, 140, 110, 80, 85)
        $g.FillEllipse($bRed, 195, 75, 90, 105)
        $g.FillEllipse($bRed, 260, 95, 85, 95)
        $g.FillEllipse($bRed, 315, 130, 70, 75)
    } else {
        # 3-lobed classic comb
        $g.FillEllipse($bRed, 155, 130, 82, 85)
        $g.FillEllipse($bRed, 205, 95, 96, 100)
        $g.FillEllipse($bRed, 265, 125, 86, 90)
    }

    # 2. Head & Neck Body Path
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    # Head circle arc
    $path.AddArc(135, 175, 190, 190, 135, 225)
    # Throat down to chest
    $path.AddBezier(320, 310, 335, 360, 345, 420, 340, 460)
    # Bottom bust curve
    $path.AddBezier(340, 460, 240, 480, 160, 480, 120, 460)
    # Back up to head
    $path.AddBezier(120, 460, 115, 380, 120, 300, 145, 235)
    $path.CloseFigure()

    $g.FillPath($bWhite, $path)
    $g.DrawPath($pOutline, $path)

    # 3. Beak (pointing right)
    $beakPath = New-Object System.Drawing.Drawing2D.GraphicsPath
    $pt1 = New-Object System.Drawing.PointF 305, 235
    $pt2 = New-Object System.Drawing.PointF 378, 265
    $pt3 = New-Object System.Drawing.PointF 305, 290
    $beakPath.AddLine($pt1, $pt2)
    $beakPath.AddLine($pt2, $pt3)
    $beakPath.CloseFigure()
    $g.FillPath($bOrange, $beakPath)

    # 4. Wattle (under beak)
    $wattleW = if ($IsRooster) { 84 } else { 74 }
    $wattleH = if ($IsRooster) { 92 } else { 80 }
    $g.FillEllipse($bRed, 280, 280, $wattleW, $wattleH)

    # 5. Eye
    $g.FillEllipse($bEye, 260, 230, 28, 28)
    $g.FillEllipse($pWhiteEye, 266, 234, 8, 8)

    $bWhite.Dispose()
    $pOutline.Dispose()
    $bRed.Dispose()
    $bOrange.Dispose()
    $bEye.Dispose()
    $pWhiteEye.Dispose()
    $path.Dispose()
    $beakPath.Dispose()
}

# Base chicken SVG string builder
function Get-BaseChickenSvg {
    param([bool]$IsRooster = $false, [string]$ExtraSvg = "")
    $comb = if ($IsRooster) {
        '<circle cx="180" cy="152" r="41" fill="#E62E3D"/>' +
        '<circle cx="240" cy="128" r="48" fill="#E62E3D"/>' +
        '<circle cx="302" cy="142" r="43" fill="#E62E3D"/>' +
        '<circle cx="350" cy="168" r="36" fill="#E62E3D"/>'
    } else {
        '<circle cx="196" cy="172" r="41" fill="#E62E3D"/>' +
        '<circle cx="253" cy="145" r="48" fill="#E62E3D"/>' +
        '<circle cx="308" cy="170" r="43" fill="#E62E3D"/>'
    }
    $wattle = if ($IsRooster) {
        '<ellipse cx="322" cy="326" rx="42" ry="46" fill="#E62E3D"/>'
    } else {
        '<ellipse cx="317" cy="320" rx="37" ry="40" fill="#E62E3D"/>'
    }

    @"
<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="0 0 512 512" fill="none">
  <g id="comb">$comb</g>
  <path id="bust" d="M 145,235 C 130,280 135,320 180,365 C 240,410 320,310 320,310 C 335,360 345,420 340,460 C 240,480 160,480 120,460 C 115,380 120,300 145,235 Z" fill="#FFFFFF" stroke="#E2E0D8" stroke-width="4" stroke-linejoin="round"/>
  <ellipse cx="230" cy="270" rx="95" ry="95" fill="#FFFFFF"/>
  <path id="beak" d="M 305,235 L 378,265 L 305,290 Z" fill="#F57C20"/>
  <g id="wattle">$wattle</g>
  <circle id="eye" cx="274" cy="244" r="14" fill="#181818"/>
  <circle id="eye-highlight" cx="278" cy="240" r="4" fill="#FFFFFF"/>
  $ExtraSvg
</svg>
"@
}

# 1. ChickenBrowser (Browser)
Save-IconImages "chickenbrowser" (Get-BaseChickenSvg $false @"
  <g id="browser-compass" opacity="0.95">
    <circle cx="395" cy="385" r="44" fill="#0284C7"/>
    <circle cx="395" cy="385" r="40" fill="#E0F2FE"/>
    <path d="M 395,355 L 405,385 L 395,415 L 385,385 Z" fill="#0284C7"/>
    <path d="M 395,355 L 405,385 L 395,385 Z" fill="#EF4444"/>
    <circle cx="395" cy="385" r="5" fill="#FFFFFF"/>
  </g>
"@) {
    param($g)
    Draw-BaseChicken $g $false
    # Compass Badge
    $bBlue = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 2, 132, 199))
    $bLightBlue = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 224, 242, 254))
    $bRed = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 239, 68, 68))
    $bWhite = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
    $g.FillEllipse($bBlue, 351, 341, 88, 88)
    $g.FillEllipse($bLightBlue, 355, 345, 80, 80)
    $p1 = New-Object System.Drawing.Drawing2D.GraphicsPath
    $p1.AddPolygon(@(
        (New-Object System.Drawing.PointF 395, 355),
        (New-Object System.Drawing.PointF 405, 385),
        (New-Object System.Drawing.PointF 395, 415),
        (New-Object System.Drawing.PointF 385, 385)
    ))
    $g.FillPath($bBlue, $p1)
    $p2 = New-Object System.Drawing.Drawing2D.GraphicsPath
    $p2.AddPolygon(@(
        (New-Object System.Drawing.PointF 395, 355),
        (New-Object System.Drawing.PointF 405, 385),
        (New-Object System.Drawing.PointF 395, 385)
    ))
    $g.FillPath($bRed, $p2)
    $g.FillEllipse($bWhite, 390, 380, 10, 10)
    $bBlue.Dispose(); $bLightBlue.Dispose(); $bRed.Dispose(); $bWhite.Dispose(); $p1.Dispose(); $p2.Dispose()
}

# 2. ChickenWM-Files (Files)
Save-IconImages "chickenwm-files" (Get-BaseChickenSvg $false @"
  <g id="folder-tab">
    <path d="M 110,400 L 195,400 L 220,425 L 350,425 Q 360,425 360,435 L 360,480 Q 360,490 350,490 L 110,490 Q 100,490 100,480 L 100,410 Q 100,400 110,400 Z" fill="#F59E0B" stroke="#D97706" stroke-width="4"/>
    <path d="M 100,430 L 360,430 L 350,490 L 110,490 Z" fill="#FBBF24"/>
  </g>
"@) {
    param($g)
    Draw-BaseChicken $g $false
    $bAmber1 = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 245, 158, 11))
    $bAmber2 = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 251, 191, 36))
    $pBorder = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 217, 119, 6)), 4
    $folderBack = New-Object System.Drawing.Drawing2D.GraphicsPath
    $folderBack.AddPolygon(@(
        (New-Object System.Drawing.PointF 100, 400),
        (New-Object System.Drawing.PointF 195, 400),
        (New-Object System.Drawing.PointF 220, 425),
        (New-Object System.Drawing.PointF 360, 425),
        (New-Object System.Drawing.PointF 360, 480),
        (New-Object System.Drawing.PointF 100, 480)
    ))
    $g.FillPath($bAmber1, $folderBack)
    $g.DrawPath($pBorder, $folderBack)
    $g.FillRectangle($bAmber2, 100, 430, 260, 52)
    $bAmber1.Dispose(); $bAmber2.Dispose(); $pBorder.Dispose(); $folderBack.Dispose()
}

# 3. ChickenWM-Settings (Settings)
Save-IconImages "chickenwm-settings" (Get-BaseChickenSvg $false @"
  <g id="gear-badge">
    <circle cx="395" cy="385" r="48" fill="#475569"/>
    <circle cx="395" cy="385" r="22" fill="#FFFFFF"/>
    <!-- Gear teeth -->
    <rect x="387" y="327" width="16" height="16" rx="4" fill="#475569"/>
    <rect x="387" y="427" width="16" height="16" rx="4" fill="#475569"/>
    <rect x="337" y="377" width="16" height="16" rx="4" fill="#475569"/>
    <rect x="437" y="377" width="16" height="16" rx="4" fill="#475569"/>
    <rect x="352" y="342" width="16" height="16" rx="4" fill="#475569" transform="rotate(45 360 350)"/>
    <rect x="422" y="412" width="16" height="16" rx="4" fill="#475569" transform="rotate(45 430 420)"/>
    <rect x="422" y="342" width="16" height="16" rx="4" fill="#475569" transform="rotate(-45 430 350)"/>
    <rect x="352" y="412" width="16" height="16" rx="4" fill="#475569" transform="rotate(-45 360 420)"/>
  </g>
"@) {
    param($g)
    Draw-BaseChicken $g $false
    $bSlate = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 71, 85, 105))
    $bWhite = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
    $cx = 395; $cy = 385
    for ($angle = 0; $angle -lt 360; $angle += 45) {
        $rad = $angle * [Math]::PI / 180.0
        $tx = $cx + [Math]::Cos($rad) * 44 - 9
        $ty = $cy + [Math]::Sin($rad) * 44 - 9
        $g.FillRectangle($bSlate, [float]$tx, [float]$ty, 18, 18)
    }
    $g.FillEllipse($bSlate, ($cx - 42), ($cy - 42), 84, 84)
    $g.FillEllipse($bWhite, ($cx - 18), ($cy - 18), 36, 36)
    $bSlate.Dispose(); $bWhite.Dispose()
}

# 4. ChickenWM-Launcher (Launcher)
Save-IconImages "chickenwm-launcher" (Get-BaseChickenSvg $false @"
  <g id="search-monocle">
    <circle cx="380" cy="375" r="42" fill="none" stroke="#3B82F6" stroke-width="14"/>
    <line x1="410" y1="405" x2="445" y2="440" stroke="#3B82F6" stroke-width="16" stroke-linecap="round"/>
    <circle cx="380" cy="375" r="34" fill="#DBEAFE" opacity="0.6"/>
  </g>
"@) {
    param($g)
    Draw-BaseChicken $g $false
    $pBlue = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 59, 130, 246)), 14
    $pHandle = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 59, 130, 246)), 16
    $pHandle.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pHandle.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    $bGlass = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(140, 219, 234, 254))
    $g.FillEllipse($bGlass, 338, 333, 84, 84)
    $g.DrawEllipse($pBlue, 338, 333, 84, 84)
    $g.DrawLine($pHandle, 410, 405, 445, 440)
    $pBlue.Dispose(); $pHandle.Dispose(); $bGlass.Dispose()
}

# 5. ChickenWM-WM (Tiling Window Manager)
Save-IconImages "chickenwm-wm" (Get-BaseChickenSvg $false @"
  <g id="tiling-grid">
    <rect x="345" y="335" width="96" height="96" rx="14" fill="#8B5CF6"/>
    <rect x="353" y="343" width="38" height="38" rx="6" fill="#FFFFFF"/>
    <rect x="395" y="343" width="38" height="38" rx="6" fill="#EDE9FE"/>
    <rect x="353" y="385" width="38" height="38" rx="6" fill="#EDE9FE"/>
    <rect x="395" y="385" width="38" height="38" rx="6" fill="#C4B5FD"/>
  </g>
"@) {
    param($g)
    Draw-BaseChicken $g $false
    $bPurple = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 139, 92, 246))
    $bWhite = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
    $bLavender = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 237, 233, 254))
    $bAccent = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 196, 181, 253))
    $g.FillRectangle($bPurple, 345, 335, 96, 96)
    $g.FillRectangle($bWhite, 353, 343, 38, 38)
    $g.FillRectangle($bLavender, 395, 343, 38, 38)
    $g.FillRectangle($bLavender, 353, 385, 38, 38)
    $g.FillRectangle($bAccent, 395, 385, 38, 38)
    $bPurple.Dispose(); $bWhite.Dispose(); $bLavender.Dispose(); $bAccent.Dispose()
}

# 6. Roobar (Status Bar)
Save-IconImages "roobar" (Get-BaseChickenSvg $false @"
  <g id="status-bar-visor">
    <rect x="150" y="210" width="165" height="24" rx="12" fill="#10B981"/>
    <circle cx="168" cy="222" r="5" fill="#FFFFFF"/>
    <circle cx="186" cy="222" r="5" fill="#A7F3D0"/>
    <circle cx="204" cy="222" r="5" fill="#A7F3D0"/>
    <rect x="250" y="217" width="50" height="10" rx="5" fill="#FFFFFF" opacity="0.9"/>
  </g>
"@) {
    param($g)
    Draw-BaseChicken $g $false
    $bGreen = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 16, 185, 129))
    $bWhite = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
    $bMint = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 167, 243, 208))
    $g.FillRectangle($bGreen, 150, 210, 165, 24)
    $g.FillEllipse($bWhite, 163, 217, 10, 10)
    $g.FillEllipse($bMint, 181, 217, 10, 10)
    $g.FillEllipse($bMint, 199, 217, 10, 10)
    $g.FillRectangle($bWhite, 250, 217, 50, 10)
    $bGreen.Dispose(); $bWhite.Dispose(); $bMint.Dispose()
}

# 7. ChickenCTL (Terminal / CLI)
Save-IconImages "chickenctl" (Get-BaseChickenSvg $false @"
  <g id="cli-badge">
    <rect x="345" y="340" width="95" height="85" rx="14" fill="#18181B"/>
    <path d="M 365,365 L 380,380 L 365,395" stroke="#22C55E" stroke-width="8" stroke-linecap="round" stroke-linejoin="round" fill="none"/>
    <line x1="390" y1="395" x2="415" y2="395" stroke="#22C55E" stroke-width="8" stroke-linecap="round"/>
  </g>
"@) {
    param($g)
    Draw-BaseChicken $g $false
    $bDark = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 24, 24, 27))
    $pNeon = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 34, 197, 94)), 8
    $pNeon.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pNeon.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    $g.FillRectangle($bDark, 345, 340, 95, 85)
    $g.DrawLines($pNeon, @(
        (New-Object System.Drawing.PointF 365, 365),
        (New-Object System.Drawing.PointF 380, 380),
        (New-Object System.Drawing.PointF 365, 395)
    ))
    $g.DrawLine($pNeon, 390, 395, 415, 395)
    $bDark.Dispose(); $pNeon.Dispose()
}

# 8. ChickenWM-Notify (Notifications)
Save-IconImages "chickenwm-notify" (Get-BaseChickenSvg $false @"
  <g id="bell-badge">
    <circle cx="395" cy="385" r="46" fill="#EC4899"/>
    <!-- Bell -->
    <path d="M 395,355 C 385,355 377,363 377,373 L 372,395 L 418,395 L 413,373 C 413,363 405,355 395,355 Z" fill="#FFFFFF"/>
    <circle cx="395" cy="403" r="6" fill="#FFFFFF"/>
  </g>
"@) {
    param($g)
    Draw-BaseChicken $g $false
    $bPink = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 236, 72, 153))
    $bWhite = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
    $g.FillEllipse($bPink, 349, 339, 92, 92)
    $bell = New-Object System.Drawing.Drawing2D.GraphicsPath
    $bell.AddBezier(395, 355, 380, 355, 375, 365, 375, 375)
    $bell.AddLine(375, 375, 370, 397)
    $bell.AddLine(370, 397, 420, 397)
    $bell.AddLine(420, 397, 415, 375)
    $bell.AddBezier(415, 375, 415, 365, 410, 355, 395, 355)
    $g.FillPath($bWhite, $bell)
    $g.FillEllipse($bWhite, 390, 399, 10, 10)
    $bPink.Dispose(); $bWhite.Dispose(); $bell.Dispose()
}

# 9. ChickenWM-Paper (Wallpaper Engine)
Save-IconImages "chickenwm-paper" (Get-BaseChickenSvg $false @"
  <g id="palette-badge">
    <circle cx="395" cy="385" r="46" fill="#F97316"/>
    <circle cx="380" cy="370" r="8" fill="#FFFFFF"/>
    <circle cx="410" cy="370" r="8" fill="#FEF08A"/>
    <circle cx="415" cy="395" r="8" fill="#93C5FD"/>
    <circle cx="385" cy="405" r="12" fill="#C2410C"/>
  </g>
"@) {
    param($g)
    Draw-BaseChicken $g $false
    $bOrange = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 249, 115, 22))
    $bWhite = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
    $bYellow = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 254, 240, 138))
    $bBlue = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 147, 197, 253))
    $bHole = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 194, 65, 12))
    $g.FillEllipse($bOrange, 349, 339, 92, 92)
    $g.FillEllipse($bWhite, 372, 362, 16, 16)
    $g.FillEllipse($bYellow, 402, 362, 16, 16)
    $g.FillEllipse($bBlue, 407, 387, 16, 16)
    $g.FillEllipse($bHole, 373, 393, 24, 24)
    $bOrange.Dispose(); $bWhite.Dispose(); $bYellow.Dispose(); $bBlue.Dispose(); $bHole.Dispose()
}

# 10. ChickenWM-Binds (Keybindings)
Save-IconImages "chickenwm-binds" (Get-BaseChickenSvg $false @"
  <g id="keycap-badge">
    <rect x="345" y="340" width="95" height="90" rx="18" fill="#4F46E5"/>
    <rect x="353" y="346" width="79" height="72" rx="14" fill="#6366F1"/>
    <text x="392" y="394" font-family="Segoe UI, monospace, sans-serif" font-size="34" font-weight="bold" fill="#FFFFFF" text-anchor="middle">⌘</text>
  </g>
"@) {
    param($g)
    Draw-BaseChicken $g $false
    $bDarkIndigo = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 79, 70, 229))
    $bIndigo = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 99, 102, 241))
    $bWhite = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
    $font = New-Object System.Drawing.Font "Segoe UI Symbol", 28, ([System.Drawing.FontStyle]::Bold)
    $g.FillRectangle($bDarkIndigo, 345, 340, 95, 90)
    $g.FillRectangle($bIndigo, 353, 346, 79, 72)
    $sf = New-Object System.Drawing.StringFormat
    $sf.Alignment = [System.Drawing.StringAlignment]::Center
    $sf.LineAlignment = [System.Drawing.StringAlignment]::Center
    $rect = New-Object System.Drawing.RectangleF 353, 346, 79, 72
    $g.DrawString([char]0x2318, $font, $bWhite, $rect, $sf)
    $bDarkIndigo.Dispose(); $bIndigo.Dispose(); $bWhite.Dispose(); $font.Dispose(); $sf.Dispose()
}

# 11. ChickenWM-Shot (Screenshot Tool)
Save-IconImages "chickenwm-shot" (Get-BaseChickenSvg $false @"
  <g id="crop-corners" stroke="#06B6D4" stroke-width="12" stroke-linecap="round" fill="none">
    <!-- Top-left -->
    <path d="M 120,170 L 120,120 L 170,120"/>
    <!-- Top-right -->
    <path d="M 370,120 L 420,120 L 420,170"/>
    <!-- Bottom-left -->
    <path d="M 120,410 L 120,460 L 170,460"/>
    <!-- Bottom-right -->
    <path d="M 370,460 L 420,460 L 420,410"/>
  </g>
"@) {
    param($g)
    Draw-BaseChicken $g $false
    $pCyan = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 6, 182, 212)), 12
    $pCyan.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pCyan.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    $g.DrawLines($pCyan, @((New-Object System.Drawing.PointF 120, 170), (New-Object System.Drawing.PointF 120, 120), (New-Object System.Drawing.PointF 170, 120)))
    $g.DrawLines($pCyan, @((New-Object System.Drawing.PointF 370, 120), (New-Object System.Drawing.PointF 420, 120), (New-Object System.Drawing.PointF 420, 170)))
    $g.DrawLines($pCyan, @((New-Object System.Drawing.PointF 120, 410), (New-Object System.Drawing.PointF 120, 460), (New-Object System.Drawing.PointF 170, 460)))
    $g.DrawLines($pCyan, @((New-Object System.Drawing.PointF 370, 460), (New-Object System.Drawing.PointF 420, 460), (New-Object System.Drawing.PointF 420, 410)))
    $pCyan.Dispose()
}

# 12. ChickenWM-Rooster (Supervisor / Watchdog)
Save-IconImages "chickenwm-rooster" (Get-BaseChickenSvg $true @"
  <g id="shield-star">
    <path d="M 395,340 L 440,360 L 440,405 C 440,430 395,450 395,450 C 395,450 350,430 350,405 L 350,360 Z" fill="#EAB308"/>
    <path d="M 395,348 L 432,365 L 432,403 C 432,423 395,440 395,440 C 395,440 358,423 358,403 L 358,365 Z" fill="#FACC15"/>
    <polygon points="395,372 400,387 416,387 403,397 408,412 395,402 382,412 387,397 374,387 390,387" fill="#FFFFFF"/>
  </g>
"@) {
    param($g)
    Draw-BaseChicken $g $true
    $bGold = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 234, 179, 8))
    $bYellow = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 250, 204, 21))
    $bWhite = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
    $shieldPath = New-Object System.Drawing.Drawing2D.GraphicsPath
    $shieldPath.AddPolygon(@(
        (New-Object System.Drawing.PointF 395, 340),
        (New-Object System.Drawing.PointF 440, 360),
        (New-Object System.Drawing.PointF 440, 405),
        (New-Object System.Drawing.PointF 395, 450),
        (New-Object System.Drawing.PointF 350, 405),
        (New-Object System.Drawing.PointF 350, 360)
    ))
    $g.FillPath($bGold, $shieldPath)
    $innerShield = New-Object System.Drawing.Drawing2D.GraphicsPath
    $innerShield.AddPolygon(@(
        (New-Object System.Drawing.PointF 395, 348),
        (New-Object System.Drawing.PointF 432, 365),
        (New-Object System.Drawing.PointF 432, 403),
        (New-Object System.Drawing.PointF 395, 440),
        (New-Object System.Drawing.PointF 358, 403),
        (New-Object System.Drawing.PointF 358, 365)
    ))
    $g.FillPath($bYellow, $innerShield)
    $star = New-Object System.Drawing.Drawing2D.GraphicsPath
    $star.AddPolygon(@(
        (New-Object System.Drawing.PointF 395, 372),
        (New-Object System.Drawing.PointF 400, 387),
        (New-Object System.Drawing.PointF 416, 387),
        (New-Object System.Drawing.PointF 403, 397),
        (New-Object System.Drawing.PointF 408, 412),
        (New-Object System.Drawing.PointF 395, 402),
        (New-Object System.Drawing.PointF 382, 412),
        (New-Object System.Drawing.PointF 387, 397),
        (New-Object System.Drawing.PointF 374, 387),
        (New-Object System.Drawing.PointF 390, 387)
    ))
    $g.FillPath($bWhite, $star)
    $bGold.Dispose(); $bYellow.Dispose(); $bWhite.Dispose(); $shieldPath.Dispose(); $innerShield.Dispose(); $star.Dispose()
}

Write-Host "All icons generated successfully!"
