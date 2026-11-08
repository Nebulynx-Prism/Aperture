[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$DriverPath, [switch]$Remove, [switch]$DryRun)
$ErrorActionPreference='Stop'
$DriverPath=[IO.Path]::GetFullPath($DriverPath)
if (!(Test-Path -LiteralPath (Join-Path $DriverPath 'driver.vrdrivermanifest'))) {throw 'Aperture driver manifest missing'}
$roots=@()
$paths=Join-Path $env:LOCALAPPDATA 'openvr/openvrpaths.vrpath'
if(Test-Path -LiteralPath $paths) { $roots+=@((Get-Content -LiteralPath $paths -Raw|ConvertFrom-Json).runtime) }
foreach($key in @('HKCU:\Software\Valve\Steam','HKLM:\SOFTWARE\WOW6432Node\Valve\Steam')) {
    $value=Get-ItemProperty -LiteralPath $key -ErrorAction SilentlyContinue
    if($value) {
        foreach($property in @('SteamPath','InstallPath')) {
            $p=$value.PSObject.Properties[$property]
            if($p -and $p.Value) {$roots+=Join-Path $p.Value 'steamapps/common/SteamVR'}
        }
    }
}
$roots+=@("${env:ProgramFiles(x86)}/Steam/steamapps/common/SteamVR", "$env:ProgramFiles/Steam/steamapps/common/SteamVR")
$tool=@($roots | Where-Object {$_} | ForEach-Object {Join-Path $_ 'bin/win64/vrpathreg.exe'} | Where-Object {Test-Path -LiteralPath $_} | Select-Object -First 1)
if(!$tool.Count) {throw 'SteamVR not found. Install SteamVR, then register the Aperture driver from its setup shortcut.'}
$verb=if($Remove){'removedriver'}else{'adddriver'}
if($DryRun) {Write-Output "Would run vrpathreg $verb with the Aperture driver directory. No changes made.";exit 0}
& $tool[0] $verb $DriverPath
if($LASTEXITCODE -ne 0) {throw "SteamVR driver registration exited $LASTEXITCODE"}
