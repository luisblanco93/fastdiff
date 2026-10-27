$ErrorActionPreference = 'Stop'
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('fastdiff-package-test-' + [Guid]::NewGuid())
try {
    New-Item -ItemType Directory -Path "$fixture/scripts", "$fixture/binaries", "$fixture/docs", "$fixture/integrations", "$fixture/crates/fastdiff-gui/assets/material", "$fixture/crates/fastdiff-gui/assets/themes" -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'package-windows.ps1') -Destination "$fixture/scripts/package-windows.ps1"
    Set-Content -LiteralPath "$fixture/Cargo.toml" -Value "[workspace.package]`nversion = `"0.1.0`""
    foreach ($file in 'README.md', 'Cargo.lock', 'docs/usage.md', 'LICENSE-MIT', 'LICENSE-APACHE', 'binaries/fastdiff.exe', 'binaries/fastdiff-gui.exe', 'helper') {
        Set-Content -LiteralPath (Join-Path $fixture $file) -Value 'fixture'
    }
    $licenseContent = "Material icons license fixture`n"
    [IO.File]::WriteAllText("$fixture/crates/fastdiff-gui/assets/material/LICENSE", $licenseContent)
    [IO.File]::WriteAllText("$fixture/crates/fastdiff-gui/assets/themes/NOTICE", "Color themes notice fixture`n")
    Set-Content -LiteralPath "$fixture/integrations/.manifest" -Value 'repository-only hidden file'
    [IO.File]::SetAttributes("$fixture/integrations/.manifest", [IO.FileAttributes]::Hidden)
    & "$fixture/scripts/package-windows.ps1" -WslHelper "$fixture/helper" -BinaryDir "$fixture/binaries" -OutputDir "$fixture/dist"
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead("$fixture/dist/fastdiff-0.1.0-windows-x64.zip")
    try {
        foreach ($entry in $zip.Entries) {
            if ($entry.FullName.Contains('\')) { throw "Nonportable ZIP path: $($entry.FullName)" }
        }
        $prefix = 'fastdiff-0.1.0-windows-x64'
        $expected = 'bin/fastdiff.exe', 'bin/fastdiff-gui.exe', 'wsl/fastdiff', 'licenses/material-icons-LICENSE', 'licenses/color-themes-NOTICE'
        foreach ($name in $expected) {
            if (!$zip.GetEntry("$prefix/$name")) { throw "Missing ZIP entry: $name" }
        }
        if ($zip.Entries.Count -ne $expected.Count) { throw 'Windows ZIP contains unexpected files' }
        $reader = [IO.StreamReader]::new($zip.GetEntry("$prefix/licenses/material-icons-LICENSE").Open())
        try {
            if ($reader.ReadToEnd() -cne $licenseContent) { throw 'Material icons license contents differ' }
        } finally { $reader.Dispose() }
    } finally { $zip.Dispose() }
    Write-Output 'Windows package paths, exact contents, and Material icons license passed.'
} finally {
    if (Test-Path -LiteralPath $fixture) { Remove-Item -LiteralPath $fixture -Recurse -Force }
}
