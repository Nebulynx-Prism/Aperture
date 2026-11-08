$ErrorActionPreference = 'Stop'

$pfxPath = (Resolve-Path "tools\nebulynx_codesign.pfx").Path
$flags = [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]"Exportable"
$cert = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2(
    $pfxPath,
    "nebulynx",
    $flags
)

$targets = @(
    "target\release\prism-aperture-server.exe",
    "target\release\prism-aperture-service.exe",
    "target\release\PrismAperture-Setup.exe"
)

foreach ($target in $targets) {
    if (Test-Path $target) {
        $sig = Set-AuthenticodeSignature -Certificate $cert -FilePath $target -HashAlgorithm SHA256
        Write-Host "Signed $target -> $($sig.SignerCertificate.Subject)"
    }
}
