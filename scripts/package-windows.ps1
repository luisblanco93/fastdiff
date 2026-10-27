param(
    [Parameter(Mandatory = $true)][string]$WslHelper,
    [string]$BinaryDir = 'target/release',
    [string]$OutputDir = 'dist'
)
$ErrorActionPreference = 'Stop'
$repoDir = Split-Path -Parent $PSScriptRoot
Push-Location $repoDir
$stage = Join-Path ([IO.Path]::GetTempPath()) ('fastdiff-package-' + [Guid]::NewGuid())
try {
    $cargoText = Get-Content Cargo.toml -Raw
    $workspaceSection = ($cargoText -split '\[workspace.package\]')[1] -split '\[' | Select-Object -First 1
    $versionMatch = [regex]::Match($workspaceSection, '(?m)^version\s*=\s*"([^"]+)"')
    if (!$versionMatch.Success) { throw 'Cannot read workspace version from Cargo.toml' }
    $version = $versionMatch.Groups[1].Value
    $package = "fastdiff-$version-windows-x64"
    $packageDir = Join-Path $stage $package
    New-Item -ItemType Directory -Path (Join-Path $packageDir 'bin'), (Join-Path $packageDir 'wsl') -Force | Out-Null
    foreach ($binary in 'fastdiff.exe', 'fastdiff-gui.exe') {
        $source = Join-Path $BinaryDir $binary
        if (!(Test-Path -LiteralPath $source -PathType Leaf)) { throw "Missing release binary: $source" }
        Copy-Item -LiteralPath $source -Destination (Join-Path $packageDir 'bin')
    }
    if (!(Test-Path -LiteralPath $WslHelper -PathType Leaf)) { throw "Missing paired WSL helper: $WslHelper" }
    Copy-Item -LiteralPath $WslHelper -Destination (Join-Path $packageDir 'wsl/fastdiff')
    New-Item -ItemType Directory -Path (Join-Path $packageDir 'licenses') -Force | Out-Null
    Copy-Item -LiteralPath 'crates/fastdiff-gui/assets/material/LICENSE' -Destination (Join-Path $packageDir 'licenses/material-icons-LICENSE')
    Copy-Item -LiteralPath 'crates/fastdiff-gui/assets/themes/NOTICE' -Destination (Join-Path $packageDir 'licenses/color-themes-NOTICE')
    New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
    $archive = Join-Path $OutputDir "$package.zip"
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $stagedArchive = Join-Path $stage "$package.zip"
    $zip = [IO.Compression.ZipFile]::Open($stagedArchive, [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($file in Get-ChildItem -LiteralPath $packageDir -File -Recurse -Force) {
            $relative = $file.FullName.Substring($packageDir.Length + 1).Replace('\', '/')
            [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $file.FullName, "$package/$relative", [IO.Compression.CompressionLevel]::Optimal) | Out-Null
        }
    } finally { $zip.Dispose() }
    Move-Item -LiteralPath $stagedArchive -Destination $archive -Force
    Write-Output "Created $archive ($((Get-Item -LiteralPath $archive).Length) bytes)"
} finally {
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
    Pop-Location
}
