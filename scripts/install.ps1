# SafeHell installer for Windows.
#
#   irm https://raw.githubusercontent.com/suiflex/SafeHell/develop/scripts/install.ps1 | iex
#
# Environment overrides:
#   SAFEHELL_VERSION      release tag to install (default: latest)
#   SAFEHELL_INSTALL_DIR  destination directory (default: %LOCALAPPDATA%\Programs\SafeHell\bin)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
# Windows PowerShell 5.1 still negotiates SSL3/TLS1.0 by default, which GitHub refuses.
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = "suiflex/SafeHell"

function Die($message) {
    Write-Error "safehell: $message"
    exit 1
}

# Maps the machine architecture to a release asset name. Every supported value
# must match an asset produced by .github/workflows/release-build.yml.
function Get-Asset {
    # On a 64-bit host running 32-bit PowerShell, PROCESSOR_ARCHITECTURE reports
    # the emulated x86 and PROCESSOR_ARCHITEW6432 holds the real one.
    $machine = $env:PROCESSOR_ARCHITEW6432
    if (-not $machine) { $machine = $env:PROCESSOR_ARCHITECTURE }
    switch ($machine) {
        "AMD64" { return "safehell-windows-x86_64" }
        "ARM64" { return "safehell-windows-aarch64" }
        default { Die "unsupported architecture: $machine" }
    }
}

# Resolves the latest tag by following the /releases/latest redirect, so no
# response body has to be parsed.
function Get-LatestVersion {
    try {
        $response = Invoke-WebRequest -Uri "https://github.com/$Repo/releases/latest" `
            -MaximumRedirection 0 -ErrorAction SilentlyContinue -UseBasicParsing
        $location = $response.Headers.Location
    } catch {
        $location = $_.Exception.Response.Headers.Location
    }
    if (-not $location) { Die "could not reach GitHub to resolve the latest release" }
    $tag = ([string]$location).Split("/")[-1]
    if (-not $tag -or $tag -eq "releases") { Die "no published release found for $Repo" }
    return $tag
}

$asset = Get-Asset
$version = if ($env:SAFEHELL_VERSION) { $env:SAFEHELL_VERSION } else { Get-LatestVersion }
$installDir = if ($env:SAFEHELL_INSTALL_DIR) {
    $env:SAFEHELL_INSTALL_DIR
} else {
    Join-Path $env:LOCALAPPDATA "Programs\SafeHell\bin"
}
$base = "https://github.com/$Repo/releases/download/$version"

$scratch = Join-Path ([IO.Path]::GetTempPath()) ("safehell-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $scratch -Force | Out-Null
try {
    $archive = "$asset.zip"
    $archivePath = Join-Path $scratch $archive
    $sumsPath = Join-Path $scratch "SHA256SUMS"

    Write-Host "Downloading $archive $version"
    try {
        Invoke-WebRequest -Uri "$base/$archive" -OutFile $archivePath -UseBasicParsing
    } catch {
        Die "no asset '$archive' in release $version; see https://github.com/$Repo/releases"
    }
    try {
        Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile $sumsPath -UseBasicParsing
    } catch {
        Die "release $version has no SHA256SUMS; refusing to install an unverified binary"
    }

    # The archive is unpacked and then executed, so verify it first. This must
    # not be a weaker path to the same binary than the POSIX installer.
    $entry = Get-Content $sumsPath | Where-Object { $_ -match "[ *]$([regex]::Escape($archive))$" }
    if (-not $entry) { Die "SHA256SUMS has no entry for $archive" }
    $expected = ($entry -split '\s+')[0]
    $actual = (Get-FileHash -Path $archivePath -Algorithm SHA256).Hash.ToLower()
    if ($actual -ne $expected.ToLower()) {
        Die "checksum mismatch for $archive (expected $expected, got $actual)"
    }

    Expand-Archive -Path $archivePath -DestinationPath $scratch -Force
    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
    $target = Join-Path $installDir "safehell.exe"
    Copy-Item -Path (Join-Path $scratch "safehell.exe") -Destination $target -Force
    # Clears the mark-of-the-web the download leaves behind, which would
    # otherwise prompt on every run. Not fatal if the provider is unavailable.
    try { Unblock-File -Path $target } catch {}

    Write-Host "Installed safehell $version to $target"

    $paths = $env:PATH -split ";" | ForEach-Object { $_.TrimEnd("\") }
    if ($paths -notcontains $installDir.TrimEnd("\")) {
        Write-Host ""
        Write-Host "$installDir is not on your PATH. Add it, then run: safehell setup"
    }
} finally {
    Remove-Item -Recurse -Force $scratch -ErrorAction SilentlyContinue
}
