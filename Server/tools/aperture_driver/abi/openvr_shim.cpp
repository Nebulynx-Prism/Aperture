// Valve's public C++ ABI only. Aperture policy, tracking, state and GPU work are Rust.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <cstring>
#include <cstdio>
#include "../../prism_openvr_driver/vendor/openvr_driver.h"
#include "aperture_core.h"
using namespace vr;
static void driverLog(const char* text){VRDriverLog()->Log(text);}
static DriverPose_t pose(int hand){
    AperturePose source{};aperture_pose(hand,&source);
    DriverPose_t result{};result.qWorldFromDriverRotation.w=1;result.qDriverFromHeadRotation.w=1;result.qRotation.w=1;
    result.deviceIsConnected=source.connected!=0;result.poseIsValid=source.valid!=0;
    result.result=result.poseIsValid?TrackingResult_Running_OK:TrackingResult_Uninitialized;
    if(result.poseIsValid){
        result.qRotation={source.orientation[3],source.orientation[0],source.orientation[1],source.orientation[2]};
        result.poseTimeOffset=source.timeOffset;
        for(int i=0;i<3;i++){result.vecPosition[i]=source.position[i];result.vecVelocity[i]=source.linearVelocity[i];result.vecAngularVelocity[i]=source.angularVelocity[i];}
    }
    return result;
}
class Device:public ITrackedDeviceServerDriver {
public:
    int hand;TrackedDeviceIndex_t index=k_unTrackedDeviceIndexInvalid;PropertyContainerHandle_t properties=0;
    VRInputComponentHandle_t buttons[4]{},axes[4]{};
    explicit Device(int h):hand(h){}
    EVRInitError Activate(uint32_t id) override {
        index=id;properties=VRProperties()->TrackedDeviceToPropertyContainer(id);auto p=VRProperties();
        p->SetStringProperty(properties,Prop_TrackingSystemName_String,"aperture");
        p->SetStringProperty(properties,Prop_ManufacturerName_String,"Nebulynx");
        p->SetStringProperty(properties,Prop_ModelNumber_String,hand<0?"Aperture Quest HMD":"Aperture Touch Controller");
        if(hand>=0){
            p->SetInt32Property(properties,Prop_ControllerRoleHint_Int32,hand?TrackedControllerRole_RightHand:TrackedControllerRole_LeftHand);
            p->SetStringProperty(properties,Prop_ControllerType_String,"oculus_touch");
            p->SetStringProperty(properties,Prop_InputProfilePath_String,"{aperture}/input/touch_profile.json");
            p->SetStringProperty(properties,Prop_RenderModelName_String,hand?"oculus_quest2_controller_right":"oculus_quest2_controller_left");
            const char* keys[4]={hand?"/input/a/click":"/input/x/click",hand?"/input/b/click":"/input/y/click","/input/system/click","/input/joystick/click"};
            const char* values[4]={"/input/trigger/value","/input/grip/value","/input/joystick/x","/input/joystick/y"};
            for(int i=0;i<4;i++){
                if(VRDriverInput()->CreateBooleanComponent(properties,keys[i],&buttons[i])!=VRInputError_None)return VRInitError_Driver_Failed;
                if(VRDriverInput()->CreateScalarComponent(properties,values[i],&axes[i],VRScalarType_Absolute,i<2?VRScalarUnits_NormalizedOneSided:VRScalarUnits_NormalizedTwoSided)!=VRInputError_None)return VRInitError_Driver_Failed;
            }
        }
        return VRInitError_None;
    }
    void Deactivate() override{index=k_unTrackedDeviceIndexInvalid;}
    void EnterStandby() override{}
    void* GetComponent(const char*) override{return nullptr;}
    void DebugRequest(const char*,char* out,uint32_t size) override{if(size)std::snprintf(out,size,"Aperture independent Rust driver; no foreign VR runtime");}
    DriverPose_t GetPose() override{return pose(hand);}
    void update(){
        if(index==k_unTrackedDeviceIndexInvalid)return;auto current=pose(hand);
        VRServerDriverHost()->TrackedDevicePoseUpdated(index,current,sizeof(current));
        if(hand>=0){ApertureInput input{};aperture_input(hand,&input);float values[4]={input.trigger,input.grip,input.stick[0],input.stick[1]};
            for(int i=0;i<4;i++){VRDriverInput()->UpdateBooleanComponent(buttons[i],current.poseIsValid&&((input.buttons>>i)&1u),0);VRDriverInput()->UpdateScalarComponent(axes[i],current.poseIsValid?values[i]:0,0);}}
    }
};
class Display final:public Device,public IVRDisplayComponent,public IVRVirtualDisplay {
    ApertureDisplay config{};
    bool redirect;
public:
    explicit Display(bool isRedirect=false):Device(-1),redirect(isRedirect){}
    EVRInitError Activate(uint32_t id) override {
        if(!aperture_display(&config))return VRInitError_Driver_Failed;
        auto error=Device::Activate(id);if(error!=VRInitError_None)return error;auto p=VRProperties();
        p->SetBoolProperty(properties,Prop_HasVirtualDisplayComponent_Bool,true);p->SetBoolProperty(properties,Prop_IsOnDesktop_Bool,false);
        p->SetBoolProperty(properties,Prop_HasDriverDirectModeComponent_Bool,false);p->SetBoolProperty(properties,Prop_DeviceIsWireless_Bool,true);
        if(!redirect)p->SetUint64Property(properties,Prop_CurrentUniverseId_Uint64,0x41504552);
        if(config.adapterLuid)p->SetUint64Property(properties,Prop_GraphicsAdapterLuid_Uint64,config.adapterLuid);
        p->SetFloatProperty(properties,Prop_UserIpdMeters_Float,config.ipd);p->SetFloatProperty(properties,Prop_DisplayFrequency_Float,float(config.refresh));
        p->SetFloatProperty(properties,Prop_SecondsFromVsyncToPhotons_Float,0);
        return VRInitError_None;
    }
    void* GetComponent(const char* name) override {
        if(!redirect&&!std::strcmp(name,IVRDisplayComponent_Version))return static_cast<IVRDisplayComponent*>(this);
        if(!std::strcmp(name,IVRVirtualDisplay_Version))return static_cast<IVRVirtualDisplay*>(this);return nullptr;
    }
    void GetWindowBounds(int32_t*x,int32_t*y,uint32_t*w,uint32_t*h) override{*x=*y=0;*w=config.panelWidth;*h=config.panelHeight;}
    bool IsDisplayOnDesktop() override{return false;}bool IsDisplayRealDisplay() override{return false;}
    void GetRecommendedRenderTargetSize(uint32_t*w,uint32_t*h) override{*w=config.eyeWidth;*h=config.eyeHeight;}
    void GetEyeOutputViewport(EVREye eye,uint32_t*x,uint32_t*y,uint32_t*w,uint32_t*h) override{*w=config.panelWidth/2;*h=config.panelHeight;*x=eye==Eye_Left?0:*w;*y=0;}
    void GetProjectionRaw(EVREye eye,float*l,float*r,float*t,float*b) override{const float* f=config.projection+(eye==Eye_Left?0:4);*l=f[0];*r=f[1];*t=f[2];*b=f[3];}
    DistortionCoordinates_t ComputeDistortion(EVREye,float u,float v) override{DistortionCoordinates_t out{};out.rfRed[0]=out.rfGreen[0]=out.rfBlue[0]=u;out.rfRed[1]=out.rfGreen[1]=out.rfBlue[1]=v;return out;}
    bool ComputeInverseDistortion(HmdVector2_t*out,EVREye,uint32_t,float u,float v) override{if(!out)return false;out->v[0]=u;out->v[1]=v;return true;}
    void Present(const PresentInfo_t* info,uint32_t bytes) override{if(info&&bytes>=sizeof(*info))aperture_present(uint64_t(info->backbufferTextureHandle),info->nFrameId);}
    void WaitForPresent() override{}
    bool GetTimeSinceLastVsync(float*seconds,uint64_t*frame) override{return aperture_vsync(seconds,frame);}
    void refresh(){ApertureDisplay next{};if(!aperture_display(&next))return;
        if(!redirect&&std::memcmp(next.projection,config.projection,sizeof(config.projection))&&index!=k_unTrackedDeviceIndexInvalid){HmdRect2_t eyes[2]{};for(int i=0;i<2;i++){eyes[i].vTopLeft.v[0]=next.projection[i*4];eyes[i].vTopLeft.v[1]=next.projection[i*4+2];eyes[i].vBottomRight.v[0]=next.projection[i*4+1];eyes[i].vBottomRight.v[1]=next.projection[i*4+3];}VRServerDriverHost()->SetDisplayProjectionRaw(index,eyes[0],eyes[1]);}
        if(properties){if(next.ipd!=config.ipd)VRProperties()->SetFloatProperty(properties,Prop_UserIpdMeters_Float,next.ipd);if(next.refresh!=config.refresh)VRProperties()->SetFloatProperty(properties,Prop_DisplayFrequency_Float,float(next.refresh));}
        if(!redirect&&(next.eyeWidth!=config.eyeWidth||next.eyeHeight!=config.eyeHeight)&&index!=k_unTrackedDeviceIndexInvalid)VRServerDriverHost()->SetRecommendedRenderTargetSize(index,next.eyeWidth,next.eyeHeight);
        config=next;
    }
};
class Provider final:public IServerTrackedDeviceProvider {
    Display hmd,redirect{true};Device left{0},right{1};
    bool registered=false,failed=false;
    void registerWhenObserved(){
        if(registered||failed)return;ApertureDisplay display{};if(!aperture_display(&display))return;
        auto host=VRServerDriverHost();
        if(!host->TrackedDeviceAdded("aperture_hmd",TrackedDeviceClass_HMD,&hmd)||!host->TrackedDeviceAdded("aperture_display_redirect",TrackedDeviceClass_DisplayRedirect,&redirect)||!host->TrackedDeviceAdded("aperture_left",TrackedDeviceClass_Controller,&left)||!host->TrackedDeviceAdded("aperture_right",TrackedDeviceClass_Controller,&right)){
            failed=true;driverLog("Aperture: tracked device registration failed");aperture_stop();return;
        }
        registered=true;
    }
public:
    EVRInitError Init(IVRDriverContext* context) override{
        VR_INIT_SERVER_DRIVER_CONTEXT(context);
        if(!aperture_start(driverLog))return VRInitError_Driver_Failed;
        registered=failed=false;registerWhenObserved();
        if(failed)return VRInitError_Driver_Failed;
        return VRInitError_None;
    }
    void Cleanup() override{aperture_stop();VR_CLEANUP_SERVER_DRIVER_CONTEXT();}
    const char*const* GetInterfaceVersions() override{return k_InterfaceVersions;}
    void RunFrame() override{if(aperture_tick()){registerWhenObserved();if(registered){hmd.refresh();redirect.refresh();hmd.update();left.update();right.update();}}}
    bool ShouldBlockStandbyMode() override{return false;}void EnterStandby() override{}void LeaveStandby() override{}
};
static Provider provider;
extern "C" __declspec(dllexport) void* HmdDriverFactory(const char* interfaceName,int* error){
    if(interfaceName&&!std::strcmp(interfaceName,IServerTrackedDeviceProvider_Version)){if(error)*error=VRInitError_None;return &provider;}
    if(error)*error=VRInitError_Init_InterfaceNotFound;return nullptr;
}
