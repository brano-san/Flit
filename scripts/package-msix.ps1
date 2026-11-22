[CmdletBinding()]
param(
    [string]$IdentityName = 'Flit',
    [string]$Publisher = 'CN=FlitDev',
    [string]$PublisherDisplayName = 'Flit',
    [string]$BinaryDirectory = (Join-Path $PSScriptRoot '../target/dist'),
    [string]$OutputPath,
    [string]$MakeAppx
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$versionLine = Select-String -LiteralPath (Join-Path $repo 'Cargo.toml') -Pattern '^version = "(\d+\.\d+\.\d+)"$' | Select-Object -First 1
if (!$versionLine) { throw 'The workspace does not declare a numeric release version.' }
$version = $versionLine.Matches[0].Groups[1].Value
if (!$OutputPath) { $OutputPath = Join-Path $repo "target/packages/flit-$version-x86_64-windows.msix" }
$output = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputPath)
if (!$MakeAppx) {
    $tool = Get-Command MakeAppx.exe -ErrorAction SilentlyContinue
    if ($tool) {
        $MakeAppx = $tool.Source
    } else {
        $sdk = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/bin'
        $MakeAppx = Get-ChildItem -LiteralPath $sdk -Directory -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -match '^\d+(\.\d+){3}$' -and (Test-Path -LiteralPath (Join-Path $_.FullName 'x64/MakeAppx.exe')) } |
            Sort-Object { [version]$_.Name } -Descending |
            Select-Object -First 1 |
            ForEach-Object { Join-Path $_.FullName 'x64/MakeAppx.exe' }
    }
}
if (!$MakeAppx) { throw 'MakeAppx.exe was not found. Install the Windows SDK or pass -MakeAppx.' }

$directory = Split-Path -Parent $output
New-Item -ItemType Directory -Path $directory -Force | Out-Null
$stage = Join-Path $directory ('.flit-msix-' + [guid]::NewGuid().ToString('N'))
$images = Join-Path $stage 'Images'
New-Item -ItemType Directory -Path $images | Out-Null
try {
    foreach ($exe in @('flit.exe', 'flit-indexd.exe')) {
        Copy-Item -LiteralPath (Join-Path $BinaryDirectory $exe) -Destination (Join-Path $stage $exe)
    }
    Copy-Item -LiteralPath (Join-Path $repo 'LICENSE') -Destination $stage
    Add-Type -AssemblyName System.Drawing
    $source = [System.Drawing.Image]::FromFile((Join-Path $repo 'assets/icon/hicolor/256x256/apps/flit.png'))
    try {
        foreach ($logo in @(@('StoreLogo', 50), @('Square44x44Logo', 44), @('Square150x150Logo', 150))) {
            $bitmap = [System.Drawing.Bitmap]::new($source, [int]$logo[1], [int]$logo[1])
            try {
                $bitmap.Save((Join-Path $images "$($logo[0]).png"), [System.Drawing.Imaging.ImageFormat]::Png)
            } finally {
                $bitmap.Dispose()
            }
        }
    } finally {
        $source.Dispose()
    }
    [xml]$manifest = Get-Content -Raw -LiteralPath (Join-Path $repo 'packaging/windows/AppxManifest.xml')
    $identity = $manifest.Package.Identity
    $identity.SetAttribute('Name', $IdentityName)
    $identity.SetAttribute('Publisher', $Publisher)
    $identity.SetAttribute('Version', "$version.0")
    $manifest.Package.Properties.PublisherDisplayName = $PublisherDisplayName
    $manifest.Save((Join-Path $stage 'AppxManifest.xml'))
    & $MakeAppx pack /d $stage /p $output /o
    if ($LASTEXITCODE -ne 0) { throw "MakeAppx pack failed with exit code $LASTEXITCODE." }
} finally {
    $resolved = (Resolve-Path -LiteralPath $stage).ProviderPath
    if ($resolved -eq $stage -and (Split-Path -Parent $resolved) -eq $directory) {
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}

