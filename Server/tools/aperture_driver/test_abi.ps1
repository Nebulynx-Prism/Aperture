$ErrorActionPreference='Stop'
$vswhere="${env:ProgramFiles(x86)}/Microsoft Visual Studio/Installer/vswhere.exe"
$msbuild=@(& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -find 'MSBuild/**/Bin/MSBuild.exe')[0]
$out=Join-Path $PSScriptRoot 'target/abi-test'
New-Item -ItemType Directory -Force -Path $out | Out-Null
foreach($item in @(@('abi_fixture','DynamicLibrary'),@('abi_host','Application'))) {
    & $msbuild (Join-Path $PSScriptRoot 'abi/aperture.vcxproj') /m /p:Configuration=Release /p:Platform=x64 "/p:NativeSource=$PSScriptRoot/tests/$($item[0]).cpp" "/p:NativeKind=$($item[1])" "/p:TargetName=$($item[0])" "/p:OutDir=$out/" "/p:IntDir=$out/$($item[0])_obj/" /verbosity:minimal
    if($LASTEXITCODE -ne 0){throw "ABI fixture build failed: $($item[0])"}
}
& (Join-Path $out 'abi_host.exe') (Join-Path $out 'abi_fixture.dll')
if($LASTEXITCODE -ne 0){throw 'Aperture ABI test failed'}
