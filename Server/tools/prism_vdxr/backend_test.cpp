#define PRISM_VDXR_TEST
#include "prism_backend.cpp"
#include <cassert>
#include <cstdio>

int main() {
    // Tests act as the server; never replace another live mapping.
    HANDLE mapping=CreateFileMappingW(INVALID_HANDLE_VALUE,nullptr,PAGE_READWRITE,0,PRISM_MAPPING_BYTES,prism::MappingName);
    assert(mapping && GetLastError()!=ERROR_ALREADY_EXISTS);
    HANDLE mutex=CreateMutexW(nullptr,FALSE,prism::MutexName); assert(mutex);
    auto* h=static_cast<PrismSharedHeader*>(MapViewOfFile(mapping,FILE_MAP_ALL_ACCESS,0,0,PRISM_MAPPING_BYTES)); assert(h);
    auto set=[&](bool live) {
        WaitForSingleObject(mutex,INFINITE);
        *h={}; h->magic=PRISM_MAGIC; h->version=1; h->client_connected=live;
        h->server_heartbeat_ms=h->tracking_received_ms=GetTickCount64();
        h->tracking.version=1; h->tracking.ipd=.064f; h->tracking.refresh_rate=72;
        h->tracking.head.valid=1; h->tracking.head.orientation[3]=1; h->tracking.head.position[1]=.2f;
        for(int i=0;i<2;++i) {
            h->tracking.hands[i].valid=1; h->tracking.hands[i].orientation[3]=1;
            h->tracking.fov[4*i]=-.7f; h->tracking.fov[4*i+1]=.7f; h->tracking.fov[4*i+2]=.8f; h->tracking.fov[4*i+3]=-.8f;
        }
        ReleaseMutex(mutex);
    };
    set(false);
    ovrSession s=nullptr; ovrGraphicsLuid luid{};
    assert(ovr_Create(&s,&luid)==ovrError_NoHmd && !s);
    set(true);
    assert(ovr_Create(&s,&luid)==ovrSuccess && s);
    for(uint32_t type=1;type<=5;++type) {
        WaitForSingleObject(mutex,INFINITE);
        auto* identity=reinterpret_cast<PrismDeviceInfo*>(reinterpret_cast<uint8_t*>(h)+PRISM_DEVICE_INFO_OFFSET);
        *identity={PRISM_DEVICE_INFO_MAGIC,1,type,0};
        ReleaseMutex(mutex);
        auto d=ovr_GetHmdDesc(s);
        assert(std::strcmp(d.ProductName,prismQuestIdentity(type).headset)==0);
        assert(std::strcmp(d.Manufacturer,"Meta")==0);
    }
    auto desc=ovr_GetHmdDesc(s); assert(desc.DisplayRefreshRate==72 && desc.Resolution.w==2048);
    auto eye=ovr_GetRenderDesc(s,ovrEye_Right,desc.DefaultEyeFov[1]); assert(std::abs(eye.HmdToEyePose.Position.x-.032f)<.0001f);
    ovrTrackedDeviceType device=ovrTrackedDevice_HMD; ovrPoseStatef pose{};
    assert(ovr_GetDevicePoses(s,&device,1,prism::now(),&pose)==ovrSuccess && std::abs(pose.ThePose.Position.y-.2f)<.0001f);
    assert(ovr_SetTrackingOriginType(s,ovrTrackingOrigin_FloorLevel)==ovrSuccess);
    assert(ovr_GetDevicePoses(s,&device,1,0,&pose)==ovrSuccess && std::abs(pose.ThePose.Position.y-1.8f)<.0001f);
    WaitForSingleObject(mutex,INFINITE); h->tracking.flags=1; h->tracking.head.position[1]=1.8f;
    h->tracking.buttons[0]=15; h->tracking.trigger[0]=.5f; h->tracking.stick[1]=-.5f; ReleaseMutex(mutex);
    assert(ovr_GetDevicePoses(s,&device,1,0,&pose)==ovrSuccess && std::abs(pose.ThePose.Position.y-1.8f)<.0001f);
    ovrInputState input{}; assert(ovr_GetInputState(s,ovrControllerType_Touch,&input)==ovrSuccess);
    assert((input.Buttons&(ovrButton_X|ovrButton_Y|ovrButton_Enter|ovrButton_LThumb))==(ovrButton_X|ovrButton_Y|ovrButton_Enter|ovrButton_LThumb));
    assert(input.IndexTrigger[0]==.5f && input.Thumbstick[0].y==-.5f);
    // Submit actual D3D11 eye textures and verify committed BGRA pixels.
    ID3D11Device* gpu=nullptr; ID3D11DeviceContext* context=nullptr;
    HRESULT hr=D3D11CreateDevice(nullptr,D3D_DRIVER_TYPE_HARDWARE,nullptr,D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        nullptr,0,D3D11_SDK_VERSION,&gpu,nullptr,&context);
    if(FAILED(hr)) { std::fprintf(stderr,"D3D11 hardware device failed: %08lx\n",(unsigned long)hr); return 2; }
    ovrTextureSwapChainDesc td{}; td.Type=ovrTexture_2D; td.Format=OVR_FORMAT_R8G8B8A8_UNORM_SRGB;
    td.ArraySize=1; td.Width=td.Height=64; td.MipLevels=td.SampleCount=1; td.StaticImage=ovrTrue;
    ovrTextureSwapChain chains[3]{};
    const float colors[3][4]={{1,0,0,1},{0,0,1,1},{0,1,0,1}};
    for(int i=0;i<3;++i) {
        assert(ovr_CreateTextureSwapChainDX(s,gpu,&td,&chains[i])==ovrSuccess);
        ID3D11Texture2D* texture=nullptr; assert(ovr_GetTextureSwapChainBufferDX(s,chains[i],0,__uuidof(ID3D11Texture2D),reinterpret_cast<void**>(&texture))==ovrSuccess);
        ID3D11RenderTargetView* target=nullptr;
        D3D11_RENDER_TARGET_VIEW_DESC rv{}; rv.Format=DXGI_FORMAT_R8G8B8A8_UNORM_SRGB; rv.ViewDimension=D3D11_RTV_DIMENSION_TEXTURE2D;
        assert(SUCCEEDED(gpu->CreateRenderTargetView(texture,&rv,&target)));
        context->ClearRenderTargetView(target,colors[i]); target->Release(); texture->Release();
        assert(ovr_CommitTextureSwapChain(s,chains[i])==ovrSuccess);
    }
    set(true); // Refresh timestamps before GPU readback.
    ovrLayerEyeFov projection{}; projection.Header.Type=ovrLayerType_EyeFov;
    for(int i=0;i<2;++i) {
        projection.ColorTexture[i]=chains[i]; projection.Viewport[i]={{0,0},{64,64}};
        projection.Fov[i]=prism::fov(h->tracking,i); projection.RenderPose[i].Orientation.w=1;
    }
    const ovrLayerHeader* layers[2]={&projection.Header,nullptr};
    assert(ovr_EndFrame(s,1,nullptr,layers,1)==ovrSuccess);
    assert(h->frame_sequence==1 && h->frame_width==2048 && h->frame_height==1024 && h->frame_stride==8192 && h->driver_ready);
    auto pixel=[&](unsigned x,unsigned y) { return reinterpret_cast<const uint8_t*>(h)+PRISM_PIXEL_OFFSET+y*8192+x*4; };
    auto l=pixel(512,512),r=pixel(1536,512); assert(l[2]>240 && l[0]<10 && r[0]>240 && r[2]<10);
    ovrLayerQuad quad{}; quad.Header.Type=ovrLayerType_Quad; quad.Header.Flags=ovrLayerFlag_HeadLocked;
    quad.ColorTexture=chains[2]; quad.Viewport={{0,0},{64,64}}; quad.QuadPoseCenter.Orientation.w=1;
    quad.QuadPoseCenter.Position.z=-2; quad.QuadSize={1,1}; layers[1]=&quad.Header;
    assert(ovr_EndFrame(s,2,nullptr,layers,2)==ovrSuccess && h->frame_sequence==2);
    assert(pixel(512,512)[1]>240 && pixel(1536,512)[1]>240 && pixel(100,100)[2]>240);
    assert(ovr_EndFrame(s,3,nullptr,nullptr,0)==ovrSuccess && h->frame_sequence==2);
    for(auto chain:chains) ovr_DestroyTextureSwapChain(s,chain);
    context->Release(); gpu->Release();
    WaitForSingleObject(mutex,INFINITE); h->tracking.head.valid=0; h->tracking.hands[0].valid=0; ReleaseMutex(mutex);
    assert(ovr_GetDevicePoses(s,&device,1,0,&pose)==ovrError_LostTracking);
    assert(ovr_GetInputState(s,ovrControllerType_Touch,&input)==ovrSuccess && !input.Buttons && !input.IndexTrigger[0]);
    WaitForSingleObject(mutex,INFINITE); h->tracking_received_ms-=501; ReleaseMutex(mutex);
    ovrSessionStatus status{}; assert(ovr_GetSessionStatus(s,&status)==ovrSuccess && !status.HmdPresent && !status.HasInputFocus);
    ovr_Destroy(s); assert(h->driver_ready==0);
    UnmapViewOfFile(h); CloseHandle(mutex); CloseHandle(mapping);
    std::puts("PASS: IPC, freshness, floor/local pose, IPD/FOV, inputs, invalid tracking, D3D11 stereo pixels, spatial quad, empty submit, cleanup");
}
