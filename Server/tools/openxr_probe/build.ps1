$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$out = Join-Path $PSScriptRoot 'target'
New-Item -ItemType Directory -Force -Path $out | Out-Null
& (Join-Path $root 'w64devkit/bin/g++.exe') '-std=c++17' '-O2' '-Wall' '-Wextra' '-Wno-missing-field-initializers' '-static' '-I' (Join-Path $root 'client/app/src/main/cpp/include') (Join-Path $PSScriptRoot 'main.cpp') '-o' (Join-Path $out 'openxr-probe.exe') '-ld3d11' '-ldxgi'
if ($LASTEXITCODE -ne 0) { throw "Probe compilation failed: $LASTEXITCODE" }
Write-Host "Built $out/openxr-probe.exe (not executed)"
