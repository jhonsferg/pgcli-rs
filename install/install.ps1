#!/usr/bin/env pwsh
# pgcli (PostgreSQL CLI client) - Windows installer
#
# Usage (one-liner):
#   irm https://raw.githubusercontent.com/jhonsferg/pgcli-rs/main/install/install.ps1 | iex
#
# Customise with environment variables before piping:
#   $env:PGCLI_INSTALL_DIR = "$env:USERPROFILE\.local\bin"
#   $env:PGCLI_VERSION     = "v1.1.3"
#
# Variables:
#   PGCLI_VERSION         release tag to install (default: latest)
#   PGCLI_INSTALL_DIR     install directory (default: %USERPROFILE%\.local\bin)
#   PGCLI_REPO            owner/repo to download from (default: jhonsferg/pgcli-rs)
#   PGCLI_TEST_API_BASE   replaces https://api.github.com (testing only)
#   PGCLI_TEST_DL_BASE    replaces https://github.com (testing only)
#   PGCLI_TEST_ALLOW_HTTP=1  permits http:// test bases (testing only)
#
# The checksum in checksums.txt is mandatory: if it is missing or does not
# list the archive, nothing is installed.
#
# This script never needs administrator rights.

$ErrorActionPreference = "Stop"

# Older PowerShell 5.1 / .NET Framework hosts default to TLS 1.0/1.1, which
# GitHub rejects. Force TLS 1.2 (and 1.3 where the host knows it) up front.
try {
    [Net.ServicePointManager]::SecurityProtocol = `
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch {}
try {
    [Net.ServicePointManager]::SecurityProtocol = `
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls13
} catch {}

$Repo       = if ($env:PGCLI_REPO)           { $env:PGCLI_REPO }           else { "jhonsferg/pgcli-rs" }
$InstallDir = if ($env:PGCLI_INSTALL_DIR)    { $env:PGCLI_INSTALL_DIR }    else { "$env:USERPROFILE\.local\bin" }
$Version    = if ($env:PGCLI_VERSION)        { $env:PGCLI_VERSION }        else { "latest" }
$ApiBase    = if ($env:PGCLI_TEST_API_BASE)  { $env:PGCLI_TEST_API_BASE }  else { "https://api.github.com" }
$DlBase     = if ($env:PGCLI_TEST_DL_BASE)   { $env:PGCLI_TEST_DL_BASE }   else { "https://github.com" }
$AllowHttp  = ($env:PGCLI_TEST_ALLOW_HTTP -eq "1")

# -- Terminal helpers ----------------------------------------------------------
# Abort throws instead of calling `exit`: this script is usually run through
# `iex`, and `exit` would close the user's whole PowerShell session.
function Write-Step([string]$msg) { Write-Host "  -> $msg" -ForegroundColor Cyan }
function Write-Ok([string]$msg)   { Write-Host "  v  $msg" -ForegroundColor Green }
function Write-Warn([string]$msg) { Write-Host "  !  $msg" -ForegroundColor Yellow }
function Abort([string]$msg) {
    Write-Host "`n  x  $msg" -ForegroundColor Red
    throw "pgcli install aborted: $msg"
}

# Extracts the HTTP status code from a web-request error, if any. Works
# across both the WebException thrown by Windows PowerShell 5.1 and the
# HttpResponseException thrown by PowerShell 7+.
function Get-HttpStatusCode($ErrorRecord) {
    try {
        if ($ErrorRecord.Exception.Response) {
            return [int]$ErrorRecord.Exception.Response.StatusCode
        }
    }
    catch {}
    return $null
}

# Downloads $Uri to $OutFile, retrying transient failures with a short
# exponential back-off instead of aborting on the first hiccup.
function Invoke-DownloadWithRetry([string]$Uri, [string]$OutFile, [int]$Retries = 3, [int]$TimeoutSec = 300) {
    $attempt = 0
    while ($true) {
        try {
            $ProgressPreference = "SilentlyContinue"
            Invoke-WebRequest -Uri $Uri -OutFile $OutFile -UseBasicParsing -TimeoutSec $TimeoutSec
            return
        }
        catch {
            $attempt++
            if ($attempt -gt $Retries) { throw }
            Write-Step "Download failed, retrying ($attempt/$Retries)..."
            Start-Sleep -Seconds ([Math]::Pow(2, $attempt))
        }
    }
}

# Returns $true only for https:// URLs, or http:// when PGCLI_TEST_ALLOW_HTTP=1.
function Test-AllowedBase([string]$Base) {
    if ($Base -like "https://*") { return $true }
    if ($Base -like "http://*" -and $AllowHttp) { return $true }
    return $false
}

Write-Host ""
Write-Host "  pgcli" -ForegroundColor Cyan -NoNewline
Write-Host " -- PostgreSQL CLI client installer" -ForegroundColor White
Write-Host ""

# -- 1. Validate inputs --------------------------------------------------------
if (-not (Test-AllowedBase $ApiBase)) {
    Abort "PGCLI_TEST_API_BASE must use https:// (http:// needs PGCLI_TEST_ALLOW_HTTP=1)."
}
if (-not (Test-AllowedBase $DlBase)) {
    Abort "PGCLI_TEST_DL_BASE must use https:// (http:// needs PGCLI_TEST_ALLOW_HTTP=1)."
}
if ($ApiBase -ne "https://api.github.com" -or $DlBase -ne "https://github.com") {
    Write-Warn "Using test endpoints: API=$ApiBase DL=$DlBase"
}

# -- 2. Detect architecture ----------------------------------------------------
# OSArchitecture (not ProcessArchitecture) so this reports the real hardware
# even when PowerShell runs under x64 emulation on Windows ARM64.
$isArm = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture `
         -eq [System.Runtime.InteropServices.Architecture]::Arm64

if (-not [System.Environment]::Is64BitOperatingSystem) {
    Abort "32-bit Windows is not supported."
}

$Arch = if ($isArm) { "arm64" } else { "x86_64" }
Write-Step "Detected platform: windows-$Arch"

# -- 3. Resolve version --------------------------------------------------------
if ($Version -eq "latest") {
    Write-Step "Fetching latest release from $ApiBase..."
    try {
        $rel     = Invoke-RestMethod "$ApiBase/repos/$Repo/releases/latest" -TimeoutSec 30
        $Version = $rel.tag_name
    }
    catch {
        $status = Get-HttpStatusCode $_
        if ($status -eq 403) {
            Abort "GitHub API rate limit exceeded (HTTP 403). Try again later, or set `$env:PGCLI_VERSION explicitly."
        }
        elseif ($status) {
            Abort "Could not fetch latest version (HTTP $status): $_"
        }
        else {
            Abort "Could not fetch latest version: $_"
        }
    }
}

if ($Version -notlike "v*") { $Version = "v$Version" }
if ($Version -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$') {
    Abort "Invalid version '$Version'. Expected a tag such as v1.1.3."
}

Write-Step "Installing pgcli $Version"

# -- 4. Fetch and parse checksums.txt (mandatory) -----------------------------
# Checksums are fetched first: if they are missing, nothing is downloaded or
# installed.
$ArchiveName  = "pgcli-rs_windows_$Arch.zip"
$DownloadUrl  = "$DlBase/$Repo/releases/download/$Version/$ArchiveName"
$ChecksumsUrl = "$DlBase/$Repo/releases/download/$Version/checksums.txt"

$StagingDir   = Join-Path ([System.IO.Path]::GetTempPath()) "pgcli-install-$([System.Guid]::NewGuid())"
New-Item -ItemType Directory -Force -Path $StagingDir | Out-Null
$TmpChecksums = Join-Path $StagingDir "checksums.txt"
$TmpZip       = Join-Path $StagingDir $ArchiveName

try {
    Write-Step "Fetching checksums.txt..."
    try {
        Invoke-DownloadWithRetry -Uri $ChecksumsUrl -OutFile $TmpChecksums -TimeoutSec 30
    }
    catch {
        Abort "checksums.txt is not available for $Version; refusing to install without verification.`n  URL: $ChecksumsUrl`n  Error: $_"
    }

    $entries = @()
    foreach ($line in Get-Content -Path $TmpChecksums) {
        $parts = $line.Trim() -split '\s+'
        if ($parts.Count -ge 2 -and $parts[1] -eq $ArchiveName) {
            $entries += $parts[0]
        }
    }
    if ($entries.Count -ne 1) {
        Abort "Expected exactly one checksum entry for $ArchiveName in checksums.txt, found $($entries.Count)."
    }
    $expectedSha = $entries[0].ToLowerInvariant()
    if ($expectedSha -notmatch '^[0-9a-f]{64}$') {
        Abort "Malformed checksum entry for $ArchiveName in checksums.txt."
    }

    # -- 5. Download and verify archive --------------------------------------
    Write-Step "Downloading $ArchiveName from $DownloadUrl..."
    try {
        Invoke-DownloadWithRetry -Uri $DownloadUrl -OutFile $TmpZip
    }
    catch {
        Abort "Download failed.`n  URL: $DownloadUrl`n  Error: $_"
    }

    Write-Step "Verifying checksum..."
    $actualSha = (Get-FileHash -Path $TmpZip -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($expectedSha -ne $actualSha) {
        Abort "Checksum mismatch for $ArchiveName; nothing was installed.`n  expected: $expectedSha`n  got:      $actualSha"
    }
    Write-Ok "Checksum verified"

    # -- 6. Extract and install binary ---------------------------------------
    $ExtractDir = Join-Path $StagingDir "extract"
    Write-Step "Extracting..."
    try {
        Expand-Archive -Path $TmpZip -DestinationPath $ExtractDir -Force
    }
    catch {
        Abort "Extraction failed. The archive may be corrupted: $_"
    }
    $ExtractedExe = Join-Path $ExtractDir "pgcli.exe"
    if (-not (Test-Path $ExtractedExe)) {
        Abort "Archive did not contain pgcli.exe"
    }

    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    }
    $Dest  = Join-Path $InstallDir "pgcli.exe"
    # Copy next to the destination first, then rename over it, so an
    # interrupted install never leaves a half-written binary behind.
    $Stage = Join-Path $InstallDir ".pgcli-install-$([System.Guid]::NewGuid()).exe"
    try {
        Copy-Item -Path $ExtractedExe -Destination $Stage -Force
        Move-Item -Path $Stage -Destination $Dest -Force
    }
    catch {
        if (Test-Path $Stage) { Remove-Item -Force $Stage -ErrorAction SilentlyContinue }
        Abort "Could not replace $Dest. If pgcli is running, close it and retry: $_"
    }
    Write-Ok "Installed to $Dest"

    try {
        $installedVersion = & $Dest --version 2>&1
        Write-Ok "Binary check: $installedVersion"
    }
    catch {
        Abort "Installed binary failed to run: $_"
    }
}
finally {
    if (Test-Path $StagingDir) { Remove-Item -Recurse -Force $StagingDir -ErrorAction SilentlyContinue }
}

# -- 7. Summary ------------------------------------------------------------------
# pgcli is a single binary: it needs no shell integration, profile block or
# registry entry. The installer only reports whether the directory is on PATH.
Write-Host ""
Write-Host "  pgcli $Version installed!" -ForegroundColor Green
Write-Host ""

$pathEntries = $env:Path -split ';' | ForEach-Object { $_.TrimEnd('\') }
if ($pathEntries -notcontains $InstallDir.TrimEnd('\')) {
    Write-Warn "$InstallDir is not in your PATH."
    Write-Host "  Run it as $Dest, or add the directory to your user PATH yourself." -ForegroundColor White
    Write-Host ""
}

Write-Host "  Connect to a database:" -ForegroundColor White
Write-Host "       pgcli postgresql://user@localhost/dbname" -ForegroundColor Cyan
Write-Host ""
Write-Host "  Upgrade later with:" -ForegroundColor White
Write-Host "       pgcli upgrade" -ForegroundColor Cyan
Write-Host ""
