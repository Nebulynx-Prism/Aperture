[CmdletBinding()]
param(
 [string]$JobDirectory,
 [string]$InstallDirectory,
 [int]$ParentPid,
 [Parameter(Mandatory=$true)][string]$ExpectedVersion,
 [switch]$Urgent,
 [switch]$ValidateOnly,
 [string]$PackagePath
)
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
function Set-Status([string]$State,[string]$Message) {
 if(!$JobDirectory){return}
 $json=@{state=$State;message=$Message}|ConvertTo-Json -Compress
 $temporary=Join-Path $JobDirectory 'status.next'
 [IO.File]::WriteAllText($temporary,$json)
 Move-Item -LiteralPath $temporary -Destination (Join-Path $JobDirectory 'status.json') -Force
}
function Assert-AssetUrl([string]$Url) {
 $uri=[Uri]$Url
 if($uri.Scheme -ne 'https' -or $uri.Host -ne 'github.com' -or $uri.Port -ne 443 -or $uri.UserInfo -or !$uri.AbsolutePath.StartsWith('/Nebulynx-Prism/Aperture/releases/download/',[StringComparison]::Ordinal)) {
  throw 'Update asset URL is outside the authorized GitHub repository'
 }
}
function Read-Package([string]$Path,[string]$Version,[string]$Destination) {
 if((Get-Item -LiteralPath $Path).Length -gt 268435456){throw 'Update ZIP exceeds 256 MiB'}
 $zip=[IO.Compression.ZipFile]::OpenRead($Path)
 try {
  if($zip.Entries.Count -gt 128){throw 'Too many ZIP entries'}
  $total=0L
  foreach($entry in $zip.Entries){
   $name=$entry.FullName.Replace('\','/')
   if($name.StartsWith('/') -or $name.Contains(':') -or ($name.Split('/') -contains '..') -or (($entry.ExternalAttributes -shr 16) -band 0xf000) -eq 0xa000){throw 'Unsafe ZIP path or symbolic link'}
   $total+=$entry.Length
   if($entry.Length -gt 268435456 -or $total -gt 536870912){throw 'Expanded update exceeds limits'}
  }
  $metadata=@($zip.Entries|Where-Object {$_.FullName -ceq 'update-manifest.json'})
  $setups=@($zip.Entries|Where-Object {$_.FullName -ceq 'Aperture-Setup.exe'})
  if($metadata.Count -ne 1 -or $setups.Count -ne 1 -or $metadata[0].Length -gt 8192){throw 'ZIP must contain one root update-manifest.json and Aperture-Setup.exe'}
  $reader=[IO.StreamReader]::new($metadata[0].Open())
  try{$manifest=$reader.ReadToEnd()|ConvertFrom-Json}finally{$reader.Dispose()}
  if($manifest.version -cne $Version -or $manifest.setup_sha256 -notmatch '^[a-fA-F0-9]{64}$' -or $manifest.server_sha256 -notmatch '^[a-fA-F0-9]{64}$'){throw 'Update manifest version or hashes are invalid'}
  $setup=Join-Path $Destination 'Aperture-Setup.exe'
  $source=$setups[0].Open();$target=[IO.File]::Create($setup)
  try{
   $buffer=New-Object byte[] 65536;$copied=0L;$clock=[Diagnostics.Stopwatch]::StartNew()
   while(($count=$source.Read($buffer,0,$buffer.Length)) -gt 0){
    $copied+=$count
    if($copied -gt $setups[0].Length -or $copied -gt 268435456 -or $clock.Elapsed.TotalSeconds -gt 45){throw 'Expanded installer exceeds bounds'}
    $target.Write($buffer,0,$count)
   }
   if($copied -ne $setups[0].Length){throw 'Expanded installer length mismatch'}
  }finally{$source.Dispose();$target.Dispose()}
  if((Get-FileHash -LiteralPath $setup).Hash -ine $manifest.setup_sha256){throw 'Embedded installer SHA256 mismatch'}
  return $manifest
 }finally{$zip.Dispose()}
}
if($ValidateOnly){
 if(!$PackagePath -or !$JobDirectory){throw 'Validation requires local PackagePath and JobDirectory'}
 $null=Read-Package $PackagePath $ExpectedVersion $JobDirectory
 Write-Output 'Package validation passed. No software was launched or installed.'
 exit 0
}
$closedForUpdate=$false
try {
 $null=New-Item -ItemType Directory -Force -Path $JobDirectory
 $server=Join-Path $InstallDirectory 'prism-aperture-server.exe'
 $parent=[Diagnostics.Process]::GetProcessById($ParentPid)
 if(![string]::Equals([IO.Path]::GetFullPath($parent.MainModule.FileName),[IO.Path]::GetFullPath($server),[StringComparison]::OrdinalIgnoreCase)){throw 'Parent process is not the target Aperture Server'}
 Set-Status 'preparing' 'Downloading and verifying update...'
 [Net.ServicePointManager]::SecurityProtocol=[Net.SecurityProtocolType]::Tls12
 $ProgressPreference='SilentlyContinue'
 $headers=@{'User-Agent'='Aperture-Updater';'Accept'='application/vnd.github+json';'X-GitHub-Api-Version'='2022-11-28'}
 $release=Invoke-RestMethod -Uri 'https://api.github.com/repos/Nebulynx-Prism/Aperture/releases/latest' -Headers $headers -TimeoutSec 20
 if($release.draft -or $release.prerelease -or $release.tag_name.TrimStart([char[]]'vV') -cne $ExpectedVersion){throw 'Latest stable release changed; check again'}
 $name=if($Urgent){'Aperture-UrgentUpdate.zip'}else{'Aperture-UpdatePackage.zip'}
 $assets=@($release.assets|Where-Object {$_.name -ceq $name})
 if($assets.Count -ne 1){throw 'Requested update package is missing or ambiguous'}
 $asset=$assets[0];Assert-AssetUrl $asset.browser_download_url
 $digest=$null
 if($asset.PSObject.Properties['digest']){$digest=[string]$asset.digest}
 if($digest -match '^sha256:([a-fA-F0-9]{64})$'){$expectedHash=$Matches[1]}
 else {
  $checksums=@($release.assets|Where-Object {$_.name -ceq ($name+'.sha256')})
  if($checksums.Count -ne 1){throw 'Update asset requires a GitHub SHA256 digest or matching .sha256 release asset'}
  Assert-AssetUrl $checksums[0].browser_download_url
  $content=(Invoke-WebRequest -UseBasicParsing -Uri $checksums[0].browser_download_url -Headers $headers -TimeoutSec 20).Content
  $checksum=if($content -is [byte[]]){[Text.Encoding]::UTF8.GetString($content)}else{[string]$content}
  if($checksum -notmatch '^\s*([a-fA-F0-9]{64})(?:\s|$)'){throw 'Invalid checksum asset'}
  $expectedHash=$Matches[1]
 }
 if($asset.size -gt 268435456){throw 'Update download exceeds 256 MiB'}
 $package=Join-Path $JobDirectory $name
 Invoke-WebRequest -UseBasicParsing -Uri $asset.browser_download_url -OutFile $package -Headers $headers -TimeoutSec 180
 if((Get-FileHash -LiteralPath $package).Hash -ine $expectedHash){throw 'Downloaded package SHA256 mismatch'}
 $manifest=Read-Package $package $ExpectedVersion $JobDirectory
 Set-Status 'ready' 'Verified update is ready. Closing Aperture...'
 if(!$parent.WaitForExit(60000)){throw 'Aperture did not close; update cancelled'}
 $closedForUpdate=$true
 Set-Status 'installing' 'Installing verified update...'
 $setup=Join-Path $JobDirectory 'Aperture-Setup.exe'
 $options=@{FilePath=$setup;ArgumentList=@('/S',('/D='+$InstallDirectory));PassThru=$true;Wait=$true}
 $identity=[Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
 if(!$identity.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)){$options.Verb='RunAs'}
 $process=Start-Process @options
 if($process.ExitCode -notin @(0,3010)){throw "Installer failed with exit code $($process.ExitCode)"}
 if((Get-FileHash -LiteralPath $server).Hash -ine $manifest.server_sha256){throw 'Installed server verification failed'}
 Set-Status 'complete' 'Update installed and verified'
 $arguments=@('--updated-version',$ExpectedVersion)
 if($Urgent){$arguments+='--updated-urgent'}
 Start-Process -FilePath $server -ArgumentList $arguments
}catch{
 Set-Status 'failed' $_.Exception.Message
 if($closedForUpdate -and (Test-Path -LiteralPath (Join-Path $InstallDirectory 'prism-aperture-server.exe'))){Start-Process -FilePath (Join-Path $InstallDirectory 'prism-aperture-server.exe') -ArgumentList @('--update-failed-key', ($ExpectedVersion+':'+$Urgent.IsPresent.ToString().ToLowerInvariant()))}
 throw
}



