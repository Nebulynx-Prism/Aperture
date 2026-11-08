#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <cmath>
#include <cstring>
#include <cstdio>
#include <atomic>
#include "vendor/openvr_driver.h"
#include "prism_ipc.h"
#include "rust_core/prism_core.h"

// MSVC ABI shell only. Driver policy, persistent state, IPC and GPU capture live in Rust.
using namespace vr;
static_assert(sizeof(PrismCorePose)==sizeof(PrismPose), "Rust/C++ pose FFI layout");
static_assert(sizeof(PrismCoreFramePose)==sizeof(PrismFramePose), "Rust/C++ frame pose FFI layout");
static_assert(sizeof(PrismCoreDriverPose)==64, "Rust/C++ tracked pose FFI layout");
static_assert(sizeof(PrismRuntimeTick)==76, "Rust/C++ provider tick FFI layout");
static_assert(sizeof(PrismRuntimeInput)==20, "Rust/C++ input FFI layout");
static_assert(sizeof(PrismControllerConfig)==500, "Rust/C++ controller config FFI layout");
static_assert(sizeof(PrismDisplayProperties)==40, "Rust/C++ display config FFI layout");
static void log(const char* s) { VRDriverLog()->Log(s); }
static DriverPose_t makeOpenVrPose(const PrismCoreDriverPose& data){
 DriverPose_t p{};p.qWorldFromDriverRotation.w=p.qDriverFromHeadRotation.w=p.qRotation.w=1;
 p.deviceIsConnected=data.connected!=0;p.poseIsValid=data.valid!=0;p.result=p.poseIsValid?TrackingResult_Running_OK:TrackingResult_Uninitialized;
 if(p.poseIsValid){p.qRotation={data.orientation_xyzw[3],data.orientation_xyzw[0],data.orientation_xyzw[1],data.orientation_xyzw[2]};for(int i=0;i<3;i++)p.vecPosition[i]=data.position[i];}
 return p;
}
class Device:public ITrackedDeviceServerDriver {
public:
 int hand; TrackedDeviceIndex_t id=k_unTrackedDeviceIndexInvalid; PropertyContainerHandle_t props=0;
 VRInputComponentHandle_t buttons[4]{}, axes[4]{};
 int identityType=-1;
 explicit Device(int h):hand(h){}
 void identify(uint32_t type){if(!props)return;if(identityType==int(type))return;identityType=int(type);auto v=VRProperties();char model[128]{},render[128]{};prism_runtime_identity(type,hand,model,sizeof(model),render,sizeof(render));
  v->SetStringProperty(props,Prop_ModelNumber_String,model);v->SetStringProperty(props,Prop_ManufacturerName_String,"Meta");
  if(hand>=0)v->SetStringProperty(props,Prop_RenderModelName_String,render);
  char message[192];std::snprintf(message,sizeof(message),"Prism device identity: %s",model);log(message);
 }
 EVRInitError Activate(uint32_t index)override{
  id=index;props=VRProperties()->TrackedDeviceToPropertyContainer(id);auto v=VRProperties();
  v->SetStringProperty(props,Prop_TrackingSystemName_String,"prism_aperture");v->SetStringProperty(props,Prop_ModelNumber_String,hand<0?"Prism Quest HMD":"Prism Quest Controller");
  v->SetStringProperty(props,Prop_ManufacturerName_String,"Meta"); identify(prism_runtime_device_type());
  if(hand>=0){PrismControllerConfig config{};prism_runtime_controller_config(hand,&config);v->SetInt32Property(props,Prop_ControllerRoleHint_Int32,config.roleHint);v->SetStringProperty(props,Prop_ControllerType_String,config.controllerType);v->SetStringProperty(props,Prop_InputProfilePath_String,config.profile);
   for(int i=0;i<4;i++){if(VRDriverInput()->CreateBooleanComponent(props,config.buttons[i],&buttons[i])!=VRInputError_None)log("Prism: boolean component failed");if(VRDriverInput()->CreateScalarComponent(props,config.axes[i],&axes[i],VRScalarType_Absolute,config.scalarUnits[i]==0?VRScalarUnits_NormalizedOneSided:VRScalarUnits_NormalizedTwoSided)!=VRInputError_None)log("Prism: scalar component failed");}
  }return VRInitError_None;
 }
 void Deactivate()override{id=k_unTrackedDeviceIndexInvalid;}
 void EnterStandby()override{}
 void* GetComponent(const char*)override{return nullptr;}
 void DebugRequest(const char*,char* out,uint32_t n)override{if(n)std::snprintf(out,n,"Prism IPC ABI 1; real tracking only");}
 DriverPose_t GetPose()override{PrismCoreDriverPose data{};prism_runtime_pose(hand,&data);return makeOpenVrPose(data);}
 void update(uint32_t type){identify(type);if(id==k_unTrackedDeviceIndexInvalid)return;PrismCoreDriverPose data{};prism_runtime_pose(hand,&data);auto p=makeOpenVrPose(data);VRServerDriverHost()->TrackedDevicePoseUpdated(id,p,sizeof(p));if(hand>=0){PrismRuntimeInput input{};prism_runtime_input(hand,&input);for(int i=0;i<4;i++)VRDriverInput()->UpdateBooleanComponent(buttons[i],p.poseIsValid&&((input.buttons>>i)&1),0);for(int i=0;i<4;i++)VRDriverInput()->UpdateScalarComponent(axes[i],input.axes[i],0);}}
};
class Display:public Device,public IVRDisplayComponent,public IVRVirtualDisplay {
 std::atomic<uint64_t> activeAdapterLuid{0}; bool redirect;
public:
 explicit Display(bool r):Device(-1),redirect(r){}
 EVRInitError Activate(uint32_t i)override{Device::Activate(i);auto v=VRProperties();PrismDisplayProperties config{};prism_runtime_display_properties(redirect,&config);
  uint64_t luid=0;if(prism_gpu_preferred_luid(&luid))v->SetUint64Property(props,Prop_GraphicsAdapterLuid_Uint64,luid);v->SetBoolProperty(props,Prop_HasVirtualDisplayComponent_Bool,config.hasVirtualDisplay!=0);v->SetBoolProperty(props,Prop_DeviceIsWireless_Bool,config.wireless!=0);v->SetBoolProperty(props,Prop_IsOnDesktop_Bool,config.isOnDesktop!=0);v->SetBoolProperty(props,Prop_HasDriverDirectModeComponent_Bool,config.directMode!=0);if(!redirect){v->SetUint64Property(props,Prop_CurrentUniverseId_Uint64,config.universeId);v->SetFloatProperty(props,Prop_DisplayFrequency_Float,config.refreshRate);v->SetFloatProperty(props,Prop_UserIpdMeters_Float,config.ipd);v->SetFloatProperty(props,Prop_SecondsFromVsyncToPhotons_Float,config.secondsToPhotons);}return VRInitError_None;}
 void* GetComponent(const char* n)override{if(!std::strcmp(n,IVRDisplayComponent_Version)&&!redirect)return static_cast<IVRDisplayComponent*>(this);if(!std::strcmp(n,IVRVirtualDisplay_Version)){log(redirect?"Prism: redirect virtual display requested":"Prism: HMD virtual display requested");return static_cast<IVRVirtualDisplay*>(this);}return nullptr;}
 void GetWindowBounds(int32_t*x,int32_t*y,uint32_t*w,uint32_t*h)override{*x=*y=0;prism_gpu_dimensions(w,h);}
 bool IsDisplayOnDesktop()override{return false;}bool IsDisplayRealDisplay()override{return false;}
 void GetRecommendedRenderTargetSize(uint32_t*w,uint32_t*h)override{if(!prism_runtime_recommended(w,h)){*w=PRISM_EYE_WIDTH;*h=PRISM_EYE_HEIGHT;}}
 void GetEyeOutputViewport(EVREye e,uint32_t*x,uint32_t*y,uint32_t*w,uint32_t*h)override{prism_gpu_eye_viewport(e==Eye_Left?0u:1u,x,y,w,h);}
 void GetProjectionRaw(EVREye eye,float*l,float*r,float*t,float*b)override{
  float p[4]{-1.f,1.f,-1.f,1.f};prism_runtime_projection(eye==Eye_Left?0u:1u,p);*l=p[0];*r=p[1];*t=p[2];*b=p[3];
 }
 DistortionCoordinates_t ComputeDistortion(EVREye,float u,float v)override{DistortionCoordinates_t d{};float coordinates[6]{};prism_runtime_distortion(u,v,coordinates);for(int i=0;i<2;i++){d.rfRed[i]=coordinates[i];d.rfGreen[i]=coordinates[2+i];d.rfBlue[i]=coordinates[4+i];}return d;}
 // OpenXR handles the physical lenses; compositor distortion and its inverse are identity.
 // Explicitly provide the inverse: recent SteamVR versions reject
 // distortion surface creation when inverse lookup reports failure.
 bool ComputeInverseDistortion(HmdVector2_t* out,EVREye,uint32_t,float u,float v)override{return out&&prism_runtime_inverse_distortion(u,v,out->v);}
 void Present(const PresentInfo_t* p,uint32_t n)override{
  if(!p||n<sizeof(PresentInfo_t)||!p->backbufferTextureHandle)return;
  PrismCoreFramePose framePose{};prism_runtime_frame_pose(&framePose);
  uint32_t width=0,height=0;uint64_t luid=0;
  const uint64_t sequence=prism_gpu_present((uint64_t)p->backbufferTextureHandle,p->nFrameId,&framePose,&width,&height,&luid);
  if(sequence&&luid&&activeAdapterLuid.exchange(luid)!=luid)VRProperties()->SetUint64Property(props,Prop_GraphicsAdapterLuid_Uint64,luid);
 }
  void WaitForPresent()override{} // OpenXR paces the physical headset; never sleep the compositor thread here.
 bool GetTimeSinceLastVsync(float*s,uint64_t*f)override{return prism_runtime_vsync(s,f);}
};
class Provider:public IServerTrackedDeviceProvider {
 Display hmd{false},redirect{true};Device left{0},right{1};
public:
 EVRInitError Init(IVRDriverContext*c)override{
  VR_INIT_SERVER_DRIVER_CONTEXT(c);prism_gpu_set_logger(log);log("Prism: Rust driver core init, Prism IPC ABI 1");
  if(!prism_runtime_start()){log("Prism: Rust core could not open server mapping/mutex");return VRInitError_Driver_Failed;}
  uint32_t width=PRISM_EYE_WIDTH,height=PRISM_EYE_HEIGHT;prism_runtime_recommended(&width,&height);
  auto host=VRServerDriverHost();bool ok=host->TrackedDeviceAdded("prism_hmd",TrackedDeviceClass_HMD,&hmd);
  ok=host->TrackedDeviceAdded("prism_display_redirect",TrackedDeviceClass_DisplayRedirect,&redirect)&&ok;
  ok=host->TrackedDeviceAdded("prism_left",TrackedDeviceClass_Controller,&left)&&ok;
  ok=host->TrackedDeviceAdded("prism_right",TrackedDeviceClass_Controller,&right)&&ok;
  if(!ok){log("Prism: TrackedDeviceAdded failed");prism_runtime_cleanup();return VRInitError_Driver_Failed;}
  if(hmd.id!=k_unTrackedDeviceIndexInvalid)host->SetRecommendedRenderTargetSize(hmd.id,width,height);
  log("Prism: Rust-backed provider ready");return VRInitError_None;
 }
 void Cleanup()override{prism_runtime_cleanup();VR_CLEANUP_SERVER_DRIVER_CONTEXT();}
 const char*const* GetInterfaceVersions()override{return k_InterfaceVersions;}
 void RunFrame()override{
  PrismRuntimeTick state{};if(!prism_runtime_tick(&state))return;
  if(state.connection_changed)log(state.connected?"Prism: client tracking fresh":"Prism: tracking stale/disconnected");
  hmd.update(state.device_type);redirect.update(state.device_type);left.update(state.device_type);right.update(state.device_type);
  if(state.recommendation_changed&&hmd.id!=k_unTrackedDeviceIndexInvalid){VRServerDriverHost()->SetRecommendedRenderTargetSize(hmd.id,state.recommended_width,state.recommended_height);}
  if(state.connected&&hmd.id!=k_unTrackedDeviceIndexInvalid){
   if(state.fov_changed){HmdRect2_t rect[2]{};for(uint32_t eye=0;eye<2;eye++){float p[4]{};if(!prism_runtime_projection(eye,p))continue;rect[eye].vTopLeft.v[0]=p[0];rect[eye].vTopLeft.v[1]=p[2];rect[eye].vBottomRight.v[0]=p[1];rect[eye].vBottomRight.v[1]=p[3];}VRServerDriverHost()->SetDisplayProjectionRaw(hmd.id,rect[0],rect[1]);}
   if(state.ipd_changed){VRProperties()->SetFloatProperty(hmd.props,Prop_UserIpdMeters_Float,state.ipd);HmdMatrix34_t a{},b{};for(int i=0;i<3;i++)a.m[i][i]=b.m[i][i]=1;a.m[0][3]=-state.ipd/2;b.m[0][3]=state.ipd/2;VRServerDriverHost()->SetDisplayEyeToHead(hmd.id,a,b);}
   if(state.refresh_changed)VRProperties()->SetFloatProperty(hmd.props,Prop_DisplayFrequency_Float,float(state.refresh_rate));
  }
 }
 bool ShouldBlockStandbyMode()override{return false;}void EnterStandby()override{}void LeaveStandby()override{}
};
static Provider provider;
extern "C" __declspec(dllexport) void* HmdDriverFactory(const char* name,int* error){if(name&&!std::strcmp(name,IServerTrackedDeviceProvider_Version)){if(error)*error=VRInitError_None;return &provider;}if(error)*error=VRInitError_Init_InterfaceNotFound;return nullptr;}

