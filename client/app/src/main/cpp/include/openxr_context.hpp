#pragma once

#define XR_USE_PLATFORM_ANDROID
#define XR_USE_GRAPHICS_API_OPENGL_ES
#define XR_USE_TIMESPEC
#include <time.h>

#include <jni.h>
#include <EGL/egl.h>
#include <GLES3/gl32.h>
#include <GLES3/gl3ext.h>

#ifndef GL_TEXTURE_EXTERNAL_OES
#define GL_TEXTURE_EXTERNAL_OES 0x8D65
#endif

#include <openxr/openxr.h>
#include <openxr/openxr_platform.h>
#include <cstdint>
#include <array>
#include <algorithm>
#include <vector>
#include <memory>
#include "messages.hpp"
#include "pcvr_protocol.hpp"
#include "vr_gui.hpp"
#include "input_manager.hpp"
#include "video_frame_pose.hpp"
#include "motion_packet.hpp"

namespace openreality {

struct SwapchainInfo {
    XrSwapchain handle{XR_NULL_HANDLE};
    int32_t width{0};
    int32_t height{0};
    std::vector<XrSwapchainImageOpenGLESKHR> images;
};

class OpenXRContext {
public:
    OpenXRContext();
    ~OpenXRContext();

    bool init(void* vm, void* activity, HmdType hmdType, float targetRefreshRate);
    void release();

    bool createSession();
    void destroySession();

    bool pollEvents(bool& exitRenderLoop);
    bool beginFrame(bool& outShouldRender);
    
    // Stereo rendering into OpenXR swapchains
    void renderScene(
        uint32_t desktopTextureId,
        VRGui* gui,
        const InputManager* input,
        bool curvedScreen,
        bool enablePassthrough,
        bool vrMode = false,
        int desktopWidth = 16,
        int desktopHeight = 9,
        float desktopScale = 1.0f
    );

    bool endFrame(bool enablePassthrough);
    void endFrameEmpty();

    bool isSessionRunning() const { return m_sessionRunning; }
    bool shouldRender() const { return m_frameState.shouldRender != XR_FALSE; }

    // Refresh rate control
    bool setRefreshRate(float rateHz);
    std::vector<float> getSupportedRefreshRates();

    // Standby texture ID
    GLuint getStandbyTextureId() const { return m_standbyTexId; }

    // Tracking
    TrackingPacket sampleTracking(const InputManager& input) const;
    MotionPacket sampleMotion(const InputManager& input,uint64_t sampleUs) const;
    void setVideoPose(const VideoFramePose& pose) { m_videoPose=pose; }
    void setVideoTextureTransform(const float* transform) {
        if (transform) std::copy(transform, transform + m_videoTextureTransform.size(), m_videoTextureTransform.begin());
    }
    XrPosef getHeadPose() const { return m_headPose; }
    XrSpace getLocalSpace() const { return m_localSpace; }
    XrSession getSession() const { return m_session; }
    uint32_t getRecommendedEyeWidth() const { return m_configViews.empty() ? 0 : m_configViews[0].recommendedImageRectWidth; }
    uint32_t getRecommendedEyeHeight() const { return m_configViews.empty() ? 0 : m_configViews[0].recommendedImageRectHeight; }
    XrInstance getInstance() const { return m_instance; }
    XrTime getPredictedTime() const { return m_frameState.predictedDisplayTime; }

    // EGL thread management — must release from init thread before render thread acquires
    bool makeEGLCurrent();
    void releaseEGLFromCurrentThread();

    int32_t getEyeWidth(int eye) const { return m_swapchains[eye].width; }
    int32_t getEyeHeight(int eye) const { return m_swapchains[eye].height; }

private:
    XrInstance m_instance{XR_NULL_HANDLE};
    XrSystemId m_systemId{XR_NULL_SYSTEM_ID};
    XrSession m_session{XR_NULL_HANDLE};
    XrSpace m_localSpace{XR_NULL_HANDLE};
    XrSpace m_viewSpace{XR_NULL_HANDLE};

    XrSessionState m_sessionState{XR_SESSION_STATE_UNKNOWN};
    bool m_sessionRunning{false};

    EGLDisplay m_eglDisplay{EGL_NO_DISPLAY};
    EGLContext m_eglContext{EGL_NO_CONTEXT};
    EGLSurface m_eglSurface{EGL_NO_SURFACE};

    std::vector<XrViewConfigurationView> m_configViews;
    std::vector<XrView> m_views;
    SwapchainInfo m_swapchains[2];

    // Framebuffer objects for rendering into swapchains
    GLuint m_fbo[2]{0, 0};
    GLuint m_depthRb[2]{0, 0};

    // Shaders & GPU buffers
    GLuint m_screenProg{0};
    GLuint m_oesVideoProg{0};
    GLuint m_laserProg{0};
    GLuint m_gridProg{0};

    GLuint m_quadVao{0};
    GLuint m_quadVbo{0};
    GLuint m_curvedVao{0};
    GLuint m_curvedVbo{0};
    int m_curvedVertexCount{0};

    GLuint m_gridVao{0};
    GLuint m_gridVbo{0};
    int m_gridVertexCount{0};

    GLuint m_laserVao{0};
    GLuint m_laserVbo{0};

    GLuint m_standbyTexId{0};
    GLuint m_logoTexId{0};
    float m_logoAspect{1.0f};
    void loadSpatialLogo(void* vm, void* activity);
    void drawSpatialLogo(const float viewProj[16]);

    XrFrameState m_frameState{XR_TYPE_FRAME_STATE};
    XrPosef m_headPose{};
    bool m_headValid{false};
    XrSpaceVelocity m_headVelocity{XR_TYPE_SPACE_VELOCITY};
    PFN_xrConvertTimeToTimespecTimeKHR m_convertTime{nullptr};
    bool m_viewsValid{false};
    VideoFramePose m_videoPose;
    std::array<float, 16> m_videoTextureTransform{1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1};

    // Extension function pointers
    PFN_xrGetDisplayRefreshRateFB pfn_xrGetDisplayRefreshRateFB{nullptr};
    PFN_xrRequestDisplayRefreshRateFB pfn_xrRequestDisplayRefreshRateFB{nullptr};
    PFN_xrEnumerateDisplayRefreshRatesFB pfn_xrEnumerateDisplayRefreshRatesFB{nullptr};

    PFN_xrCreatePassthroughFB pfn_xrCreatePassthroughFB{nullptr};
    PFN_xrDestroyPassthroughFB pfn_xrDestroyPassthroughFB{nullptr};
    PFN_xrPassthroughStartFB pfn_xrPassthroughStartFB{nullptr};
    PFN_xrPassthroughPauseFB pfn_xrPassthroughPauseFB{nullptr};
    PFN_xrCreatePassthroughLayerFB pfn_xrCreatePassthroughLayerFB{nullptr};
    PFN_xrDestroyPassthroughLayerFB pfn_xrDestroyPassthroughLayerFB{nullptr};

    XrPassthroughFB m_passthrough{XR_NULL_HANDLE};
    XrPassthroughLayerFB m_passthroughLayer{XR_NULL_HANDLE};
    bool m_passthroughActive{false};

    bool initEGL();
    bool initOpenXRInstance(void* vm, void* activity);
    bool initOpenXRSession();
    bool initSwapchains();
    bool initGraphics();
    void initPassthrough();

    void renderEye(
        int eye,
        const float viewProj[16],
        uint32_t desktopTexId,
        VRGui* gui,
        const InputManager* input,
        bool curvedScreen,
        bool enablePassthrough,
        bool vrMode = false,
        float desktopAspect = 16.0f / 9.0f,
        float desktopScale = 1.0f
    );

    void drawFullscreenVrEye(int eye, uint32_t textureId);
    void drawDesktopScreen(const float viewProj[16], uint32_t textureId, bool curvedScreen, float aspect, float scale);
    void drawGuiPanel(const float viewProj[16], uint32_t textureId);
    void drawLaserAndCursor(const float viewProj[16], const float origin[3], const float dir[3], float length, const float hitPos[3], bool hasHit);
    void drawGridFloor(const float viewProj[16]);
};

} // namespace openreality
