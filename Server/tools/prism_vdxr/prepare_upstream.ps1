[CmdletBinding()]
param()
$ErrorActionPreference='Stop'
$checkout=Join-Path $PSScriptRoot 'upstream'
$pinned='6da20fe478e331c05672243fd82a9a1500cd39f5'
$actual=(& git -C $checkout rev-parse HEAD).Trim()
if($LASTEXITCODE -ne 0 -or $actual -ne $pinned) { throw "Expected VDXR commit $pinned; found $actual" }
function Original([string]$Path) {
    $content=& git -C $checkout show "HEAD:$Path"
    if($LASTEXITCODE -ne 0) { throw "Cannot read pinned source $Path" }
    return ($content -join "`n")+"`n"
}
function Replace-Required([string]$Text,[string]$Before,[string]$After) {
    if(!$Text.Contains($Before)) { throw "Upstream patch does not match: $Before" }
    return $Text.Replace($Before,$After)
}
$source=Join-Path $checkout 'virtualdesktop-openxr'
# Regenerate from pinned originals; reruns never accumulate patch fragments.
$instance=Original 'virtualdesktop-openxr/instance.cpp'
$instance=Replace-Required $instance 'm_useApplicationDeviceForSubmission = getSetting("quirk_use_application_device_for_submission").value_or(false);' 'm_useApplicationDeviceForSubmission = false; // Prism owns a dedicated submission context.'
[IO.File]::WriteAllText((Join-Path $source 'instance.cpp'),$instance)
$session=Original 'virtualdesktop-openxr/session.cpp'
$session=Replace-Required $session "m_useAsyncSubmission = !m_isHeadless && !m_useApplicationDeviceForSubmission &&`n                               !getSetting(`"quirk_disable_async_submission`").value_or(false);" 'm_useAsyncSubmission = false; // Submit once per application frame; no duplicate stream frames.'
$session=Replace-Required $session 'm_useMirrorWindow = getSetting("mirror_window").value_or(false);' 'm_useMirrorWindow = false; // Prism desktop stream is the mirror.'
[IO.File]::WriteAllText((Join-Path $source 'session.cpp'),$session)
$system=Original 'virtualdesktop-openxr/system.cpp'
$system=Replace-Required $system 'initializeBodyTrackingMmf();' '// Prism has no VirtualDesktop.BodyState dependency.'
[IO.File]::WriteAllText((Join-Path $source 'system.cpp'),$system)
$config=Original 'virtualdesktop-openxr/RuntimeConfiguration.h'
$config=$config.Replace('VirtualDesktopXR (Standalone)','Prism VDXR').Replace('"Oculus"','"Prism"').Replace('"VirtualDesktop-OpenXR"','"Prism-VDXR"').Replace('L"VirtualDesktop-OpenXR"','L"Prism-VDXR"')
# Do not expose optional sensor/layer features absent from the wire protocol.
$config=[regex]::Replace($config,'(?m)^#define HAS_(CYLINDER_LAYERS|HAND_JOINTS_TRACKING|HAND_TRACKING_AIM|EYE_TRACKING|FACIAL_EYE_TRACKING|FACIAL_EXPRESSION_TRACKING|BODY_TRACKING)\s*$', '')
[IO.File]::WriteAllText((Join-Path $source 'RuntimeConfiguration.h'),$config)
$project=Original 'virtualdesktop-openxr/virtualdesktop-openxr.vcxproj'
$project=Replace-Required $project '<ClCompile Include="OVR_CAPIShim.c">' '<ClCompile Include="..\..\prism_backend.cpp">'
$project=$project.Replace('STANDALONE_RUNTIME;','STANDALONE_RUNTIME;PRISM_VDXR;LOAD_LOCAL_LIBOVR;')
$project=$project.Replace('dxgi.lib;dxguid.lib;d3d11.lib;','dxgi.lib;dxguid.lib;d3d11.lib;d3dcompiler.lib;')
[IO.File]::WriteAllText((Join-Path $source 'prism-vdxr.vcxproj'),$project)
Write-Output "Prepared Prism VDXR from upstream $pinned"
