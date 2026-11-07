#pragma once

#include <cstdint>
#include <cmath>
#include <deque>
#include <openxr/openxr.h>
#include "messages.hpp"

namespace openreality {

namespace input_detail {
// Resolve ownership on release/hold: short presses reach SteamVR as one frame
// of menu-down; a long hold belongs exclusively to the client menu.
struct MenuGesture {
    static constexpr uint64_t HoldMs=500;
    bool down{false}, fired{false}, armed{true}; uint64_t began{};
    void cancel() { down=fired=false; began=0; armed=false; }
    bool step(bool active,bool pressed,uint64_t nowMs,bool& steamPulse) {
        steamPulse=false;
        if(!active || (down && nowMs<began)) { cancel(); return false; }
        if(!armed) { if(!pressed) armed=true; return false; }
        if(pressed) {
            if(!down) { down=true; fired=false; began=nowMs; }
            if(!fired && nowMs-began>=HoldMs) { fired=true; return true; }
        } else if(down) { steamPulse=!fired; down=fired=false; began=0; }
        return false;
    }
};
// Pure, monotonic-millisecond gesture state. A hold clicks once at 500 ms;
// a shorter press clicks on release. Cancellation requires a fresh release.
struct TriggerGesture {
    static constexpr uint64_t HoldMs = 500;
    bool armed{false}, down{false}, fired{false};
    uint64_t began{0};

    void cancel() { armed = down = fired = false; }
    uint32_t step(bool active, bool allowed, float value, uint64_t nowMs) {
        if (!active || !allowed || !std::isfinite(value)) { cancel(); return 0; }
        if (!armed) {
            if (value <= 0.45f) armed = true;
            return 0;
        }
        const bool pressed = down ? value > 0.45f : value >= 0.55f;
        if (!down) {
            if (pressed) { down = true; fired = false; began = nowMs; }
            return 0;
        }
        if (nowMs < began) { cancel(); return 0; }
        const bool held = nowMs - began >= HoldMs;
        uint32_t click = 0;
        if (!fired && (held || !pressed)) { click = held ? 2u : 1u; fired = true; }
        if (!pressed) { down = false; fired = false; }
        return click;
    }
};

// Button edges must survive same-frame click generation and transmission.
// Only adjacent movement-only events may coalesce. Overflow cancels with an
// explicit neutral state, never with a stranded mouse-down.
class MouseEvents {
public:
    static constexpr size_t Limit = 64;
    bool empty() const { return events.empty(); }
    MouseInput frontOr(const MouseInput& fallback) const { return empty() ? fallback : events.front(); }
    bool pop(MouseInput& out) {
        if (empty()) return false;
        out = events.front(); events.pop_front(); return true;
    }
    void motion(const MouseInput& position) {
        if (!empty() && events.back().button_flags == 0 && events.back().scroll_delta == 0
            && position.button_flags == 0 && position.scroll_delta == 0) events.back() = position;
        else if (events.size() < Limit) events.push_back(position);
    }
    void cancel(const MouseInput& position) {
        events.clear(); MouseInput release = position;
        release.button_flags = 0; release.scroll_delta = 0; events.push_back(release);
    }
    bool click(const MouseInput& position, uint32_t button) {
        if (events.size() + 2 > Limit) { cancel(position); return false; }
        MouseInput event = position; event.scroll_delta = 0; event.button_flags = button;
        events.push_back(event); event.button_flags = 0; events.push_back(event); return true;
    }
private:
    std::deque<MouseInput> events;
};
} // namespace input_detail

class InputManager {
public:
    InputManager();
    ~InputManager();

    bool init(XrInstance instance, XrSession session);
    void update(XrTime predictedTime, XrSpace localSpace);

    // Call AFTER GUI hit testing, even on disconnected/inactive frames.
    // allowDesktop = connected && !guiHit. Suppression cancels through release.
    void updateDesktopInput(bool allowDesktop);
    // Drain every frame, INCLUDING GUI hits (only safety releases then).
    bool pollMouseInput(MouseInput& out) { return m_mouseEvents.pop(out); }

    MouseInput getMouseInput() const { return m_mouseEvents.frontOr(m_mouseInput); }
    ControllerInput getControllerInput() const { return m_controllerInput; }

    bool hasNewMouseInput() const { return !m_mouseEvents.empty(); }
    void clearMouseDirty() { MouseInput ignored; m_mouseEvents.pop(ignored); }

    // Laser & Pose Tracking
    bool isControllerActive(int hand) const { return m_controllerActive[hand]; }
    XrPosef getAimPose(int hand) const { return m_aimPoses[hand]; }
    XrPosef getGripPose(int hand) const { return m_gripPoses[hand]; }
    float getTrigger(int hand) const { return m_triggerValues[hand]; }
    float getGrip(int hand) const { return m_gripValues[hand]; }
    bool isGripPoseValid(int hand) const { return m_gripValid[hand]; }
    XrVector2f getStick(int hand) const { return m_sticks[hand]; }
    uint32_t getButtons(int hand) const { return m_buttons[hand]; }
    
    // Laser forward direction from aim pose
    void getLaserRay(int hand, float outOrigin[3], float outDir[3]) const;

    // Menu toggle request (Left Menu Button pressed)
    bool wasMenuToggleRequested() {
        bool req = m_menuToggleRequested;
        m_menuToggleRequested = false;
        return req;
    }

private:
    XrInstance m_instance{XR_NULL_HANDLE};
    XrSession m_session{XR_NULL_HANDLE};

    XrActionSet m_actionSet{XR_NULL_HANDLE};
    XrAction m_aimPoseAction{XR_NULL_HANDLE};
    XrAction m_gripPoseAction{XR_NULL_HANDLE};
    XrAction m_triggerAction{XR_NULL_HANDLE};
    XrAction m_gripAction{XR_NULL_HANDLE};
    XrAction m_thumbstickAction{XR_NULL_HANDLE};
    XrAction m_primaryButtonAction{XR_NULL_HANDLE};
    XrAction m_secondaryButtonAction{XR_NULL_HANDLE};
    XrAction m_stickClickAction{XR_NULL_HANDLE};
    XrAction m_menuButtonAction{XR_NULL_HANDLE};

    XrPath m_handSubpaths[2]{XR_NULL_PATH, XR_NULL_PATH};
    XrSpace m_aimSpaces[2]{XR_NULL_HANDLE, XR_NULL_HANDLE};
    XrSpace m_gripSpaces[2]{XR_NULL_HANDLE, XR_NULL_HANDLE};

    bool m_gripValid[2]{};
    XrVector2f m_sticks[2]{};
    uint32_t m_buttons[2]{};
    bool m_controllerActive[2]{false, false};
    XrPosef m_aimPoses[2]{};
    XrPosef m_gripPoses[2]{};
    float m_triggerValues[2]{0.0f, 0.0f};
    bool m_triggerActive[2]{false, false};
    float m_gripValues[2]{0.0f, 0.0f};

    MouseInput m_mouseInput{};
    ControllerInput m_controllerInput{};
    input_detail::TriggerGesture m_triggerGesture;
    input_detail::MouseEvents m_mouseEvents;
    bool m_desktopEnabled{false};

    float m_lastAimX{0.5f};
    float m_lastAimY{0.5f};
    input_detail::MenuGesture m_menuGesture;
    bool m_menuToggleRequested{false};

    bool raycastScreen(float& outNormalizedX, float& outNormalizedY) const;
};

} // namespace openreality
