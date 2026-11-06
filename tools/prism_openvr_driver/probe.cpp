#include "driver.cpp"
#include <cassert>
int main(){
 PrismSharedHeader s{};s.client_connected=1;s.server_heartbeat_ms=s.tracking_received_ms=GetTickCount64();s.tracking.version=1;s.tracking.head.valid=1;s.tracking.head.orientation[3]=1;s.tracking.head.position[1]=.2f;
 auto p=pose(s,-1);assert(p.poseIsValid&&p.deviceIsConnected);assert(std::abs(p.vecPosition[1]-1.8)<.0001);
 s.tracking.flags=1;p=pose(s,-1);assert(std::abs(p.vecPosition[1]-.2)<.0001);
 s.tracking_received_ms-=501;p=pose(s,-1);assert(!p.poseIsValid&&!p.deviceIsConnected);
 s.tracking_received_ms=GetTickCount64();s.tracking.head.orientation[3]=NAN;assert(!pose(s,-1).poseIsValid);
 s.client_connected=0;assert(!pose(s,-1).deviceIsConnected);
 Display d(false);uint32_t x,y,w,h;d.GetEyeOutputViewport(Eye_Right,&x,&y,&w,&h);assert(x==1024&&y==0&&w==1024&&h==1024);assert(!d.IsDisplayOnDesktop()&&!d.IsDisplayRealDisplay());auto uv=d.ComputeDistortion(Eye_Left,.2f,.7f);assert(uv.rfBlue[0]==.2f&&uv.rfRed[1]==.7f);
 // Open-only IPC: this executable temporarily acts as the server, refusing to
 // interfere with an existing runtime mapping.
 HANDLE mapping=CreateFileMappingW(INVALID_HANDLE_VALUE,nullptr,PAGE_READWRITE,0,PRISM_MAPPING_BYTES,PRISM_MAPPING_NAME);assert(mapping&&GetLastError()!=ERROR_ALREADY_EXISTS);
 HANDLE mutex=CreateMutexW(nullptr,FALSE,PRISM_MUTEX_NAME);assert(mutex&&GetLastError()!=ERROR_ALREADY_EXISTS);
 auto header=(PrismSharedHeader*)MapViewOfFile(mapping,FILE_MAP_ALL_ACCESS,0,0,PRISM_MAPPING_BYTES);assert(header);header->magic=PRISM_MAGIC;header->version=PRISM_ABI_VERSION;
 assert(ipc.open());ipc.beat(true);PrismSharedHeader snapshot{};assert(ipc.read(snapshot)&&snapshot.driver_ready&&snapshot.driver_heartbeat_ms);ipc.beat(false);assert(ipc.read(snapshot)&&!snapshot.driver_ready);ipc.close();UnmapViewOfFile(header);CloseHandle(mutex);CloseHandle(mapping);
 auto dll=LoadLibraryW(L"driver/bin/win64/driver_prism_aperture.dll");assert(dll);auto factory=(void*(*)(const char*,int*))GetProcAddress(dll,"HmdDriverFactory");assert(factory);int error=-1;assert(factory(IServerTrackedDeviceProvider_Version,&error)&&error==VRInitError_None);assert(!factory("unsupported",&error)&&error==VRInitError_Init_InterfaceNotFound);FreeLibrary(dll);
 std::puts("PASS: tracking validity/floor origin, display, IPC lifecycle, DLL factory");
}
