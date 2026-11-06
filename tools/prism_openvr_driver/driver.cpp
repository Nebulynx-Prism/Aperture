#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <d3d11.h>
#include <dxgi.h>
#include <cmath>
#include <cstring>
#include <cstdio>
#include <mutex>
#include <atomic>
#include <map>
#include "vendor/openvr_driver.h"
#include "prism_ipc.h"
#include "quest_identity.hpp"
static std::atomic<uint32_t> headsetRefresh{72};
using namespace vr;
static void log(const char* s) { VRDriverLog()->Log(s); }
template<class T> static void drop(T*& p) { if(p) p->Release(); p=nullptr; }
struct IPC {
 HANDLE mapping=nullptr, mutex=nullptr; PrismSharedHeader* h=nullptr;
 bool open() {
  mutex=OpenMutexW(SYNCHRONIZE|MUTEX_MODIFY_STATE,FALSE,PRISM_MUTEX_NAME);
  mapping=OpenFileMappingW(FILE_MAP_ALL_ACCESS,FALSE,PRISM_MAPPING_NAME);
  if(mapping) h=(PrismSharedHeader*)MapViewOfFile(mapping,FILE_MAP_ALL_ACCESS,0,0,PRISM_MAPPING_BYTES);
  return h && mutex;
 }
 bool lock(DWORD timeout=10) { if(!mutex)return false; DWORD r=WaitForSingleObject(mutex,timeout); return r==WAIT_OBJECT_0||r==WAIT_ABANDONED; }
 void unlock(){ReleaseMutex(mutex);}
 bool valid(){return h && h->magic==PRISM_MAGIC && h->version==PRISM_ABI_VERSION;}
 uint32_t deviceType(){if(!lock())return 0;uint32_t type=0;if(valid()){auto* d=reinterpret_cast<const PrismDeviceInfo*>(reinterpret_cast<const unsigned char*>(h)+PRISM_DEVICE_INFO_OFFSET);if(d->magic==PRISM_DEVICE_INFO_MAGIC&&d->version==1&&d->hmd_type<=5)type=d->hmd_type;}unlock();return type;}
 bool deviceResolution(uint32_t& width,uint32_t& height){if(!lock())return false;bool ok=false;if(valid()){auto* d=reinterpret_cast<const PrismDeviceInfo*>(reinterpret_cast<const unsigned char*>(h)+PRISM_DEVICE_INFO_OFFSET);if(d->magic==PRISM_DEVICE_INFO_MAGIC&&d->version==1){uint32_t w=d->reserved&0xffff,hgt=d->reserved>>16;if(w>=512&&w<=4096&&hgt>=512&&hgt<=4096){width=w;height=hgt;ok=true;}}}unlock();return ok;}
 bool read(PrismSharedHeader& out,DWORD timeout=10){if(!lock(timeout))return false; bool ok=valid(); if(ok)out=*h; unlock();return ok;}
 void beat(bool ready){if(lock()){if(valid()){h->driver_heartbeat_ms=GetTickCount64();h->driver_ready=ready;}unlock();}}
 void close(){if(h)UnmapViewOfFile(h); if(mapping)CloseHandle(mapping);if(mutex)CloseHandle(mutex);h=nullptr;mapping=mutex=nullptr;}
} ipc;
static bool fresh(const PrismSharedHeader& s){auto now=GetTickCount64();return s.client_connected && s.server_heartbeat_ms && now>=s.server_heartbeat_ms && now-s.server_heartbeat_ms<500 && s.tracking_received_ms && now>=s.tracking_received_ms && now-s.tracking_received_ms<500 && s.tracking.version==1;}
static DriverPose_t pose(const PrismSharedHeader& s,int hand){
 DriverPose_t p{};p.qWorldFromDriverRotation.w=p.qDriverFromHeadRotation.w=p.qRotation.w=1;
 const auto& q=hand<0?s.tracking.head:s.tracking.hands[hand];
 bool ok=fresh(s)&&q.valid; double norm=0;
 for(float v:q.orientation){ok=ok&&std::isfinite(v);norm+=v*v;}for(float v:q.position)ok=ok&&std::isfinite(v);
 ok=ok&&norm>0.25&&norm<4; p.deviceIsConnected=fresh(s)&&q.valid;p.poseIsValid=ok;p.result=ok?TrackingResult_Running_OK:TrackingResult_Uninitialized;
 if(ok){double n=std::sqrt(norm);p.qRotation={q.orientation[3]/n,q.orientation[0]/n,q.orientation[1]/n,q.orientation[2]/n};for(int i=0;i<3;i++)p.vecPosition[i]=q.position[i];if(!(s.tracking.flags&1))p.vecPosition[1]+=1.6;}
 return p;
}
class Device:public ITrackedDeviceServerDriver {
public:
 int hand; TrackedDeviceIndex_t id=k_unTrackedDeviceIndexInvalid; PropertyContainerHandle_t props=0;
 VRInputComponentHandle_t buttons[4]{}, axes[4]{};
 int identityType=-1;
 explicit Device(int h):hand(h){}
 void identify(){if(!props)return;auto type=ipc.deviceType();if(identityType==int(type))return;identityType=int(type);auto identity=prismQuestIdentity(type);auto v=VRProperties();
  char model[128],render[128];
  if(hand<0)std::snprintf(model,sizeof(model),"%s",identity.headset);else std::snprintf(model,sizeof(model),"%s %s (%s)",identity.headset,identity.controller,hand==0?"Left":"Right");
  v->SetStringProperty(props,Prop_ModelNumber_String,model);v->SetStringProperty(props,Prop_ManufacturerName_String,"Meta");
  if(hand>=0){std::snprintf(render,sizeof(render),"%s_%s",identity.render_model,hand==0?"left":"right");v->SetStringProperty(props,Prop_RenderModelName_String,render);}
  char message[192];std::snprintf(message,sizeof(message),"Prism device identity: %s",model);log(message);
 }
 EVRInitError Activate(uint32_t index)override{
  id=index;props=VRProperties()->TrackedDeviceToPropertyContainer(id);auto v=VRProperties();
  v->SetStringProperty(props,Prop_TrackingSystemName_String,"prism_aperture");v->SetStringProperty(props,Prop_ModelNumber_String,hand<0?"Prism Quest HMD":"Prism Quest Controller");
  v->SetStringProperty(props,Prop_ManufacturerName_String,"Meta"); identify();
  if(hand>=0){v->SetInt32Property(props,Prop_ControllerRoleHint_Int32,hand==0?TrackedControllerRole_LeftHand:TrackedControllerRole_RightHand);v->SetStringProperty(props,Prop_ControllerType_String,"oculus_touch");v->SetStringProperty(props,Prop_InputProfilePath_String,"{oculus}/input/touch_profile.json");
   const char* b[]={hand==0?"/input/x/click":"/input/a/click",hand==0?"/input/y/click":"/input/b/click","/input/system/click","/input/joystick/click"};
   const char* a[]={"/input/trigger/value","/input/grip/value","/input/joystick/x","/input/joystick/y"};
   for(int i=0;i<4;i++){if(VRDriverInput()->CreateBooleanComponent(props,b[i],&buttons[i])!=VRInputError_None)log("Prism: boolean component failed");if(VRDriverInput()->CreateScalarComponent(props,a[i],&axes[i],VRScalarType_Absolute,i<2?VRScalarUnits_NormalizedOneSided:VRScalarUnits_NormalizedTwoSided)!=VRInputError_None)log("Prism: scalar component failed");}
  }return VRInitError_None;
 }
 void Deactivate()override{id=k_unTrackedDeviceIndexInvalid;}
 void EnterStandby()override{}
 void* GetComponent(const char*)override{return nullptr;}
 void DebugRequest(const char*,char* out,uint32_t n)override{if(n)std::snprintf(out,n,"Prism IPC ABI 1; real tracking only");}
 DriverPose_t GetPose()override{PrismSharedHeader s{};ipc.read(s);return pose(s,hand);}
 void update(const PrismSharedHeader& s){identify();if(id==k_unTrackedDeviceIndexInvalid)return;auto p=pose(s,hand);VRServerDriverHost()->TrackedDevicePoseUpdated(id,p,sizeof(p));if(hand>=0){bool ok=p.poseIsValid;for(int i=0;i<4;i++)VRDriverInput()->UpdateBooleanComponent(buttons[i],ok&&((s.tracking.buttons[hand]>>i)&1),0);float vals[]={s.tracking.trigger[hand],s.tracking.grip[hand],s.tracking.stick[hand*2],s.tracking.stick[hand*2+1]};for(int i=0;i<4;i++){float f=ok&&std::isfinite(vals[i])?vals[i]:0;f=std::fmax(i<2?0.f:-1.f,std::fmin(1.f,f));VRDriverInput()->UpdateScalarComponent(axes[i],f,0);}}}
};
class Display:public Device,public IVRDisplayComponent,public IVRVirtualDisplay {
 std::mutex gpu; ID3D11Device* dev=nullptr; ID3D11DeviceContext* ctx=nullptr; SharedTextureHandle_t handle=0;
 std::atomic<uint64_t> tick{0},counter{0}; std::atomic<uint32_t> frameWidth{2048},frameHeight{1024}; uint64_t lastLog=0; uint64_t epochMs=0; bool redirect;
 void fail(const char* msg,HRESULT hr){auto now=GetTickCount64();if(now-lastLog>2000){char b[240];std::snprintf(b,sizeof(b),"Prism capture: %s hr=0x%08lx",msg,(unsigned long)hr);log(b);lastLog=now;}}
 struct CaptureSlot { ID3D11Texture2D* stage{}; ID3D11Query* ready{}; uint64_t frame{}; PrismFramePose pose{}; bool pending{}; };
 struct CachedTexture { ID3D11Texture2D* source{}; IDXGIKeyedMutex* keyed{}; CaptureSlot slots[3]{}; UINT next{}; };
 std::map<SharedTextureHandle_t,CachedTexture> textures;
 CachedTexture* current{};
 void reset(){for(auto& item:textures){drop(item.second.keyed);drop(item.second.source);for(auto& s:item.second.slots){drop(s.ready);drop(s.stage);}}textures.clear();current=nullptr;handle=0;}
public:
 explicit Display(bool r):Device(-1),redirect(r){}
 void cleanup(){std::lock_guard<std::mutex> l(gpu);reset();drop(ctx);drop(dev);}
 EVRInitError Activate(uint32_t i)override{Device::Activate(i);epochMs=GetTickCount64();auto v=VRProperties();
 if(redirect){IDXGIFactory1* factory=nullptr;IDXGIAdapter1* adapter=nullptr;if(SUCCEEDED(CreateDXGIFactory1(__uuidof(IDXGIFactory1),(void**)&factory))&&SUCCEEDED(factory->EnumAdapters1(0,&adapter))){DXGI_ADAPTER_DESC1 desc{};if(SUCCEEDED(adapter->GetDesc1(&desc))){uint64_t luid=0;std::memcpy(&luid,&desc.AdapterLuid,sizeof(luid));v->SetUint64Property(props,Prop_GraphicsAdapterLuid_Uint64,luid);}}drop(adapter);drop(factory);}v->SetBoolProperty(props,Prop_HasVirtualDisplayComponent_Bool,true);v->SetBoolProperty(props,Prop_DeviceIsWireless_Bool,true);v->SetBoolProperty(props,Prop_IsOnDesktop_Bool,false);v->SetBoolProperty(props,Prop_HasDriverDirectModeComponent_Bool,false);if(!redirect){v->SetUint64Property(props,Prop_CurrentUniverseId_Uint64,42);v->SetFloatProperty(props,Prop_DisplayFrequency_Float,72);v->SetFloatProperty(props,Prop_UserIpdMeters_Float,.064f);v->SetFloatProperty(props,Prop_SecondsFromVsyncToPhotons_Float,0);}return VRInitError_None;}
 void* GetComponent(const char* n)override{if(!std::strcmp(n,IVRDisplayComponent_Version)&&!redirect)return static_cast<IVRDisplayComponent*>(this);if(!std::strcmp(n,IVRVirtualDisplay_Version)){log(redirect?"Prism: redirect virtual display requested":"Prism: HMD virtual display requested");return static_cast<IVRVirtualDisplay*>(this);}return nullptr;}
 void GetWindowBounds(int32_t*x,int32_t*y,uint32_t*w,uint32_t*h)override{*x=*y=0;*w=frameWidth.load();*h=frameHeight.load();}
 bool IsDisplayOnDesktop()override{return false;}bool IsDisplayRealDisplay()override{return false;}
 void GetRecommendedRenderTargetSize(uint32_t*w,uint32_t*h)override{if(ipc.deviceResolution(*w,*h))return;switch(ipc.deviceType()){case 1:*w=1440;*h=1600;break;case 2:*w=1832;*h=1920;break;case 3:*w=1800;*h=1920;break;case 4:*w=2064;*h=2208;break;case 5:*w=1832;*h=1920;break;default:*w=PRISM_EYE_WIDTH;*h=PRISM_EYE_HEIGHT;break;}}
 void GetEyeOutputViewport(EVREye e,uint32_t*x,uint32_t*y,uint32_t*w,uint32_t*h)override{auto fw=frameWidth.load(),fh=frameHeight.load();*x=e==Eye_Left?0:fw/2;*y=0;*w=fw/2;*h=fh;}
 void GetProjectionRaw(EVREye eye,float*l,float*r,float*t,float*b)override{
  *l=*t=-1.f;*r=*b=1.f;PrismSharedHeader sample{};
  if(ipc.read(sample)&&fresh(sample)){const float* f=sample.tracking.fov+(eye==Eye_Left?0:4);
   if(std::isfinite(f[0])&&std::isfinite(f[1])&&std::isfinite(f[2])&&std::isfinite(f[3])&&f[0]<0&&f[1]>0&&f[2]>0&&f[3]<0&&f[0]>-1.57f&&f[1]<1.57f&&f[2]<1.57f&&f[3]>-1.57f){*l=std::tan(f[0]);*r=std::tan(f[1]);*t=std::tan(f[3]);*b=std::tan(f[2]);}}
 }
 DistortionCoordinates_t ComputeDistortion(EVREye eye,float u,float v)override{static std::atomic<unsigned> calls{0};DistortionCoordinates_t d{};d.rfRed[0]=d.rfGreen[0]=d.rfBlue[0]=u;d.rfRed[1]=d.rfGreen[1]=d.rfBlue[1]=v;if(calls.fetch_add(1)<3){char b[128];std::snprintf(b,sizeof(b),"Prism distortion eye=%d uv=%.4f,%.4f",int(eye),u,v);log(b);}return d;}
 // OpenXR handles the physical lenses; compositor distortion and its inverse are identity.
 // Explicitly provide the inverse: recent SteamVR versions reject
 // distortion surface creation when inverse lookup reports failure.
 bool ComputeInverseDistortion(HmdVector2_t* out,EVREye eye,uint32_t channel,float u,float v)override{static std::atomic<unsigned> calls{0};if(!out||!std::isfinite(u)||!std::isfinite(v))return false;out->v[0]=u;out->v[1]=v;if(calls.fetch_add(1)<8){char b[128];std::snprintf(b,sizeof(b),"Prism inverse eye=%d channel=%u uv=%.4f,%.4f -> %.4f,%.4f",int(eye),channel,u,v,out->v[0],out->v[1]);log(b);}return true;}
 void Present(const PresentInfo_t* p,uint32_t n)override{
  static std::atomic<unsigned> presentCalls{0};if(presentCalls.fetch_add(1)==0)log("Prism: compositor Present callback reached");if(!p||n<sizeof(PresentInfo_t)||!p->backbufferTextureHandle)return;
  // Both the HMD and its redirect can receive a callback for one compositor
  // frame. Export it once, and never queue stale frames behind another copy.
  static std::mutex publishMutex; static uint64_t lastFrame=0; static bool haveFrame=false;
  std::unique_lock<std::mutex> publication(publishMutex,std::try_to_lock);
  if(!publication.owns_lock() || (haveFrame && lastFrame==p->nFrameId))return;
  lastFrame=p->nFrameId;haveFrame=true;
  PrismSharedHeader tracking{};const bool haveTracking=ipc.read(tracking,0)&&fresh(tracking)&&tracking.tracking.head.valid;
  std::lock_guard<std::mutex> lock(gpu);
  if(!dev){HRESULT hr=D3D11CreateDevice(nullptr,D3D_DRIVER_TYPE_HARDWARE,nullptr,D3D11_CREATE_DEVICE_BGRA_SUPPORT,nullptr,0,D3D11_SDK_VERSION,&dev,nullptr,&ctx);if(FAILED(hr)){fail("D3D11CreateDevice",hr);return;}}
  if(handle!=p->backbufferTextureHandle){
   auto found=textures.find(p->backbufferTextureHandle);
   if(found!=textures.end()){current=&found->second;handle=found->first;}
   else {
    if(textures.size()>=8)reset();
    CachedTexture entry{};HRESULT hr=dev->OpenSharedResource((HANDLE)(uintptr_t)p->backbufferTextureHandle,__uuidof(ID3D11Texture2D),(void**)&entry.source);
    if(FAILED(hr)){fail("OpenSharedResource (default adapter)",hr);return;}
    hr=entry.source->QueryInterface(__uuidof(IDXGIKeyedMutex),(void**)&entry.keyed);
    if(FAILED(hr)){fail("shared backbuffer lacks keyed mutex",hr);drop(entry.source);return;}
    D3D11_TEXTURE2D_DESC d{};entry.source->GetDesc(&d);
    bool format=d.Format==DXGI_FORMAT_B8G8R8A8_UNORM||d.Format==DXGI_FORMAT_B8G8R8A8_UNORM_SRGB||d.Format==DXGI_FORMAT_R8G8B8A8_UNORM||d.Format==DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;
    if(!format||d.Width<512||d.Width>PRISM_MAX_WIDTH||d.Width%2||d.Height<256||d.Height>PRISM_MAX_HEIGHT||d.SampleDesc.Count!=1||d.ArraySize!=1||d.MipLevels!=1){fail("unsupported SBS texture size/format/MSAA",E_INVALIDARG);drop(entry.keyed);drop(entry.source);return;}
    d.Usage=D3D11_USAGE_STAGING;d.BindFlags=0;d.CPUAccessFlags=D3D11_CPU_ACCESS_READ;d.MiscFlags=0;
    D3D11_QUERY_DESC qd{D3D11_QUERY_EVENT,0};
    for(auto& slot:entry.slots){hr=dev->CreateTexture2D(&d,nullptr,&slot.stage);if(SUCCEEDED(hr))hr=dev->CreateQuery(&qd,&slot.ready);if(FAILED(hr)){fail("Create asynchronous staging ring",hr);drop(entry.keyed);drop(entry.source);for(auto& s:entry.slots){drop(s.ready);drop(s.stage);}return;}}
    handle=p->backbufferTextureHandle;auto inserted=textures.emplace(handle,std::move(entry)).first;current=&inserted->second;
    log("Prism: cached stereo compositor backbuffer");
   }
  }
  auto& cache=*current;HRESULT hr=cache.keyed->AcquireSync(0,1);if(hr!=S_OK){fail("AcquireSync skipped frame",hr);return;}
  auto& write=cache.slots[cache.next++%3];ctx->CopyResource(write.stage,cache.source);ctx->End(write.ready);HRESULT released=cache.keyed->ReleaseSync(0);if(FAILED(released)){fail("ReleaseSync",released);return;}write.frame=p->nFrameId;write.pending=true;write.pose={};write.pose.magic=PRISM_FRAME_POSE_MAGIC;write.pose.version=1;if(haveTracking){write.pose.head=tracking.tracking.head;std::memcpy(write.pose.fov,tracking.tracking.fov,sizeof(write.pose.fov));write.pose.sample_time_us=tracking.tracking.sample_time_us;}
  CaptureSlot* ready=nullptr;for(auto& slot:cache.slots){if(!slot.pending)continue;BOOL done=FALSE;hr=ctx->GetData(slot.ready,&done,sizeof(done),D3D11_ASYNC_GETDATA_DONOTFLUSH);if(hr==S_OK&&done&&(!ready||slot.frame>ready->frame))ready=&slot;}
  if(!ready)return;D3D11_MAPPED_SUBRESOURCE m{};hr=ctx->Map(ready->stage,0,D3D11_MAP_READ,D3D11_MAP_FLAG_DO_NOT_WAIT,&m);if(FAILED(hr)){if(hr!=DXGI_ERROR_WAS_STILL_DRAWING)fail("Nonblocking staging Map",hr);return;}
  D3D11_TEXTURE2D_DESC d{};ready->stage->GetDesc(&d);bool rgba=d.Format==DXGI_FORMAT_R8G8B8A8_UNORM||d.Format==DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;
  bool published=false;if(ipc.lock(1)){if(ipc.valid()){auto dst=(unsigned char*)ipc.h+PRISM_PIXEL_OFFSET;for(UINT y=0;y<d.Height;y++){auto src=(const unsigned char*)m.pData+y*m.RowPitch;auto row=dst+y*d.Width*4;if(!rgba)std::memcpy(row,src,d.Width*4);else for(UINT x=0;x<d.Width;x++){row[x*4]=src[x*4+2];row[x*4+1]=src[x*4+1];row[x*4+2]=src[x*4];row[x*4+3]=src[x*4+3];}}ipc.h->frame_width=d.Width;ipc.h->frame_height=d.Height;ipc.h->frame_stride=d.Width*4;ipc.h->frame_format=0;ipc.h->frame_time_ms=GetTickCount64();++ipc.h->frame_sequence;ready->pose.frame_sequence=ipc.h->frame_sequence;auto* pose=reinterpret_cast<PrismFramePose*>(reinterpret_cast<unsigned char*>(ipc.h)+PRISM_FRAME_POSE_OFFSET);*pose=ready->pose;published=true;}ipc.unlock();}ctx->Unmap(ready->stage,0);for(auto& slot:cache.slots)if(slot.frame<=ready->frame)slot.pending=false;if(published){frameWidth=d.Width;frameHeight=d.Height;tick=GetTickCount64();counter=ready->frame;}
 }
  void WaitForPresent()override{} // OpenXR paces the physical headset; never sleep the compositor thread here.
 bool GetTimeSinceLastVsync(float*s,uint64_t*f)override{if(!s||!f||!epochMs)return false;const double interval=1000.0/headsetRefresh.load();const double elapsed=double(GetTickCount64()-epochMs);*s=float(std::fmod(elapsed,interval)/1000.0);*f=uint64_t(elapsed/interval);return true;}
};
class Provider:public IServerTrackedDeviceProvider {
 Display hmd{false},redirect{true};Device left{0},right{1};bool connected=false;float lastIpd=0;uint32_t lastHz=0,lastRecommendedWidth=0,lastRecommendedHeight=0;float lastFov[8]{};
public:
 EVRInitError Init(IVRDriverContext*c)override{VR_INIT_SERVER_DRIVER_CONTEXT(c);log("Prism: native provider init, IPC ABI 1");if(!ipc.open()){log("Prism: server-owned mapping/mutex missing");ipc.close();return VRInitError_Driver_Failed;}PrismSharedHeader s{};if(!ipc.read(s)){ipc.close();return VRInitError_Driver_Failed;}
  auto host=VRServerDriverHost();bool ok=host->TrackedDeviceAdded("prism_hmd",TrackedDeviceClass_HMD,&hmd);ok=host->TrackedDeviceAdded("prism_display_redirect",TrackedDeviceClass_DisplayRedirect,&redirect)&&ok;ok=host->TrackedDeviceAdded("prism_left",TrackedDeviceClass_Controller,&left)&&ok;ok=host->TrackedDeviceAdded("prism_right",TrackedDeviceClass_Controller,&right)&&ok;if(!ok){log("Prism: TrackedDeviceAdded failed");ipc.close();return VRInitError_Driver_Failed;}ipc.beat(true);log("Prism: provider ready");return VRInitError_None;}
 void Cleanup()override{redirect.cleanup();hmd.cleanup();ipc.beat(false);ipc.close();VR_CLEANUP_SERVER_DRIVER_CONTEXT();}
 const char*const* GetInterfaceVersions()override{return k_InterfaceVersions;}
 void RunFrame()override{ipc.beat(true);PrismSharedHeader s{};ipc.read(s);bool live=fresh(s);if(live!=connected){log(live?"Prism: client tracking fresh":"Prism: tracking stale/disconnected");connected=live;}hmd.update(s);redirect.update(s);left.update(s);right.update(s);
 uint32_t eyeWidth=0,eyeHeight=0;if(hmd.id!=k_unTrackedDeviceIndexInvalid&&ipc.deviceResolution(eyeWidth,eyeHeight)&&(eyeWidth!=lastRecommendedWidth||eyeHeight!=lastRecommendedHeight)){VRServerDriverHost()->SetRecommendedRenderTargetSize(hmd.id,eyeWidth,eyeHeight);lastRecommendedWidth=eyeWidth;lastRecommendedHeight=eyeHeight;char msg[128];std::snprintf(msg,sizeof(msg),"Prism: dynamic per-eye target set to %ux%u",eyeWidth,eyeHeight);log(msg);}
  if(live&&hmd.id!=k_unTrackedDeviceIndexInvalid){
 bool changed=false;for(int i=0;i<8;i++)if(std::abs(lastFov[i]-s.tracking.fov[i])>0.001f)changed=true;
 if(changed){HmdRect2_t rect[2]{};for(int eye=0;eye<2;eye++){float l,r,t,b;hmd.GetProjectionRaw(eye==0?Eye_Left:Eye_Right,&l,&r,&t,&b);rect[eye].vTopLeft.v[0]=l;rect[eye].vTopLeft.v[1]=t;rect[eye].vBottomRight.v[0]=r;rect[eye].vBottomRight.v[1]=b;}VRServerDriverHost()->SetDisplayProjectionRaw(hmd.id,rect[0],rect[1]);std::memcpy(lastFov,s.tracking.fov,sizeof(lastFov));}
 float ipd=s.tracking.ipd;if(std::isfinite(ipd)&&ipd>.04f&&ipd<.09f&&ipd!=lastIpd){lastIpd=ipd;VRProperties()->SetFloatProperty(hmd.props,Prop_UserIpdMeters_Float,ipd);HmdMatrix34_t a{},b{};for(int i=0;i<3;i++)a.m[i][i]=b.m[i][i]=1;a.m[0][3]=-ipd/2;b.m[0][3]=ipd/2;VRServerDriverHost()->SetDisplayEyeToHead(hmd.id,a,b);}auto hz=s.tracking.refresh_rate;if(hz>=60&&hz<=144&&hz!=lastHz){lastHz=hz;headsetRefresh=hz;VRProperties()->SetFloatProperty(hmd.props,Prop_DisplayFrequency_Float,float(hz));}}
 }
 bool ShouldBlockStandbyMode()override{return false;}void EnterStandby()override{}void LeaveStandby()override{}
};
static Provider provider;
extern "C" __declspec(dllexport) void* HmdDriverFactory(const char* name,int* error){if(name&&!std::strcmp(name,IServerTrackedDeviceProvider_Version)){if(error)*error=VRInitError_None;return &provider;}if(error)*error=VRInitError_Init_InterfaceNotFound;return nullptr;}

