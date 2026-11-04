; ==============================================================================
; Prism Aperture Server & Service Installer Script (NSIS)
; ==============================================================================

!include "MUI2.nsh"
!include "FileFunc.nsh"

; General Configuration
Name "Prism Aperture Server"
OutFile "target\release\PrismAperture-Setup.exe"
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
!define MUI_WELCOMEPAGE_TITLE "Welcome to Prism Aperture Server Setup"
!define MUI_WELCOMEPAGE_TEXT "This wizard will install Prism Aperture Server and Prism Aperture Service on your computer.$\r$\n$\r$\nPrism Aperture provides high-performance, ultra-low-latency VR desktop streaming for Meta Quest 2, Quest 3, and Quest 3S.$\r$\n$\r$\nClick Next to continue."
!insertmacro MUI_PAGE_WELCOME

; License & Directory Pages
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES

; Finish Page
!define MUI_FINISHPAGE_RUN "$INSTDIR\prism-aperture-server.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Launch Prism Aperture Server"
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
VIAddVersionKey "ProductName" "Prism Aperture Server"
VIAddVersionKey "CompanyName" "NebulynxGames"
VIAddVersionKey "LegalCopyright" "MIT License"
VIAddVersionKey "FileDescription" "Prism Aperture Server & Service Installer"
VIAddVersionKey "FileVersion" "0.1.0.0"
VIAddVersionKey "ProductVersion" "0.1.0.0"

; ==============================================================================
; Installer Sections
; ==============================================================================

Section "Prism Aperture Core" SecCore
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

    ; Install Assets
    SetOutPath "$INSTDIR\assets"
    File "assets\icon.ico"
    File "assets\icon.png"
    File "assets\appv2.png"
    File "assets\aperture.png"
    File "assets\installer.ico"
    File "assets\installer.png"
    File "assets\logo.png"
    File "assets\sidebar.png"
    File "assets\sidebar.bmp"
    File "assets\prismbar.png"
    File "assets\prismbar.bmp"

    SetOutPath "$INSTDIR"

    ; Install and start the Prism Aperture Service
    DetailPrint "Installing Prism Aperture Service..."
    ExecWait '"$INSTDIR\prism-aperture-service.exe" --install'

    DetailPrint "Starting Prism Aperture Service..."
    ExecWait '"$INSTDIR\prism-aperture-service.exe" --start'

    ; Write installation path to registry
    WriteRegStr HKLM "Software\NebulynxGames\PrismSuite\Aperture" "InstallDir" "$INSTDIR"
    WriteRegStr HKLM "Software\Prism Aperture" "InstallDir" "$INSTDIR"

    ; Create uninstaller
    WriteUninstaller "$INSTDIR\Uninstall.exe"

    ; Write Add/Remove Programs registry keys
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismAperture" "DisplayName" "Prism Aperture Server"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismAperture" "DisplayIcon" "$INSTDIR\assets\icon.ico"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismAperture" "DisplayVersion" "0.1.0"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismAperture" "Publisher" "NebulynxGames"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismAperture" "UninstallString" '"$INSTDIR\Uninstall.exe"'
    WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismAperture" "NoModify" 1
    WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismAperture" "NoRepair" 1

    ; Create Start Menu shortcuts
    CreateDirectory "$SMPROGRAMS\NebulynxGames\PrismSuite\Aperture"
    CreateShortcut "$SMPROGRAMS\NebulynxGames\PrismSuite\Aperture\Prism Aperture Server.lnk" "$INSTDIR\prism-aperture-server.exe" "" "$INSTDIR\assets\icon.ico" 0
    CreateShortcut "$SMPROGRAMS\NebulynxGames\PrismSuite\Aperture\Uninstall Prism Aperture.lnk" "$INSTDIR\Uninstall.exe" "" "$INSTDIR\assets\installer.ico" 0

    CreateDirectory "$SMPROGRAMS\Prism Aperture"
    CreateShortcut "$SMPROGRAMS\Prism Aperture\Prism Aperture Server.lnk" "$INSTDIR\prism-aperture-server.exe" "" "$INSTDIR\assets\icon.ico" 0
    CreateShortcut "$SMPROGRAMS\Prism Aperture\Uninstall Prism Aperture.lnk" "$INSTDIR\Uninstall.exe" "" "$INSTDIR\assets\installer.ico" 0

    ; Create Desktop shortcut
    CreateShortcut "$DESKTOP\Prism Aperture Server.lnk" "$INSTDIR\prism-aperture-server.exe" "" "$INSTDIR\assets\icon.ico" 0
SectionEnd

; ==============================================================================
; Uninstaller Section
; ==============================================================================

Section "Uninstall"
    DetailPrint "Stopping Prism Aperture processes and service..."
    ExecWait 'taskkill /F /IM prism-aperture-server.exe'
    ExecWait '"$INSTDIR\prism-aperture-service.exe" --stop'
    ExecWait '"$INSTDIR\prism-aperture-service.exe" --uninstall'

    ; Delete files
    Delete "$INSTDIR\prism-aperture-server.exe"
    Delete "$INSTDIR\prism-aperture-service.exe"
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
    Delete "$SMPROGRAMS\NebulynxGames\PrismSuite\Aperture\Prism Aperture Server.lnk"
    Delete "$SMPROGRAMS\NebulynxGames\PrismSuite\Aperture\Uninstall Prism Aperture.lnk"
    RMDir "$SMPROGRAMS\NebulynxGames\PrismSuite\Aperture"
    RMDir "$SMPROGRAMS\NebulynxGames\PrismSuite"
    RMDir "$SMPROGRAMS\NebulynxGames"

    Delete "$SMPROGRAMS\Prism Aperture\Prism Aperture Server.lnk"
    Delete "$SMPROGRAMS\Prism Aperture\Uninstall Prism Aperture.lnk"
    RMDir "$SMPROGRAMS\Prism Aperture"
    Delete "$DESKTOP\Prism Aperture Server.lnk"

    ; Delete registry keys
    DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PrismAperture"
    DeleteRegKey HKLM "Software\NebulynxGames\PrismSuite\Aperture"
    DeleteRegKey HKLM "Software\NebulynxGames\PrismSuite"
    DeleteRegKey HKLM "Software\NebulynxGames"
    DeleteRegKey HKLM "Software\Prism Aperture"
    DeleteRegKey HKLM "Software\PrismAperture"
SectionEnd
