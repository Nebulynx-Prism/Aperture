[CmdletBinding()]
param()
$ErrorActionPreference='Stop'
$checkout=Join-Path $PSScriptRoot 'upstream'
$pinned='6da20fe478e331c05672243fd82a9a1500cd39f5'
if(!(Test-Path -LiteralPath $checkout)) {
    & git clone --no-checkout --filter=blob:none https://github.com/mbucchia/VirtualDesktop-OpenXR.git $checkout
    if($LASTEXITCODE -ne 0) { throw 'VDXR clone failed' }
    & git -C $checkout checkout --detach $pinned
    if($LASTEXITCODE -ne 0) { throw 'Cannot check out pinned VDXR commit' }
}
if((& git -C $checkout rev-parse HEAD).Trim() -ne $pinned) { throw 'Existing upstream checkout has a different commit; it was left unchanged.' }
& git -C $checkout submodule update --init --depth 1 external/LibOVR external/OpenXR-SDK external/OpenXR-MixedReality external/Vulkan-SDK external/cJSON external/fmt external/openvr external/FidelityFX-FSR external/FidelityFX-CAS
if($LASTEXITCODE -ne 0) { throw 'VDXR dependency download failed' }
$nuget=Join-Path $PSScriptRoot 'nuget.exe'
if(!(Test-Path -LiteralPath $nuget)) { Invoke-WebRequest -Uri https://dist.nuget.org/win-x86-commandline/latest/nuget.exe -OutFile $nuget }
& $nuget restore (Join-Path $checkout 'virtualdesktop-openxr/packages.config') -PackagesDirectory (Join-Path $checkout 'packages') -NonInteractive
if($LASTEXITCODE -ne 0) { throw 'VDXR NuGet restore failed' }
