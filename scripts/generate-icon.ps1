# Generates the Drumpad application icon as a 256x256 PNG-compressed ICO.
#
# Run from anywhere:
#   pwsh ./scripts/generate-icon.ps1
#
# The output is committed so release builds do not depend on System.Drawing.

[CmdletBinding()]
param(
    [string]$OutputPath = (Join-Path $PSScriptRoot "..\crates\drumpad-app\assets\drumpad.ico")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Drawing

$size = 256
$padding = 26
$gap = 10
$pad = [int](($size - (2 * $padding) - (3 * $gap)) / 4)

$bitmap = [System.Drawing.Bitmap]::new($size, $size)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias

$background = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(255, 18, 16, 15))
$padBrush = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(255, 224, 122, 63))
$highlightBrush = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(255, 245, 195, 145))

try {
    $graphics.Clear([System.Drawing.Color]::FromArgb(255, 18, 16, 15))
    for ($row = 0; $row -lt 4; $row++) {
        for ($column = 0; $column -lt 4; $column++) {
            $x = $padding + ($column * ($pad + $gap))
            $y = $padding + ($row * ($pad + $gap))
            $brush = if (($row + $column) % 5 -eq 0) { $highlightBrush } else { $padBrush }
            $graphics.FillRectangle($brush, $x, $y, $pad, $pad)
        }
    }
}
finally {
    $background.Dispose()
    $padBrush.Dispose()
    $highlightBrush.Dispose()
    $graphics.Dispose()
}

$pngStream = [System.IO.MemoryStream]::new()
try {
    $bitmap.Save($pngStream, [System.Drawing.Imaging.ImageFormat]::Png)
    $pngBytes = $pngStream.ToArray()
}
finally {
    $pngStream.Dispose()
    $bitmap.Dispose()
}

$directory = Split-Path -Parent $OutputPath
New-Item -ItemType Directory -Force -Path $directory | Out-Null

# ICO container: 6-byte ICONDIR, 16-byte ICONDIRENTRY, PNG payload.
$stream = [System.IO.File]::Create($OutputPath)
$writer = [System.IO.BinaryWriter]::new($stream)
try {
    $writer.Write([uint16]0)             # reserved
    $writer.Write([uint16]1)             # type: icon
    $writer.Write([uint16]1)             # image count
    $writer.Write([byte]0)               # width: 0 means 256
    $writer.Write([byte]0)               # height: 0 means 256
    $writer.Write([byte]0)               # palette size
    $writer.Write([byte]0)               # reserved
    $writer.Write([uint16]1)             # color planes
    $writer.Write([uint16]32)            # bits per pixel
    $writer.Write([uint32]$pngBytes.Length)
    $writer.Write([uint32]22)            # offset: 6 + 16
    $writer.Write($pngBytes)
}
finally {
    $writer.Dispose()
    $stream.Dispose()
}

Write-Host "Wrote $OutputPath"
