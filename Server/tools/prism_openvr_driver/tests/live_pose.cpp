#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <cmath>
#include <cstdio>
#include "../../prism_vdxr/upstream/external/openvr/headers/openvr.h"
#include "../prism_ipc.h"
int main() {
    auto library=LoadLibraryW(L"C:/Program Files (x86)/Steam/steamapps/common/SteamVR/bin/win64/openvr_api.dll");
    if(!library) return 2;
    auto init=reinterpret_cast<uint32_t(*)(vr::EVRInitError*,vr::EVRApplicationType,const char*)>(GetProcAddress(library,"VR_InitInternal2"));
    auto get=reinterpret_cast<void*(*)(const char*,vr::EVRInitError*)>(GetProcAddress(library,"VR_GetGenericInterface"));
    auto shutdown=reinterpret_cast<void(*)()>(GetProcAddress(library,"VR_ShutdownInternal"));
    if(!init || !get || !shutdown) return 2;
    vr::EVRInitError error=vr::VRInitError_None; init(&error,vr::VRApplication_Background,nullptr);
    if(error) { std::printf("OpenVR init error %d\n",int(error)); return 2; }
    auto* system=static_cast<vr::IVRSystem*>(get(vr::IVRSystem_Version,&error));
    if(!system || error) return 2;
    for(unsigned i=0;i<vr::k_unMaxTrackedDeviceCount;i++) {
        if(!system->IsTrackedDeviceConnected(i)) continue;
        char model[256]{},type[256]{};
        system->GetStringTrackedDeviceProperty(i,vr::Prop_ModelNumber_String,model,sizeof(model));
        system->GetStringTrackedDeviceProperty(i,vr::Prop_ControllerType_String,type,sizeof(type));
        std::printf("device %u model=%s controller_type=%s\n",i,model,type);
    }
    auto mapping=OpenFileMappingW(FILE_MAP_READ,FALSE,PRISM_MAPPING_NAME);
    auto mutex=OpenMutexW(SYNCHRONIZE|MUTEX_MODIFY_STATE,FALSE,PRISM_MUTEX_NAME);
    auto* header=mapping?static_cast<const PrismSharedHeader*>(MapViewOfFile(mapping,FILE_MAP_READ,0,0,sizeof(PrismSharedHeader))):nullptr;
    if(!header || !mutex) return 2;
    unsigned valid=0; double directError=0,inverseError=0;
    for(unsigned n=0;n<90;n++) {
        vr::TrackedDevicePose_t poses[vr::k_unMaxTrackedDeviceCount]{};
        system->GetDeviceToAbsoluteTrackingPose(vr::TrackingUniverseRawAndUncalibrated,0,poses,vr::k_unMaxTrackedDeviceCount);
        PrismSharedHeader h{};
        if(WaitForSingleObject(mutex,100)==WAIT_OBJECT_0) { h=*header; ReleaseMutex(mutex); }
        if(h.tracking.head.valid && poses[0].bPoseIsValid) {
            auto& q=h.tracking.head.orientation; double x=q[0],y=q[1],z=q[2],w=q[3];
            double r[3][3]={{1-2*(y*y+z*z),2*(x*y-z*w),2*(x*z+y*w)},
                {2*(x*y+z*w),1-2*(x*x+z*z),2*(y*z-x*w)},
                {2*(x*z-y*w),2*(y*z+x*w),1-2*(x*x+y*y)}};
            double d=0,inv=0;
            for(int row=0;row<3;row++) for(int col=0;col<3;col++) {
                auto actual=poses[0].mDeviceToAbsoluteTracking.m[row][col];
                d+=(actual-r[row][col])*(actual-r[row][col]);
                inv+=(actual-r[col][row])*(actual-r[col][row]);
            }
            directError+=std::sqrt(d); inverseError+=std::sqrt(inv); valid++;
        }
        Sleep(10);
    }
    std::printf("pose_samples=%u average_direct_rotation_error=%.5f inverted_rotation_error=%.5f\n",valid,valid?directError/valid:0,valid?inverseError/valid:0);
    UnmapViewOfFile(header); CloseHandle(mutex); CloseHandle(mapping); shutdown(); FreeLibrary(library);
    return valid && directError/valid<.15?0:1;
}
