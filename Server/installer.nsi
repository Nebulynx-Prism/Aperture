; ==============================================================================
; Aperture Server & Service Installer Script (NSIS)
; ==============================================================================

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"
SetCompressor /SOLID lzma

; General Configuration
Name "Aperture Server"
OutFile "target\release\Aperture-Setup.exe"
InstallDir "$PROGRAMFILES64\NebulynxGames\PrismSuite\Aperture"
InstallDirRegKey HKLM "Software\NebulynxGames\PrismSuite\Aperture" "InstallDir"
RequestExecutionLevel admin
Unicode True

; Interface Configuration
!define MUI_ABORTWARNING
!define MUI_ICON "assets\installer.ico"
!define MUI_UNICON "assets\installer.ico"
!define MUI_HEADERIMAGE
!define MUI_HEADERIMAGE_BITMAP_NOSTRETCH
!define MUI_WELCOMEFINISHPAGE_BITMAP "assets\prismbar.bmp"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP "assets\prismbar.bmp"

; Welcome Page
!define MUI_WELCOMEPAGE_TITLE "Welcome to Aperture Server Setup"
!define MUI_WELCOMEPAGE_TEXT "This wizard will install Aperture Server and Aperture Service on your computer.$\r$\n$\r$\nAperture provides high-performance, ultra-low-latency VR desktop streaming for Meta Quest 2, Quest 3, and Quest 3S.$\r$\n$\r$\nClick Next to continue."
!insertmacro MUI_PAGE_WELCOME

; License & Directory Pages
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES

; Finish Page
!define MUI_FINISHPAGE_RUN "$INSTDIR\prism-aperture-server.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Launch Aperture Server"
!insertmacro MUI_PAGE_FINISH

; Uninstaller Pages
!insertmacro MUI_UNPAGE_WELCOME
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH

; Language
!insertmacro MUI_LANGUAGE "English"

; Version Information
VIProductVersion "0.1.0.0"
VIAddVersionKey "ProductName" "Aperture Server"
VIAddVersionKey "CompanyName" "Nebulynx Game Studios"
VIAddVersionKey "LegalCopyright" "Nebulynx Game Studios"
VIAddVersionKey "FileDescription" "Aperture Server & Service Installer"
VIAddVersionKey "FileVersion" "0.1.0.0"
VIAddVersionKey "ProductVersion" "0.1.0.0"

; ==============================================================================
; Installer Sections
; ==============================================================================

Section "Aperture Core" SecCore
    SectionIn RO

    ; Kill any running instances before overwriting
    DetailPrint "Stopping existing processes..."
    ExecWait 'taskkill /F /IM prism-aperture-server.exe'
    ExecWait 'taskkill /F /IM open-reality.exe'
    ExecWait '"$INSTDIR\prism-aperture-service.exe" --stop'

    SetOutPath "$INSTDIR"

    ; Install Binaries
    File "target\release\prism-aperture-server.exe"
    File "target\release\prism-aperture-service.exe"
    File "target\release\Aperture-Updater.exe"


    ; Portable encoder; no dependency on another installed streaming application.
    File "target\installer-payload\ffmpeg.exe"
    SetOutPath "$INSTDIR\licenses\ffmpeg"
    File /r "target\installer-payload\ffmpeg-license\*.*"
    SetOutPath "$INSTDIR"

    ; Install Assets
    SetOutPath "$INSTDIR\assets"
    File "assets\icon.ico"
    File "assets\icon.png"
    File "assets\appv2.png"
    File "assets\aperture.png"
    File "assets\installer.ico"
    File "assets\installer.png"
    File /oname=logo.png "assets\logo-aperture.png"
    File "assets\sidebar.png"
    File "assets\sidebar.bmp"
    File "assets\prismbar.png"
    File "assets\prismbar.bmp"

    SetOutPath "$INSTDIR"

    ; Aperture-owned Rust PCVR driver. No ALVR/VDXR implementation is packaged.
    SetOutPath "$INSTDIR\aperture_driver"
    File /r /x *.pdb /x *.lib /x *.exp /x *.ilk "tools\aperture_driver\driver\*.*"
    SetOutPath "$INSTDIR"

    SetOutPath "$INSTDIR\tools"
    File "tools\register_driver.ps1"
    File "tools\install_vbcable.ps1"
    SetOutPath "$INSTDIR"
    DetailPrint "Registering the Aperture SteamVR driver..."
    ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\tools\register_driver.ps1" -DriverPath "$INSTDIR\aperture_driver"' $0
    ${If} $0 != 0
        MessageBox MB_OK|MB_ICONEXCLAMATION "Driver files installed. SteamVR registration was unavailable. Install SteamVR, then use the Register Aperture Driver shortcut." /SD IDOK
    ${EndIf}

    ; Install and start the Aperture Service
    DetailPrint "Installing Aperture Service..."
    ExecWait '"$INSTDIR\prism-aperture-service.exe" --install' $0
    ${If} $0 != 0
        MessageBox MB_OK|MB_ICONSTOP "Aperture Service installation failed (exit $0)." /SD IDOK
        SetErrorLevel 1
        Abort
    ${EndIf}

    ExecWait 'sc.exe config PrismApertureService DisplayName= "Aperture Service"'
    DetailPrint "Starting Aperture Service..."
    ExecWait '"$INSTDIR\prism-aperture-service.exe" --start' $0
    ${If} $0 != 0
        MessageBox MB_OK|MB_ICONEXCLAMATION "Aperture Service did not start (exit $0). Check Windows Services." /SD IDOK
    ${EndIf}

    ; Configure Windows Firewall rules
    DetailPrint "Configuring Windows Firewall rules..."
    ExecWait 'netsh advfirewall firewall add rule name="Aperture Server" dir=in action=allow program="$INSTDIR\prism-aperture-server.exe" enable=yes'
    ExecWait 'netsh advfirewall firewall add rule name="Aperture Server TCP" dir=in action=allow protocol=TCP localport=38810 enable=yes'
    ExecWait 'netsh advfirewall firewall add rule name="Aperture Server UDP" dir=in action=allow protocol=UDP localport=38810 enable=yes'

    ; Write installation path to registry
    WriteRegStr HKLM "Software\NebulynxGames\PrismSuite\Aperture" "InstallDir" "$INSTDIR"
    WriteRegStr HKLM "Software\Prism Aperture" "InstallDir" "$INSTDIR"

    ; Create uninstaller
    WriteUninstaller "$INSTDIR\Uninstall.exe"

    ; Write Add/Remove Programs registry keys
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismAperture" "DisplayName" "Aperture Server"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismAperture" "DisplayIcon" "$INSTDIR\assets\icon.ico"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismAperture" "DisplayVersion" "0.1.0"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismAperture" "Publisher" "Nebulynx Game Studios"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismAperture" "UninstallString" '"$INSTDIR\Uninstall.exe"'
    WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismAperture" "NoModify" 1
    WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismAperture" "NoRepair" 1

    ; Create Start Menu shortcuts
    CreateDirectory "$SMPROGRAMS\NebulynxGames\PrismSuite\Aperture"
    CreateShortcut "$SMPROGRAMS\NebulynxGames\PrismSuite\Aperture\Aperture Server.lnk" "$INSTDIR\prism-aperture-server.exe" "" "$INSTDIR\assets\icon.ico" 0
    CreateShortcut "$SMPROGRAMS\NebulynxGames\PrismSuite\Aperture\Uninstall Aperture.lnk" "$INSTDIR\Uninstall.exe" "" "$INSTDIR\assets\installer.ico" 0

    CreateDirectory "$SMPROGRAMS\Aperture"
    CreateShortcut "$SMPROGRAMS\Aperture\Aperture Server.lnk" "$INSTDIR\prism-aperture-server.exe" "" "$INSTDIR\assets\icon.ico" 0
    CreateShortcut "$SMPROGRAMS\Aperture\Uninstall Aperture.lnk" "$INSTDIR\Uninstall.exe" "" "$INSTDIR\assets\installer.ico" 0

    CreateShortcut "$SMPROGRAMS\Aperture\Register Aperture Driver.lnk" "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" '-NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\tools\register_driver.ps1" -DriverPath "$INSTDIR\aperture_driver"'

    ; Create Desktop shortcut
    CreateShortcut "$DESKTOP\Aperture Server.lnk" "$INSTDIR\prism-aperture-server.exe" "" "$INSTDIR\assets\icon.ico" 0
SectionEnd

Section "Aperture OpenXR runtime (experimental, not activated)" SecXR
    SetOutPath "$INSTDIR\openxr"
    File "tools\prism_vdxr\bin\prism-vdxr.dll"
    File "tools\prism_vdxr\bin\prism-vdxr.json"
    File "tools\prism_vdxr\bin\aperture-xr.json"
    File "tools\prism_vdxr\bin\LICENSE-VDXR.txt"
    SetOutPath "$INSTDIR"
SectionEnd

Section /o "Download and install VB-Audio VB-CABLE (optional)" SecCable
    DetailPrint "Downloading verified VB-CABLE and opening the vendor installer..."
    ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\tools\install_vbcable.ps1"' $0
    ${If} $0 != 0
        MessageBox MB_OK|MB_ICONEXCLAMATION "VB-CABLE download, verification or setup failed (exit $0). Aperture is installed. You can retry VB-CABLE from vb-audio.com." /SD IDOK
    ${Else}
        MessageBox MB_OK|MB_ICONINFORMATION "VB-CABLE setup finished. Follow the vendor restart instructions. Route applications to CABLE Input, then select VB-CABLE in Aperture." /SD IDOK
        SetRebootFlag true
    ${EndIf}
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
    !insertmacro MUI_DESCRIPTION_TEXT ${SecCore} "Aperture Server, Windows Service, portable FFmpeg encoder, and Aperture SteamVR driver."
    !insertmacro MUI_DESCRIPTION_TEXT ${SecXR} "Install experimental OpenXR runtime files. The active system runtime is unchanged. Use SteamVR for OpenXR by default; do not run both compositor backends together."
    !insertmacro MUI_DESCRIPTION_TEXT ${SecCable} "Download VB-CABLE from VB-Audio, verify its pinned SHA256 and publisher signature, and show its installer. Optional; may require restart. Donationware terms apply."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Function .onInit
    ${IfNot} ${RunningX64}
        MessageBox MB_OK|MB_ICONSTOP "Aperture requires 64-bit Windows." /SD IDOK
        SetErrorLevel 1
        Abort
    ${EndIf}
    SetRegView 64
FunctionEnd

Function un.onInit
    SetRegView 64
FunctionEnd

; ==============================================================================
; Uninstaller Section
; ==============================================================================

Section "Uninstall"
    DetailPrint "Stopping Aperture processes and service..."
    ExecWait 'taskkill /F /IM prism-aperture-server.exe'
    ExecWait '"$INSTDIR\prism-aperture-service.exe" --stop'
    ExecWait '"$INSTDIR\prism-aperture-service.exe" --restore-owned-runtime'
    ExecWait '"$INSTDIR\prism-aperture-service.exe" --uninstall'

    ; Remove Windows Firewall rules
    ExecWait 'netsh advfirewall firewall delete rule name="Aperture Server"'
    ExecWait 'netsh advfirewall firewall delete rule name="Aperture Server TCP"'
    ExecWait 'netsh advfirewall firewall delete rule name="Aperture Server UDP"'

    ; Unregister only Aperture's own driver. VB-CABLE may be shared by other apps.
    ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\tools\register_driver.ps1" -DriverPath "$INSTDIR\aperture_driver" -Remove'
    RMDir /r "$INSTDIR\aperture_driver"
    RMDir /r "$INSTDIR\openxr"
    Delete "$INSTDIR\tools\register_driver.ps1"
    Delete "$INSTDIR\tools\install_vbcable.ps1"
    RMDir "$INSTDIR\tools"
    Delete "$INSTDIR\ffmpeg.exe"
    RMDir /r "$INSTDIR\licenses\ffmpeg"
    RMDir "$INSTDIR\licenses"
    Delete "$SMPROGRAMS\Aperture\Register Aperture Driver.lnk"

    ; Delete files
    Delete "$INSTDIR\prism-aperture-server.exe"
    Delete "$INSTDIR\prism-aperture-service.exe"
    Delete "$INSTDIR\Aperture-Updater.exe"
    Delete "$INSTDIR\Uninstall.exe"
    Delete "$INSTDIR\assets\icon.ico"
    Delete "$INSTDIR\assets\icon.png"
    Delete "$INSTDIR\assets\appv2.png"
    Delete "$INSTDIR\assets\aperture.png"
    Delete "$INSTDIR\assets\installer.ico"
    Delete "$INSTDIR\assets\installer.png"
    Delete "$INSTDIR\assets\logo.png"
    Delete "$INSTDIR\assets\sidebar.png"
    Delete "$INSTDIR\assets\sidebar.bmp"
    Delete "$INSTDIR\assets\prismbar.png"
    Delete "$INSTDIR\assets\prismbar.bmp"
    RMDir "$INSTDIR\assets"
    RMDir "$INSTDIR"
    RMDir "$PROGRAMFILES64\NebulynxGames\PrismSuite"
    RMDir "$PROGRAMFILES64\NebulynxGames"

    ; Delete shortcuts
    Delete "$SMPROGRAMS\NebulynxGames\PrismSuite\Aperture\Aperture Server.lnk"
    Delete "$SMPROGRAMS\NebulynxGames\PrismSuite\Aperture\Uninstall Aperture.lnk"
    RMDir "$SMPROGRAMS\NebulynxGames\PrismSuite\Aperture"
    RMDir "$SMPROGRAMS\NebulynxGames\PrismSuite"
    RMDir "$SMPROGRAMS\NebulynxGames"

    Delete "$SMPROGRAMS\Aperture\Aperture Server.lnk"
    Delete "$SMPROGRAMS\Aperture\Uninstall Aperture.lnk"
    RMDir "$SMPROGRAMS\Aperture"
    Delete "$DESKTOP\Aperture Server.lnk"

    ; Delete registry keys
    DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismAperture"
    DeleteRegKey HKLM "Software\NebulynxGames\PrismSuite\Aperture"
    DeleteRegKey /ifempty HKLM "Software\NebulynxGames\PrismSuite"
    DeleteRegKey /ifempty HKLM "Software\NebulynxGames"
    DeleteRegKey HKLM "Software\Prism Aperture"
    DeleteRegKey HKLM "Software\PrismAperture"
SectionEnd








