<#
.SYNOPSIS
Builds the buffer animation, docs/assets/pipeline/buffers-as-a-watershed.gif.

.DESCRIPTION
Compiles Paint.cs, Scene.cs and Render.cs with Add-Type, renders every frame with GDI+,
writes the delta-frame GIF and a hash per frame, then runs verify_animation.py, which
decodes the GIF again and checks every frame against its hash. It needs PowerShell 7 on
Windows and Python 3. It changes only the work folder and the output GIF.

.PARAMETER WorkDir
Where the frame hashes and the PNG previews go. The default is target/buffer-animation
in the repository. That folder is ignored by git.

.PARAMETER Gif
The output GIF. The default is docs/assets/pipeline/buffers-as-a-watershed.gif.

.PARAMETER Report
Print the water levels and the counters of every step, then stop. Use it to tune Scene.cs.

.PARAMETER Snap
Write full-colour PNG snapshots at these times (seconds) to the snaps folder, then stop.

.EXAMPLE
.\scripts\docs\buffer-animation\Build-Animation.ps1
#>
[CmdletBinding()]
param(
    [string]$WorkDir,
    [string]$Gif,
    [switch]$Report,
    [double[]]$Snap
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
if (-not $WorkDir) { $WorkDir = Join-Path $repo 'target\buffer-animation' }
if (-not $Gif) { $Gif = Join-Path $repo 'docs\assets\pipeline\buffers-as-a-watershed.gif' }

Add-Type -AssemblyName System.Drawing
$drawingAssembly = [System.Drawing.Bitmap].Assembly.Location
$sources = @('Paint.cs', 'Scene.cs', 'Render.cs') | ForEach-Object { Join-Path $PSScriptRoot $_ }
Add-Type -Path $sources -IgnoreWarnings -WarningAction SilentlyContinue -ReferencedAssemblies @(
    $drawingAssembly,
    (Join-Path $PSHOME 'System.Private.Windows.Core.dll'),
    (Join-Path $PSHOME 'System.Private.Windows.GdiPlus.dll'),
    'System.Drawing.Primitives', 'System.Runtime.InteropServices', 'System.Console', 'System.Collections',
    'System.Globalization', 'System.Security.Cryptography', 'System.Memory')

if ($Report) { [Watershed]::Report(); return }
if ($Snap) { [Watershed]::Snap((Join-Path $WorkDir 'snaps'), $Snap); return }

New-Item -ItemType Directory -Force (Split-Path $Gif) | Out-Null
[Watershed]::Render($WorkDir, $Gif)

python -I (Join-Path $PSScriptRoot 'verify_animation.py') $Gif (Join-Path $WorkDir 'frames.sha256')
if ($LASTEXITCODE -ne 0) { throw 'verify_animation.py failed' }
