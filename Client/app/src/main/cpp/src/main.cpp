#include <jni.h>
#include <android/native_window_jni.h>
#include <android/log.h>
#include <thread>
#include <atomic>
#include <algorithm>
#include <memory>
#include <chrono>
#include <cmath>
#include <GLES3/gl32.h>
#include <GLES3/gl3ext.h>

#ifndef GL_TEXTURE_EXTERNAL_OES
#define GL_TEXTURE_EXTERNAL_OES 0x8D65
#endif

#include "openxr_context.hpp"
#include "video_decoder.hpp"
#include "audio_player.hpp"
#include "network_client.hpp"
#include "input_manager.hpp"
#include "vr_gui.hpp"

#define LOG_TAG "PrismAperture_Main"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define LOGW(...) __android_log_print(ANDROID_LOG_WARN, LOG_TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

namespace openreality {

struct ClientEngine {
    std::unique_ptr<OpenXRContext> openxr;
    std::unique_ptr<VRGui> gui;
    std::unique_ptr<InputManager> input;
    std::unique_ptr<VideoDecoder> decoder;
    std::unique_ptr<AudioPlayer> audio;
    std::unique_ptr<NetworkClient> network;

    JavaVM* jvm{nullptr};
    jobject activityObj{nullptr};
    ANativeWindow* surfaceWindow{nullptr};
    GLuint videoTextureId{0};
    jmethodID updateVideoTexMid{nullptr};
    jmethodID getVideoTimestampMid{nullptr};

    std::thread renderThread;
    std::thread connectionThread;
    std::mutex connectionMutex;
    bool shuttingDown{false};
    std::atomic<bool> serverVrMode{false};
    std::atomic<bool> isRunning{false};
    std::atomic<bool> isPaused{false};
    std::atomic<bool> isConnecting{false};
    std::atomic<bool> connectionIsUsb{false};
    std::string currentHost{""};
    int currentPort{0};

    HmdType hmdType{HmdType::QUEST_2};
    int recommendedEyeWidth{1920};
    int recommendedEyeHeight{1080};
    std::atomic<int> renderWidth{1920};
    std::atomic<int> renderHeight{1080};
    std::atomic<float> resolutionScale{0.85f};
    float refreshRate{90.0f};
    std::string codecName{"HEVC"};
};

static void doConnect(ClientEngine* engine, const std::string& host, int port) {
    if (!engine || !engine->network) return;
    std::lock_guard<std::mutex> connectionLock(engine->connectionMutex);
    if (engine->shuttingDown) return;

    if (engine->network->isConnected() && engine->currentHost == host && engine->currentPort == port) {
        LOGI("Already connected to %s:%d, ignoring redundant request", host.c_str(), port);
        return;
    }

    bool wasConnecting = engine->isConnecting.exchange(true);
    if (wasConnecting) {
        LOGI("Connection attempt already in progress to %s:%d, ignoring request to %s:%d",
             engine->currentHost.c_str(), engine->currentPort, host.c_str(), port);
        return;
    }

    if (engine->connectionThread.joinable()) engine->connectionThread.join();
    engine->currentHost = host;
    engine->currentPort = port;
    bool isUsb = (host == "127.0.0.1");
    engine->connectionIsUsb = isUsb;

    // Own the worker so teardown cannot destroy its captured engine.
    engine->connectionThread = std::thread([engine, host, port, isUsb]() {
        LOGI("Background connect thread started for %s:%d (USB=%d)...", host.c_str(), port, isUsb);
        // A new connection must receive type 102 before any video is accepted.
        engine->decoder->release();
        bool ok = engine->network->connect(host, static_cast<uint16_t>(port));
        if (ok) {
            MobileSettings settings{};
            settings.hmd_type = static_cast<uint32_t>(engine->hmdType);
            settings.resolution_width = static_cast<uint32_t>(engine->renderWidth.load());
            settings.resolution_height = static_cast<uint32_t>(engine->renderHeight.load());
            settings.refresh_rate = static_cast<uint32_t>(engine->refreshRate);
            settings.desktop_framerate = static_cast<uint32_t>(engine->refreshRate);
            settings.vr_framerate = static_cast<uint32_t>(engine->refreshRate);
            engine->network->sendMobileSettings(settings);

            LOGI("Connected successfully to %s:%d (USB=%d)!", host.c_str(), port, isUsb);
        } else {
            LOGW("Failed to connect to %s:%d (USB=%d)", host.c_str(), port, isUsb);
        }
        engine->isConnecting = false;
    });
}

static void runRenderLoop(ClientEngine* engine) {
    // CRITICAL: Make EGL context current on THIS thread.
    // EGL contexts are thread-local. initEGL() made it current on the main thread,
    // but this render loop runs on a separate std::thread. Without this call,
    // every single GL operation (framebuffer bind, draw, texture upload) is a no-op,
    // producing a permanent black screen.
    if (!engine->openxr->makeEGLCurrent()) {
        engine->isRunning = false;
        return;
    }

    JNIEnv* env = nullptr;
    if (engine->jvm) {
        engine->jvm->AttachCurrentThread(&env, nullptr);
    }

    LOGI("VR stereo render loop started at %.1f FPS target", engine->refreshRate);

    auto lastFpsTime = std::chrono::steady_clock::now();
    int frameCounter = 0;
    float currentFps = engine->refreshRate;

    // Always consume the FIFO, including safety releases queued by suppression.
    // Disconnected events are discarded rather than replayed on a new connection.
    auto updateDesktopInput = [engine](bool allowDesktop) {
        const bool connected = engine->network->isConnected();
        engine->input->updateDesktopInput(connected && allowDesktop);
        MouseInput event{};
        while (engine->input->pollMouseInput(event)) {
            if (engine->network->isConnected()) engine->network->sendInputEvent(event);
        }
    };

    while (engine->isRunning) {
        if (engine->isPaused) {
            updateDesktopInput(false);
            std::this_thread::sleep_for(std::chrono::milliseconds(20));
            continue;
        }

        engine->gui->setConnectionStatus(engine->network->isConnected(),
            engine->isConnecting.load(), engine->connectionIsUsb.load());
        bool exitLoop = false;
        engine->openxr->pollEvents(exitLoop);
        if (exitLoop) {
            LOGI("OpenXR requested exit from render loop");
            break;
        }

        if (!engine->openxr->isSessionRunning()) {
            updateDesktopInput(false);
            std::this_thread::sleep_for(std::chrono::milliseconds(10));
            continue;
        }

        bool shouldRender = false;
        if (engine->openxr->beginFrame(shouldRender)) {
            engine->input->update(engine->openxr->getPredictedTime(), engine->openxr->getLocalSpace());
            engine->network->sendTrackingPacket(engine->openxr->sampleTracking(*engine->input));
            const bool stereoReady = engine->network->isConnected() && engine->serverVrMode.load()
                && engine->network->isStereoStream() && engine->decoder->hasOutputFrame();
            engine->gui->setVrMode(stereoReady);
            if (!shouldRender) {
                updateDesktopInput(false);
                // Per OpenXR spec: frame submitted with 0 layers, continue waiting
                continue;
            }


            // 1. Update Controller Inputs & Action Sets
            // Inputs and tracking were sampled once above, even on non-rendering frames.

            // Toggle In-VR Menu when Left Menu Button is clicked
            if (engine->input->wasMenuToggleRequested()) {
                engine->gui->toggleVisible();
            }

            // 2. Controller Laser Interaction (Right hand preferred, Left hand fallback)
            bool guiHit = false;
            int pointerHand = -1;
            if (engine->input->isControllerActive(1)) {
                pointerHand = 1;
            } else if (engine->input->isControllerActive(0)) {
                pointerHand = 0;
            }

            if (pointerHand != -1) {
                float rayOrigin[3], rayDir[3];
                engine->input->getLaserRay(pointerHand, rayOrigin, rayDir);
                bool triggerDown = (engine->input->getTrigger(pointerHand) > 0.5f);

                guiHit = engine->gui->updateRay(rayOrigin, rayDir, triggerDown);
            }

            // GUI misses allow desktop interaction even while the menu is visible.
            // Drain all events, including release pairs when the GUI takes focus.
            updateDesktopInput(!guiHit && !stereoReady);

            // 4. Update Telemetry & FPS
            frameCounter++;
            auto now = std::chrono::steady_clock::now();
            auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(now - lastFpsTime).count();
            if (elapsed >= 1000) {
                currentFps = (frameCounter * 1000.0f) / elapsed;
                frameCounter = 0;
                lastFpsTime = now;
                engine->gui->setTelemetry(-1.0f, currentFps, -1.0f);
            }

            // 4b. Ensure GUI texture is up-to-date even without active controller input
            engine->gui->forceRedraw();

        engine->decoder->drainOutput();
        engine->gui->setStreamResolution(engine->decoder->getDecodedFrameWidth(), engine->decoder->getDecodedFrameHeight());

        // Latch latest hardware video frame from SurfaceTexture
        if (env && engine->updateVideoTexMid && engine->activityObj) {
            env->CallVoidMethod(engine->activityObj, engine->updateVideoTexMid);
            VideoFramePose pose{};
            if(engine->getVideoTimestampMid){jlong timestamp=env->CallLongMethod(engine->activityObj,engine->getVideoTimestampMid);if(timestamp>0)pose=engine->decoder->framePose(static_cast<uint64_t>(timestamp/1000));}
            engine->openxr->setVideoPose(pose);
        }


            // 5. Stereo Render Eye Scene (Hardware Video Stream + VR Environment / Desktop Quad)
            engine->openxr->renderScene(
                engine->network->isConnected() && engine->decoder->hasOutputFrame() ? engine->videoTextureId : 0,
                engine->gui.get(),
                engine->input.get(),
                engine->gui->isCurvedScreen(),
                engine->gui->isPassthrough(),
                stereoReady,
                engine->decoder->getDecodedFrameWidth(),
                engine->decoder->getDecodedFrameHeight(),
                engine->gui->getDesktopScale()
            );

            // 6. Submit OpenXR Projection Layer
            engine->openxr->endFrame(engine->gui->isPassthrough());
        } else {
            updateDesktopInput(false);
            std::this_thread::sleep_for(std::chrono::milliseconds(5));
        }
    }

    updateDesktopInput(false);
    engine->openxr->releaseEGLFromCurrentThread();
    if (engine->jvm) {
        engine->jvm->DetachCurrentThread();
    }

    LOGI("VR stereo render loop stopped cleanly");
}

} // namespace openreality

using namespace openreality;

extern "C" {

JNIEXPORT jlong JNICALL
Java_org_nebulynx_aperture_MainActivity_nativeInit(
    JNIEnv* env,
    jobject activity,
    jint hmdType,
    jint width,
    jint height,
    jfloat refreshRate,
    jstring preferredCodec
) {
    LOGI("nativeInit: HMD %d (%dx%d @ %.1f Hz)", hmdType, width, height, refreshRate);

    auto* engine = new ClientEngine();
    engine->hmdType = static_cast<HmdType>(hmdType);
    engine->renderWidth = width;
    engine->renderHeight = height;
    engine->refreshRate = refreshRate;

    const char* codecStr = env->GetStringUTFChars(preferredCodec, nullptr);
    engine->codecName = codecStr ? codecStr : "HEVC";
    env->ReleaseStringUTFChars(preferredCodec, codecStr);

    env->GetJavaVM(&engine->jvm);
    engine->activityObj = env->NewGlobalRef(activity);

    // 1. Initialize OpenXR Context
    engine->openxr = std::make_unique<OpenXRContext>();
    if (!engine->openxr->init(engine->jvm, engine->activityObj, engine->hmdType, engine->refreshRate)) {
        LOGE("Failed to initialize OpenXRContext!");
        env->DeleteGlobalRef(engine->activityObj);
        delete engine;
        return 0;
    }
    const auto eyeWidth = engine->openxr->getRecommendedEyeWidth();
    const auto eyeHeight = engine->openxr->getRecommendedEyeHeight();
    if (eyeWidth >= 512 && eyeWidth <= 4096 && eyeHeight >= 512 && eyeHeight <= 4096) {
        engine->recommendedEyeWidth = static_cast<int>(eyeWidth);
        engine->recommendedEyeHeight = static_cast<int>(eyeHeight);
        engine->renderWidth = std::min(2048, static_cast<int>(std::lround(eyeWidth * engine->resolutionScale.load())));
        engine->renderHeight = std::min(2048, static_cast<int>(std::lround(eyeHeight * engine->resolutionScale.load())));
        LOGI("OpenXR per-eye target %ux%u; selected stream target %dx%d (%.0f%%)", eyeWidth, eyeHeight,
             engine->renderWidth.load(), engine->renderHeight.load(), engine->resolutionScale.load() * 100.0f);
    } else {
        LOGW("OpenXR recommended eye size unavailable (%ux%u); retaining device-profile dimensions",
             eyeWidth, eyeHeight);
    }

    // 2. Initialize In-VR GUI System
    engine->gui = std::make_unique<VRGui>();
    engine->gui->init();

    // 3. Initialize Input Manager with Oculus Touch bindings
    engine->input = std::make_unique<InputManager>();
    engine->input->init(engine->openxr->getInstance(), engine->openxr->getSession());

    // 4. Initialize Hardware Video Decoder with off-screen SurfaceTexture
    jclass actCls = env->GetObjectClass(activity);
    engine->updateVideoTexMid = env->GetMethodID(actCls, "updateVideoTexture", "()V");
    engine->getVideoTimestampMid = env->GetMethodID(actCls, "getVideoTextureTimestamp", "()J");

    glGenTextures(1, &engine->videoTextureId);
    glBindTexture(GL_TEXTURE_EXTERNAL_OES, engine->videoTextureId);
    glTexParameteri(GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    glBindTexture(GL_TEXTURE_EXTERNAL_OES, 0);

    jmethodID createSurfaceMid = env->GetMethodID(actCls, "createVideoSurface", "(I)Landroid/view/Surface;");
    if (createSurfaceMid) {
        jobject jSurface = env->CallObjectMethod(activity, createSurfaceMid, static_cast<jint>(engine->videoTextureId));
        if (jSurface) {
            engine->surfaceWindow = ANativeWindow_fromSurface(env, jSurface);
        }
    }

    VideoCodecType codecType = VideoCodecType::HEVC;
    if (engine->codecName == "AV1") codecType = VideoCodecType::AV1;
    else if (engine->codecName == "H264") codecType = VideoCodecType::H264;
    engine->decoder = std::make_unique<VideoDecoder>();
    if (engine->surfaceWindow) {
        engine->decoder->init(engine->surfaceWindow, codecType, width, height);
    }

    // 5. Initialize Low-Latency AAudio
    engine->audio = std::make_unique<AudioPlayer>();
    engine->audio->init(48000, 2);

    // 6. Initialize Network Client
    engine->network = std::make_unique<NetworkClient>(engine->decoder.get(), engine->audio.get());

    // Hook GUI Callbacks
    engine->gui->setConnectCallback([engine](const std::string& h, int p) {
        doConnect(engine, h, p);
    });
    engine->gui->setDisconnectCallback([engine]() {
        std::lock_guard<std::mutex> connectionLock(engine->connectionMutex);
        if (engine->shuttingDown) return;
        if (engine->connectionThread.joinable()) engine->connectionThread.join();
        engine->isConnecting = true;
        engine->connectionThread = std::thread([engine]() {
            engine->network->disconnect();
            engine->decoder->release();
            engine->isConnecting = false;
        });
    });
    engine->gui->setLaunchSteamVRCallback([engine]() {
        engine->network->launchSteamVR();
    });
    engine->gui->setLaunchGameCallback([engine](const std::string& g) {
        engine->network->launchGame(g);
    });
    engine->gui->setRefreshRateCallback([engine](int rate) {
        engine->openxr->setRefreshRate((float)rate);
    });
    engine->gui->setResolutionScaleCallback([engine](float scale) {
        engine->resolutionScale = scale;
        const int width = std::min(2048, static_cast<int>(std::lround(engine->recommendedEyeWidth * scale)));
        const int height = std::min(2048, static_cast<int>(std::lround(engine->recommendedEyeHeight * scale)));
        engine->renderWidth = width;
        engine->renderHeight = height;
        if (engine->network->isConnected()) {
            MobileSettings settings{};
            settings.hmd_type = static_cast<uint32_t>(engine->hmdType);
            settings.resolution_width = static_cast<uint32_t>(width);
            settings.resolution_height = static_cast<uint32_t>(height);
            settings.refresh_rate = static_cast<uint32_t>(engine->refreshRate);
            settings.desktop_framerate = static_cast<uint32_t>(engine->refreshRate);
            settings.vr_framerate = static_cast<uint32_t>(engine->refreshRate);
            engine->network->sendMobileSettings(settings);
        }
        LOGI("Requested dynamic PCVR eye target %dx%d (%.0f%%)", width, height, scale * 100.0f);
    });
    engine->gui->setVrModeCallback([engine](bool vr) {
        if (vr) engine->network->launchSteamVR();
        else engine->network->showDesktop();
    });

    // Receiver only publishes atomic state. GUI and GL remain render-thread owned.
    engine->network->setVrModeCallback([engine](bool vr) { engine->serverVrMode = vr; });

    // Release EGL context from main thread BEFORE spawning render thread.
    // EGL contexts are thread-local: the render thread must call makeEGLCurrent().
    // All GL resources (shaders, VAOs, textures) were created above and are owned by
    // the context, not the thread — they'll be available once the render thread acquires it.
    engine->openxr->releaseEGLFromCurrentThread();

    // Start VR stereo render loop
    engine->isRunning = true;
    engine->renderThread = std::thread(runRenderLoop, engine);

    return reinterpret_cast<jlong>(engine);
}

JNIEXPORT void JNICALL
Java_org_nebulynx_aperture_MainActivity_nativeSetDiscoveredServer(
    JNIEnv* env,
    jobject /*thiz*/,
    jlong handle,
    jstring name,
    jstring host,
    jint port,
    jboolean isUsb
) {
    auto* engine = reinterpret_cast<ClientEngine*>(handle);
    if (!engine || !engine->gui) return;

    const char* nStr = env->GetStringUTFChars(name, nullptr);
    const char* hStr = env->GetStringUTFChars(host, nullptr);
    std::string nameString = nStr ? nStr : "Prism Aperture PC";
    std::string hostString = hStr ? hStr : "127.0.0.1";
    env->ReleaseStringUTFChars(name, nStr);
    env->ReleaseStringUTFChars(host, hStr);

    LOGI("Discovered server pushed to In-VR GUI: %s at %s:%d (USB=%d)",
         nameString.c_str(), hostString.c_str(), port, isUsb);
    engine->gui->setServerInfo(nameString, hostString, port, isUsb);

    // Auto-connect if not already connected and no connection attempt in progress
    if (engine->network && !engine->network->isConnected() && !engine->isConnecting) {
        LOGI("Auto-connecting to discovered server at %s:%d (USB=%d)...", hostString.c_str(), port, isUsb);
        doConnect(engine, hostString, port);
    }
}

JNIEXPORT void JNICALL
Java_org_nebulynx_aperture_MainActivity_nativeConnect(
    JNIEnv* env,
    jobject /*thiz*/,
    jlong handle,
    jstring host,
    jint port
) {
    auto* engine = reinterpret_cast<ClientEngine*>(handle);
    if (!engine) return;

    const char* hostStr = env->GetStringUTFChars(host, nullptr);
    std::string hostString = hostStr ? hostStr : "127.0.0.1";
    env->ReleaseStringUTFChars(host, hostStr);

    doConnect(engine, hostString, port);
}

JNIEXPORT void JNICALL
Java_org_nebulynx_aperture_MainActivity_nativeLaunchSteamVR(
    JNIEnv* /*env*/,
    jobject /*thiz*/,
    jlong handle
) {
    auto* engine = reinterpret_cast<ClientEngine*>(handle);
    if (engine && engine->network) {
        engine->network->launchSteamVR();
    }
}

JNIEXPORT void JNICALL
Java_org_nebulynx_aperture_MainActivity_nativeLaunchGame(
    JNIEnv* env,
    jobject /*thiz*/,
    jlong handle,
    jstring gameId
) {
    auto* engine = reinterpret_cast<ClientEngine*>(handle);
    if (!engine || !engine->network) return;

    const char* gStr = env->GetStringUTFChars(gameId, nullptr);
    std::string gid = gStr ? gStr : "250820";
    env->ReleaseStringUTFChars(gameId, gStr);

    engine->network->launchGame(gid);
}

JNIEXPORT void JNICALL
Java_org_nebulynx_aperture_MainActivity_nativeOnResume(
    JNIEnv* /*env*/,
    jobject /*thiz*/,
    jlong handle
) {
    auto* engine = reinterpret_cast<ClientEngine*>(handle);
    if (engine) {
        LOGI("nativeOnResume: resuming render loop");
        engine->isPaused = false;
    }
}

JNIEXPORT void JNICALL
Java_org_nebulynx_aperture_MainActivity_nativeOnPause(
    JNIEnv* /*env*/,
    jobject /*thiz*/,
    jlong handle
) {
    auto* engine = reinterpret_cast<ClientEngine*>(handle);
    if (engine) {
        LOGI("nativeOnPause: keeping OpenXR VR compositor active");
    }
}

JNIEXPORT void JNICALL
Java_org_nebulynx_aperture_MainActivity_nativeDestroy(
    JNIEnv* env,
    jobject /*thiz*/,
    jlong handle
) {
    auto* engine = reinterpret_cast<ClientEngine*>(handle);
    if (!engine) return;

    LOGI("nativeDestroy: tearing down engine");
    {
        std::lock_guard<std::mutex> connectionLock(engine->connectionMutex);
        engine->shuttingDown = true;
    }
    engine->isRunning = false;
    if (engine->renderThread.joinable()) {
        engine->renderThread.join();
    }
    // connect() may still use NetworkClient; let it finish before disconnect/destruction.
    if (engine->connectionThread.joinable()) engine->connectionThread.join();
    if (engine->network) {
        engine->network->disconnect();
    }
    engine->openxr->makeEGLCurrent();
    if (engine->decoder) engine->decoder->release();
    if (engine->videoTextureId) glDeleteTextures(1, &engine->videoTextureId);
    if (engine->gui) {
        engine->gui->release();
    }
    if (engine->surfaceWindow) {
        ANativeWindow_release(engine->surfaceWindow);
        engine->surfaceWindow = nullptr;
    }
    if (engine->activityObj) {
        env->DeleteGlobalRef(engine->activityObj);
        engine->activityObj = nullptr;
    }

    delete engine;
    LOGI("ClientEngine destroyed cleanly");
}

} // extern "C"
