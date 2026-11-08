#include "../abi/openvr_shim.cpp"
extern "C" __declspec(dllexport) vr::ITrackedDeviceServerDriver* ApertureTestDevice(){static Device device(0);return &device;}
extern "C" __declspec(dllexport) vr::IVRDisplayComponent* ApertureTestDisplay(){static Display display;return &display;}
