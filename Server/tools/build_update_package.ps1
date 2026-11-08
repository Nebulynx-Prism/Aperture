[CmdletBinding()]
param([switch]$Urgent,[switch]$SkipBuild)
$ErrorActionPreference='Stop'
$server=Split-Path $PSScriptRoot
if(!$SkipBuild){& (Join-Path $PSScriptRoot 'build_installer.ps1')}
$release=Join-Path $server 'target/release'
$setup=Join-Path $release 'Aperture-Setup.exe'
$binary=Join-Path $release 'prism-aperture-server.exe'
if(!(Test-Path -LiteralPath $setup) -or !(Test-Path -LiteralPath $binary)){throw 'Build installer and release binaries first'}
$version=[regex]::Match((Get-Content (Join-Path $server 'Cargo.toml') -Raw),'(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if(!$version){throw 'Package version not found'}
$work=Join-Path $server ('target/update-package-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work|Out-Null
$manifest=Join-Path $work 'update-manifest.json'
[IO.File]::WriteAllText($manifest,(@{version=$version;setup_sha256=(Get-FileHash -LiteralPath $setup).Hash;server_sha256=(Get-FileHash -LiteralPath $binary).Hash}|ConvertTo-Json -Compress))
$name=if($Urgent){'Aperture-UrgentUpdate.zip'}else{'Aperture-UpdatePackage.zip'}
$output=Join-Path $release $name
$temporary=Join-Path $work $name
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip=[IO.Compression.ZipFile]::Open($temporary,[IO.Compression.ZipArchiveMode]::Create)
try{
 [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip,$setup,'Aperture-Setup.exe',[IO.Compression.CompressionLevel]::NoCompression)|Out-Null
 [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip,$manifest,'update-manifest.json',[IO.Compression.CompressionLevel]::NoCompression)|Out-Null
}finally{$zip.Dispose()}
Move-Item -LiteralPath $temporary -Destination $output -Force
$hash=(Get-FileHash -LiteralPath $output).Hash
[IO.File]::WriteAllText(($output+'.sha256'),($hash.ToLowerInvariant()+'  '+$name+"`n"))
Write-Output "$name version=$version SHA256=$hash"

