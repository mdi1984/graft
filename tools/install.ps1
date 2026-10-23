# Build Graft in release mode and install it for the current user:
# copies graft.exe to %LOCALAPPDATA%\Programs\Graft and adds a Start menu
# shortcut. Run again to update.
#
#   powershell -ExecutionPolicy Bypass -File tools\install.ps1
param(
  [string] $Dest = (Join-Path $env:LOCALAPPDATA 'Programs\Graft'),
  [switch] $NoShortcut
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
  throw 'Rust is not installed. Get it from https://rustup.rs, then run this again.'
}

Write-Host 'Building Graft (release)...'
cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'The build failed.' }

$running = Get-Process graft -ErrorAction SilentlyContinue |
  Where-Object { $_.Path -and $_.Path.StartsWith($Dest, [StringComparison]::OrdinalIgnoreCase) }
if ($running) { throw 'Graft is running from the install folder. Close it and run this again.' }

New-Item -ItemType Directory -Force -Path $Dest | Out-Null
$exe = Join-Path $Dest 'graft.exe'
Copy-Item (Join-Path $root 'target\release\graft.exe') $exe -Force
Write-Host "Installed $exe"

if (-not $NoShortcut) {
  $menu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
  $shell = New-Object -ComObject WScript.Shell
  $link = $shell.CreateShortcut((Join-Path $menu 'Graft.lnk'))
  $link.TargetPath = $exe
  $link.WorkingDirectory = $Dest
  $link.Description = 'Graft - a fast git client'
  $link.Save()
  Write-Host 'Added Graft to the Start menu.'
}
