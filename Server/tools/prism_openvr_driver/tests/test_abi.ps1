[CmdletBinding()]
param()
$ErrorActionPreference='Stop'
$driverRoot=Split-Path $PSScriptRoot -Parent
$repo=Split-Path (Split-Path $driverRoot -Parent) -Parent
$vswhere="${env:ProgramFiles(x86)}/Microsoft Visual Studio/Installer/vswhere.exe"
$msbuild=@(& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -find 'MSBuild/**/Bin/MSBuild.exe')[0]
if(!$msbuild) { throw 'MSVC is required to test SteamVR C++ ABI compatibility.' }
$out=Join-Path $driverRoot 'target/abi'
New-Item -ItemType Directory -Force $out | Out-Null
foreach($item in @(@('abi_display','DynamicLibrary'),@('abi_host','Application'))) {
    & $msbuild (Join-Path $driverRoot 'native_msvc.vcxproj') /m /p:Configuration=Release /p:Platform=x64 "/p:NativeSource=$PSScriptRoot/$($item[0]).cpp" "/p:NativeKind=$($item[1])" "/p:TargetName=$($item[0])" "/p:OutDir=$out/" "/p:IntDir=$driverRoot/target/$($item[0])_obj/" /verbosity:quiet
    if($LASTEXITCODE -ne 0) { throw "MSVC ABI fixture build failed: $($item[0])" }
}
& (Join-Path $repo 'w64devkit/bin/g++.exe') -std=c++17 -O2 -shared -static-libgcc -static-libstdc++ (Join-Path $PSScriptRoot 'abi_display.cpp') -o (Join-Path $out 'abi_mingw.dll')
if($LASTEXITCODE -ne 0) { throw 'MinGW ABI negative-control build failed' }
& (Join-Path $out 'abi_host.exe') (Join-Path $out 'abi_display.dll')
if($LASTEXITCODE -ne 0) { throw 'MSVC/OpenVR callback ABI regression failed' }
& (Join-Path $out 'abi_host.exe') (Join-Path $out 'abi_mingw.dll')
if($LASTEXITCODE -ne 1) { throw "Expected MinGW struct-return incompatibility; got $LASTEXITCODE" }
Write-Output 'PASS: MSVC callbacks compatible; MinGW negative control reproduces the struct-return failure.'
