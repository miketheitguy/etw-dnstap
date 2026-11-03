<#
.SYNOPSIS
Builds the ETW-to-TCP pipeline animation, docs/assets/pipeline/etw-to-tcp-pipeline.gif.

.DESCRIPTION
Compiles render.cs, renders the frames with GDI+, and runs make_animation.py to
assemble and verify the GIF. It needs PowerShell 7 on Windows and Python 3. It
changes only the two output locations.

.PARAMETER WorkDir
Where the raw frames and the PNG previews go. The default is target/pipeline-animation
in the repository. That folder is ignored by git.

.PARAMETER Gif
The output GIF. The default is docs/assets/pipeline/etw-to-tcp-pipeline.gif.

.EXAMPLE
.\scripts\docs\pipeline-animation\Build-Animation.ps1
#>
[CmdletBinding()]
param(
    [string]$WorkDir,
    [string]$Gif
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
if (-not $WorkDir) { $WorkDir = Join-Path $repo 'target\pipeline-animation' }
if (-not $Gif) { $Gif = Join-Path $repo 'docs\assets\pipeline\etw-to-tcp-pipeline.gif' }

Add-Type -AssemblyName System.Drawing
$drawingAssembly = [System.Drawing.Bitmap].Assembly.Location
Add-Type -Path (Join-Path $PSScriptRoot 'render.cs') -ReferencedAssemblies @(
    $drawingAssembly,
    (Join-Path $PSHOME 'System.Private.Windows.Core.dll'),
    (Join-Path $PSHOME 'System.Private.Windows.GdiPlus.dll'),
    'System.Drawing.Primitives', 'System.Runtime.InteropServices', 'System.Console', 'System.Collections')

[PipelineAnimation]::Render($WorkDir)

New-Item -ItemType Directory -Force (Split-Path $Gif) | Out-Null
python -I (Join-Path $PSScriptRoot 'make_animation.py') (Join-Path $WorkDir 'frames.bin') $Gif
if ($LASTEXITCODE -ne 0) { throw 'make_animation.py failed' }
