#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <cstdio>
#include "../vendor/openvr_driver.h"
static bool verify(vr::IVRDisplayComponent* display) {
    // Capture the known-incompatible DLL's access violation as a test result,
    // rather than crashing this test or touching SteamVR.
    __try {
        auto d=display->ComputeDistortion(vr::Eye_Left,.25f,.75f);
        return d.rfRed[0]==.25f && d.rfRed[1]==.75f && d.rfGreen[0]==.25f && d.rfBlue[1]==.75f;
    } __except(EXCEPTION_EXECUTE_HANDLER) { return false; }
}
int main(int argc,char** argv) {
    if(argc!=2) return 2;
    auto dll=LoadLibraryA(argv[1]); if(!dll) { std::printf("LoadLibrary failed %lu\n",GetLastError()); return 2; }
    auto get=reinterpret_cast<vr::IVRDisplayComponent*(*)()>(GetProcAddress(dll,"GetTestDisplay"));
    bool ok=get && verify(get());
    std::printf("MSVC host -> %s: OpenVR struct-return ABI %s\n",argv[1],ok?"PASS":"FAIL");
    FreeLibrary(dll); return ok?0:1;
}
