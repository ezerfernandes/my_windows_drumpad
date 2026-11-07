# Builds the Drumpad portable ZIP for Windows 11 x64.
#
# Usage:
#   pwsh ./scripts/package-portable.ps1              # build + package
#   pwsh ./scripts/package-portable.ps1 -SkipBuild   # package an existing release build
#
# Output:
#   dist\Drumpad-<version>-windows-x64-portable.zip
#   dist\Drumpad-<version>-windows-x64-portable.zip.sha256

[CmdletBinding()]
param(
    [string]$Configuration = "Release",
    [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Push-Location $repoRoot
try {
    $metadataJson = & cargo metadata --format-version 1 --locked
    if ($LASTEXITCODE -ne 0) {
        throw "cargo metadata failed with exit code $LASTEXITCODE"
    }
    $metadata = $metadataJson | ConvertFrom-Json

    $app = $metadata.packages |
        Where-Object { $_.name -eq "drumpad-app" } |
        Select-Object -First 1
    if ($null -eq $app) {
        throw "drumpad-app package not found in the workspace"
    }
    $version = $app.version

    $targetDirectory = Join-Path $repoRoot ("target\" + $Configuration.ToLowerInvariant())
    $executable = Join-Path $targetDirectory "drumpad-app.exe"

    if (-not $SkipBuild) {
        & cargo build -p drumpad-app --release --locked
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build failed with exit code $LASTEXITCODE"
        }
    }

    if (-not (Test-Path -LiteralPath $executable)) {
        throw "missing release executable: $executable"
    }

    $notices = Join-Path $repoRoot "docs\release\THIRD_PARTY_NOTICES.md"
    & (Join-Path $PSScriptRoot "generate-third-party-notices.ps1") -OutputPath $notices

    $stageName = "Drumpad-$version-windows-x64"
    $distDirectory = Join-Path $repoRoot "dist"
    $stageDirectory = Join-Path $distDirectory $stageName
    if (Test-Path -LiteralPath $stageDirectory) {
        Remove-Item -LiteralPath $stageDirectory -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $stageDirectory | Out-Null

    Copy-Item -LiteralPath $executable -Destination (Join-Path $stageDirectory "Drumpad.exe")
    Copy-Item -LiteralPath (Join-Path $repoRoot "README.md") -Destination $stageDirectory
    Copy-Item -LiteralPath $notices -Destination $stageDirectory

    # The marker makes the unzipped folder self-contained: all data is written
    # to DrumpadData\ beside the executable.
    Set-Content -LiteralPath (Join-Path $stageDirectory "drumpad.portable") -Value "" -NoNewline

    $archive = Join-Path $distDirectory "$stageName-portable.zip"
    if (Test-Path -LiteralPath $archive) {
        Remove-Item -LiteralPath $archive -Force
    }
    Compress-Archive -Path (Join-Path $stageDirectory "*") -DestinationPath $archive -CompressionLevel Optimal

    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
    $hashFile = "$archive.sha256"
    Set-Content -LiteralPath $hashFile -Value "$hash  $(Split-Path -Leaf $archive)" -Encoding ascii

    Write-Host "Created $archive"
    Write-Host "SHA256 $hash"
}
finally {
    Pop-Location
}
