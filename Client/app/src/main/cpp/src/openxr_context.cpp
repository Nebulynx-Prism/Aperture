#include "openxr_context.hpp"
#include <chrono>
#include <android/log.h>
#include <android/asset_manager_jni.h>
#include <android/bitmap.h>
#include <cstring>
#include <cmath>
#include <algorithm>

#define LOG_TAG "PrismAperture_OpenXR"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define LOGW(...) __android_log_print(ANDROID_LOG_WARN, LOG_TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

namespace openreality {

// Matrix Math Helpers
static void mat4Multiply(float out[16], const float a[16], const float b[16]) {
    float res[16];
    for (int r = 0; r < 4; ++r) {
        for (int c = 0; c < 4; ++c) {
            res[c * 4 + r] = a[0 * 4 + r] * b[c * 4 + 0] +
                             a[1 * 4 + r] * b[c * 4 + 1] +
                             a[2 * 4 + r] * b[c * 4 + 2] +
                             a[3 * 4 + r] * b[c * 4 + 3];
        }
    }
    std::memcpy(out, res, sizeof(res));
}

static void mat4Identity(float m[16]) {
    std::fill_n(m, 16, 0.0f);
    m[0] = m[5] = m[10] = m[15] = 1.0f;
}

static void mat4ProjectionFromFov(float m[16], const XrFovf& fov, float nearZ, float farZ) {
    float tanLeft = std::tan(fov.angleLeft);
    float tanRight = std::tan(fov.angleRight);
    float tanDown = std::tan(fov.angleDown);
    float tanUp = std::tan(fov.angleUp);

    std::fill_n(m, 16, 0.0f);
    m[0] = 2.0f / (tanRight - tanLeft);
    m[5] = 2.0f / (tanUp - tanDown);
    m[8] = (tanRight + tanLeft) / (tanRight - tanLeft);
    m[9] = (tanUp + tanDown) / (tanUp - tanDown);
    m[10] = -(farZ + nearZ) / (farZ - nearZ);
    m[11] = -1.0f;
    m[14] = -(2.0f * farZ * nearZ) / (farZ - nearZ);
}

static void mat4ViewFromPose(float m[16], const XrPosef& pose) {
    float qx = pose.orientation.x;
    float qy = pose.orientation.y;
    float qz = pose.orientation.z;
    float qw = pose.orientation.w;

    float px = pose.position.x;
    float py = pose.position.y;
    float pz = pose.position.z;

    // Rotation transpose (inverse)
    float r00 = 1.0f - 2.0f * (qy * qy + qz * qz);
    float r01 = 2.0f * (qx * qy + qz * qw);
    float r02 = 2.0f * (qx * qz - qy * qw);

    float r10 = 2.0f * (qx * qy - qz * qw);
    float r11 = 1.0f - 2.0f * (qx * qx + qz * qz);
    float r12 = 2.0f * (qy * qz + qx * qw);

    float r20 = 2.0f * (qx * qz + qy * qw);
    float r21 = 2.0f * (qy * qz - qx * qw);
    float r22 = 1.0f - 2.0f * (qx * qx + qy * qy);

    m[0] = r00; m[4] = r01; m[8]  = r02; m[12] = -(r00 * px + r01 * py + r02 * pz);
    m[1] = r10; m[5] = r11; m[9]  = r12; m[13] = -(r10 * px + r11 * py + r12 * pz);
    m[2] = r20; m[6] = r21; m[10] = r22; m[14] = -(r20 * px + r21 * py + r22 * pz);
    m[3] = 0.0f; m[7] = 0.0f; m[11] = 0.0f; m[15] = 1.0f;
}

static GLuint compileShader(GLenum type, const char* src) {
    GLuint s = glCreateShader(type);
    glShaderSource(s, 1, &src, nullptr);
    glCompileShader(s);

    GLint success = 0;
    glGetShaderiv(s, GL_COMPILE_STATUS, &success);
    if (!success) {
        char log[512];
        glGetShaderInfoLog(s, sizeof(log), nullptr, log);
        LOGE("Shader compile error: %s", log);
        glDeleteShader(s);
        return 0;
    }
    return s;
}

static GLuint createProgram(const char* vs, const char* fs) {
    GLuint v = compileShader(GL_VERTEX_SHADER, vs);
    GLuint f = compileShader(GL_FRAGMENT_SHADER, fs);
    GLuint prog = glCreateProgram();
    glAttachShader(prog, v);
    glAttachShader(prog, f);
    glLinkProgram(prog);

    GLint success = 0;
    glGetProgramiv(prog, GL_LINK_STATUS, &success);
    if (!success) {
        char log[512];
        glGetProgramInfoLog(prog, sizeof(log), nullptr, log);
        LOGE("Program link error: %s", log);
    }
    glDeleteShader(v);
    glDeleteShader(f);
    return prog;
}

OpenXRContext::OpenXRContext() = default;

OpenXRContext::~OpenXRContext() {
    release();
}

bool OpenXRContext::init(void* vm, void* activity, HmdType hmdType, float targetRefreshRate) {
    LOGI("Initializing OpenXR Context (HMD Type %u, Target %.1f Hz)", (uint32_t)hmdType, targetRefreshRate);

    if (!initEGL()) return false;
    if (!initOpenXRInstance(vm, activity)) return false;
    if (!createSession()) return false;

    initPassthrough();
    initGraphics();
    loadSpatialLogo(vm, activity);

    if (pfn_xrRequestDisplayRefreshRateFB) {
        setRefreshRate(targetRefreshRate);
    }

    LOGI("OpenXR Context fully initialized!");
    return true;
}

bool OpenXRContext::initEGL() {
    m_eglDisplay = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    if (m_eglDisplay == EGL_NO_DISPLAY) {
        LOGE("eglGetDisplay failed");
        return false;
    }

    if (!eglInitialize(m_eglDisplay, nullptr, nullptr)) {
        LOGE("eglInitialize failed");
        return false;
    }

    const EGLint configAttribs[] = {
        EGL_RENDERABLE_TYPE, EGL_OPENGL_ES3_BIT,
        EGL_SURFACE_TYPE, EGL_WINDOW_BIT | EGL_PBUFFER_BIT,
        EGL_RED_SIZE, 8,
        EGL_GREEN_SIZE, 8,
        EGL_BLUE_SIZE, 8,
        EGL_ALPHA_SIZE, 8,
        EGL_DEPTH_SIZE, 24,
        EGL_NONE
    };

    EGLConfig config;
    EGLint numConfigs = 0;
    if (!eglChooseConfig(m_eglDisplay, configAttribs, &config, 1, &numConfigs) || numConfigs == 0) {
        LOGE("eglChooseConfig failed");
        return false;
    }

    const EGLint contextAttribs[] = {
        EGL_CONTEXT_CLIENT_VERSION, 3,
        EGL_NONE
    };

    m_eglContext = eglCreateContext(m_eglDisplay, config, EGL_NO_CONTEXT, contextAttribs);
    if (m_eglContext == EGL_NO_CONTEXT) {
        LOGE("eglCreateContext failed");
        return false;
    }

    const EGLint pbufferAttribs[] = {
        EGL_WIDTH, 16,
        EGL_HEIGHT, 16,
        EGL_NONE
    };
    m_eglSurface = eglCreatePbufferSurface(m_eglDisplay, config, pbufferAttribs);
    if (m_eglSurface == EGL_NO_SURFACE) {
        LOGE("eglCreatePbufferSurface failed: 0x%x", eglGetError());
        return false;
    }
    if (!eglMakeCurrent(m_eglDisplay, m_eglSurface, m_eglSurface, m_eglContext)) {
        LOGE("Initial eglMakeCurrent failed: 0x%x", eglGetError());
        return false;
    }

    LOGI("EGL initialized: %s", reinterpret_cast<const char*>(glGetString(GL_VERSION)));
    return true;
}

bool OpenXRContext::initOpenXRInstance(void* vm, void* activity) {
    // 1. Android loader initialization
    auto pfnInitializeLoaderKHR = (PFN_xrInitializeLoaderKHR)nullptr;
    xrGetInstanceProcAddr(XR_NULL_HANDLE, "xrInitializeLoaderKHR", (PFN_xrVoidFunction*)&pfnInitializeLoaderKHR);
    if (pfnInitializeLoaderKHR) {
        XrLoaderInitInfoAndroidKHR loaderInitInfo{XR_TYPE_LOADER_INIT_INFO_ANDROID_KHR};
        loaderInitInfo.applicationVM = vm;
        loaderInitInfo.applicationContext = activity;
        pfnInitializeLoaderKHR((const XrLoaderInitInfoBaseHeaderKHR*)&loaderInitInfo);
    }

    // 2. Extensions
    std::vector<const char*> enabledExtensions = {
        XR_KHR_OPENGL_ES_ENABLE_EXTENSION_NAME,
        XR_FB_DISPLAY_REFRESH_RATE_EXTENSION_NAME,
        XR_FB_PASSTHROUGH_EXTENSION_NAME
    };

    XrInstanceCreateInfo createInfo{XR_TYPE_INSTANCE_CREATE_INFO};
    std::strncpy(createInfo.applicationInfo.applicationName, "Prism Aperture", XR_MAX_APPLICATION_NAME_SIZE);
    createInfo.applicationInfo.applicationVersion = 1;
    std::strncpy(createInfo.applicationInfo.engineName, "Prism Aperture XR", XR_MAX_ENGINE_NAME_SIZE);
    createInfo.applicationInfo.engineVersion = 1;
    createInfo.applicationInfo.apiVersion = XR_CURRENT_API_VERSION;
    createInfo.enabledExtensionCount = (uint32_t)enabledExtensions.size();
    createInfo.enabledExtensionNames = enabledExtensions.data();

    XrResult res = xrCreateInstance(&createInfo, &m_instance);
    if (XR_FAILED(res)) {
        LOGE("xrCreateInstance failed: %d", res);
        return false;
    }

    // 3. System
    XrSystemGetInfo systemInfo{XR_TYPE_SYSTEM_GET_INFO};
    systemInfo.formFactor = XR_FORM_FACTOR_HEAD_MOUNTED_DISPLAY;
    res = xrGetSystem(m_instance, &systemInfo, &m_systemId);
    if (XR_FAILED(res)) {
        LOGE("xrGetSystem failed: %d", res);
        return false;
    }

    // 4. Function pointers
    xrGetInstanceProcAddr(m_instance, "xrGetDisplayRefreshRateFB", (PFN_xrVoidFunction*)&pfn_xrGetDisplayRefreshRateFB);
    xrGetInstanceProcAddr(m_instance, "xrRequestDisplayRefreshRateFB", (PFN_xrVoidFunction*)&pfn_xrRequestDisplayRefreshRateFB);
    xrGetInstanceProcAddr(m_instance, "xrEnumerateDisplayRefreshRatesFB", (PFN_xrVoidFunction*)&pfn_xrEnumerateDisplayRefreshRatesFB);

    xrGetInstanceProcAddr(m_instance, "xrCreatePassthroughFB", (PFN_xrVoidFunction*)&pfn_xrCreatePassthroughFB);
    xrGetInstanceProcAddr(m_instance, "xrDestroyPassthroughFB", (PFN_xrVoidFunction*)&pfn_xrDestroyPassthroughFB);
    xrGetInstanceProcAddr(m_instance, "xrPassthroughStartFB", (PFN_xrVoidFunction*)&pfn_xrPassthroughStartFB);
    xrGetInstanceProcAddr(m_instance, "xrPassthroughPauseFB", (PFN_xrVoidFunction*)&pfn_xrPassthroughPauseFB);
    xrGetInstanceProcAddr(m_instance, "xrCreatePassthroughLayerFB", (PFN_xrVoidFunction*)&pfn_xrCreatePassthroughLayerFB);
    xrGetInstanceProcAddr(m_instance, "xrDestroyPassthroughLayerFB", (PFN_xrVoidFunction*)&pfn_xrDestroyPassthroughLayerFB);

    return true;
}

bool OpenXRContext::createSession() {
    auto pfnGetOpenGLESGraphicsRequirementsKHR = (PFN_xrGetOpenGLESGraphicsRequirementsKHR)nullptr;
    xrGetInstanceProcAddr(m_instance, "xrGetOpenGLESGraphicsRequirementsKHR", (PFN_xrVoidFunction*)&pfnGetOpenGLESGraphicsRequirementsKHR);
    if (pfnGetOpenGLESGraphicsRequirementsKHR) {
        XrGraphicsRequirementsOpenGLESKHR reqs{XR_TYPE_GRAPHICS_REQUIREMENTS_OPENGL_ES_KHR};
        pfnGetOpenGLESGraphicsRequirementsKHR(m_instance, m_systemId, &reqs);
    }

    XrGraphicsBindingOpenGLESAndroidKHR graphicsBinding{XR_TYPE_GRAPHICS_BINDING_OPENGL_ES_ANDROID_KHR};
    graphicsBinding.display = m_eglDisplay;
    graphicsBinding.config = nullptr;
    graphicsBinding.context = m_eglContext;

    XrSessionCreateInfo sessionInfo{XR_TYPE_SESSION_CREATE_INFO};
    sessionInfo.next = &graphicsBinding;
    sessionInfo.systemId = m_systemId;

    XrResult res = xrCreateSession(m_instance, &sessionInfo, &m_session);
    if (XR_FAILED(res)) {
        LOGE("xrCreateSession failed: %d", res);
        return false;
    }

    // Reference spaces: LOCAL and VIEW
    XrReferenceSpaceCreateInfo spaceInfo{XR_TYPE_REFERENCE_SPACE_CREATE_INFO};
    spaceInfo.referenceSpaceType = XR_REFERENCE_SPACE_TYPE_LOCAL;
    spaceInfo.poseInReferenceSpace.orientation.w = 1.0f;
    xrCreateReferenceSpace(m_session, &spaceInfo, &m_localSpace);

    spaceInfo.referenceSpaceType = XR_REFERENCE_SPACE_TYPE_VIEW;
    xrCreateReferenceSpace(m_session, &spaceInfo, &m_viewSpace);

    initSwapchains();
    return true;
}

bool OpenXRContext::initSwapchains() {
    uint32_t viewCount = 0;
    xrEnumerateViewConfigurationViews(m_instance, m_systemId, XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO, 0, &viewCount, nullptr);
    m_configViews.resize(viewCount, {XR_TYPE_VIEW_CONFIGURATION_VIEW});
    xrEnumerateViewConfigurationViews(m_instance, m_systemId, XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO, viewCount, &viewCount, m_configViews.data());
    m_views.resize(viewCount, {XR_TYPE_VIEW});

    glGenFramebuffers(2, m_fbo);
    glGenRenderbuffers(2, m_depthRb);

    for (uint32_t eye = 0; eye < 2 && eye < viewCount; ++eye) {
        m_swapchains[eye].width = m_configViews[eye].recommendedImageRectWidth;
        m_swapchains[eye].height = m_configViews[eye].recommendedImageRectHeight;

        XrSwapchainCreateInfo createInfo{XR_TYPE_SWAPCHAIN_CREATE_INFO};
        createInfo.usageFlags = XR_SWAPCHAIN_USAGE_COLOR_ATTACHMENT_BIT | XR_SWAPCHAIN_USAGE_SAMPLED_BIT;
        createInfo.format = GL_RGBA8;
        createInfo.sampleCount = 1;
        createInfo.width = m_swapchains[eye].width;
        createInfo.height = m_swapchains[eye].height;
        createInfo.faceCount = 1;
        createInfo.arraySize = 1;
        createInfo.mipCount = 1;

        xrCreateSwapchain(m_session, &createInfo, &m_swapchains[eye].handle);

        uint32_t imageCount = 0;
        xrEnumerateSwapchainImages(m_swapchains[eye].handle, 0, &imageCount, nullptr);
        m_swapchains[eye].images.resize(imageCount, {XR_TYPE_SWAPCHAIN_IMAGE_OPENGL_ES_KHR});
        xrEnumerateSwapchainImages(m_swapchains[eye].handle, imageCount, &imageCount,
                                   (XrSwapchainImageBaseHeader*)m_swapchains[eye].images.data());

        // Setup Depth Renderbuffer for FBO
        glBindRenderbuffer(GL_RENDERBUFFER, m_depthRb[eye]);
        glRenderbufferStorage(GL_RENDERBUFFER, GL_DEPTH_COMPONENT24, m_swapchains[eye].width, m_swapchains[eye].height);

        glBindFramebuffer(GL_FRAMEBUFFER, m_fbo[eye]);
        glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_DEPTH_ATTACHMENT, GL_RENDERBUFFER, m_depthRb[eye]);
        glBindFramebuffer(GL_FRAMEBUFFER, 0);

        LOGI("Eye %u swapchain created: %dx%d (%u images)", eye, m_swapchains[eye].width, m_swapchains[eye].height, imageCount);
    }
    return true;
}

void OpenXRContext::initPassthrough() {
    if (!pfn_xrCreatePassthroughFB || !m_session) return;

    XrPassthroughCreateInfoFB ptInfo{XR_TYPE_PASSTHROUGH_CREATE_INFO_FB};
    ptInfo.flags = 0;
    XrResult res = pfn_xrCreatePassthroughFB(m_session, &ptInfo, &m_passthrough);
    if (XR_SUCCEEDED(res)) {
        XrPassthroughLayerCreateInfoFB layerInfo{XR_TYPE_PASSTHROUGH_LAYER_CREATE_INFO_FB};
        layerInfo.passthrough = m_passthrough;
        layerInfo.purpose = XR_PASSTHROUGH_LAYER_PURPOSE_RECONSTRUCTION_FB;
        pfn_xrCreatePassthroughLayerFB(m_session, &layerInfo, &m_passthroughLayer);
        LOGI("Meta Quest Passthrough FB extension initialized");
    }
}

bool OpenXRContext::initGraphics() {
    // 1. Screen & UI Shader
    const char* vsScreen = R"(#version 300 es
        layout(location = 0) in vec3 a_Pos;
        layout(location = 1) in vec2 a_UV;
        uniform mat4 u_MVP;
        uniform vec4 u_UVScaleOffset;
        out vec2 v_UV;
        void main() {
            vec2 scale = (u_UVScaleOffset.xy == vec2(0.0)) ? vec2(1.0, 1.0) : u_UVScaleOffset.xy;
            v_UV = a_UV * scale + u_UVScaleOffset.zw;
            gl_Position = u_MVP * vec4(a_Pos, 1.0);
        }
    )";
    const char* fsScreen = R"(#version 300 es
        precision mediump float;
        in vec2 v_UV;
        uniform sampler2D u_Texture;
        uniform vec4 u_ColorTint;
        uniform int u_Premultiplied;
        out vec4 FragColor;
        void main() {
            vec4 col = texture(u_Texture, v_UV);
            vec3 rgb = col.rgb;
            if(u_Premultiplied != 0 && col.a > 0.0) rgb /= col.a;
            rgb = mix(rgb / 12.92, pow((rgb + 0.055) / 1.055, vec3(2.4)), step(vec3(0.04045), rgb));
            if(u_Premultiplied != 0) rgb *= col.a;
            FragColor = vec4(rgb, col.a) * u_ColorTint;
        }
    )";
    m_screenProg = createProgram(vsScreen, fsScreen);

    // 1b. Hardware Video Stream Shader (GL_TEXTURE_EXTERNAL_OES from Android SurfaceTexture)
    const char* vsOes = R"(#version 300 es
        layout(location = 0) in vec3 a_Pos;
        layout(location = 1) in vec2 a_UV;
        uniform mat4 u_MVP;
        uniform vec4 u_UVScaleOffset;
        out vec2 v_UV;
        void main() {
            vec2 scale = (u_UVScaleOffset.xy == vec2(0.0)) ? vec2(1.0, 1.0) : u_UVScaleOffset.xy;
            v_UV = a_UV * scale + u_UVScaleOffset.zw;
            gl_Position = u_MVP * vec4(a_Pos, 1.0);
        }
    )";
    const char* fsOes = R"(#version 300 es
        #extension GL_OES_EGL_image_external_essl3 : require
        precision mediump float;
        in vec2 v_UV;
        uniform samplerExternalOES u_Texture;
        uniform vec4 u_ColorTint;
        uniform vec4 u_TargetFov;
        uniform vec4 u_SourceFov;
        uniform vec4 u_RotationDelta;
        uniform int u_Reproject;
        uniform int u_Eye;
        out vec4 FragColor;
        void main() {
            vec2 uv=v_UV;
            if(u_Reproject!=0) {
                vec3 ray=vec3(mix(u_TargetFov.x,u_TargetFov.y,uv.x),mix(u_TargetFov.z,u_TargetFov.w,uv.y),-1.0);
                ray+=2.0*cross(u_RotationDelta.xyz,cross(u_RotationDelta.xyz,ray)+u_RotationDelta.w*ray);
                if(ray.z>=-0.0001){FragColor=vec4(0.0,0.0,0.0,1.0);return;}
                float depth=-ray.z;
                uv.x=(ray.x/depth-u_SourceFov.x)/(u_SourceFov.y-u_SourceFov.x);
                uv.y=(u_SourceFov.z-ray.y/depth)/(u_SourceFov.z-u_SourceFov.w);
            }
            if(any(lessThan(uv,vec2(0.0)))||any(greaterThan(uv,vec2(1.0)))){FragColor=vec4(0.0,0.0,0.0,1.0);return;}
            vec2 stereoUv=vec2((float(u_Eye)+uv.x)*0.5,uv.y);
            vec4 col = texture(u_Texture, stereoUv);
            // MediaCodec supplies display-encoded RGB; the OpenXR compositor
            // consumes linear-light RGBA8. Decode once before composition.
            vec3 rgb = mix(col.rgb / 12.92, pow((col.rgb + 0.055) / 1.055, vec3(2.4)), step(vec3(0.04045), col.rgb));
            FragColor = vec4(rgb, col.a) * u_ColorTint;
        }
    )";
    m_oesVideoProg = createProgram(vsOes, fsOes);

    // 2. Laser Ray Shader
    const char* vsLaser = R"(#version 300 es
        layout(location = 0) in vec3 a_Pos;
        uniform mat4 u_MVP;
        void main() {
            gl_Position = u_MVP * vec4(a_Pos, 1.0);
        }
    )";
    const char* fsLaser = R"(#version 300 es
        precision mediump float;
        uniform vec4 u_Color;
        out vec4 FragColor;
        void main() {
            FragColor = u_Color;
        }
    )";
    m_laserProg = createProgram(vsLaser, fsLaser);

    // 3. Infinite Cyber Grid Shader
    const char* vsGrid = R"(#version 300 es
        layout(location = 0) in vec3 a_Pos;
        uniform mat4 u_MVP;
        out vec3 v_WorldPos;
        void main() {
            v_WorldPos = a_Pos;
            gl_Position = u_MVP * vec4(a_Pos, 1.0);
        }
    )";
    const char* fsGrid = R"(#version 300 es
        precision mediump float;
        in vec3 v_WorldPos;
        out vec4 FragColor;
        void main() {
            vec2 coord = v_WorldPos.xz * 1.5;
            vec2 grid = abs(fract(coord - 0.5) - 0.5) / fwidth(coord);
            float line = min(grid.x, grid.y);
            float c = 1.0 - min(line, 1.0);
            float dist = length(v_WorldPos.xz);
            float fade = clamp(1.0 - dist / 18.0, 0.0, 1.0);
            vec3 gridCol = vec3(0.06847817); // Linear-light equivalent of #4A4A4A
            FragColor = vec4(gridCol, fade * c);
        }
    )";
    m_gridProg = createProgram(vsGrid, fsGrid);

    // Flat Quad Geometry: [-0.5, +0.5]
    float quadVerts[] = {
        // Pos (X, Y, Z), UV (U, V)
        -0.5f,  0.5f, 0.0f,  0.0f, 0.0f,
        -0.5f, -0.5f, 0.0f,  0.0f, 1.0f,
         0.5f,  0.5f, 0.0f,  1.0f, 0.0f,
         0.5f, -0.5f, 0.0f,  1.0f, 1.0f
    };
    glGenVertexArrays(1, &m_quadVao);
    glGenBuffers(1, &m_quadVbo);
    glBindVertexArray(m_quadVao);
    glBindBuffer(GL_ARRAY_BUFFER, m_quadVbo);
    glBufferData(GL_ARRAY_BUFFER, sizeof(quadVerts), quadVerts, GL_STATIC_DRAW);
    glEnableVertexAttribArray(0);
    glVertexAttribPointer(0, 3, GL_FLOAT, GL_FALSE, 5 * sizeof(float), (void*)0);
    glEnableVertexAttribArray(1);
    glVertexAttribPointer(1, 2, GL_FLOAT, GL_FALSE, 5 * sizeof(float), (void*)(3 * sizeof(float)));

    // Curved Cylindrical Geometry (60 degree arc, radius 2.2m, height 1.24m)
    std::vector<float> curvedVerts;
    const int segments = 32;
    const float radius = 2.2f;
    const float height = 1.24f;
    const float arcRad = 1.0472f; // ~60 degrees
    const float startAngle = -arcRad * 0.5f;
    const float angleStep = arcRad / segments;

    for (int i = 0; i < segments; ++i) {
        float a0 = startAngle + i * angleStep;
        float a1 = a0 + angleStep;

        float x0 = std::sin(a0) * radius;
        float z0 = -std::cos(a0) * radius;
        float u0 = (float)i / segments;

        float x1 = std::sin(a1) * radius;
        float z1 = -std::cos(a1) * radius;
        float u1 = (float)(i + 1) / segments;

        // Two triangles per segment
        // Top-left, Bot-left, Top-right
        curvedVerts.insert(curvedVerts.end(), {x0,  height * 0.5f, z0, u0, 0.0f});
        curvedVerts.insert(curvedVerts.end(), {x0, -height * 0.5f, z0, u0, 1.0f});
        curvedVerts.insert(curvedVerts.end(), {x1,  height * 0.5f, z1, u1, 0.0f});

        // Top-right, Bot-left, Bot-right
        curvedVerts.insert(curvedVerts.end(), {x1,  height * 0.5f, z1, u1, 0.0f});
        curvedVerts.insert(curvedVerts.end(), {x0, -height * 0.5f, z0, u0, 1.0f});
        curvedVerts.insert(curvedVerts.end(), {x1, -height * 0.5f, z1, u1, 1.0f});
    }
    m_curvedVertexCount = (int)curvedVerts.size() / 5;

    glGenVertexArrays(1, &m_curvedVao);
    glGenBuffers(1, &m_curvedVbo);
    glBindVertexArray(m_curvedVao);
    glBindBuffer(GL_ARRAY_BUFFER, m_curvedVbo);
    glBufferData(GL_ARRAY_BUFFER, curvedVerts.size() * sizeof(float), curvedVerts.data(), GL_STATIC_DRAW);
    glEnableVertexAttribArray(0);
    glVertexAttribPointer(0, 3, GL_FLOAT, GL_FALSE, 5 * sizeof(float), (void*)0);
    glEnableVertexAttribArray(1);
    glVertexAttribPointer(1, 2, GL_FLOAT, GL_FALSE, 5 * sizeof(float), (void*)(3 * sizeof(float)));

    // Grid Floor Quad: Y = -1.6m, size = 50x50m
    float gridVerts[] = {
        -25.0f, -1.6f, -25.0f,
        -25.0f, -1.6f,  25.0f,
         25.0f, -1.6f, -25.0f,
         25.0f, -1.6f,  25.0f
    };
    m_gridVertexCount = 4;
    glGenVertexArrays(1, &m_gridVao);
    glGenBuffers(1, &m_gridVbo);
    glBindVertexArray(m_gridVao);
    glBindBuffer(GL_ARRAY_BUFFER, m_gridVbo);
    glBufferData(GL_ARRAY_BUFFER, sizeof(gridVerts), gridVerts, GL_STATIC_DRAW);
    glEnableVertexAttribArray(0);
    glVertexAttribPointer(0, 3, GL_FLOAT, GL_FALSE, 3 * sizeof(float), (void*)0);

    // Laser Line Buffer (Dynamic)
    glGenVertexArrays(1, &m_laserVao);
    glGenBuffers(1, &m_laserVbo);
    glBindVertexArray(m_laserVao);
    glBindBuffer(GL_ARRAY_BUFFER, m_laserVbo);
    glBufferData(GL_ARRAY_BUFFER, 6 * sizeof(float), nullptr, GL_DYNAMIC_DRAW);
    glEnableVertexAttribArray(0);
    glVertexAttribPointer(0, 3, GL_FLOAT, GL_FALSE, 3 * sizeof(float), (void*)0);

    // Generate Standby Texture
    std::vector<uint32_t> standbyPixels(512 * 288, 0xFF161A22);
    // Draw central grid and subtle gradient
    for (int y = 0; y < 288; ++y) {
        for (int x = 0; x < 512; ++x) {
            bool onGrid = (x % 32 == 0 || y % 32 == 0);
            uint32_t col = onGrid ? 0xFF2A3445 : 0xFF141822;
            // Center highlight
            int dx = x - 256;
            int dy = y - 144;
            if (dx * dx + dy * dy < 7000) {
                col = 0xFF00B0FF; // Center glowing badge
            }
            standbyPixels[y * 512 + x] = col;
        }
    }
    glGenTextures(1, &m_standbyTexId);
    glBindTexture(GL_TEXTURE_2D, m_standbyTexId);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, 512, 288, 0, GL_RGBA, GL_UNSIGNED_BYTE, standbyPixels.data());

    glBindVertexArray(0);
    return true;
}

bool OpenXRContext::setRefreshRate(float rateHz) {
    if (!pfn_xrRequestDisplayRefreshRateFB || !m_session) return false;
    XrResult res = pfn_xrRequestDisplayRefreshRateFB(m_session, rateHz);
    if (XR_SUCCEEDED(res)) {
        LOGI("Requested display refresh rate: %.1f Hz", rateHz);
        return true;
    }
    return false;
}

std::vector<float> OpenXRContext::getSupportedRefreshRates() {
    std::vector<float> rates;
    if (!pfn_xrEnumerateDisplayRefreshRatesFB || !m_session) return rates;

    uint32_t count = 0;
    pfn_xrEnumerateDisplayRefreshRatesFB(m_session, 0, &count, nullptr);
    if (count > 0) {
        rates.resize(count);
        pfn_xrEnumerateDisplayRefreshRatesFB(m_session, count, &count, rates.data());
    }
    return rates;
}

bool OpenXRContext::pollEvents(bool& exitRenderLoop) {
    exitRenderLoop = false;
    XrEventDataBuffer event{XR_TYPE_EVENT_DATA_BUFFER};

    while (xrPollEvent(m_instance, &event) == XR_SUCCESS) {
        switch (event.type) {
            case XR_TYPE_EVENT_DATA_SESSION_STATE_CHANGED: {
                auto* stateEvent = (XrEventDataSessionStateChanged*)&event;
                m_sessionState = stateEvent->state;
                LOGI("XrEventDataSessionStateChanged: state = %d (IDLE=1, READY=2, SYNC=3, VISIBLE=4, FOCUSED=5, STOPPING=6, LOSS=7, EXITING=8)", (int)m_sessionState);
                if (m_sessionState == XR_SESSION_STATE_READY) {
                    XrSessionBeginInfo beginInfo{XR_TYPE_SESSION_BEGIN_INFO};
                    beginInfo.primaryViewConfigurationType = XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO;
                    XrResult bRes = xrBeginSession(m_session, &beginInfo);
                    if (XR_SUCCEEDED(bRes)) {
                        m_sessionRunning = true;
                        LOGI("OpenXR Session RUNNING (xrBeginSession succeeded)");
                    } else {
                        LOGE("xrBeginSession failed: %d", (int)bRes);
                    }
                } else if (m_sessionState == XR_SESSION_STATE_STOPPING) {
                    xrEndSession(m_session);
                    m_sessionRunning = false;
                    LOGI("OpenXR Session STOPPED");
                } else if (m_sessionState == XR_SESSION_STATE_LOSS_PENDING ||
                           m_sessionState == XR_SESSION_STATE_EXITING) {
                    exitRenderLoop = true;
                }
                break;
            }
            default:
                break;
        }
        event.type = XR_TYPE_EVENT_DATA_BUFFER;
    }
    return true;
}

void OpenXRContext::endFrameEmpty() {
    if (!m_sessionRunning) return;
    XrFrameEndInfo endInfo{XR_TYPE_FRAME_END_INFO};
    endInfo.displayTime = m_frameState.predictedDisplayTime;
    endInfo.environmentBlendMode = XR_ENVIRONMENT_BLEND_MODE_OPAQUE;
    endInfo.layerCount = 0;
    endInfo.layers = nullptr;
    xrEndFrame(m_session, &endInfo);
}

bool OpenXRContext::beginFrame(bool& outShouldRender) {
    outShouldRender = false;
    if (!m_sessionRunning) return false;

    XrFrameWaitInfo waitInfo{XR_TYPE_FRAME_WAIT_INFO};
    XrResult res = xrWaitFrame(m_session, &waitInfo, &m_frameState);
    if (XR_FAILED(res)) {
        LOGE("xrWaitFrame failed: %d", (int)res);
        return false;
    }

    XrFrameBeginInfo beginInfo{XR_TYPE_FRAME_BEGIN_INFO};
    res = xrBeginFrame(m_session, &beginInfo);
    if (XR_FAILED(res)) {
        LOGE("xrBeginFrame failed: %d", (int)res);
        return false;
    }

    m_headValid = false;
    m_viewsValid = false;
    XrSpaceLocation headLocation{XR_TYPE_SPACE_LOCATION};
    const auto headFlags = XR_SPACE_LOCATION_POSITION_VALID_BIT | XR_SPACE_LOCATION_ORIENTATION_VALID_BIT;
    if (xrLocateSpace(m_viewSpace, m_localSpace, m_frameState.predictedDisplayTime, &headLocation) == XR_SUCCESS
        && (headLocation.locationFlags & headFlags) == headFlags) {
        m_headPose = headLocation.pose;
        m_headValid = true;
    }
    outShouldRender = (m_frameState.shouldRender != XR_FALSE);
    XrViewState viewState{XR_TYPE_VIEW_STATE};
    XrViewLocateInfo locateInfo{XR_TYPE_VIEW_LOCATE_INFO};
    locateInfo.viewConfigurationType = XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO;
    locateInfo.displayTime = m_frameState.predictedDisplayTime;
    locateInfo.space = m_localSpace;

    uint32_t viewCount = (uint32_t)m_views.size();
    res = xrLocateViews(m_session, &locateInfo, &viewState, viewCount, &viewCount, m_views.data());
    const XrViewStateFlags valid = XR_VIEW_STATE_ORIENTATION_VALID_BIT | XR_VIEW_STATE_POSITION_VALID_BIT;
    if (XR_FAILED(res) || viewCount != 2 || (viewState.viewStateFlags & valid) != valid) {
        outShouldRender = false;
        endFrameEmpty();
        return true;
    }

    m_viewsValid = true;
    if (!outShouldRender) endFrameEmpty();
    return true;
}

TrackingPacket OpenXRContext::sampleTracking(const InputManager& input) const {
    TrackingPacket packet;
    // LOCAL head-level space is used for all three poses, so flags stays zero.
    packet.sample_time_us = static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(
        std::chrono::steady_clock::now().time_since_epoch()).count());
    auto copyPose = [](TrackingPose& dst, const XrPosef& src, bool valid) {
        dst.valid = valid ? 1 : 0;
        if (!valid) return;
        dst.position[0]=src.position.x; dst.position[1]=src.position.y; dst.position[2]=src.position.z;
        dst.orientation[0]=src.orientation.x; dst.orientation[1]=src.orientation.y;
        dst.orientation[2]=src.orientation.z; dst.orientation[3]=src.orientation.w;
    };
    copyPose(packet.head, m_headPose, m_headValid);
    for (int hand=0; hand<2; ++hand) {
        copyPose(packet.hands[hand], input.getGripPose(hand), input.isGripPoseValid(hand));
        packet.trigger[hand]=input.getTrigger(hand); packet.grip[hand]=input.getGrip(hand);
        const auto stick=input.getStick(hand);
        packet.stick[2*hand]=stick.x; packet.stick[2*hand+1]=stick.y;
        packet.buttons[hand]=input.getButtons(hand);
    }
    if (m_viewsValid) {
        for(int eye=0;eye<2;++eye) {
            const auto& f=m_views[eye].fov;
            packet.fov[4*eye]=f.angleLeft; packet.fov[4*eye+1]=f.angleRight;
            packet.fov[4*eye+2]=f.angleUp; packet.fov[4*eye+3]=f.angleDown;
        }
        const auto& l=m_views[0].pose.position; const auto& r=m_views[1].pose.position;
        packet.ipd=std::sqrt((l.x-r.x)*(l.x-r.x)+(l.y-r.y)*(l.y-r.y)+(l.z-r.z)*(l.z-r.z));
    }
    float refresh = m_frameState.predictedDisplayPeriod > 0 ? 1.0e9f / static_cast<float>(m_frameState.predictedDisplayPeriod) : 0.0f;
    if (pfn_xrGetDisplayRefreshRateFB) {
        float reported=0;
        if (XR_SUCCEEDED(pfn_xrGetDisplayRefreshRateFB(m_session, &reported)) && reported>0) refresh=reported;
    }
    if (refresh>0)
        packet.refresh_rate=static_cast<uint32_t>(refresh+0.5f);
    return packet;
}

void OpenXRContext::renderScene(
    uint32_t desktopTextureId,
    VRGui* gui,
    const InputManager* input,
    bool curvedScreen,
    bool enablePassthrough,
    bool vrMode,
    int desktopWidth,
    int desktopHeight,
    float desktopScale
) {
    if (!m_sessionRunning) return;

    float desktopAspect = desktopWidth > 0 && desktopHeight > 0
        ? static_cast<float>(desktopWidth) / static_cast<float>(desktopHeight)
        : 16.0f / 9.0f;
    desktopAspect = std::clamp(desktopAspect, 0.5f, 3.5f);
    desktopScale = std::clamp(desktopScale, 0.5f, 1.5f);

    for (int eye = 0; eye < 2; ++eye) {
        // Calculate Projection & View Matrices
        float projMat[16], viewMat[16], viewProj[16];
        mat4ProjectionFromFov(projMat, m_views[eye].fov, 0.05f, 100.0f);
        mat4ViewFromPose(viewMat, m_views[eye].pose);
        mat4Multiply(viewProj, projMat, viewMat);

        renderEye(eye, viewProj, desktopTextureId, gui, input, curvedScreen, enablePassthrough, vrMode, desktopAspect, desktopScale);
    }
}

void OpenXRContext::renderEye(
    int eye,
    const float viewProj[16],
    uint32_t desktopTexId,
    VRGui* gui,
    const InputManager* input,
    bool curvedScreen,
    bool enablePassthrough,
    bool vrMode,
    float desktopAspect,
    float desktopScale
) {
    uint32_t imgIdx = 0;
    XrSwapchainImageAcquireInfo acqInfo{XR_TYPE_SWAPCHAIN_IMAGE_ACQUIRE_INFO};
    xrAcquireSwapchainImage(m_swapchains[eye].handle, &acqInfo, &imgIdx);

    XrSwapchainImageWaitInfo waitInfo{XR_TYPE_SWAPCHAIN_IMAGE_WAIT_INFO};
    waitInfo.timeout = XR_INFINITE_DURATION;
    xrWaitSwapchainImage(m_swapchains[eye].handle, &waitInfo);

    // Bind Eye Framebuffer
    glBindFramebuffer(GL_FRAMEBUFFER, m_fbo[eye]);
    glFramebufferTexture2D(
        GL_FRAMEBUFFER,
        GL_COLOR_ATTACHMENT0,
        GL_TEXTURE_2D,
        m_swapchains[eye].images[imgIdx].image,
        0
    );

    glViewport(0, 0, m_swapchains[eye].width, m_swapchains[eye].height);
    glEnable(GL_DEPTH_TEST);
    glEnable(GL_BLEND);
    glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);

    if (enablePassthrough) {
        glClearColor(0.0f, 0.0f, 0.0f, 0.0f); // Transparent for Passthrough
    } else {
        glClearColor(0.01850022f, 0.01850022f, 0.01850022f, 1.0f); // Linear-light #252525
    }
    glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);

    const uint32_t tex = desktopTexId; // No placeholder desktop when disconnected.

    if (vrMode) {
        // --- IMMERSIVE VR MODE: Drops directly into the VR environment ---
        drawFullscreenVrEye(eye, tex);
    } else {
        // --- VIRTUAL DESKTOP DESKTOP MODE ---
        // 1. Render Cyber Grid Floor
        if (!enablePassthrough) {
            drawGridFloor(viewProj);
        }

        // 2. Render Virtual Desktop Screen (Curved or Flat)
        drawSpatialLogo(viewProj); // A world-space image behind the desktop.
        if (tex != 0) drawDesktopScreen(viewProj, tex, curvedScreen, desktopAspect, desktopScale);
    }

    // 3. Render In-VR Floating GUI Menu (if visible)
    if (gui && gui->isVisible()) {
        drawGuiPanel(viewProj, gui->getTextureId());
    }

    // 4. Render Laser Beams and Reticles from active controllers
    if (input && (!vrMode || (gui && gui->isVisible()))) {
        for (int hand = 0; hand < 2; ++hand) {
            if (input->isControllerActive(hand)) {
                float origin[3], dir[3];
                input->getLaserRay(hand, origin, dir);

                float hitPos[3] = {0.0f, 0.0f, 0.0f};
                bool hasHit = false;
                float length = 2.5f;

                // Priority 1: Hit test GUI if visible
                if (gui && gui->isVisible() && gui->isRayHitting()) {
                    hasHit = true;
                    length = gui->getHitDistance();
                    gui->getHitPosition(hitPos);
                } else if (!vrMode && tex != 0) {
                    // Priority 2: Hit test Virtual Desktop Screen
                    float screenZ = -2.2f;
                    if (std::abs(dir[2]) > 0.001f) {
                        float t = (screenZ - origin[2]) / dir[2];
                        if (t > 0.0f && t < 5.0f) {
                            hasHit = true;
                            length = t;
                            hitPos[0] = origin[0] + dir[0] * t;
                            hitPos[1] = origin[1] + dir[1] * t;
                            hitPos[2] = screenZ;
                        }
                    }
                }

                drawLaserAndCursor(viewProj, origin, dir, length, hitPos, hasHit);
            }
        }
    }

    glBindFramebuffer(GL_FRAMEBUFFER, 0);

    XrSwapchainImageReleaseInfo relInfo{XR_TYPE_SWAPCHAIN_IMAGE_RELEASE_INFO};
    xrReleaseSwapchainImage(m_swapchains[eye].handle, &relInfo);
}

void OpenXRContext::drawFullscreenVrEye(int eye, uint32_t textureId) {
    bool isOes = (textureId != m_standbyTexId && textureId != 0 && m_oesVideoProg != 0);
    GLuint prog = isOes ? m_oesVideoProg : m_screenProg;
    glUseProgram(prog);

    GLint locTex = glGetUniformLocation(prog, "u_Texture");
    if (locTex >= 0) glUniform1i(locTex, 0);

    GLint locUV = glGetUniformLocation(prog, "u_UVScaleOffset");
    if (locUV >= 0) {
        if(isOes) glUniform4f(locUV,1.0f,1.0f,0.0f,0.0f);
        else if (eye == 0) {
            // Left eye: left half of SBS stereo frame
            glUniform4f(locUV, 0.5f, 1.0f, 0.0f, 0.0f);
        } else {
            // Right eye: right half of SBS stereo frame
            glUniform4f(locUV, 0.5f, 1.0f, 0.5f, 0.0f);
        }
    }

    float model[16];
    mat4Identity(model);
    model[0] = 2.0f; // Scale [-0.5, 0.5] quad to [-1.0, 1.0] NDC fullscreen
    model[5] = 2.0f;

    GLint locMVP = glGetUniformLocation(prog, "u_MVP");
    GLint locTint = glGetUniformLocation(prog, "u_ColorTint");
    glUniformMatrix4fv(locMVP, 1, GL_FALSE, model);
    glUniform4f(locTint, 1.0f, 1.0f, 1.0f, 1.0f);
    if(isOes){
        const auto& target=m_views[eye].fov;
        const auto& source=m_videoPose.fov+(eye*4);
        const auto& current=m_views[eye].pose.orientation;
        const auto& old=m_videoPose.orientation;
        const float dx=old[3]*current.x-old[0]*current.w-old[1]*current.z+old[2]*current.y;
        const float dy=old[3]*current.y+old[0]*current.z-old[1]*current.w-old[2]*current.x;
        const float dz=old[3]*current.z-old[0]*current.y+old[1]*current.x-old[2]*current.w;
        const float dw=old[3]*current.w+old[0]*current.x+old[1]*current.y+old[2]*current.z;
        const uint64_t nowUs=static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(std::chrono::steady_clock::now().time_since_epoch()).count());
        const bool fresh=m_videoPose.valid&&m_videoPose.sampleTimeUs<=nowUs&&nowUs-m_videoPose.sampleTimeUs<200000;
        const bool smallRotation=std::abs(dw)>=0.94f;
        const bool reproject=fresh&&smallRotation&&m_viewsValid;
        glUniform1i(glGetUniformLocation(prog,"u_Eye"),eye);
        glUniform1i(glGetUniformLocation(prog,"u_Reproject"),reproject?1:0);
        glUniform4f(glGetUniformLocation(prog,"u_TargetFov"),std::tan(target.angleLeft),std::tan(target.angleRight),std::tan(target.angleUp),std::tan(target.angleDown));
        glUniform4f(glGetUniformLocation(prog,"u_SourceFov"),std::tan(source[0]),std::tan(source[1]),std::tan(source[2]),std::tan(source[3]));
        glUniform4f(glGetUniformLocation(prog,"u_RotationDelta"),dx,dy,dz,dw);
        static uint64_t lastPoseLog=0;if(reproject&&nowUs-lastPoseLog>1000000){LOGI("Local rotational reprojection active; tracking sample age %llu us",(unsigned long long)(nowUs-m_videoPose.sampleTimeUs));lastPoseLog=nowUs;}
    }

    glActiveTexture(GL_TEXTURE0);
    if (isOes) {
        glBindTexture(GL_TEXTURE_EXTERNAL_OES, textureId);
    } else {
        glBindTexture(GL_TEXTURE_2D, textureId);
    }

    glDisable(GL_DEPTH_TEST);
    glBindVertexArray(m_quadVao);
    glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    glEnable(GL_DEPTH_TEST);
}

void OpenXRContext::drawDesktopScreen(const float viewProj[16], uint32_t textureId, bool curvedScreen, float aspect, float scale) {
    bool isOes = (textureId != m_standbyTexId && textureId != 0 && m_oesVideoProg != 0);
    GLuint prog = isOes ? m_oesVideoProg : m_screenProg;
    glUseProgram(prog);

    GLint locTex = glGetUniformLocation(prog, "u_Texture");
    if (locTex >= 0) glUniform1i(locTex, 0);

    GLint locUV = glGetUniformLocation(prog, "u_UVScaleOffset");
    if (locUV >= 0) {
        glUniform4f(locUV, 1.0f, 1.0f, 0.0f, 0.0f);
    }

    // Model matrix
    float model[16];
    mat4Identity(model);

    const float aspectScale = aspect / (16.0f / 9.0f);
    if (!curvedScreen) {
        // Keep the stream's pixel aspect ratio; the user scale changes size uniformly.
        model[0] = 2.2f * aspectScale * scale;
        model[5] = 1.2375f * scale;
        model[14] = -2.2f;

        float mvp[16];
        mat4Multiply(mvp, viewProj, model);

        GLint locMVP = glGetUniformLocation(prog, "u_MVP");
        GLint locTint = glGetUniformLocation(prog, "u_ColorTint");
        glUniformMatrix4fv(locMVP, 1, GL_FALSE, mvp);
        glUniform4f(locTint, 1.0f, 1.0f, 1.0f, 1.0f);

        glActiveTexture(GL_TEXTURE0);
        if (isOes) {
            glBindTexture(GL_TEXTURE_EXTERNAL_OES, textureId);
        } else {
            glBindTexture(GL_TEXTURE_2D, textureId);
        }

        glBindVertexArray(m_quadVao);
        glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    } else {
        // Curved Cylindrical screen (already subdivided at radius 2.2m)
        model[0] = aspectScale * scale;
        model[5] = scale;
        float mvp[16];
        mat4Multiply(mvp, viewProj, model);

        GLint locMVP = glGetUniformLocation(prog, "u_MVP");
        GLint locTint = glGetUniformLocation(prog, "u_ColorTint");
        glUniformMatrix4fv(locMVP, 1, GL_FALSE, mvp);
        glUniform4f(locTint, 1.0f, 1.0f, 1.0f, 1.0f);

        glActiveTexture(GL_TEXTURE0);
        if (isOes) {
            glBindTexture(GL_TEXTURE_EXTERNAL_OES, textureId);
        } else {
            glBindTexture(GL_TEXTURE_2D, textureId);
        }

        glBindVertexArray(m_curvedVao);
        glDrawArrays(GL_TRIANGLES, 0, m_curvedVertexCount);
    }
}

void OpenXRContext::loadSpatialLogo(void* vm, void* activity) {
    // Decode the original transparent PNG with Android; no Java source changes.
    auto* javaVm = static_cast<JavaVM*>(vm);
    JNIEnv* env = nullptr;
    if (!javaVm || javaVm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6) != JNI_OK) return;
    if (env->PushLocalFrame(12) != JNI_OK) return;
    auto load = [&]() {
        jclass activityClass = env->GetObjectClass(static_cast<jobject>(activity));
        jmethodID getAssets = env->GetMethodID(activityClass, "getAssets", "()Landroid/content/res/AssetManager;");
        if (!getAssets) return;
        jobject assets = env->CallObjectMethod(static_cast<jobject>(activity), getAssets);
        if (env->ExceptionCheck() || !assets) return;
        AAssetManager* manager = AAssetManager_fromJava(env, assets);
        if (!manager) return;
        AAsset* asset = AAssetManager_open(manager, "logo_spatial.png", AASSET_MODE_BUFFER);
        if (!asset) { LOGW("Spatial logo asset is missing"); return; }
        const auto length = AAsset_getLength(asset);
        jbyteArray bytes = length > 0 && length <= 16 * 1024 * 1024 ? env->NewByteArray(length) : nullptr;
        const void* data = AAsset_getBuffer(asset);
        if (bytes && data) env->SetByteArrayRegion(bytes, 0, length, static_cast<const jbyte*>(data));
        AAsset_close(asset);
        if (!bytes || !data || env->ExceptionCheck()) return;
        jclass factory = env->FindClass("android/graphics/BitmapFactory");
        if (!factory) return;
        jmethodID decode = env->GetStaticMethodID(factory, "decodeByteArray", "([BII)Landroid/graphics/Bitmap;");
        if (!decode) return;
        jobject bitmap = env->CallStaticObjectMethod(factory, decode, bytes, 0, length);
        if (env->ExceptionCheck() || !bitmap) return;
        AndroidBitmapInfo info{};
        void* pixels = nullptr;
        if (AndroidBitmap_getInfo(env, bitmap, &info) != ANDROID_BITMAP_RESULT_SUCCESS
            || info.format != ANDROID_BITMAP_FORMAT_RGBA_8888 || !info.width || !info.height
            || AndroidBitmap_lockPixels(env, bitmap, &pixels) != ANDROID_BITMAP_RESULT_SUCCESS) return;
        // Android bitmaps can have padded rows. Upload tightly packed RGBA.
        std::vector<uint8_t> packed(size_t(info.width) * info.height * 4);
        for (uint32_t y=0; y<info.height; ++y)
            std::memcpy(packed.data() + size_t(y) * info.width * 4,
                static_cast<const uint8_t*>(pixels) + size_t(y) * info.stride, size_t(info.width) * 4);
        AndroidBitmap_unlockPixels(env, bitmap);
        glGenTextures(1, &m_logoTexId);
        glBindTexture(GL_TEXTURE_2D, m_logoTexId);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
        glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, info.width, info.height, 0, GL_RGBA, GL_UNSIGNED_BYTE, packed.data());
        glBindTexture(GL_TEXTURE_2D, 0);
        m_logoAspect = float(info.width) / float(info.height);
        LOGI("Spatial logo loaded: %ux%u", info.width, info.height);
    };
    load();
    if (env->ExceptionCheck()) { env->ExceptionClear(); LOGW("Spatial logo could not be decoded"); }
    env->PopLocalFrame(nullptr);
}

void OpenXRContext::drawSpatialLogo(const float viewProj[16]) {
    if (!m_logoTexId) return;
    glUseProgram(m_screenProg);
    glUniform1i(glGetUniformLocation(m_screenProg, "u_Premultiplied"), 1);
    glUniform1i(glGetUniformLocation(m_screenProg, "u_Texture"), 0);
    glUniform4f(glGetUniformLocation(m_screenProg, "u_UVScaleOffset"), 1, 1, 0, 0);
    float model[16], mvp[16];
    mat4Identity(model);
    model[0] = 2.8f;
    model[5] = 2.8f / m_logoAspect;
    model[13] = 1.25f;
    model[14] = -3.4f; // Behind the desktop, fixed in space rather than head-locked.
    mat4Multiply(mvp, viewProj, model);
    glUniformMatrix4fv(glGetUniformLocation(m_screenProg, "u_MVP"), 1, GL_FALSE, mvp);
    glUniform4f(glGetUniformLocation(m_screenProg, "u_ColorTint"), 1, 1, 1, 1);
    glActiveTexture(GL_TEXTURE0);
    glBindTexture(GL_TEXTURE_2D, m_logoTexId);
    // BitmapFactory produces premultiplied RGBA. Transparent pixels do not
    // write depth, so the image has no invisible rectangular occluder.
    glBlendFunc(GL_ONE, GL_ONE_MINUS_SRC_ALPHA);
    glDepthMask(GL_FALSE);
    glBindVertexArray(m_quadVao);
    glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    glDepthMask(GL_TRUE);
    glUniform1i(glGetUniformLocation(m_screenProg, "u_Premultiplied"), 0);
    glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
}

void OpenXRContext::drawGuiPanel(const float viewProj[16], uint32_t textureId) {
    glUseProgram(m_screenProg);

    GLint locTex = glGetUniformLocation(m_screenProg, "u_Texture");
    if (locTex >= 0) glUniform1i(locTex, 0);
    GLint locUV = glGetUniformLocation(m_screenProg, "u_UVScaleOffset");
    if (locUV >= 0) glUniform4f(locUV, 1.0f, 1.0f, 0.0f, 0.0f);

    // Panel placed at (0, 0.05m, -1.3m), size 0.96m x 0.60m
    float model[16];
    mat4Identity(model);
    model[0] = 0.96f;
    model[5] = 0.60f;
    model[13] = 0.05f;
    model[14] = -1.30f;

    float mvp[16];
    mat4Multiply(mvp, viewProj, model);

    GLint locMVP = glGetUniformLocation(m_screenProg, "u_MVP");
    GLint locTint = glGetUniformLocation(m_screenProg, "u_ColorTint");
    glUniformMatrix4fv(locMVP, 1, GL_FALSE, mvp);
    glUniform4f(locTint, 1.0f, 1.0f, 1.0f, 1.0f);

    glActiveTexture(GL_TEXTURE0);
    glBindTexture(GL_TEXTURE_2D, textureId);

    glBindVertexArray(m_quadVao);
    glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
}

void OpenXRContext::drawLaserAndCursor(
    const float viewProj[16],
    const float origin[3],
    const float dir[3],
    float length,
    const float hitPos[3],
    bool hasHit
) {
    // Pointers are an aiming overlay. The curved desktop lies in front of the
    // legacy flat-plane ray hit near its edges; depth testing hid the reticle.
    const GLboolean depthWasEnabled = glIsEnabled(GL_DEPTH_TEST);
    glDisable(GL_DEPTH_TEST);
    glUseProgram(m_laserProg);
    GLint locMVP = glGetUniformLocation(m_laserProg, "u_MVP");
    GLint locCol = glGetUniformLocation(m_laserProg, "u_Color");

    float idMat[16];
    mat4Identity(idMat);
    float mvp[16];
    mat4Multiply(mvp, viewProj, idMat);
    glUniformMatrix4fv(locMVP, 1, GL_FALSE, mvp);

    // 1. Draw Laser Ray Line (Cyan gradient)
    float p1[3] = {
        origin[0] + dir[0] * length,
        origin[1] + dir[1] * length,
        origin[2] + dir[2] * length
    };
    float lineVerts[6] = {
        origin[0], origin[1], origin[2],
        p1[0], p1[1], p1[2]
    };

    glBindBuffer(GL_ARRAY_BUFFER, m_laserVbo);
    glBufferSubData(GL_ARRAY_BUFFER, 0, sizeof(lineVerts), lineVerts);

    glLineWidth(3.0f);
    glUniform4f(locCol, 0.0f, 0.9f, 1.0f, 0.7f); // Neon Cyan
    glBindVertexArray(m_laserVao);
    glDrawArrays(GL_LINES, 0, 2);

    // 2. Draw Cursor Dot at hit position
    if (hasHit) {
        float cursorModel[16];
        mat4Identity(cursorModel);
        cursorModel[0] = 0.025f; // 2.5cm dot
        cursorModel[5] = 0.025f;
        cursorModel[12] = hitPos[0];
        cursorModel[13] = hitPos[1];
        cursorModel[14] = hitPos[2] + 0.005f; // slightly in front

        float cursorMvp[16];
        mat4Multiply(cursorMvp, viewProj, cursorModel);
        glUniformMatrix4fv(locMVP, 1, GL_FALSE, cursorMvp);
        glUniform4f(locCol, 0.0f, 1.0f, 0.8f, 0.95f); // Bright reticle

        glBindVertexArray(m_quadVao);
        glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    }
    if (depthWasEnabled) glEnable(GL_DEPTH_TEST);
}

void OpenXRContext::drawGridFloor(const float viewProj[16]) {
    glUseProgram(m_gridProg);

    float idMat[16];
    mat4Identity(idMat);
    float mvp[16];
    mat4Multiply(mvp, viewProj, idMat);

    GLint locMVP = glGetUniformLocation(m_gridProg, "u_MVP");
    glUniformMatrix4fv(locMVP, 1, GL_FALSE, mvp);

    glBindVertexArray(m_gridVao);
    glDrawArrays(GL_TRIANGLE_STRIP, 0, m_gridVertexCount);
}

bool OpenXRContext::endFrame(bool enablePassthrough) {
    if (!m_sessionRunning) return false;

    std::vector<const XrCompositionLayerBaseHeader*> layers;

    // Optional: Passthrough Layer
    XrCompositionLayerPassthroughFB passthroughLayer{XR_TYPE_COMPOSITION_LAYER_PASSTHROUGH_FB};
    if (enablePassthrough && pfn_xrCreatePassthroughFB && m_passthrough) {
        if (!m_passthroughActive && pfn_xrPassthroughStartFB) {
            pfn_xrPassthroughStartFB(m_passthrough);
            m_passthroughActive = true;
        }
        passthroughLayer.flags = XR_COMPOSITION_LAYER_BLEND_TEXTURE_SOURCE_ALPHA_BIT;
        passthroughLayer.layerHandle = m_passthroughLayer;
        layers.push_back((XrCompositionLayerBaseHeader*)&passthroughLayer);
    } else {
        if (m_passthroughActive && pfn_xrPassthroughPauseFB) {
            pfn_xrPassthroughPauseFB(m_passthrough);
            m_passthroughActive = false;
        }
    }

    // Stereo Projection Layer
    XrCompositionLayerProjectionView projViews[2];
    for (int eye = 0; eye < 2; ++eye) {
        projViews[eye] = {XR_TYPE_COMPOSITION_LAYER_PROJECTION_VIEW};
        projViews[eye].pose = m_views[eye].pose;
        projViews[eye].fov = m_views[eye].fov;
        projViews[eye].subImage.swapchain = m_swapchains[eye].handle;
        projViews[eye].subImage.imageRect = {{0, 0}, {m_swapchains[eye].width, m_swapchains[eye].height}};
        projViews[eye].subImage.imageArrayIndex = 0;
    }

    XrCompositionLayerProjection projLayer{XR_TYPE_COMPOSITION_LAYER_PROJECTION};
    projLayer.space = m_localSpace;
    projLayer.viewCount = 2;
    projLayer.views = projViews;
    layers.push_back((XrCompositionLayerBaseHeader*)&projLayer);

    XrFrameEndInfo endInfo{XR_TYPE_FRAME_END_INFO};
    endInfo.displayTime = m_frameState.predictedDisplayTime;
    endInfo.environmentBlendMode = XR_ENVIRONMENT_BLEND_MODE_OPAQUE;
    endInfo.layerCount = (uint32_t)layers.size();
    endInfo.layers = layers.data();

    XrResult res = xrEndFrame(m_session, &endInfo);
    if (XR_FAILED(res)) {
        LOGE("xrEndFrame failed: %d", (int)res);
        return false;
    }
    return true;
}

void OpenXRContext::destroySession() {
    if (m_session) {
        xrDestroySession(m_session);
        m_session = XR_NULL_HANDLE;
    }
}

bool OpenXRContext::makeEGLCurrent() {
    if (m_eglDisplay != EGL_NO_DISPLAY && m_eglContext != EGL_NO_CONTEXT) {
        EGLBoolean ok = eglMakeCurrent(m_eglDisplay, m_eglSurface, m_eglSurface, m_eglContext);
        LOGI("makeEGLCurrent on render thread: %s", ok ? "SUCCESS" : "FAILED");
        return ok == EGL_TRUE;
    }
    return false;
}

void OpenXRContext::releaseEGLFromCurrentThread() {
    if (m_eglDisplay != EGL_NO_DISPLAY) {
        eglMakeCurrent(m_eglDisplay, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
        LOGI("releaseEGLFromCurrentThread: EGL context released from calling thread");
    }
}

void OpenXRContext::release() {
    if (m_logoTexId && eglGetCurrentContext() == m_eglContext) glDeleteTextures(1, &m_logoTexId);
    m_logoTexId = 0;
    destroySession();
    if (m_instance) {
        xrDestroyInstance(m_instance);
        m_instance = XR_NULL_HANDLE;
    }
    if (m_eglDisplay != EGL_NO_DISPLAY) {
        eglMakeCurrent(m_eglDisplay, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
        if (m_eglContext != EGL_NO_CONTEXT) eglDestroyContext(m_eglDisplay, m_eglContext);
        if (m_eglSurface != EGL_NO_SURFACE) eglDestroySurface(m_eglDisplay, m_eglSurface);
        eglTerminate(m_eglDisplay);
    }
}

} // namespace openreality
