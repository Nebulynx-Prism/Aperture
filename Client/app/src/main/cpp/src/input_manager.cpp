#include "input_manager.hpp"
#if !defined(PRISM_INPUT_GESTURE_TEST)
#include <android/log.h>
#include <cmath>
#include <cstring>
#include <algorithm>
#include <vector>
#include <chrono>

#define LOG_TAG "PrismAperture_Input"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define LOGW(...) __android_log_print(ANDROID_LOG_WARN, LOG_TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

namespace openreality {

InputManager::InputManager() {
    // The wire contract is normalized absolute coordinates, not pixels.
    m_mouseInput.x = m_mouseInput.y = 0.5f;
    m_mouseInput.absolute = true;
}

InputManager::~InputManager() {
    for (int i = 0; i < 2; ++i) {
        if (m_aimSpaces[i]) xrDestroySpace(m_aimSpaces[i]);
        if (m_gripSpaces[i]) xrDestroySpace(m_gripSpaces[i]);
    }
    if (m_actionSet) {
        xrDestroyActionSet(m_actionSet);
    }
}

bool InputManager::init(XrInstance instance, XrSession session) {
    m_instance = instance;
    m_session = session;

    xrStringToPath(m_instance, "/user/hand/left", &m_handSubpaths[0]);
    xrStringToPath(m_instance, "/user/hand/right", &m_handSubpaths[1]);

    XrActionSetCreateInfo setInfo{XR_TYPE_ACTION_SET_CREATE_INFO};
    std::strncpy(setInfo.actionSetName, "desktop_input", XR_MAX_ACTION_SET_NAME_SIZE);
    std::strncpy(setInfo.localizedActionSetName, "Prism Aperture Controller Input", XR_MAX_LOCALIZED_ACTION_SET_NAME_SIZE);
    setInfo.priority = 0;
    XrResult res = xrCreateActionSet(m_instance, &setInfo, &m_actionSet);
    if (XR_FAILED(res)) {
        LOGE("Failed to create action set: %d", res);
        return false;
    }

    auto createAction = [&](const char* name, const char* localized, XrActionType type, XrAction& outAction) {
        XrActionCreateInfo actInfo{XR_TYPE_ACTION_CREATE_INFO};
        std::strncpy(actInfo.actionName, name, XR_MAX_ACTION_NAME_SIZE);
        std::strncpy(actInfo.localizedActionName, localized, XR_MAX_LOCALIZED_ACTION_NAME_SIZE);
        actInfo.actionType = type;
        actInfo.countSubactionPaths = 2;
        actInfo.subactionPaths = m_handSubpaths;
        return xrCreateAction(m_actionSet, &actInfo, &outAction);
    };

    createAction("aim_pose", "Aim Pose", XR_ACTION_TYPE_POSE_INPUT, m_aimPoseAction);
    createAction("grip_pose", "Grip Pose", XR_ACTION_TYPE_POSE_INPUT, m_gripPoseAction);
    createAction("trigger", "Trigger", XR_ACTION_TYPE_FLOAT_INPUT, m_triggerAction);
    createAction("grip", "Grip", XR_ACTION_TYPE_FLOAT_INPUT, m_gripAction);
    createAction("thumbstick", "Thumbstick", XR_ACTION_TYPE_VECTOR2F_INPUT, m_thumbstickAction);
    createAction("primary_button", "Button A/X", XR_ACTION_TYPE_BOOLEAN_INPUT, m_primaryButtonAction);
    createAction("secondary_button", "Button B/Y", XR_ACTION_TYPE_BOOLEAN_INPUT, m_secondaryButtonAction);
    createAction("stick_click", "Stick Click", XR_ACTION_TYPE_BOOLEAN_INPUT, m_stickClickAction);
    createAction("menu_button", "Menu Button", XR_ACTION_TYPE_BOOLEAN_INPUT, m_menuButtonAction);

    // Create action spaces for laser aim and grip positions
    for (int i = 0; i < 2; ++i) {
        XrActionSpaceCreateInfo spaceInfo{XR_TYPE_ACTION_SPACE_CREATE_INFO};
        spaceInfo.action = m_aimPoseAction;
        spaceInfo.subactionPath = m_handSubpaths[i];
        spaceInfo.poseInActionSpace.orientation.w = 1.0f;
        xrCreateActionSpace(m_session, &spaceInfo, &m_aimSpaces[i]);

        spaceInfo.action = m_gripPoseAction;
        xrCreateActionSpace(m_session, &spaceInfo, &m_gripSpaces[i]);
    }

    // 1. Suggest bindings for Meta Quest / Oculus Touch Controllers
    {
        XrPath oculusProfilePath = XR_NULL_PATH;
        xrStringToPath(m_instance, "/interaction_profiles/oculus/touch_controller", &oculusProfilePath);

        std::vector<XrActionSuggestedBinding> bindings;
        auto bind = [&](XrAction action, const char* pathStr) {
            XrPath path = XR_NULL_PATH;
            if (XR_SUCCEEDED(xrStringToPath(m_instance, pathStr, &path))) {
                bindings.push_back({action, path});
            }
        };

        // Left hand bindings
        bind(m_aimPoseAction, "/user/hand/left/input/aim/pose");
        bind(m_gripPoseAction, "/user/hand/left/input/grip/pose");
        bind(m_triggerAction, "/user/hand/left/input/trigger/value");
        bind(m_gripAction, "/user/hand/left/input/squeeze/value");
        bind(m_thumbstickAction, "/user/hand/left/input/thumbstick");
        bind(m_stickClickAction, "/user/hand/left/input/thumbstick/click");
        bind(m_primaryButtonAction, "/user/hand/left/input/x/click");
        bind(m_secondaryButtonAction, "/user/hand/left/input/y/click");
        bind(m_menuButtonAction, "/user/hand/left/input/menu/click");

        // Right hand bindings
        bind(m_aimPoseAction, "/user/hand/right/input/aim/pose");
        bind(m_gripPoseAction, "/user/hand/right/input/grip/pose");
        bind(m_triggerAction, "/user/hand/right/input/trigger/value");
        bind(m_gripAction, "/user/hand/right/input/squeeze/value");
        bind(m_thumbstickAction, "/user/hand/right/input/thumbstick");
        bind(m_stickClickAction, "/user/hand/right/input/thumbstick/click");
        bind(m_primaryButtonAction, "/user/hand/right/input/a/click");
        bind(m_secondaryButtonAction, "/user/hand/right/input/b/click");

        XrInteractionProfileSuggestedBinding profileBindings{XR_TYPE_INTERACTION_PROFILE_SUGGESTED_BINDING};
        profileBindings.interactionProfile = oculusProfilePath;
        profileBindings.countSuggestedBindings = (uint32_t)bindings.size();
        profileBindings.suggestedBindings = bindings.data();
        res = xrSuggestInteractionProfileBindings(m_instance, &profileBindings);
        if (XR_FAILED(res)) {
            LOGW("xrSuggestInteractionProfileBindings for Oculus Touch returned: %d", res);
        } else {
            LOGI("Oculus Touch interaction profile successfully bound (%zu actions)", bindings.size());
        }
    }

    // 2. Suggest fallback bindings for Simple Controller
    {
        XrPath simpleProfilePath = XR_NULL_PATH;
        xrStringToPath(m_instance, "/interaction_profiles/khr/simple_controller", &simpleProfilePath);

        std::vector<XrActionSuggestedBinding> bindings;
        auto bind = [&](XrAction action, const char* pathStr) {
            XrPath path = XR_NULL_PATH;
            if (XR_SUCCEEDED(xrStringToPath(m_instance, pathStr, &path))) {
                bindings.push_back({action, path});
            }
        };

        bind(m_aimPoseAction, "/user/hand/left/input/aim/pose");
        bind(m_aimPoseAction, "/user/hand/right/input/aim/pose");
        bind(m_gripPoseAction, "/user/hand/left/input/grip/pose");
        bind(m_gripPoseAction, "/user/hand/right/input/grip/pose");
        bind(m_triggerAction, "/user/hand/left/input/select/click");
        bind(m_triggerAction, "/user/hand/right/input/select/click");
        bind(m_menuButtonAction, "/user/hand/left/input/menu/click");

        XrInteractionProfileSuggestedBinding profileBindings{XR_TYPE_INTERACTION_PROFILE_SUGGESTED_BINDING};
        profileBindings.interactionProfile = simpleProfilePath;
        profileBindings.countSuggestedBindings = (uint32_t)bindings.size();
        profileBindings.suggestedBindings = bindings.data();
        xrSuggestInteractionProfileBindings(m_instance, &profileBindings);
    }

    // Attach action set
    XrSessionActionSetsAttachInfo attachInfo{XR_TYPE_SESSION_ACTION_SETS_ATTACH_INFO};
    attachInfo.countActionSets = 1;
    attachInfo.actionSets = &m_actionSet;
    res = xrAttachSessionActionSets(m_session, &attachInfo);
    if (XR_FAILED(res)) {
        LOGE("xrAttachSessionActionSets failed: %d", res);
        return false;
    }

    LOGI("InputManager initialized and attached to OpenXR session successfully");
    return true;
}

void InputManager::update(XrTime predictedTime, XrSpace localSpace) {
    for (int hand = 0; hand < 2; ++hand) {
        m_gripValid[hand] = false; m_sticks[hand] = {}; m_buttons[hand] = 0;
        m_controllerActive[hand] = m_triggerActive[hand] = false;
        m_triggerValues[hand] = m_gripValues[hand] = 0.0f;
    }
    if (!m_session || !m_actionSet) return;

    // 1. Sync actions
    XrActiveActionSet activeSet{m_actionSet, XR_NULL_PATH};
    XrActionsSyncInfo syncInfo{XR_TYPE_ACTIONS_SYNC_INFO};
    syncInfo.countActiveActionSets = 1;
    syncInfo.activeActionSets = &activeSet;
    // XR_SESSION_NOT_FOCUSED is a positive result, but must not inject input.
    if (xrSyncActions(m_session, &syncInfo) != XR_SUCCESS) {
        m_menuGesture.cancel();
        return;
    }

    // 2. Query both hands
    for (int hand = 0; hand < 2; ++hand) {
        // Query Aim Pose
        XrSpaceLocation aimLocation{XR_TYPE_SPACE_LOCATION};
        XrActionStateGetInfo aimInfo{XR_TYPE_ACTION_STATE_GET_INFO};
        aimInfo.action = m_aimPoseAction;
        aimInfo.subactionPath = m_handSubpaths[hand];
        XrActionStatePose aimState{XR_TYPE_ACTION_STATE_POSE};
        const bool aimActive = xrGetActionStatePose(m_session, &aimInfo, &aimState) == XR_SUCCESS
            && aimState.isActive;
        const bool located = aimActive && xrLocateSpace(m_aimSpaces[hand], localSpace, predictedTime, &aimLocation) == XR_SUCCESS;
        bool validPos = (aimLocation.locationFlags & XR_SPACE_LOCATION_POSITION_VALID_BIT) != 0;
        bool validRot = (aimLocation.locationFlags & XR_SPACE_LOCATION_ORIENTATION_VALID_BIT) != 0;

        m_controllerActive[hand] = aimActive && located && validPos && validRot;
        if (m_controllerActive[hand]) {
            m_aimPoses[hand] = aimLocation.pose;
        }

        // Query Grip Pose
        XrSpaceLocation gripLocation{XR_TYPE_SPACE_LOCATION};
        aimInfo.action = m_gripPoseAction;
        XrActionStatePose gripPoseState{XR_TYPE_ACTION_STATE_POSE};
        m_gripValid[hand] = xrGetActionStatePose(m_session, &aimInfo, &gripPoseState) == XR_SUCCESS
            && gripPoseState.isActive
            && xrLocateSpace(m_gripSpaces[hand], localSpace, predictedTime, &gripLocation) == XR_SUCCESS
            && (gripLocation.locationFlags & XR_SPACE_LOCATION_POSITION_VALID_BIT)
            && (gripLocation.locationFlags & XR_SPACE_LOCATION_ORIENTATION_VALID_BIT);
        if (m_gripValid[hand] && (gripLocation.locationFlags & XR_SPACE_LOCATION_POSITION_VALID_BIT) &&
            (gripLocation.locationFlags & XR_SPACE_LOCATION_ORIENTATION_VALID_BIT)) {
            m_gripPoses[hand] = gripLocation.pose;
        }

        // Query Trigger
        XrActionStateFloat triggerState{XR_TYPE_ACTION_STATE_FLOAT};
        XrActionStateGetInfo getInfo{XR_TYPE_ACTION_STATE_GET_INFO};
        getInfo.action = m_triggerAction;
        getInfo.subactionPath = m_handSubpaths[hand];
        m_triggerActive[hand] = xrGetActionStateFloat(m_session, &getInfo, &triggerState) == XR_SUCCESS
            && triggerState.isActive && std::isfinite(triggerState.currentState);
        if (m_triggerActive[hand]) m_triggerValues[hand] = triggerState.currentState;

        XrActionStateVector2f stick{XR_TYPE_ACTION_STATE_VECTOR2F};
        getInfo.action = m_thumbstickAction;
        if (xrGetActionStateVector2f(m_session, &getInfo, &stick) == XR_SUCCESS && stick.isActive
            && std::isfinite(stick.currentState.x) && std::isfinite(stick.currentState.y)) m_sticks[hand] = stick.currentState;
        XrAction buttons[] = {m_primaryButtonAction, m_secondaryButtonAction, m_menuButtonAction, m_stickClickAction};
        for (int bit=0; bit<4; ++bit) {
            getInfo.action = buttons[bit];
            XrActionStateBoolean state{XR_TYPE_ACTION_STATE_BOOLEAN};
            if (xrGetActionStateBoolean(m_session, &getInfo, &state) == XR_SUCCESS && state.isActive && state.currentState)
                m_buttons[hand] |= 1u << bit;
        }
        // Query Grip Squeeze
        XrActionStateFloat gripState{XR_TYPE_ACTION_STATE_FLOAT};
        getInfo.action = m_gripAction;
        if (xrGetActionStateFloat(m_session, &getInfo, &gripState) == XR_SUCCESS
            && gripState.isActive && std::isfinite(gripState.currentState))
            m_gripValues[hand] = gripState.currentState;
    }

    // 3. Query Left Controller Menu Button (for In-VR Menu toggle)
    {
        XrActionStateBoolean menuState{XR_TYPE_ACTION_STATE_BOOLEAN};
        XrActionStateGetInfo getInfo{XR_TYPE_ACTION_STATE_GET_INFO};
        getInfo.action = m_menuButtonAction;
        getInfo.subactionPath = m_handSubpaths[0]; // Left hand
        const bool menuActive = xrGetActionStateBoolean(m_session, &getInfo, &menuState) == XR_SUCCESS
            && menuState.isActive;
        bool isDown = menuActive && (menuState.currentState != XR_FALSE);
        const uint64_t nowMs=static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::milliseconds>(
            std::chrono::steady_clock::now().time_since_epoch()).count());
        bool steamPulse=false;
        m_buttons[0]&=~4u;
        if (m_menuGesture.step(menuActive,isDown,nowMs,steamPulse)) {
            m_menuToggleRequested = true;
            LOGI("Menu held for 500ms -> Toggle client GUI");
        }
        if(steamPulse) m_buttons[0]|=4u;
    }

}

void InputManager::updateDesktopInput(bool allowDesktop) {
    const auto now = std::chrono::steady_clock::now().time_since_epoch();
    const uint64_t nowMs = static_cast<uint64_t>(
        std::chrono::duration_cast<std::chrono::milliseconds>(now).count());
    float normX = m_mouseInput.x, normY = m_mouseInput.y;
    const bool active = m_controllerActive[1] && m_triggerActive[1];
    const bool enabled = allowDesktop && active && raycastScreen(normX, normY);
    const uint32_t click = m_triggerGesture.step(active, enabled, m_triggerValues[1], nowMs);
    if (!enabled) {
        // Remove unsent desktop clicks when GUI/focus/tracking takes ownership.
        // A neutral packet also releases a down edge already sent by the caller.
        if (m_desktopEnabled || !m_mouseEvents.empty()) m_mouseEvents.cancel(m_mouseInput);
        m_desktopEnabled = false;
        return;
    }

    m_mouseInput.x = normX;
    m_mouseInput.y = normY;
    m_mouseInput.absolute = true;
    m_mouseInput.button_flags = 0;
    m_mouseInput.scroll_delta = 0;
    if (!m_desktopEnabled || std::abs(normX - m_lastAimX) > 0.0005f
        || std::abs(normY - m_lastAimY) > 0.0005f) {
        m_lastAimX = normX; m_lastAimY = normY;
        m_mouseEvents.motion(m_mouseInput);
    }
    m_desktopEnabled = true;
    if (click && !m_mouseEvents.click(m_mouseInput, click)) m_triggerGesture.cancel();

    // Squeeze no longer maps to right-click: trigger hold owns that gesture.
    XrActionStateVector2f stickState{XR_TYPE_ACTION_STATE_VECTOR2F};
    XrActionStateGetInfo getInfo{XR_TYPE_ACTION_STATE_GET_INFO};
    getInfo.action = m_thumbstickAction;
    getInfo.subactionPath = m_handSubpaths[1];
    if (xrGetActionStateVector2f(m_session, &getInfo, &stickState) == XR_SUCCESS
        && stickState.isActive && std::isfinite(stickState.currentState.y)
        && std::abs(stickState.currentState.y) > 0.2f) {
        MouseInput scroll = m_mouseInput;
        scroll.scroll_delta = static_cast<int32_t>(
            std::max(-1.0f, std::min(1.0f, stickState.currentState.y)) * 120.0f);
        m_mouseEvents.motion(scroll);
    }
}
void InputManager::getLaserRay(int hand, float outOrigin[3], float outDir[3]) const {
    const XrPosef& pose = m_aimPoses[hand];
    outOrigin[0] = pose.position.x;
    outOrigin[1] = pose.position.y;
    outOrigin[2] = pose.position.z;

    // Rotate unit vector (0, 0, -1) by quaternion (qx, qy, qz, qw)
    float qx = pose.orientation.x;
    float qy = pose.orientation.y;
    float qz = pose.orientation.z;
    float qw = pose.orientation.w;

    outDir[0] = -2.0f * (qx * qz + qw * qy);
    outDir[1] = -2.0f * (qy * qz - qw * qx);
    outDir[2] = -(1.0f - 2.0f * (qx * qx + qy * qy));

    // Normalize
    float len = std::sqrt(outDir[0] * outDir[0] + outDir[1] * outDir[1] + outDir[2] * outDir[2]);
    if (len > 0.0001f) {
        outDir[0] /= len;
        outDir[1] /= len;
        outDir[2] /= len;
    }
}

bool InputManager::raycastScreen(float& outNormalizedX, float& outNormalizedY) const {
    // Virtual desktop plane is located at Z = -2.2m, width = 2.2m, height = 1.2375m (16:9)
    const float screenZ = -2.2f;
    const float screenHalfW = 1.1f;
    const float screenHalfH = 0.61875f;

    float origin[3], dir[3];
    getLaserRay(1, origin, dir); // Hand 1 = Right hand

    for (int i = 0; i < 3; ++i) {
        if (!std::isfinite(origin[i]) || !std::isfinite(dir[i])) return false;
    }
    if (std::abs(dir[2]) < 0.001f) return false;

    // Plane intersection: (screenZ - origin.z) / dir.z
    float t = (screenZ - origin[2]) / dir[2];
    if (t <= 0.0f || !std::isfinite(t)) return false;

    float hitX = origin[0] + dir[0] * t;
    float hitY = origin[1] + dir[1] * t;

    // Map [-screenHalfW, +screenHalfW] to [0.0, 1.0]
    float u = (hitX + screenHalfW) / (2.0f * screenHalfW);
    // Map [+screenHalfH, -screenHalfH] to [0.0, 1.0] (top to bottom)
    float v = (screenHalfH - hitY) / (2.0f * screenHalfH);

    if (u < 0.0f || u > 1.0f || v < 0.0f || v > 1.0f) return false;
    outNormalizedX = std::max(0.0f, std::min(1.0f, u));
    outNormalizedY = std::max(0.0f, std::min(1.0f, v));
    return true;
}

} // namespace openreality

#else // PRISM_INPUT_GESTURE_TEST: host-only tests, no Android/OpenXR runtime.
#include <cstdio>
#include <cstdlib>
#include <limits>
using openreality::MouseInput;
using openreality::input_detail::TriggerGesture;
using openreality::input_detail::MouseEvents;
static int checks = 0;
static void expect(bool ok, const char* name) {
    ++checks;
    if (!ok) { std::fprintf(stderr, "FAIL: %s\n", name); std::exit(1); }
}
static TriggerGesture ready() {
    TriggerGesture g;
    expect(g.step(true, true, 0.0f, 0) == 0, "neutral arms without click");
    return g;
}
int main() {
    {
        auto g = ready();
        expect(g.step(true, true, 1, 10) == 0, "press is deferred");
        expect(g.step(true, true, 1, 508) == 0, "498ms is not a hold");
        expect(g.step(true, true, 0, 509) == 1, "499ms release left-clicks");
        expect(g.step(true, true, 0, 510) == 0, "tap does not repeat");
    }
    {
        auto g = ready(); g.step(true, true, 1, 10);
        expect(g.step(true, true, 1, 510) == 2, "500ms hold right-clicks");
        expect(g.step(true, true, 1, 5000) == 0, "long hold right-clicks only once");
        expect(g.step(true, true, 0, 5001) == 0, "hold release never left-clicks");
        g.step(true, true, 1, 5010);
        expect(g.step(true, true, 0, 5100) == 1, "fresh tap after hold works");
    }
    {
        auto g = ready(); g.step(true, true, 1, 10);
        expect(g.step(true, true, 0, 510) == 2, "release exactly at threshold is right-click");
        g.step(true, true, 1, 1000);
        expect(g.step(true, true, 0, 2000) == 2, "skipped hold frames still classify right");
    }
    {
        auto g = ready();
        expect(g.step(true, true, .54f, 1) == 0 && !g.down, "press hysteresis");
        g.step(true, true, .55f, 10);
        expect(g.step(true, true, .50f, 40) == 0, "trigger noise does not release");
        expect(g.step(true, true, .46f, 60) == 0, "release hysteresis holds");
        expect(g.step(true, true, .45f, 80) == 1, "release threshold taps once");
    }
    {
        auto g = ready(); g.step(true, true, 1, 10);
        expect(g.step(true, false, 1, 100) == 0, "menu hit cancels press");
        expect(g.step(true, true, 1, 1000) == 0, "leaving menu held does not right-click");
        expect(g.step(true, true, 0, 1001) == 0, "GUI release does not left-click desktop");
        g.step(true, true, 1, 1010);
        expect(g.step(true, true, 0, 1100) == 1, "fresh desktop tap works after GUI");
    }
    {
        auto g = ready(); g.step(true, true, 1, 10);
        expect(g.step(false, true, 1, 100) == 0, "tracking loss cancels press");
        expect(g.step(true, true, 1, 1000) == 0, "tracking regain held stays blocked");
        expect(g.step(true, true, 0, 1001) == 0, "tracking regain release cannot click");
        g.step(true, true, 1, 1010);
        expect(g.step(true, true, 0, 1100) == 1, "tracking regain requires fresh press");
        g.step(true, true, 1, 1200);
        expect(g.step(true, true, std::numeric_limits<float>::quiet_NaN(), 1300) == 0, "NaN cancels input");
        expect(g.step(true, true, 0, 1400) == 0, "NaN cancellation cannot tap");
    }
    {
        TriggerGesture g;
        expect(g.step(true, true, 1, 0) == 0 && g.step(true, true, 1, 1000) == 0,
               "controller acquired while held cannot click");
        g.step(true, true, 0, 1001); g.step(true, true, 1, 1100);
        expect(g.step(true, true, 1, 1099) == 0, "backwards clock cancels gesture");
        expect(g.step(true, true, 0, 1200) == 0, "clock cancellation cannot tap");
    }
    MouseInput location{}; location.x = .25f; location.y = .75f; location.absolute = true;
    {
        MouseEvents q; MouseInput event{};
        expect(q.click(location, 1), "tap queues successfully");
        expect(q.pop(event) && event.button_flags == 1, "tap down delivered");
        expect(event.x == .25f && event.y == .75f && event.absolute && event.scroll_delta == 0,
               "normalized absolute coordinates preserved");
        expect(q.pop(event) && event.button_flags == 0, "tap up is distinct event");
        expect(!q.pop(event), "FIFO drained");
        q.click(location, 2);
        expect(q.pop(event) && event.button_flags == 2, "hold right down");
        expect(q.pop(event) && event.button_flags == 0, "hold right up");
    }
    {
        MouseEvents q; MouseInput event{}, next = location; next.x = .5f;
        q.motion(location); q.motion(next);
        expect(q.pop(event) && event.x == .5f && !q.pop(event), "movement may coalesce");
        q.click(location, 1); q.motion(next);
        expect(q.pop(event) && event.button_flags == 1, "movement never overwrites click down");
        expect(q.pop(event) && event.button_flags == 0, "movement preserves click release");
        MouseInput scroll = location; scroll.scroll_delta = 120;
        q.motion(scroll); q.click(location, 1);
        expect(q.pop(event) && event.scroll_delta == 120, "scroll occurs once");
        expect(q.pop(event) && event.button_flags == 1 && event.scroll_delta == 0, "click does not repeat scroll");
        expect(q.pop(event) && event.button_flags == 0 && event.scroll_delta == 0, "release does not repeat scroll");
    }
    {
        MouseEvents q; MouseInput event{};
        q.click(location, 1); q.pop(event); // down already transmitted
        q.cancel(location);
        expect(q.pop(event) && event.button_flags == 0 && !q.pop(event), "cancellation releases sent down");
        for (size_t i = 0; i < MouseEvents::Limit / 2; ++i) q.click(location, 2);
        expect(!q.click(location, 1), "queue capacity is bounded");
        expect(q.pop(event) && event.button_flags == 0 && !q.pop(event), "overflow cancels to neutral release");
        q.click(location, 1); q.cancel(location);
        expect(q.pop(event) && event.button_flags == 0 && !q.pop(event), "GUI cancellation removes pending clicks");
    }
    std::printf("PASS: %d gesture timing, GUI/inactive cancellation, FIFO/normalized-coordinate checks\n", checks);
}
#endif
