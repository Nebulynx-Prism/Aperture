$ErrorActionPreference = 'Stop'
$repo = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
& (Join-Path $repo 'w64devkit/bin/g++.exe') -std=c++17 -O2 -static-libgcc -static-libstdc++ (Join-Path $PSScriptRoot 'probe.cpp') -o (Join-Path $PSScriptRoot 'probe.exe') -ld3d11 -ldxgi -lole32 -luuid
if ($LASTEXITCODE -ne 0) { throw 'Probe compilation failed' }
Push-Location $PSScriptRoot
try { & ./probe.exe; if ($LASTEXITCODE -ne 0) { throw 'Native probe failed (stop Prism runtime first)' } } finally { Pop-Location }
