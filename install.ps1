# install.ps1 — Windows installer for lazyide
# Usage: irm https://tysonlabs.dev/lazyide/install.ps1 | iex
#        .\install.ps1 [-Prefix <path>] [-Version <tag>] [-WithDeps] [-NoDeps]
param(
    [string]$Prefix = "",
    [string]$Version = "",
    [switch]$WithDeps,
    [switch]$NoDeps,
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"
$Repo = "tgeorge06/lazyide"
$GithubApi = "https://api.github.com"
$GithubDl = "https://github.com"

# --- Resolve install directory ---
if ($Prefix) {
    $InstallDir = $Prefix
} elseif ($env:LAZYIDE_INSTALL_DIR) {
    $InstallDir = $env:LAZYIDE_INSTALL_DIR
} else {
    $InstallDir = Join-Path $env:LOCALAPPDATA "lazyide\bin"
}

# --- Helpers ---
function Write-Info  { param($Msg) Write-Host "info " -ForegroundColor Green -NoNewline; Write-Host " $Msg" }
function Write-Warn  { param($Msg) Write-Host "warn " -ForegroundColor Yellow -NoNewline; Write-Host " $Msg" }
function Write-Err   { param($Msg) Write-Host "error" -ForegroundColor Red -NoNewline; Write-Host " $Msg"; exit 1 }

# --- Resolve version ---
function Get-LatestVersion {
    if ($Version) {
        if (-not $Version.StartsWith("v")) { $script:Version = "v$Version" }
        Write-Info "Using specified version: $Version"
        return $Version
    }

    Write-Info "Fetching latest release..."
    $release = Invoke-RestMethod -Uri "$GithubApi/repos/$Repo/releases/latest" -Headers @{ "User-Agent" = "lazyide-installer" }
    $script:Version = $release.tag_name
    Write-Info "Latest version: $Version"
    return $Version
}

# --- Check existing install ---
function Test-Existing {
    $existing = Get-Command lazyide -ErrorAction SilentlyContinue
    if ($existing) {
        $currentVersion = & lazyide --version 2>$null | Select-Object -First 1
        Write-Info "Found existing install: $currentVersion at $($existing.Source)"
        $cleanVersion = $Version -replace '^v', ''
        if ($currentVersion -match [regex]::Escape($cleanVersion)) {
            Write-Info "Already up to date ($Version)"
            if (-not $DryRun) { exit 0 }
        } else {
            Write-Info "Will upgrade to $Version"
        }
    }
}

# --- Verify checksum ---
function Test-Checksum {
    param($TarballPath, $ChecksumsPath)

    $filename = Split-Path $TarballPath -Leaf
    $line = Get-Content $ChecksumsPath | Where-Object { $_ -match $filename } | Select-Object -First 1
    if (-not $line) {
        Write-Warn "No checksum found for $filename, skipping verification"
        return
    }
    $expected = ($line -split '\s+')[0]
    $actual = (Get-FileHash $TarballPath -Algorithm SHA256).Hash.ToLower()

    if ($expected -eq $actual) {
        Write-Info "Checksum verified"
    } else {
        Write-Err "Checksum mismatch!`n  Expected: $expected`n  Got:      $actual`n  The download may be corrupted. Aborting."
    }
}

# --- Install ---
function Install-Lazyide {
    $tarball = "lazyide-windows-x86_64.tar.gz"
    $downloadUrl = "$GithubDl/$Repo/releases/download/$Version/$tarball"
    $checksumsUrl = "$GithubDl/$Repo/releases/download/$Version/checksums.sha256"

    if ($DryRun) {
        Write-Host ""
        Write-Host "Dry run - would perform:" -ForegroundColor White
        Write-Host "  1. Download  $downloadUrl"
        Write-Host "  2. Download  $checksumsUrl"
        Write-Host "  3. Verify    SHA256 checksum"
        Write-Host "  4. Extract   lazyide.exe to $InstallDir\lazyide.exe"
        Write-Host "  5. Health    lazyide --version"
        return
    }

    $tmpDir = Join-Path ([System.IO.Path]::GetTempPath()) "lazyide-install-$(Get-Random)"
    New-Item -ItemType Directory -Path $tmpDir -Force | Out-Null

    try {
        Write-Info "Downloading $tarball..."
        Invoke-WebRequest -Uri $downloadUrl -OutFile (Join-Path $tmpDir $tarball) -UseBasicParsing

        Write-Info "Downloading checksums..."
        try {
            Invoke-WebRequest -Uri $checksumsUrl -OutFile (Join-Path $tmpDir "checksums.sha256") -UseBasicParsing
            Test-Checksum -TarballPath (Join-Path $tmpDir $tarball) -ChecksumsPath (Join-Path $tmpDir "checksums.sha256")
        } catch {
            Write-Warn "checksums.sha256 not found in release, skipping verification"
        }

        Write-Info "Extracting..."
        tar xzf (Join-Path $tmpDir $tarball) -C $tmpDir

        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
        Copy-Item (Join-Path $tmpDir "lazyide.exe") (Join-Path $InstallDir "lazyide.exe") -Force
        Write-Info "Installed to $InstallDir\lazyide.exe"
    } finally {
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
    }
}

# --- PATH check ---
function Update-Path {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath -split ';' | Where-Object { $_ -eq $InstallDir }) {
        return
    }

    Write-Warn "$InstallDir is not in your PATH"
    $answer = Read-Host "Add $InstallDir to your user PATH? [Y/n]"
    if ($answer -eq '' -or $answer -match '^[Yy]') {
        [Environment]::SetEnvironmentVariable("Path", "$userPath;$InstallDir", "User")
        $env:Path = "$env:Path;$InstallDir"
        Write-Info "Added to user PATH (active in new terminals)"
    } else {
        Write-Host ""
        Write-Host "  Add it manually:" -ForegroundColor Yellow
        Write-Host "    `$env:Path += `";$InstallDir`"" -ForegroundColor Cyan
        Write-Host ""
    }
}

# --- Optional dependencies ---
function Install-Dependencies {
    if ($NoDeps) { return }

    $missing = @()
    if (-not (Get-Command rg -ErrorAction SilentlyContinue)) { $missing += "ripgrep" }
    if (-not (Get-Command rust-analyzer -ErrorAction SilentlyContinue)) { $missing += "rust-analyzer" }

    if ($missing.Count -eq 0) { return }

    if (-not $WithDeps) {
        Write-Info "Optional dependencies not found: $($missing -join ', ')"
        $answer = Read-Host "Install them now? [y/N]"
        if ($answer -notmatch '^[Yy]') { return }
    }

    if (Get-Command scoop -ErrorAction SilentlyContinue) {
        foreach ($dep in $missing) {
            Write-Info "Installing $dep via Scoop..."
            scoop install $dep
        }
    } elseif (Get-Command winget -ErrorAction SilentlyContinue) {
        $wingetMap = @{ "ripgrep" = "BurntSushi.ripgrep.MSVC"; "rust-analyzer" = "rust-lang.rust-analyzer" }
        foreach ($dep in $missing) {
            Write-Info "Installing $dep via winget..."
            winget install --id $wingetMap[$dep] --accept-source-agreements --accept-package-agreements
        }
    } else {
        Write-Warn "No package manager found (scoop/winget). Install dependencies manually."
    }
}

# --- Health check ---
function Test-Health {
    if ($DryRun) { return }

    $exe = Join-Path $InstallDir "lazyide.exe"
    if (Test-Path $exe) {
        $ver = & $exe --version 2>$null | Select-Object -First 1
        if ($ver) {
            Write-Info "Verified: $ver"
        } else {
            Write-Warn "Binary installed but --version returned no output"
        }
    } else {
        Write-Warn "Binary not found at $exe after install"
    }
}

# --- Main ---
Write-Host ""
Write-Host "  lazyide installer" -ForegroundColor White
Write-Host ""

Write-Info "Detected: windows x86_64"

Get-LatestVersion | Out-Null
Test-Existing
Install-Lazyide

if (-not $DryRun) {
    Update-Path
    Install-Dependencies
    Test-Health

    Write-Host ""
    Write-Host "  lazyide $Version installed successfully!" -ForegroundColor Green
    Write-Host "  Run " -NoNewline; Write-Host "lazyide" -ForegroundColor Cyan -NoNewline; Write-Host " to get started."
    Write-Host "  Run " -NoNewline; Write-Host "lazyide --setup" -ForegroundColor Cyan -NoNewline; Write-Host " to check optional tool status."
    Write-Host ""
}
