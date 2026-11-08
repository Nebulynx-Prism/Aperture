# Validation-only update ZIP tests. Never launch the installer or stop any process.
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$root=Join-Path (Split-Path $PSScriptRoot) ('target/update-validation-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root|Out-Null
$setup=Join-Path $root 'Aperture-Setup.exe'
[IO.File]::WriteAllText($setup,'MZ test placeholder; never execute')
function New-TestZip([string]$Name,[bool]$Traversal,[bool]$BadHash) {
 $path=Join-Path $root $Name
 $manifest=Join-Path $root ($Name+'.json')
 $hash=(Get-FileHash -LiteralPath $setup).Hash
 if($BadHash){$hash='0'*64}
 [IO.File]::WriteAllText($manifest,(@{version='9.9.9';setup_sha256=$hash;server_sha256=('1'*64)}|ConvertTo-Json))
 $zip=[IO.Compression.ZipFile]::Open($path,[IO.Compression.ZipArchiveMode]::Create)
 try{
  [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip,$setup,'Aperture-Setup.exe')|Out-Null
  [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip,$manifest,'update-manifest.json')|Out-Null
  if($Traversal){[IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip,$setup,'..\outside.exe')|Out-Null}
 }finally{$zip.Dispose()}
 return $path
}
$valid=New-TestZip 'valid.zip' $false $false
$traversal=New-TestZip 'traversal.zip' $true $false
$badHash=New-TestZip 'bad-hash.zip' $false $true
$worker=Join-Path $PSScriptRoot 'apply_update.ps1'
$destination=Join-Path $root 'staging';New-Item -ItemType Directory -Path $destination|Out-Null
& powershell.exe -NoProfile -File $worker -ValidateOnly -PackagePath $valid -ExpectedVersion '9.9.9' -JobDirectory $destination
if($LASTEXITCODE -ne 0){throw 'Valid ZIP rejected'}
foreach($path in @($traversal,$badHash)){
 $ErrorActionPreference='Continue'
 $output=& powershell.exe -NoProfile -File $worker -ValidateOnly -PackagePath $path -ExpectedVersion '9.9.9' -JobDirectory $destination 2>&1
 $code=$LASTEXITCODE
 $ErrorActionPreference='Stop'
 if($code -eq 0){throw "Unsafe ZIP accepted: $path"}
}
if(Test-Path -LiteralPath (Join-Path $root 'outside.exe')){throw 'ZIP wrote outside staging'}
Write-Output '3 update ZIP tests passed: valid package, traversal rejection, hash rejection. No programs installed or executed.'


