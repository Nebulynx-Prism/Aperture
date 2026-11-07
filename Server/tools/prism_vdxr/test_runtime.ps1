[CmdletBinding()]
param([switch]$Live)
$ErrorActionPreference='Stop'
$repo=Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$compiler=Join-Path $repo 'w64devkit/bin/g++.exe'
$binary=Join-Path $PSScriptRoot 'runtime_probe.exe'
$probeSource=[IO.File]::ReadAllText((Join-Path $repo 'tools/openxr_probe/main.cpp'))
$untyped='for (uint32_t j = 0; j < n; ++j) hr(device->CreateRenderTargetView(e.images[j].texture, nullptr, &e.targets[j]), "CreateRenderTargetView");'
$typed='for (uint32_t j = 0; j < n; ++j) { D3D11_RENDER_TARGET_VIEW_DESC rtv{}; rtv.Format=static_cast<DXGI_FORMAT>(format); rtv.ViewDimension=D3D11_RTV_DIMENSION_TEXTURE2D; hr(device->CreateRenderTargetView(e.images[j].texture, &rtv, &e.targets[j]), "CreateRenderTargetView"); }'
if(!$probeSource.Contains($untyped) -and !$probeSource.Contains('rtv.Format=static_cast<DXGI_FORMAT>(format)')) { throw 'Reviewed app probe source changed; inspect its render-target view creation before regenerating.' }
New-Item -ItemType Directory -Force (Join-Path $PSScriptRoot 'obj') | Out-Null
# D3D11 OpenXR swapchain textures can be typeless. The application must
# create an RTV with its requested format; leave the shared SteamVR probe intact.
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'obj/probe_app.inc'),$probeSource.Replace($untyped,$typed))
& $compiler -std=c++17 -O2 -static "-I$PSScriptRoot/upstream/external/OpenXR-SDK/include" (Join-Path $PSScriptRoot 'runtime_probe.cpp') -o $binary -ld3d11 -ldxgi
if($LASTEXITCODE -ne 0) { throw 'Runtime probe compilation failed' }
$mode=if($Live){'--live'}else{'--fixture'}
& $binary $mode
if($LASTEXITCODE -ne 0) { throw "Runtime probe failed ($LASTEXITCODE)" }
