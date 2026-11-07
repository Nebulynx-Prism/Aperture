// Prism backend for the MIT-licensed VDXR runtime. This replaces the LibOVR
// transport shim; it never loads Virtual Desktop or Oculus runtime binaries.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <d3d11.h>
#include <dxgi1_2.h>
#include <d3dcompiler.h>
#include <OVR_CAPI.h>
#include <OVR_CAPI_D3D.h>
#include <OVR_CAPI_Audio.h>
#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstring>
#include <mutex>
#include <thread>
#include <vector>
#include "../prism_openvr_driver/prism_ipc.h"
#include "../prism_openvr_driver/quest_identity.hpp"

namespace prism {
#ifdef PRISM_VDXR_TEST
constexpr wchar_t MappingName[] = L"Local\\PrismAperture.VDXR.Test.v1";
constexpr wchar_t MutexName[] = L"Local\\PrismAperture.VDXR.Test.Mutex.v1";
#else
constexpr auto MappingName = PRISM_MAPPING_NAME;
constexpr auto MutexName = PRISM_MUTEX_NAME;
#endif
template<class T> void release(T*& p) { if (p) p->Release(); p=nullptr; }
double now() {
    LARGE_INTEGER t{}, f{}; QueryPerformanceCounter(&t); QueryPerformanceFrequency(&f);
    return double(t.QuadPart)/double(f.QuadPart);
}
struct Ipc {
    HANDLE mapping{}, mutex{}; PrismSharedHeader* header{};
    bool open() {
        if (header) return true;
        mutex=OpenMutexW(SYNCHRONIZE|MUTEX_MODIFY_STATE,FALSE,MutexName);
        mapping=OpenFileMappingW(FILE_MAP_ALL_ACCESS,FALSE,MappingName);
        if(mapping) header=static_cast<PrismSharedHeader*>(MapViewOfFile(mapping,FILE_MAP_ALL_ACCESS,0,0,PRISM_MAPPING_BYTES));
        if(!mutex || !header) { close(); return false; }
        return true;
    }
    bool lock() const {
        if(!mutex) return false;
        DWORD r=WaitForSingleObject(mutex,10); return r==WAIT_OBJECT_0 || r==WAIT_ABANDONED;
    }
    bool valid() const { return header && header->magic==PRISM_MAGIC && header->version==PRISM_ABI_VERSION; }
    bool read(PrismSharedHeader& out) const {
        if(!lock()) return false;
        bool ok=valid(); if(ok) out=*header; ReleaseMutex(mutex); return ok;
    }
    uint32_t deviceType() const {
        if(!lock()) return 0;
        uint32_t type=0;
        if(valid()) {
            auto* d=reinterpret_cast<const PrismDeviceInfo*>(reinterpret_cast<const uint8_t*>(header)+PRISM_DEVICE_INFO_OFFSET);
            if(d->magic==PRISM_DEVICE_INFO_MAGIC && d->version==1 && d->hmd_type<=5) type=d->hmd_type;
        }
        ReleaseMutex(mutex); return type;
    }
    void heartbeat(bool ready) const {
        if(lock()) { if(valid()) { header->driver_ready=ready; header->driver_heartbeat_ms=GetTickCount64(); } ReleaseMutex(mutex); }
    }
    void close() {
        if(header) UnmapViewOfFile(header); if(mapping) CloseHandle(mapping); if(mutex) CloseHandle(mutex);
        header=nullptr; mapping=mutex=nullptr;
    }
};
bool fresh(const PrismSharedHeader& h) {
    const auto t=GetTickCount64();
    return h.client_connected && h.tracking.version==1 && h.server_heartbeat_ms && h.tracking_received_ms
        && t>=h.server_heartbeat_ms && t-h.server_heartbeat_ms<500
        && t>=h.tracking_received_ms && t-h.tracking_received_ms<500;
}
bool validPose(const PrismPose& p) {
    if(p.valid!=1) return false;
    double n=0;
    for(float v:p.position) if(!std::isfinite(v)) return false;
    for(float v:p.orientation) { if(!std::isfinite(v)) return false; n+=v*v; }
    return n>0.25 && n<4;
}
ovrPosef convertPose(const PrismPose& p, uint32_t flags, ovrTrackingOrigin origin) {
    ovrPosef out{};
    double n=0; for(float v:p.orientation) n+=v*v; n=std::sqrt(n);
    out.Orientation={float(p.orientation[0]/n),float(p.orientation[1]/n),float(p.orientation[2]/n),float(p.orientation[3]/n)};
    out.Position={p.position[0],p.position[1],p.position[2]};
    // Match the existing driver floor convention without mixing client/PC clocks.
    if(origin==ovrTrackingOrigin_FloorLevel && !(flags&1)) out.Position.y+=1.6f;
    if(origin==ovrTrackingOrigin_EyeLevel && (flags&1)) out.Position.y-=1.6f;
    return out;
}
ovrFovPort fov(const PrismTracking& t, int eye) {
    const float* f=t.fov+4*eye;
    return {std::tan(f[2]),-std::tan(f[3]),-std::tan(f[0]),std::tan(f[1])};
}
}

struct ovrHmdStruct {
    prism::Ipc ipc;
    ovrTrackingOrigin origin{ovrTrackingOrigin_EyeLevel};
    std::mutex renderMutex;
    double nextFrame{};
    ID3D11Device* device{}; ID3D11DeviceContext* context{};
    ID3D11Texture2D* output{}; ID3D11Texture2D* staging{};
    ID3D11RenderTargetView* target{}; ID3D11VertexShader* vs{}; ID3D11PixelShader* ps{};
    ID3D11Buffer* constants{}; ID3D11SamplerState* sampler{}; ID3D11BlendState* blend{};
    ID3D11RasterizerState* rasterizer{};
    bool initGraphics(ID3D11Device*);
    ~ovrHmdStruct() {
        ipc.heartbeat(false); ipc.close();
        prism::release(rasterizer); prism::release(blend); prism::release(sampler); prism::release(constants);
        prism::release(ps); prism::release(vs); prism::release(target); prism::release(staging); prism::release(output);
        prism::release(context); prism::release(device);
    }
};

struct ovrTextureSwapChainData {
    ovrTextureSwapChainDesc desc{};
    std::vector<ID3D11Texture2D*> images;
    std::vector<ID3D11ShaderResourceView*> views;
    int index{}, committed{-1};
    ~ovrTextureSwapChainData() { for(auto*& v:views) prism::release(v); for(auto*& p:images) prism::release(p); }
};

namespace prism {
struct ShaderConstants {
    float viewport[4], targetFov[4], sourceFov[4], rotation[3][4], origin[4], params[4];
};
static_assert(sizeof(ShaderConstants)==128);
// Each pixel corresponds to a physical OpenXR view ray, not a desktop crop.
// Projection layers are rotated into their render pose. Quads use ray/plane
// intersection in the quad's local space, including eye offset and head lock.
const char* shader=R"(
cbuffer C : register(b0) {
 float4 uvRect, targetFov, sourceFov, row0, row1, row2, origin, params;
};
Texture2D image : register(t0); SamplerState linearSampler : register(s0);
struct V { float4 position:SV_Position; float2 uv:TEXCOORD0; };
V vertex(uint i:SV_VertexID) {
 V o; o.uv=float2((i<<1)&2,i&2); o.position=float4(o.uv.x*2-1,1-o.uv.y*2,0,1); return o;
}
float4 pixel(V p):SV_Target {
 float3 ray=float3(lerp(targetFov.x,targetFov.y,p.uv.x),lerp(targetFov.z,targetFov.w,p.uv.y),-1);
 ray=float3(dot(row0.xyz,ray),dot(row1.xyz,ray),dot(row2.xyz,ray));
 float2 uv;
 if(params.x>0.5) {
   if(abs(ray.z)<1e-6) discard;
   float distance=-origin.z/ray.z; if(distance<=0) discard;
   float3 hit=origin.xyz+ray*distance;
   uv=float2(hit.x/params.y+.5,.5-hit.y/params.z);
 } else {
   if(ray.z>=-1e-6) discard;
   uv=float2((ray.x/-ray.z-sourceFov.x)/(sourceFov.y-sourceFov.x),
     (sourceFov.z-ray.y/-ray.z)/(sourceFov.z-sourceFov.w));
 }
 if(any(uv<0)||any(uv>1)) discard;
 float4 c=image.Sample(linearSampler,uvRect.xy+uv*uvRect.zw);
 if(params.w>0.5) c.a=1;
 return c;
})";
ovrQuatf conjugate(ovrQuatf q) { return {-q.x,-q.y,-q.z,q.w}; }
ovrQuatf multiply(ovrQuatf a,ovrQuatf b) {
    return {a.w*b.x+a.x*b.w+a.y*b.z-a.z*b.y,a.w*b.y-a.x*b.z+a.y*b.w+a.z*b.x,
        a.w*b.z+a.x*b.y-a.y*b.x+a.z*b.w,a.w*b.w-a.x*b.x-a.y*b.y-a.z*b.z};
}
ovrVector3f rotate(ovrQuatf q,ovrVector3f v) {
    auto r=multiply(multiply(q,{v.x,v.y,v.z,0}),conjugate(q)); return {r.x,r.y,r.z};
}
void rotationRows(ShaderConstants& c,ovrQuatf q) {
    auto x=rotate(q,{1,0,0}),y=rotate(q,{0,1,0}),z=rotate(q,{0,0,1});
    c.rotation[0][0]=x.x; c.rotation[0][1]=y.x; c.rotation[0][2]=z.x;
    c.rotation[1][0]=x.y; c.rotation[1][1]=y.y; c.rotation[1][2]=z.y;
    c.rotation[2][0]=x.z; c.rotation[2][1]=y.z; c.rotation[2][2]=z.z;
}
DXGI_FORMAT format(ovrTextureFormat f) {
    switch(f) {
    case OVR_FORMAT_R8G8B8A8_UNORM: return DXGI_FORMAT_R8G8B8A8_UNORM;
    case OVR_FORMAT_R8G8B8A8_UNORM_SRGB: return DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;
    case OVR_FORMAT_B8G8R8A8_UNORM: return DXGI_FORMAT_B8G8R8A8_UNORM;
    case OVR_FORMAT_B8G8R8A8_UNORM_SRGB: return DXGI_FORMAT_B8G8R8A8_UNORM_SRGB;
    case OVR_FORMAT_R16G16B16A16_FLOAT: return DXGI_FORMAT_R16G16B16A16_FLOAT;
    default: return DXGI_FORMAT_UNKNOWN;
    }
}
DXGI_FORMAT typeless(DXGI_FORMAT f) {
    switch(f) {
    case DXGI_FORMAT_R8G8B8A8_UNORM: case DXGI_FORMAT_R8G8B8A8_UNORM_SRGB: return DXGI_FORMAT_R8G8B8A8_TYPELESS;
    case DXGI_FORMAT_B8G8R8A8_UNORM: case DXGI_FORMAT_B8G8R8A8_UNORM_SRGB: return DXGI_FORMAT_B8G8R8A8_TYPELESS;
    default: return f;
    }
}
}

bool ovrHmdStruct::initGraphics(ID3D11Device* d) {
    if(device) return device==d && target && vs && ps && constants && sampler && blend && rasterizer;
    device=d; device->AddRef(); device->GetImmediateContext(&context);
    ID3DBlob* blob=nullptr; ID3DBlob* errors=nullptr;
    HRESULT hr=D3DCompile(prism::shader,std::strlen(prism::shader),nullptr,nullptr,nullptr,"vertex","vs_5_0",0,0,&blob,&errors);
    prism::release(errors); if(FAILED(hr)) return false;
    hr=device->CreateVertexShader(blob->GetBufferPointer(),blob->GetBufferSize(),nullptr,&vs); prism::release(blob); if(FAILED(hr)) return false;
    hr=D3DCompile(prism::shader,std::strlen(prism::shader),nullptr,nullptr,nullptr,"pixel","ps_5_0",0,0,&blob,&errors);
    prism::release(errors); if(FAILED(hr)) return false;
    hr=device->CreatePixelShader(blob->GetBufferPointer(),blob->GetBufferSize(),nullptr,&ps); prism::release(blob); if(FAILED(hr)) return false;
    D3D11_TEXTURE2D_DESC td{}; td.Width=PRISM_EYE_WIDTH*2; td.Height=PRISM_EYE_HEIGHT;
    td.MipLevels=td.ArraySize=td.SampleDesc.Count=1; td.Format=DXGI_FORMAT_B8G8R8A8_UNORM; td.BindFlags=D3D11_BIND_RENDER_TARGET;
    if(FAILED(device->CreateTexture2D(&td,nullptr,&output))) return false;
    D3D11_RENDER_TARGET_VIEW_DESC rd{}; rd.Format=DXGI_FORMAT_B8G8R8A8_UNORM_SRGB; rd.ViewDimension=D3D11_RTV_DIMENSION_TEXTURE2D;
    // Typed UNORM output cannot have an sRGB RTV on every D3D driver. Use a
    // typeless texture to make linear-light composition and sRGB output explicit.
    prism::release(output); td.Format=DXGI_FORMAT_B8G8R8A8_TYPELESS;
    if(FAILED(device->CreateTexture2D(&td,nullptr,&output)) || FAILED(device->CreateRenderTargetView(output,&rd,&target))) return false;
    td.Format=DXGI_FORMAT_B8G8R8A8_TYPELESS; td.BindFlags=0; td.Usage=D3D11_USAGE_STAGING; td.CPUAccessFlags=D3D11_CPU_ACCESS_READ;
    if(FAILED(device->CreateTexture2D(&td,nullptr,&staging))) return false;
    D3D11_BUFFER_DESC bd{}; bd.ByteWidth=sizeof(prism::ShaderConstants); bd.Usage=D3D11_USAGE_DEFAULT; bd.BindFlags=D3D11_BIND_CONSTANT_BUFFER;
    if(FAILED(device->CreateBuffer(&bd,nullptr,&constants))) return false;
    D3D11_SAMPLER_DESC sd{}; sd.Filter=D3D11_FILTER_MIN_MAG_MIP_LINEAR; sd.AddressU=sd.AddressV=sd.AddressW=D3D11_TEXTURE_ADDRESS_CLAMP; sd.MaxLOD=D3D11_FLOAT32_MAX;
    if(FAILED(device->CreateSamplerState(&sd,&sampler))) return false;
    D3D11_BLEND_DESC blendDesc{}; auto& b=blendDesc.RenderTarget[0]; b.BlendEnable=TRUE;
    b.SrcBlend=D3D11_BLEND_ONE; b.DestBlend=D3D11_BLEND_INV_SRC_ALPHA; b.BlendOp=D3D11_BLEND_OP_ADD;
    b.SrcBlendAlpha=D3D11_BLEND_ONE; b.DestBlendAlpha=D3D11_BLEND_INV_SRC_ALPHA; b.BlendOpAlpha=D3D11_BLEND_OP_ADD; b.RenderTargetWriteMask=D3D11_COLOR_WRITE_ENABLE_ALL;
    if(FAILED(device->CreateBlendState(&blendDesc,&blend))) return false;
    D3D11_RASTERIZER_DESC r{}; r.FillMode=D3D11_FILL_SOLID; r.CullMode=D3D11_CULL_NONE; r.DepthClipEnable=TRUE;
    return SUCCEEDED(device->CreateRasterizerState(&r,&rasterizer));
}

OVR_PUBLIC_FUNCTION(ovrResult) ovr_InitializeWithPathOverride(const ovrInitParams*,const wchar_t*) { return ovrSuccess; }
OVR_PUBLIC_FUNCTION(ovrResult) ovr_ReInitialize(const ovrInitParams*) { return ovrSuccess; }
OVR_PUBLIC_FUNCTION(ovrResult) ovr_Initialize(const ovrInitParams*) { return ovrSuccess; }
OVR_PUBLIC_FUNCTION(void) ovr_Shutdown() {}
OVR_PUBLIC_FUNCTION(const char*) ovr_GetVersionString() { return "Prism VDXR backend 1 (Prism IPC ABI 1)"; }
OVR_PUBLIC_FUNCTION(double) ovr_GetTimeInSeconds() { return prism::now(); }
OVR_PUBLIC_FUNCTION(void) ovr_GetLastErrorInfo(ovrErrorInfo* e) {
    if(e) { *e={}; std::strcpy(e->ErrorString,"Prism VDXR backend: inspect runtime log and server connection"); }
}
OVR_PUBLIC_FUNCTION(ovrResult) ovr_Create(ovrSession* out,ovrGraphicsLuid* luid) {
    if(!out || !luid) return ovrError_InvalidParameter;
    *out=nullptr;
    auto* s=new ovrHmdStruct;
    PrismSharedHeader h{};
    if(!s->ipc.open() || !s->ipc.read(h) || !prism::fresh(h) || !prism::validPose(h.tracking.head)) { delete s; return ovrError_NoHmd; }
    IDXGIFactory1* factory=nullptr; IDXGIAdapter1* adapter=nullptr;
    HRESULT hr=CreateDXGIFactory1(__uuidof(IDXGIFactory1),reinterpret_cast<void**>(&factory));
    if(SUCCEEDED(hr)) hr=factory->EnumAdapters1(0,&adapter);
    DXGI_ADAPTER_DESC1 desc{};
    if(SUCCEEDED(hr)) hr=adapter->GetDesc1(&desc);
    prism::release(adapter); prism::release(factory);
    if(FAILED(hr)) { delete s; return ovrError_GraphicsDeviceReset; }
    static_assert(sizeof(desc.AdapterLuid)==sizeof(*luid));
    std::memcpy(luid,&desc.AdapterLuid,sizeof(*luid));
    s->ipc.heartbeat(true); *out=s; return ovrSuccess;
}
OVR_PUBLIC_FUNCTION(void) ovr_Destroy(ovrSession s) { delete s; }
OVR_PUBLIC_FUNCTION(ovrResult) ovr_GetSessionStatus(ovrSession s,ovrSessionStatus* status) {
    if(!s || !status) return ovrError_InvalidParameter;
    PrismSharedHeader h{}; bool live=s->ipc.read(h) && prism::fresh(h) && prism::validPose(h.tracking.head);
    *status={}; status->HmdPresent=status->HmdMounted=status->IsVisible=status->HasInputFocus=live?ovrTrue:ovrFalse;
    s->ipc.heartbeat(true); return ovrSuccess;
}
OVR_PUBLIC_FUNCTION(ovrHmdDesc) ovr_GetHmdDesc(ovrSession s) {
    ovrHmdDesc d{}; PrismSharedHeader h{};
    if(!s || !s->ipc.read(h) || !prism::fresh(h)) return d;
    const auto type=s->ipc.deviceType(); const auto identity=prismQuestIdentity(type);
    d.Type=type==1?ovrHmd_Quest:type==2?ovrHmd_Quest2:static_cast<ovrHmdType>(type==3?22:(type==4||type==5)?23:ovrHmd_Other);
    std::strncpy(d.ProductName,identity.headset,sizeof(d.ProductName)-1); std::strcpy(d.Manufacturer,"Meta");
    std::snprintf(d.SerialNumber,sizeof(d.SerialNumber),"Prism-VDXR-%u",type);
    d.AvailableHmdCaps=d.DefaultHmdCaps=0;
    d.AvailableTrackingCaps=d.DefaultTrackingCaps=ovrTrackingCap_Orientation|ovrTrackingCap_Position;
    d.Resolution={int(PRISM_EYE_WIDTH*2),int(PRISM_EYE_HEIGHT)}; d.DisplayRefreshRate=float(h.tracking.refresh_rate);
    for(int e=0;e<2;++e) d.MaxEyeFov[e]=d.DefaultEyeFov[e]=prism::fov(h.tracking,e);
    return d;
}
OVR_PUBLIC_FUNCTION(ovrResult) ovr_SetTrackingOriginType(ovrSession s,ovrTrackingOrigin origin) {
    if(!s || (origin!=ovrTrackingOrigin_EyeLevel && origin!=ovrTrackingOrigin_FloorLevel)) return ovrError_InvalidParameter;
    s->origin=origin; return ovrSuccess;
}
OVR_PUBLIC_FUNCTION(ovrTrackingOrigin) ovr_GetTrackingOriginType(ovrSession s) { return s?s->origin:ovrTrackingOrigin_EyeLevel; }
OVR_PUBLIC_FUNCTION(void) ovr_ClearShouldRecenterFlag(ovrSession) {}
OVR_PUBLIC_FUNCTION(ovrResult) ovr_GetDevicePoses(ovrSession s,ovrTrackedDeviceType* devices,int count,double,ovrPoseStatef* poses) {
    if(!s || !devices || !poses || count<0) return ovrError_InvalidParameter;
    PrismSharedHeader h{}; if(!s->ipc.read(h) || !prism::fresh(h)) return ovrError_LostTracking;
    for(int i=0;i<count;++i) {
        poses[i]={};
        const PrismPose* p=devices[i]==ovrTrackedDevice_HMD?&h.tracking.head:
            devices[i]==ovrTrackedDevice_LTouch?&h.tracking.hands[0]:devices[i]==ovrTrackedDevice_RTouch?&h.tracking.hands[1]:nullptr;
        if(!p || !prism::validPose(*p)) return ovrError_LostTracking;
        poses[i].ThePose=prism::convertPose(*p,h.tracking.flags,s->origin);
        poses[i].TimeInSeconds=prism::now(); // Never reinterpret the client monotonic timestamp as PC time.
    }
    return ovrSuccess;
}
OVR_PUBLIC_FUNCTION(ovrTrackingState) ovr_GetTrackingState(ovrSession s,double time,ovrBool) {
    ovrTrackingState state{}; state.CalibratedOrigin.Orientation.w=1;
    ovrTrackedDeviceType device=ovrTrackedDevice_HMD;
    constexpr unsigned tracked=ovrStatus_OrientationTracked|ovrStatus_PositionTracked;
    if(ovr_GetDevicePoses(s,&device,1,time,&state.HeadPose)==ovrSuccess) state.StatusFlags=tracked;
    for(int hand=0;hand<2;++hand) {
        device=hand?ovrTrackedDevice_RTouch:ovrTrackedDevice_LTouch;
        if(ovr_GetDevicePoses(s,&device,1,time,&state.HandPoses[hand])==ovrSuccess) state.HandStatusFlags[hand]=tracked;
    }
    return state;
}
OVR_PUBLIC_FUNCTION(unsigned int) ovr_GetConnectedControllerTypes(ovrSession s) {
    PrismSharedHeader h{}; if(!s || !s->ipc.read(h) || !prism::fresh(h)) return 0;
    return (prism::validPose(h.tracking.hands[0])?ovrControllerType_LTouch:0)
        | (prism::validPose(h.tracking.hands[1])?ovrControllerType_RTouch:0);
}
OVR_PUBLIC_FUNCTION(ovrResult) ovr_GetInputState(ovrSession s,ovrControllerType,ovrInputState* out) {
    if(!s || !out) return ovrError_InvalidParameter;
    *out={}; out->TimeInSeconds=prism::now();
    PrismSharedHeader h{}; if(!s->ipc.read(h) || !prism::fresh(h)) return ovrSuccess;
    for(int hand=0;hand<2;++hand) {
        if(!prism::validPose(h.tracking.hands[hand])) continue;
        out->ControllerType=static_cast<ovrControllerType>(out->ControllerType | (hand?ovrControllerType_RTouch:ovrControllerType_LTouch));
        out->IndexTrigger[hand]=out->IndexTriggerNoDeadzone[hand]=out->IndexTriggerRaw[hand]=h.tracking.trigger[hand];
        out->HandTrigger[hand]=out->HandTriggerNoDeadzone[hand]=out->HandTriggerRaw[hand]=h.tracking.grip[hand];
        out->Thumbstick[hand]=out->ThumbstickNoDeadzone[hand]=out->ThumbstickRaw[hand]={h.tracking.stick[2*hand],h.tracking.stick[2*hand+1]};
        const auto b=h.tracking.buttons[hand];
        if(b&1) out->Buttons|=hand?ovrButton_A:ovrButton_X;
        if(b&2) out->Buttons|=hand?ovrButton_B:ovrButton_Y;
        if(b&4) out->Buttons|=ovrButton_Enter;
        if(b&8) out->Buttons|=hand?ovrButton_RThumb:ovrButton_LThumb;
    }
    return ovrSuccess;
}
OVR_PUBLIC_FUNCTION(ovrEyeRenderDesc) ovr_GetRenderDesc(ovrSession s,ovrEyeType eye,ovrFovPort fov) {
    ovrEyeRenderDesc d{}; d.Eye=eye; d.Fov=fov; d.HmdToEyePose.Orientation.w=1;
    PrismSharedHeader h{}; if(s && s->ipc.read(h)) d.HmdToEyePose.Position.x=(eye==ovrEye_Left?-0.5f:0.5f)*h.tracking.ipd;
    d.DistortedViewport={{eye==ovrEye_Left?0:int(PRISM_EYE_WIDTH),0},{int(PRISM_EYE_WIDTH),int(PRISM_EYE_HEIGHT)}};
    d.PixelsPerTanAngleAtCenter={PRISM_EYE_WIDTH/(fov.LeftTan+fov.RightTan),PRISM_EYE_HEIGHT/(fov.UpTan+fov.DownTan)};
    return d;
}
OVR_PUBLIC_FUNCTION(ovrSizei) ovr_GetFovTextureSize(ovrSession,ovrEyeType,ovrFovPort,float density) {
    return {std::max(1,int(PRISM_EYE_WIDTH*density)),std::max(1,int(PRISM_EYE_HEIGHT*density))};
}
OVR_PUBLIC_FUNCTION(double) ovr_GetPredictedDisplayTime(ovrSession s,long long) {
    PrismSharedHeader h{}; unsigned hz=s && s->ipc.read(h)?h.tracking.refresh_rate:0;
    return prism::now()+(hz>=60 && hz<=144?1.0/hz:0.0);
}
OVR_PUBLIC_FUNCTION(ovrResult) ovr_WaitToBeginFrame(ovrSession s,long long) {
    if(!s) return ovrError_InvalidSession;
    PrismSharedHeader h{}; if(!s->ipc.read(h) || !prism::fresh(h)) return ovrSuccess_NotVisible;
    unsigned hz=h.tracking.refresh_rate; if(hz<60 || hz>144) return ovrError_LostTracking;
    double t=prism::now(); if(s->nextFrame>t) std::this_thread::sleep_for(std::chrono::duration<double>(s->nextFrame-t));
    s->nextFrame=std::max(s->nextFrame,t)+1.0/hz; s->ipc.heartbeat(true); return ovrSuccess;
}
OVR_PUBLIC_FUNCTION(ovrResult) ovr_BeginFrame(ovrSession s,long long) { return s?ovrResult(ovrSuccess):ovrResult(ovrError_InvalidSession); }
OVR_PUBLIC_FUNCTION(ovrBool) ovr_GetBool(ovrSession,const char*,ovrBool fallback) { return fallback; }
OVR_PUBLIC_FUNCTION(ovrBool) ovr_SetBool(ovrSession,const char*,ovrBool) { return ovrFalse; }
OVR_PUBLIC_FUNCTION(float) ovr_GetFloat(ovrSession,const char*,float fallback) { return fallback; }
OVR_PUBLIC_FUNCTION(ovrBool) ovr_SetFloat(ovrSession,const char*,float) { return ovrFalse; }
OVR_PUBLIC_FUNCTION(ovrResult) ovr_SetControllerVibration(ovrSession,ovrControllerType,float,float) { return ovrError_Unsupported; }

OVR_PUBLIC_FUNCTION(ovrResult) ovr_CreateTextureSwapChainDX(ovrSession s,IUnknown* unknown,const ovrTextureSwapChainDesc* desc,ovrTextureSwapChain* out) {
    if(!s || !unknown || !desc || !out) return ovrError_InvalidParameter;
    *out=nullptr;
    DXGI_FORMAT format=prism::format(desc->Format);
    if(format==DXGI_FORMAT_UNKNOWN || desc->Type!=ovrTexture_2D || desc->Width<=0 || desc->Height<=0
        || desc->Width>16384 || desc->Height>16384 || desc->ArraySize<1 || desc->ArraySize>16
        || desc->SampleCount!=1 || desc->MipLevels<1) return ovrError_Unsupported;
    ID3D11Device* device=nullptr;
    if(FAILED(unknown->QueryInterface(__uuidof(ID3D11Device),reinterpret_cast<void**>(&device)))) return ovrError_Unsupported;
    std::lock_guard<std::mutex> lock(s->renderMutex);
    if(!s->initGraphics(device)) { prism::release(device); return ovrError_RuntimeException; }
    auto* chain=new ovrTextureSwapChainData; chain->desc=*desc;
    D3D11_TEXTURE2D_DESC td{}; td.Width=desc->Width; td.Height=desc->Height; td.ArraySize=desc->ArraySize;
    td.MipLevels=desc->MipLevels; td.SampleDesc.Count=1; td.Format=prism::typeless(format);
    td.BindFlags=D3D11_BIND_SHADER_RESOURCE|D3D11_BIND_RENDER_TARGET;
    td.MiscFlags=D3D11_RESOURCE_MISC_SHARED; // VDXR imports submission images into the application device.
    const int length=desc->StaticImage?1:3;
    HRESULT hr=S_OK;
    for(int i=0;i<length;++i) {
        ID3D11Texture2D* image=nullptr; ID3D11ShaderResourceView* view=nullptr;
        hr=device->CreateTexture2D(&td,nullptr,&image); if(FAILED(hr)) break;
        chain->images.push_back(image);
        D3D11_SHADER_RESOURCE_VIEW_DESC vd{}; vd.Format=format;
        vd.ViewDimension=td.ArraySize==1?D3D11_SRV_DIMENSION_TEXTURE2D:D3D11_SRV_DIMENSION_TEXTURE2DARRAY;
        if(td.ArraySize==1) vd.Texture2D.MipLevels=td.MipLevels;
        else { vd.Texture2DArray.MipLevels=td.MipLevels; vd.Texture2DArray.ArraySize=td.ArraySize; }
        hr=device->CreateShaderResourceView(image,&vd,&view); if(FAILED(hr)) break;
        chain->views.push_back(view);
    }
    prism::release(device);
    if(FAILED(hr)) { delete chain; return ovrError_RuntimeException; }
    *out=chain; return ovrSuccess;
}
OVR_PUBLIC_FUNCTION(ovrResult) ovr_GetTextureSwapChainLength(ovrSession,ovrTextureSwapChain c,int* n) {
    if(!c || !n) return ovrError_InvalidParameter; *n=int(c->images.size()); return ovrSuccess;
}
OVR_PUBLIC_FUNCTION(ovrResult) ovr_GetTextureSwapChainCurrentIndex(ovrSession,ovrTextureSwapChain c,int* n) {
    if(!c || !n) return ovrError_InvalidParameter; *n=c->index; return ovrSuccess;
}
OVR_PUBLIC_FUNCTION(ovrResult) ovr_GetTextureSwapChainDesc(ovrSession,ovrTextureSwapChain c,ovrTextureSwapChainDesc* d) {
    if(!c || !d) return ovrError_InvalidParameter; *d=c->desc; return ovrSuccess;
}
OVR_PUBLIC_FUNCTION(ovrResult) ovr_GetTextureSwapChainBufferDX(ovrSession,ovrTextureSwapChain c,int index,IID iid,void** out) {
    if(!c || !out) return ovrError_InvalidParameter; *out=nullptr;
    if(index==-1) index=c->index;
    if(index<0 || size_t(index)>=c->images.size()) return ovrError_InvalidParameter;
    return SUCCEEDED(c->images[index]->QueryInterface(iid,out))?ovrResult(ovrSuccess):ovrResult(ovrError_Unsupported);
}
OVR_PUBLIC_FUNCTION(ovrResult) ovr_CommitTextureSwapChain(ovrSession s,ovrTextureSwapChain c) {
    if(!s || !c) return ovrError_InvalidParameter;
    std::lock_guard<std::mutex> lock(s->renderMutex);
    c->committed=c->index; c->index=(c->index+1)%int(c->images.size()); return ovrSuccess;
}
OVR_PUBLIC_FUNCTION(void) ovr_DestroyTextureSwapChain(ovrSession s,ovrTextureSwapChain c) {
    if(!s) return; std::lock_guard<std::mutex> lock(s->renderMutex); delete c;
}

OVR_PUBLIC_FUNCTION(ovrResult) ovr_EndFrame(ovrSession s,long long,const ovrViewScaleDesc*,ovrLayerHeader const* const* layers,unsigned count) {
    if(!s || (count && !layers) || count>ovrMaxLayerCount) return ovrError_InvalidParameter;
    if(!count) return ovrSuccess; // No invented frame on an empty submission.
    std::lock_guard<std::mutex> lock(s->renderMutex);
    if(!s->context || !s->target) return ovrError_NotInitialized;
    PrismSharedHeader h{};
    if(!s->ipc.read(h) || !prism::fresh(h) || !prism::validPose(h.tracking.head)) return ovrSuccess_NotVisible;
    for(unsigned i=0;i<count;++i) if(layers[i] && layers[i]->Type!=ovrLayerType_Disabled
        && layers[i]->Type!=ovrLayerType_EyeFov && layers[i]->Type!=ovrLayerType_EyeFovDepth
        && layers[i]->Type!=ovrLayerType_Quad) return ovrError_Unsupported;
    auto* ctx=s->context;
    // The submission device is dedicated by VDXR's default D3D11 path. Clear
    // bindings before/after use; the quirk for application-device submission is
    // disabled by the Prism build to avoid touching application pipeline state.
    ctx->ClearState();
    const float black[4]={0,0,0,1}; ctx->ClearRenderTargetView(s->target,black);
    ctx->OMSetRenderTargets(1,&s->target,nullptr); ctx->OMSetBlendState(s->blend,nullptr,~0u);
    ctx->RSSetState(s->rasterizer); ctx->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
    ctx->VSSetShader(s->vs,nullptr,0); ctx->PSSetShader(s->ps,nullptr,0);
    ctx->PSSetConstantBuffers(0,1,&s->constants); ctx->PSSetSamplers(0,1,&s->sampler);
    auto head=prism::convertPose(h.tracking.head,h.tracking.flags,s->origin);
    bool drawn=false;
    for(unsigned layerIndex=0;layerIndex<count;++layerIndex) {
        const auto* layer=layers[layerIndex]; if(!layer || layer->Type==ovrLayerType_Disabled) continue;
        for(int eye=0;eye<2;++eye) {
            if(layer->Flags & (eye==0?0x100000u:0x200000u)) continue; // VDXR per-eye layer flags.
            ovrTextureSwapChain chain=nullptr; ovrRecti viewport{}; prism::ShaderConstants c{};
            const auto f=prism::fov(h.tracking,eye);
            c.targetFov[0]=-f.LeftTan; c.targetFov[1]=f.RightTan; c.targetFov[2]=f.UpTan; c.targetFov[3]=-f.DownTan;
            if(layer->Type==ovrLayerType_Quad) {
                const auto& q=*reinterpret_cast<const ovrLayerQuad*>(layer);
                if(q.QuadSize.x<=0 || q.QuadSize.y<=0) { ctx->ClearState(); return ovrError_InvalidParameter; }
                chain=q.ColorTexture; viewport=q.Viewport;
                ovrPosef eyePose=head;
                if(layer->Flags&ovrLayerFlag_HeadLocked) { eyePose={}; eyePose.Orientation.w=1; }
                auto offset=prism::rotate(eyePose.Orientation,{(eye?0.5f:-0.5f)*h.tracking.ipd,0,0});
                eyePose.Position.x+=offset.x; eyePose.Position.y+=offset.y; eyePose.Position.z+=offset.z;
                auto inv=prism::conjugate(q.QuadPoseCenter.Orientation);
                auto o=prism::rotate(inv,{eyePose.Position.x-q.QuadPoseCenter.Position.x,eyePose.Position.y-q.QuadPoseCenter.Position.y,eyePose.Position.z-q.QuadPoseCenter.Position.z});
                c.origin[0]=o.x; c.origin[1]=o.y; c.origin[2]=o.z;
                prism::rotationRows(c,prism::multiply(inv,eyePose.Orientation));
                c.params[0]=1; c.params[1]=q.QuadSize.x; c.params[2]=q.QuadSize.y;
            } else {
                const auto& p=*reinterpret_cast<const ovrLayerEyeFov*>(layer);
                chain=p.ColorTexture[eye]?p.ColorTexture[eye]:p.ColorTexture[0]; viewport=p.Viewport[eye];
                const auto& sf=p.Fov[eye]; c.sourceFov[0]=-sf.LeftTan; c.sourceFov[1]=sf.RightTan;
                c.sourceFov[2]=sf.UpTan; c.sourceFov[3]=-sf.DownTan;
                auto view=head.Orientation; if(layer->Flags&ovrLayerFlag_HeadLocked) view={0,0,0,1};
                prism::rotationRows(c,prism::multiply(prism::conjugate(p.RenderPose[eye].Orientation),view));
            }
            if(!chain || chain->committed<0 || chain->desc.ArraySize!=1 || viewport.Pos.x<0 || viewport.Pos.y<0
                || viewport.Size.w<=0 || viewport.Size.h<=0 || viewport.Pos.x+viewport.Size.w>chain->desc.Width
                || viewport.Pos.y+viewport.Size.h>chain->desc.Height) { ctx->ClearState(); return ovrError_InvalidParameter; }
            c.viewport[0]=float(viewport.Pos.x)/chain->desc.Width; c.viewport[1]=float(viewport.Pos.y)/chain->desc.Height;
            c.viewport[2]=float(viewport.Size.w)/chain->desc.Width; c.viewport[3]=float(viewport.Size.h)/chain->desc.Height;
            if(layer->Flags&ovrLayerFlag_TextureOriginAtBottomLeft) { c.viewport[1]+=c.viewport[3]; c.viewport[3]=-c.viewport[3]; }
            ctx->UpdateSubresource(s->constants,0,nullptr,&c,0,0);
            D3D11_VIEWPORT vp{float(eye*PRISM_EYE_WIDTH),0,float(PRISM_EYE_WIDTH),float(PRISM_EYE_HEIGHT),0,1};
            ctx->RSSetViewports(1,&vp); auto* srv=chain->views[chain->committed]; ctx->PSSetShaderResources(0,1,&srv); ctx->Draw(3,0);
            ID3D11ShaderResourceView* unbind=nullptr; ctx->PSSetShaderResources(0,1,&unbind); drawn=true;
        }
    }
    ctx->ClearState(); if(!drawn) return ovrSuccess;
    ctx->CopyResource(s->staging,s->output);
    D3D11_MAPPED_SUBRESOURCE mapped{};
    if(FAILED(ctx->Map(s->staging,0,D3D11_MAP_READ,0,&mapped))) return ovrError_GraphicsDeviceReset;
    bool published=false;
    if(s->ipc.lock()) {
        if(s->ipc.valid() && prism::fresh(*s->ipc.header)) {
            auto* dst=reinterpret_cast<uint8_t*>(s->ipc.header)+PRISM_PIXEL_OFFSET;
            for(unsigned y=0;y<PRISM_EYE_HEIGHT;++y) std::memcpy(dst+size_t(y)*PRISM_EYE_WIDTH*8,
                static_cast<const uint8_t*>(mapped.pData)+size_t(y)*mapped.RowPitch,PRISM_EYE_WIDTH*8);
            auto& header=*s->ipc.header;
            header.frame_width=PRISM_EYE_WIDTH*2; header.frame_height=PRISM_EYE_HEIGHT; header.frame_stride=PRISM_EYE_WIDTH*8;
            header.frame_format=0; header.frame_time_ms=header.driver_heartbeat_ms=GetTickCount64();
            header.driver_ready=1; ++header.frame_sequence; published=true;
        }
        ReleaseMutex(s->ipc.mutex);
    }
    ctx->Unmap(s->staging,0); return published?ovrResult(ovrSuccess):ovrResult(ovrSuccess_NotVisible);
}
OVR_PUBLIC_FUNCTION(ovrResult) ovr_GetFovStencil(ovrSession,const ovrFovStencilDesc*,ovrFovStencilMeshBuffer* out) {
    if(!out) return ovrError_InvalidParameter;
    out->UsedVertexCount=out->UsedIndexCount=0; return ovrSuccess;
}
OVR_PUBLIC_FUNCTION(ovrResult) ovr_GetPerfStats(ovrSession,ovrPerfStats* out) {
    if(!out) return ovrError_InvalidParameter; *out={}; out->AdaptiveGpuPerformanceScale=1; return ovrSuccess;
}
OVR_PUBLIC_FUNCTION(ovrResult) ovr_GetAudioDeviceOutGuidStr(WCHAR out[OVR_AUDIO_MAX_DEVICE_STR_SIZE]) { if(out) out[0]=0; return ovrError_Unsupported; }
OVR_PUBLIC_FUNCTION(ovrResult) ovr_GetAudioDeviceInGuidStr(WCHAR out[OVR_AUDIO_MAX_DEVICE_STR_SIZE]) { if(out) out[0]=0; return ovrError_Unsupported; }
OVR_PUBLIC_FUNCTION(ovrResult) ovr_CreateMirrorTextureWithOptionsDX(ovrSession,IUnknown*,const ovrMirrorTextureDesc*,ovrMirrorTexture* out) { if(out) *out=nullptr; return ovrError_Unsupported; }
OVR_PUBLIC_FUNCTION(ovrResult) ovr_GetMirrorTextureBufferDX(ovrSession,ovrMirrorTexture,IID,void** out) { if(out) *out=nullptr; return ovrError_Unsupported; }
OVR_PUBLIC_FUNCTION(void) ovr_DestroyMirrorTexture(ovrSession,ovrMirrorTexture) {}
