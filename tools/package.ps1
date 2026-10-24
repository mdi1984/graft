# Package a release build into dist\: the zip, the setup program, the plain
# exe the in-app updater downloads, and SHA256SUMS.txt over all of them.
# Expects `cargo build --release -p graft` to have run. Needs Inno Setup 6.
#
#   powershell -ExecutionPolicy Bypass -File tools\package.ps1 -Version 1.2.3
param(
  [Parameter(Mandatory)] [string] $Version
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\release\graft.exe'
$dist = Join-Path $root 'dist'
$pack = Join-Path $root 'target\pack'
if (-not (Test-Path $exe)) { throw "No release build at $exe." }

$iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
if (-not $iscc) { $iscc = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe' }
if (-not (Test-Path $iscc)) { throw 'Inno Setup 6 is not installed (choco install innosetup).' }

Remove-Item -Recurse -Force $dist, $pack -ErrorAction SilentlyContinue
New-Item -ItemType Directory $dist, $pack | Out-Null

Copy-Item $exe, (Join-Path $root 'LICENSE'), (Join-Path $root 'README.md') $pack
Compress-Archive "$pack\*" (Join-Path $dist "Graft-$Version-x64.zip")
# A stable name the in-app updater downloads.
Copy-Item $exe (Join-Path $dist 'graft-x64.exe')

& $iscc /Q "/DVersion=$Version" "/DSource=$(Split-Path $exe)" "/DOutput=$dist" `
  (Join-Path $root 'installer\graft.iss')
if ($LASTEXITCODE -ne 0) { throw 'Building the setup program failed.' }

# Hash first, then write: the list must not include itself.
$sums = Get-ChildItem $dist | ForEach-Object {
  "$((Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower())  $($_.Name)"
}
$sums | Out-File -Encoding ascii (Join-Path $dist 'SHA256SUMS.txt')
Get-ChildItem $dist | Format-Table Name, Length
