$ErrorActionPreference = 'Stop'

$pfxPath = (Resolve-Path 'tools\nebulynx_codesign.pfx').Path
$cert = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2(
    $pfxPath,
    'nebulynx',
    [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]'Exportable,PersistKeySet,UserKeySet'
)

Write-Host "Signing with certificate: $($cert.Subject) [Thumbprint: $($cert.Thumbprint)]"

$targets = @(
    "target\release\prism-aperture-server.exe",
    "target\release\prism-aperture-service.exe",
    "target\release\PrismAperture-Setup.exe",
    "target\release\OpenReality-Setup.exe"
)

foreach ($target in $targets) {
    if (Test-Path $target) {
        Write-Host "Signing $target..."
        $sig = Set-AuthenticodeSignature -Certificate $cert -FilePath $target -HashAlgorithm SHA256
        Write-Host "Signed ${target} by $($sig.SignerCertificate.Subject)"
    }
}
