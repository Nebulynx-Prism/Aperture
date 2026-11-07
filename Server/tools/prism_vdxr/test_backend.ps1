[CmdletBinding()]
param()
$ErrorActionPreference='Stop'
$repo=Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$compiler=Join-Path $repo 'w64devkit/bin/g++.exe'
$binary=Join-Path $PSScriptRoot 'backend_test.exe'
& $compiler -std=c++17 -O2 "-I$PSScriptRoot/upstream/external/LibOVR/Include" (Join-Path $PSScriptRoot 'backend_test.cpp') -o $binary -ld3d11 -ldxgi -ld3dcompiler -lole32
if($LASTEXITCODE -ne 0) { throw 'Backend test compilation failed' }
& $binary
if($LASTEXITCODE -ne 0) { throw "Backend test failed ($LASTEXITCODE)" }
