#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <cassert>
#include <cstdio>
#include "../../prism_openvr_driver/vendor/openvr_driver.h"
int wmain(int argc,wchar_t**argv){
    assert(argc==2);auto library=LoadLibraryExW(argv[1],nullptr,LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR|LOAD_LIBRARY_SEARCH_SYSTEM32);assert(library);
    auto getDevice=reinterpret_cast<vr::ITrackedDeviceServerDriver*(*)()>(GetProcAddress(library,"ApertureTestDevice"));
    auto getDisplay=reinterpret_cast<vr::IVRDisplayComponent*(*)()>(GetProcAddress(library,"ApertureTestDisplay"));assert(getDevice&&getDisplay);
    auto device=getDevice();auto display=getDisplay();
    for(int i=0;i<1000;i++){
        volatile uint64_t canary=0x123456789abcdef0;
        auto pose=device->GetPose();assert(!pose.poseIsValid);assert(pose.qRotation.w==1.);assert(pose.qWorldFromDriverRotation.w==1.);
        auto distortion=display->ComputeDistortion(vr::Eye_Left,0.2f,0.7f);
        assert(distortion.rfRed[0]==0.2f&&distortion.rfGreen[1]==0.7f&&distortion.rfBlue[0]==0.2f);
        assert(canary==0x123456789abcdef0);
    }
    assert(device->GetComponent("unavailable")==nullptr);FreeLibrary(library);
    std::puts("PASS: Aperture's actual MSVC member callbacks preserve pose/distortion struct-return ABI; no server or headset fixture used.");return 0;
}
