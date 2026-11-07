// A DLL/compiler boundary test for the exact OpenVR struct-return callback.
#include "../vendor/openvr_driver.h"
struct IdentityDisplay : vr::IVRDisplayComponent {
    void GetWindowBounds(int32_t* x,int32_t* y,uint32_t* w,uint32_t* h) override { *x=*y=0; *w=2048; *h=1024; }
    bool IsDisplayOnDesktop() override { return false; }
    bool IsDisplayRealDisplay() override { return false; }
    void GetRecommendedRenderTargetSize(uint32_t* w,uint32_t* h) override { *w=*h=1024; }
    void GetEyeOutputViewport(vr::EVREye eye,uint32_t* x,uint32_t* y,uint32_t* w,uint32_t* h) override { *x=eye==vr::Eye_Left?0:1024; *y=0; *w=*h=1024; }
    void GetProjectionRaw(vr::EVREye,float* l,float* r,float* t,float* b) override { *l=*t=-1; *r=*b=1; }
    vr::DistortionCoordinates_t ComputeDistortion(vr::EVREye,float u,float v) override {
        vr::DistortionCoordinates_t d{};
        d.rfRed[0]=d.rfGreen[0]=d.rfBlue[0]=u;
        d.rfRed[1]=d.rfGreen[1]=d.rfBlue[1]=v;
        return d;
    }
    bool ComputeInverseDistortion(vr::HmdVector2_t* out,vr::EVREye,uint32_t,float u,float v) override { out->v[0]=u; out->v[1]=v; return true; }
};
extern "C" __declspec(dllexport) vr::IVRDisplayComponent* GetTestDisplay() { static IdentityDisplay display; return &display; }
