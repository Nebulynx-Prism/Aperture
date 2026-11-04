Add-Type -AssemblyName System.Drawing

$srcPath = "C:\Users\Phoenix\.gemini\antigravity\brain\49716b76-323b-4bc1-bc48-7e6a9312efcc\.user_uploaded\media_1793778608965_60db61d3.jpg"
$srcImg = [System.Drawing.Image]::FromFile($srcPath)

# 1. Save original as assets\prismbar.png and assets\sidebar.png
$srcImg.Save("assets\prismbar.png", [System.Drawing.Imaging.ImageFormat]::Png)
$srcImg.Save("assets\sidebar.png", [System.Drawing.Imaging.ImageFormat]::Png)

# 2. Save as 24bpp RGB BMP for NSIS MUI sidebar
$bmp = New-Object System.Drawing.Bitmap($srcImg.Width, $srcImg.Height, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
$g.DrawImage($srcImg, 0, 0, $srcImg.Width, $srcImg.Height)
$g.Dispose()

$bmp.Save("assets\sidebar.bmp", [System.Drawing.Imaging.ImageFormat]::Bmp)
$bmp.Save("assets\prismbar.bmp", [System.Drawing.Imaging.ImageFormat]::Bmp)
$bmp.Dispose()
$srcImg.Dispose()

Write-Output "Images saved successfully:"
Get-Item assets\prismbar.png, assets\sidebar.png, assets\sidebar.bmp, assets\prismbar.bmp | Select-Object Name, Length
