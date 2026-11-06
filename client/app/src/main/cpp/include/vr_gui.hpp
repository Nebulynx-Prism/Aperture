#pragma once

#include <cstdint>
#include <string>
#include <vector>
#include <functional>
#include <GLES3/gl3.h>

namespace openreality {

struct GuiButton {
    int id{0};
    int x{0};
    int y{0};
    int w{0};
    int h{0};
    std::string text;
    bool isSelected{false};
    bool isHovered{false};
};

class VRGui {
public:
    VRGui();
    ~VRGui();

    bool init();
    void release();

    // Interaction with laser ray
    bool updateRay(const float rayOrigin[3], const float rayDir[3], bool triggerDown);

    // Update state from system & network
    void setServerInfo(const std::string& name, const std::string& host, int port, bool isWiredUsb);
    void setConnectionStatus(bool isConnected, bool isConnecting, bool isUsb = false);
    // Negative latency/bitrate means no measurement is available.
    void setTelemetry(float latencyMs, float fps, float bitrateMbps);
    void setStreamResolution(int width, int height);
    
    // Visibility & Environment mode
    bool isVisible() const { return m_visible; }
    void setVisible(bool visible) { m_visible = visible; m_dirty = true; }
    void toggleVisible() { m_visible = !m_visible; m_dirty = true; }

    bool isVrMode() const { return m_vrMode; }
    void setVrMode(bool vrMode) { if(m_vrMode != vrMode) { m_vrMode = vrMode; if(vrMode) m_startRequested=false; m_dirty=true; } }
    void toggleVrMode() { if(m_onVrModeChanged) m_onVrModeChanged(!m_vrMode); }

    // Settings state
    int getRefreshRate() const { return m_selectedRefreshRate; }
    std::string getCodec() const { return m_selectedCodec; }
    int getBitrate() const { return m_selectedBitrate; }
    float getResolutionScale() const { return m_resolutionScale; }
    float getDesktopScale() const { return m_desktopScale; }
    bool isCurvedScreen() const { return m_curvedScreen; }
    bool isPassthrough() const { return m_passthroughEnabled; }

    // Action callbacks
    void setConnectCallback(std::function<void(const std::string&, int)> cb) { m_onConnect = cb; }
    void setDisconnectCallback(std::function<void()> cb) { m_onDisconnect = cb; }
    void setLaunchSteamVRCallback(std::function<void()> cb) { m_onLaunchSteamVR = cb; }
    void setLaunchGameCallback(std::function<void(const std::string&)> cb) { m_onLaunchGame = cb; }
    void setRefreshRateCallback(std::function<void(int)> cb) { m_onRefreshRateChanged = cb; }
    void setResolutionScaleCallback(std::function<void(float)> cb) { m_onResolutionScaleChanged = cb; }
    void setCurvedCallback(std::function<void(bool)> cb) { m_onCurvedChanged = cb; }
    void setPassthroughCallback(std::function<void(bool)> cb) { m_onPassthroughChanged = cb; }
    void setVrModeCallback(std::function<void(bool)> cb) { m_onVrModeChanged = cb; }

    // Render texture ID
    GLuint getTextureId() const { return m_textureId; }

    // Allow render loop to flush dirty state without requiring ray input
    bool isDirty() const { return m_dirty; }
    void forceRedraw() { if (m_dirty) redraw(); }
    
    // Laser hit info
    bool isRayHitting() const { return m_rayHit; }
    float getHitDistance() const { return m_hitDistance; }
    void getHitPosition(float outPos[3]) const {
        outPos[0] = m_hitPos[0];
        outPos[1] = m_hitPos[1];
        outPos[2] = m_hitPos[2];
    }

private:
    static constexpr int UI_WIDTH = 1024;
    static constexpr int UI_HEIGHT = 640;
    
    // UI Plane in VR space (meters)
    static constexpr float PLANE_WIDTH = 0.96f;
    static constexpr float PLANE_HEIGHT = 0.60f;
    static constexpr float PLANE_Y = 0.05f;
    static constexpr float PLANE_Z = -1.30f;

    bool m_visible{true};
    bool m_startRequested{false};
    bool m_vrMode{false}; // True = immersive VR mode, False = desktop mode
    GLuint m_textureId{0};
    std::vector<uint32_t> m_pixelBuffer;
    bool m_dirty{true};

    // Tabs: 0 = COMPUTERS, 1 = GAMES, 2 = SETTINGS, 3 = STATS
    int m_activeTab{0};

    // Wi-Fi Server Info
    std::string m_wifiServerName{"Searching for Wi-Fi Server..."};
    std::string m_wifiServerHost{"127.0.0.1"};
    int m_wifiServerPort{38810};
    bool m_wifiFound{false};

    // USB Wired Info
    std::string m_usbServerHost{"127.0.0.1"};
    int m_usbServerPort{38810};
    bool m_usbFound{false};

    // Connection state
    bool m_isConnected{false};
    bool m_isConnecting{false};
    bool m_isUsbConnected{false};

    // Telemetry
    float m_latencyMs{-1.0f};
    float m_fps{0.0f};
    float m_bitrateMbps{-1.0f};
    int m_streamWidth{0};
    int m_streamHeight{0};

    // Settings
    int m_selectedRefreshRate{90};
    std::string m_selectedCodec{"HEVC"};
    int m_selectedBitrate{100};
    float m_resolutionScale{0.85f};
    float m_desktopScale{1.0f};
    bool m_curvedScreen{true};
    bool m_passthroughEnabled{false};

    // Raycast state
    bool m_rayHit{false};
    float m_hitDistance{0.0f};
    float m_hitPos[3]{0.0f, 0.0f, 0.0f};
    int m_hoveredButtonId{-1};
    bool m_prevTriggerDown{false};

    // Callbacks
    std::function<void(const std::string&, int)> m_onConnect;
    std::function<void()> m_onDisconnect;
    std::function<void()> m_onLaunchSteamVR;
    std::function<void(const std::string&)> m_onLaunchGame;
    std::function<void(int)> m_onRefreshRateChanged;
    std::function<void(float)> m_onResolutionScaleChanged;
    std::function<void(bool)> m_onCurvedChanged;
    std::function<void(bool)> m_onPassthroughChanged;
    std::function<void(bool)> m_onVrModeChanged;

    void redraw();
    void drawRect(int x, int y, int w, int h, uint32_t color);
    void drawRoundRect(int x, int y, int w, int h, int radius, uint32_t fillColor, uint32_t borderColor = 0);
    void drawText(int x, int y, const std::string& text, uint32_t color, float scale = 1.0f);
    void drawTextCentered(int x, int y, int w, const std::string& text, uint32_t color, float scale = 1.0f);
    int getTextWidth(const std::string& text, float scale = 1.0f);
    void drawGlyph(int x, int y, char c, uint32_t color, float scale = 1.0f);
    void handleClick(int buttonId);
};

} // namespace openreality
