# poenv installer for Windows.
#
#   irm https://poenv.pogi.codes/install.ps1 | iex
#
# Options via env vars (set them before running):
#   $env:POENV_VERSION         install this version instead of the latest release
#   $env:POENV_INSTALL_DIR     where to put poenv.exe (default %LOCALAPPDATA%\poenv\bin)
#   $env:POENV_NO_MODIFY_PATH  set to 1 to leave your user PATH alone
# It never prompts, so agents can run it as-is. No admin rights needed.
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$Repo = "pogicodes/poenv"
$Version = if ($env:POENV_VERSION) { $env:POENV_VERSION } else { "latest" }
$Dir = if ($env:POENV_INSTALL_DIR) { $env:POENV_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "poenv\bin" }
# Where release files come from. Overridable for testing (a URL or a local directory).
$Base = $env:POENV_DOWNLOAD_BASE

# Windows on ARM gets the native build, even from an x64 PowerShell running under emulation
# (which reports AMD64 in PROCESSOR_ARCHITECTURE and the real CPU in PROCESSOR_ARCHITEW6432).
$Cpu = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
$Asset = if ($Cpu -eq "ARM64") { "poenv-windows-arm64.exe" } else { "poenv-windows-x64.exe" }

if (-not $Base) {
  if ($Version -eq "latest") {
    # Resolve the tag first: GitHub caches the releases/latest/download/... redirect for a while
    # after a release, while the API answers with the new release right away.
    try { $Version = (Invoke-RestMethod -UseBasicParsing "https://api.github.com/repos/$Repo/releases/latest").tag_name.TrimStart('v') } catch { }
  }
  if ($Version -eq "latest") { $Base = "https://github.com/$Repo/releases/latest/download" }
  else { $Base = "https://github.com/$Repo/releases/download/v$($Version.TrimStart('v'))" }
}

function Fetch($Name, $Dest) {
  if ($Base -match '^https?://') { Invoke-WebRequest -UseBasicParsing -Uri "$Base/$Name" -OutFile $Dest }
  else { Copy-Item -Path (Join-Path $Base $Name) -Destination $Dest }
}

$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("poenv-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $Tmp | Out-Null
try {
  Write-Host "Downloading $Asset ($Version)..."
  Fetch $Asset (Join-Path $Tmp "poenv.exe")
  Fetch "SHA256SUMS" (Join-Path $Tmp "SHA256SUMS")

  $Expected = (Get-Content (Join-Path $Tmp "SHA256SUMS") |
    Where-Object { ($_ -split '\s+')[1] -in @($Asset, "*$Asset") } |
    ForEach-Object { ($_ -split '\s+')[0] }) | Select-Object -First 1
  if (-not $Expected) { throw "no checksum for $Asset in SHA256SUMS" }
  $Actual = (Get-FileHash (Join-Path $Tmp "poenv.exe") -Algorithm SHA256).Hash.ToLower()
  if ($Actual -ne $Expected.ToLower()) { throw "checksum mismatch for $Asset (expected $Expected, got $Actual)" }

  New-Item -ItemType Directory -Force -Path $Dir | Out-Null
  $Exe = Join-Path $Dir "poenv.exe"
  Move-Item -Force (Join-Path $Tmp "poenv.exe") $Exe
  $Installed = & $Exe --version
  if ($LASTEXITCODE -ne 0) { throw "installed, but $Exe doesn't run" }
  Write-Host "Installed poenv $Installed to $Exe"
} finally {
  Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}

$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
$OnPath = ($UserPath -split ';') -contains $Dir
if (-not $OnPath) {
  if ($env:POENV_NO_MODIFY_PATH) {
    Write-Host "Add $Dir to your PATH to run poenv from anywhere."
  } else {
    $NewPath = if ($UserPath) { "$UserPath;$Dir" } else { $Dir }
    [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
    $env:Path = "$env:Path;$Dir"
    Write-Host "Added $Dir to your user PATH. Open a new terminal to use it."
  }
}

Write-Host ""
Write-Host "Get started"
Write-Host "  cd your-repo; poenv init      import the repo's .env into your vault"
Write-Host "  poenv env new staging         add another env"
Write-Host "  poenv use staging             switch the repo's .env to it"
Write-Host "  poenv                         browse and edit everything"
Write-Host ""
Write-Host "Stuck? poenv doctor checks your setup. All commands: poenv --help"
