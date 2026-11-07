#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#define XR_NO_PROTOTYPES
#define XR_USE_PLATFORM_WIN32
#define XR_USE_GRAPHICS_API_D3D11
#include <windows.h>
#include <d3d11.h>
#include <dxgi.h>
#include <wrl/client.h>
#include <openxr/openxr.h>
#include <openxr/openxr_platform.h>
#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <stdexcept>
#include <string>
#include <vector>
using Microsoft::WRL::ComPtr;
namespace fs = std::filesystem;
using Clock = std::chrono::steady_clock;
#define XR_FUNCTIONS(X) \
 X(xrEnumerateInstanceExtensionProperties) X(xrCreateInstance) X(xrDestroyInstance) \
 X(xrGetInstanceProperties) X(xrGetSystem) X(xrGetSystemProperties) \
 X(xrEnumerateViewConfigurations) X(xrEnumerateViewConfigurationViews) \
 X(xrGetD3D11GraphicsRequirementsKHR) X(xrCreateSession) X(xrDestroySession) \
 X(xrCreateReferenceSpace) X(xrDestroySpace) X(xrLocateSpace) \
 X(xrEnumerateSwapchainFormats) X(xrCreateSwapchain) X(xrDestroySwapchain) \
 X(xrEnumerateSwapchainImages) X(xrAcquireSwapchainImage) X(xrWaitSwapchainImage) \
 X(xrReleaseSwapchainImage) X(xrPollEvent) X(xrBeginSession) X(xrEndSession) \
 X(xrRequestExitSession) X(xrWaitFrame) X(xrBeginFrame) X(xrEndFrame) X(xrLocateViews)
struct Api {
 PFN_xrGetInstanceProcAddr get = nullptr;
#define DECL(n) PFN_##n n = nullptr;
 XR_FUNCTIONS(DECL)
#undef DECL
 template<class T> void load(XrInstance instance, const char* name, T& out) {
  XrResult r = get(instance, name, reinterpret_cast<PFN_xrVoidFunction*>(&out));
  std::cout << "resolve " << name << " result=" << r << " available=" << (out != nullptr) << '\n';
  if (XR_FAILED(r) || !out) throw std::runtime_error(std::string("Missing function: ") + name);
 }
};
static void check(XrResult r, const char* call) {
 if (XR_FAILED(r)) throw std::runtime_error(std::string(call) + " failed XrResult=" + std::to_string(r));
}
#define CHECK(c) check((c), #c)
static void hr(HRESULT r, const char* call) {
 if (FAILED(r)) throw std::runtime_error(std::string(call) + " HRESULT=" + std::to_string(static_cast<unsigned long>(r)));
}
struct Eye {
 XrSwapchain swap = XR_NULL_HANDLE;
 uint32_t width = 0, height = 0;
 std::vector<XrSwapchainImageD3D11KHR> images;
 std::vector<ComPtr<ID3D11RenderTargetView>> targets;
};
struct Probe {
 HMODULE loader = nullptr;
 Api a;
 XrInstance instance = XR_NULL_HANDLE;
 XrSession session = XR_NULL_HANDLE;
 XrSpace local = XR_NULL_HANDLE, view = XR_NULL_HANDLE;
 bool running = false;
 ComPtr<ID3D11Device> device;
 ComPtr<ID3D11DeviceContext> context;
 std::vector<Eye> eyes;
 ~Probe() {
  // Destruction is also valid when a runtime failure prevents an orderly STOPPING transition.
  for (auto& e : eyes) { e.targets.clear(); if (e.swap && a.xrDestroySwapchain) report(a.xrDestroySwapchain(e.swap), "destroy swapchain"); }
  if (view && a.xrDestroySpace) report(a.xrDestroySpace(view), "destroy VIEW");
  if (local && a.xrDestroySpace) report(a.xrDestroySpace(local), "destroy LOCAL");
  if (session && a.xrDestroySession) report(a.xrDestroySession(session), "destroy session");
  if (instance && a.xrDestroyInstance) report(a.xrDestroyInstance(instance), "destroy instance");
  context.Reset(); device.Reset();
  if (loader) FreeLibrary(loader);
 }
 static void report(XrResult r, const char* label) { if (XR_FAILED(r)) std::cerr << label << " cleanup error=" << r << '\n'; }
 void initialize(const fs::path& manifest, const fs::path& dll) {
  if (!SetEnvironmentVariableW(L"XR_RUNTIME_JSON", manifest.c_str())) throw std::runtime_error("Set XR_RUNTIME_JSON failed");
  std::wcout << L"XR_RUNTIME_JSON=" << manifest.wstring() << L"\nloader=" << dll.wstring() << '\n';
  loader = LoadLibraryExW(dll.c_str(), nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
  if (!loader) throw std::runtime_error("LoadLibraryExW failed Win32=" + std::to_string(GetLastError()));
  auto address = GetProcAddress(loader, "xrGetInstanceProcAddr");
  static_assert(sizeof(address) == sizeof(a.get));
  std::memcpy(&a.get, &address, sizeof(address));
  if (!a.get) throw std::runtime_error("Loader missing xrGetInstanceProcAddr");
  std::cout << "loader xrGetInstanceProcAddr export available; runtime negotiation follows xrCreateInstance\n";
  a.load(XR_NULL_HANDLE, "xrEnumerateInstanceExtensionProperties", a.xrEnumerateInstanceExtensionProperties);
  a.load(XR_NULL_HANDLE, "xrCreateInstance", a.xrCreateInstance);
  uint32_t count = 0;
  CHECK(a.xrEnumerateInstanceExtensionProperties(nullptr, 0, &count, nullptr));
  std::vector<XrExtensionProperties> extensions(count, {XR_TYPE_EXTENSION_PROPERTIES});
  CHECK(a.xrEnumerateInstanceExtensionProperties(nullptr, count, &count, extensions.data()));
  bool d3d = false;
  for (auto& e : extensions) { std::cout << "extension " << e.extensionName << " version=" << e.extensionVersion << '\n'; d3d |= std::string(e.extensionName) == XR_KHR_D3D11_ENABLE_EXTENSION_NAME; }
  if (!d3d) throw std::runtime_error("Runtime does not expose XR_KHR_D3D11_enable");
  const char* extension = XR_KHR_D3D11_ENABLE_EXTENSION_NAME;
  XrInstanceCreateInfo ci{XR_TYPE_INSTANCE_CREATE_INFO};
  strcpy_s(ci.applicationInfo.applicationName, "OpenReality OpenXR Probe");
  ci.applicationInfo.apiVersion = XR_MAKE_VERSION(1, 0, 0);
  ci.enabledExtensionCount = 1; ci.enabledExtensionNames = &extension;
  CHECK(a.xrCreateInstance(&ci, &instance));
  // Resolve destruction first so a later missing entry point still cleans up the instance.
  a.load(instance, "xrDestroyInstance", a.xrDestroyInstance);
#define LOAD(n) a.load(instance, #n, a.n);
  XR_FUNCTIONS(LOAD)
#undef LOAD
  XrInstanceProperties ip{XR_TYPE_INSTANCE_PROPERTIES}; CHECK(a.xrGetInstanceProperties(instance, &ip));
  std::cout << "runtime=" << ip.runtimeName << " version=" << XR_VERSION_MAJOR(ip.runtimeVersion) << '.' << XR_VERSION_MINOR(ip.runtimeVersion) << '.' << XR_VERSION_PATCH(ip.runtimeVersion) << " instance negotiation succeeded\n";
 }
 XrSystemId inspect() {
  XrSystemGetInfo gi{XR_TYPE_SYSTEM_GET_INFO}; gi.formFactor = XR_FORM_FACTOR_HEAD_MOUNTED_DISPLAY;
  XrSystemId system = XR_NULL_SYSTEM_ID;
  std::cout << "xrGetSystem HEAD_MOUNTED_DISPLAY (requires an available head system)\n";
  CHECK(a.xrGetSystem(instance, &gi, &system));
  if (!system) throw std::runtime_error("Runtime returned no head system");
  XrSystemProperties p{XR_TYPE_SYSTEM_PROPERTIES}; CHECK(a.xrGetSystemProperties(instance, system, &p));
  std::cout << "head system=" << p.systemName << " vendor=" << p.vendorId << " orientationTracking=" << p.trackingProperties.orientationTracking << " positionTracking=" << p.trackingProperties.positionTracking << " maxLayers=" << p.graphicsProperties.maxLayerCount << '\n';
  uint32_t n = 0; CHECK(a.xrEnumerateViewConfigurations(instance, system, 0, &n, nullptr));
  std::vector<XrViewConfigurationType> types(n); CHECK(a.xrEnumerateViewConfigurations(instance, system, n, &n, types.data()));
  for (auto t : types) {
   CHECK(a.xrEnumerateViewConfigurationViews(instance, system, t, 0, &n, nullptr));
   std::vector<XrViewConfigurationView> v(n, {XR_TYPE_VIEW_CONFIGURATION_VIEW});
   CHECK(a.xrEnumerateViewConfigurationViews(instance, system, t, n, &n, v.data()));
   std::cout << "view configuration=" << t << " views=" << n << '\n';
   for (auto& eye : v) std::cout << " recommended=" << eye.recommendedImageRectWidth << 'x' << eye.recommendedImageRectHeight << " samples=" << eye.recommendedSwapchainSampleCount << '\n';
  }
  return system;
 }
 void setup(XrSystemId system) {
  XrGraphicsRequirementsD3D11KHR req{XR_TYPE_GRAPHICS_REQUIREMENTS_D3D11_KHR};
  CHECK(a.xrGetD3D11GraphicsRequirementsKHR(instance, system, &req));
  ComPtr<IDXGIFactory1> factory; hr(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "CreateDXGIFactory1");
  ComPtr<IDXGIAdapter1> adapter;
  for (UINT i = 0;; ++i) {
   ComPtr<IDXGIAdapter1> candidate; HRESULT r = factory->EnumAdapters1(i, &candidate);
   if (r == DXGI_ERROR_NOT_FOUND) break;
   hr(r, "EnumAdapters1");
   DXGI_ADAPTER_DESC1 desc{}; hr(candidate->GetDesc1(&desc), "GetDesc1");
   if (desc.AdapterLuid.HighPart == req.adapterLuid.HighPart && desc.AdapterLuid.LowPart == req.adapterLuid.LowPart) { adapter = candidate; std::wcout << L"D3D11 adapter=" << desc.Description << '\n'; break; }
  }
  if (!adapter) throw std::runtime_error("No DXGI adapter matches runtime graphics requirements LUID");
  const D3D_FEATURE_LEVEL all[] = {D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0};
  std::vector<D3D_FEATURE_LEVEL> levels; for (auto l : all) if (l >= req.minFeatureLevel) levels.push_back(l);
  if (levels.empty()) throw std::runtime_error("Unsupported minimum D3D feature level");
  D3D_FEATURE_LEVEL actual{};
  hr(D3D11CreateDevice(adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr, 0, levels.data(), static_cast<UINT>(levels.size()), D3D11_SDK_VERSION, &device, &actual, &context), "D3D11CreateDevice");
  XrGraphicsBindingD3D11KHR binding{XR_TYPE_GRAPHICS_BINDING_D3D11_KHR}; binding.device = device.Get();
  XrSessionCreateInfo sci{XR_TYPE_SESSION_CREATE_INFO}; sci.next = &binding; sci.systemId = system;
  CHECK(a.xrCreateSession(instance, &sci, &session)); std::cout << "actual D3D11 OpenXR session created\n";
  XrReferenceSpaceCreateInfo space{XR_TYPE_REFERENCE_SPACE_CREATE_INFO}; space.poseInReferenceSpace.orientation.w = 1;
  space.referenceSpaceType = XR_REFERENCE_SPACE_TYPE_LOCAL; CHECK(a.xrCreateReferenceSpace(session, &space, &local));
  space.referenceSpaceType = XR_REFERENCE_SPACE_TYPE_VIEW; CHECK(a.xrCreateReferenceSpace(session, &space, &view));
  uint32_t n = 0; CHECK(a.xrEnumerateSwapchainFormats(session, 0, &n, nullptr));
  std::vector<int64_t> formats(n); CHECK(a.xrEnumerateSwapchainFormats(session, n, &n, formats.data()));
  int64_t format = 0;
  for (auto f : {DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB}) if (std::find(formats.begin(), formats.end(), f) != formats.end()) { format = f; break; }
  if (!format) throw std::runtime_error("No supported RGBA/BGRA swapchain format");
  CHECK(a.xrEnumerateViewConfigurationViews(instance, system, XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO, 0, &n, nullptr));
  if (n != 2) throw std::runtime_error("Expected two PRIMARY_STEREO views");
  std::vector<XrViewConfigurationView> configs(n, {XR_TYPE_VIEW_CONFIGURATION_VIEW});
  CHECK(a.xrEnumerateViewConfigurationViews(instance, system, XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO, n, &n, configs.data()));
  eyes.resize(n);
  for (size_t i = 0; i < eyes.size(); ++i) {
   auto& e = eyes[i]; e.width = configs[i].recommendedImageRectWidth; e.height = configs[i].recommendedImageRectHeight;
   XrSwapchainCreateInfo sc{XR_TYPE_SWAPCHAIN_CREATE_INFO}; sc.usageFlags = XR_SWAPCHAIN_USAGE_COLOR_ATTACHMENT_BIT; sc.format = format; sc.sampleCount = 1; sc.width = e.width; sc.height = e.height; sc.faceCount = sc.arraySize = sc.mipCount = 1;
   CHECK(a.xrCreateSwapchain(session, &sc, &e.swap));
   CHECK(a.xrEnumerateSwapchainImages(e.swap, 0, &n, nullptr));
   e.images.resize(n, {XR_TYPE_SWAPCHAIN_IMAGE_D3D11_KHR});
   CHECK(a.xrEnumerateSwapchainImages(e.swap, n, &n, reinterpret_cast<XrSwapchainImageBaseHeader*>(e.images.data())));
   e.targets.resize(n);
   for (uint32_t j = 0; j < n; ++j) { D3D11_RENDER_TARGET_VIEW_DESC rtv{}; rtv.Format=static_cast<DXGI_FORMAT>(format); rtv.ViewDimension=D3D11_RTV_DIMENSION_TEXTURE2D; hr(device->CreateRenderTargetView(e.images[j].texture, &rtv, &e.targets[j]), "CreateRenderTargetView"); }
  }
 }
 void render(int seconds) {
  auto start = Clock::now(); auto deadline = start + std::chrono::seconds(seconds);
  uint64_t frames = 0, submitted = 0, valid = 0, tracked = 0, changes = 0, zero = 0;
  XrPosef previous{}; bool havePrevious = false, exiting = false, exitRequested = false;
  float maxPositionDelta = 0, maxOrientationDelta = 0;
  struct Summary {
   uint64_t &frames, &submitted, &valid, &tracked, &changes, &zero;
   float &position, &orientation;
   ~Summary() { std::cout << "frames=" << frames << " projection_submissions=" << submitted << " zero_layer_frames=" << zero << " valid_head_poses=" << valid << " tracked_head_poses=" << tracked << " pose_changes=" << changes << " max_position_step_m=" << position << " max_orientation_step_rad=" << orientation << '\n'; }
  } summary{frames, submitted, valid, tracked, changes, zero, maxPositionDelta, maxOrientationDelta};
  while (Clock::now() < deadline && !exiting) {
   for (;;) {
    XrEventDataBuffer event{XR_TYPE_EVENT_DATA_BUFFER}; XrResult r = a.xrPollEvent(instance, &event);
    if (r == XR_EVENT_UNAVAILABLE) break;
    check(r, "xrPollEvent");
    if (event.type == XR_TYPE_EVENT_DATA_INSTANCE_LOSS_PENDING) throw std::runtime_error("Instance loss pending");
    if (event.type == XR_TYPE_EVENT_DATA_SESSION_STATE_CHANGED) {
     auto& s = reinterpret_cast<XrEventDataSessionStateChanged&>(event); std::cout << "session state=" << s.state << '\n';
     if (s.state == XR_SESSION_STATE_READY && !running) { XrSessionBeginInfo b{XR_TYPE_SESSION_BEGIN_INFO}; b.primaryViewConfigurationType = XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO; CHECK(a.xrBeginSession(session, &b)); running = true; }
     if (s.state == XR_SESSION_STATE_STOPPING && running) { CHECK(a.xrEndSession(session)); running = false; exiting = true; }
     if (s.state == XR_SESSION_STATE_EXITING || s.state == XR_SESSION_STATE_LOSS_PENDING) exiting = true;
    }
   }
   if (exiting) break;
   if (!running) { Sleep(10); continue; }
   XrFrameWaitInfo wi{XR_TYPE_FRAME_WAIT_INFO}; XrFrameState state{XR_TYPE_FRAME_STATE}; CHECK(a.xrWaitFrame(session, &wi, &state));
   XrFrameBeginInfo bi{XR_TYPE_FRAME_BEGIN_INFO}; CHECK(a.xrBeginFrame(session, &bi)); ++frames;
   XrFrameEndInfo end{XR_TYPE_FRAME_END_INFO}; end.displayTime = state.predictedDisplayTime; end.environmentBlendMode = XR_ENVIRONMENT_BLEND_MODE_OPAQUE;
   std::vector<XrCompositionLayerProjectionView> projections(eyes.size(), {XR_TYPE_COMPOSITION_LAYER_PROJECTION_VIEW});
   XrCompositionLayerProjection layer{XR_TYPE_COMPOSITION_LAYER_PROJECTION}; layer.space = local;
   const XrCompositionLayerBaseHeader* layers[] = {reinterpret_cast<XrCompositionLayerBaseHeader*>(&layer)};
   try {
    XrSpaceLocation pose{XR_TYPE_SPACE_LOCATION}; CHECK(a.xrLocateSpace(view, local, state.predictedDisplayTime, &pose));
    auto flags = pose.locationFlags;
    if ((flags & (XR_SPACE_LOCATION_POSITION_VALID_BIT | XR_SPACE_LOCATION_ORIENTATION_VALID_BIT)) == (XR_SPACE_LOCATION_POSITION_VALID_BIT | XR_SPACE_LOCATION_ORIENTATION_VALID_BIT)) {
     ++valid;
     if (havePrevious) {
      float dx = pose.pose.position.x - previous.position.x, dy = pose.pose.position.y - previous.position.y, dz = pose.pose.position.z - previous.position.z;
      float pd = std::sqrt(dx*dx + dy*dy + dz*dz);
      float dot = pose.pose.orientation.x*previous.orientation.x + pose.pose.orientation.y*previous.orientation.y + pose.pose.orientation.z*previous.orientation.z + pose.pose.orientation.w*previous.orientation.w;
      float od = 2 * std::acos(std::min(1.0f, std::fabs(dot)));
      maxPositionDelta = std::max(maxPositionDelta, pd); maxOrientationDelta = std::max(maxOrientationDelta, od);
      if (pd > 0.00001f || od > 0.0001f) ++changes;
     }
     previous = pose.pose; havePrevious = true;
    }
    if ((flags & (XR_SPACE_LOCATION_POSITION_TRACKED_BIT | XR_SPACE_LOCATION_ORIENTATION_TRACKED_BIT)) == (XR_SPACE_LOCATION_POSITION_TRACKED_BIT | XR_SPACE_LOCATION_ORIENTATION_TRACKED_BIT)) ++tracked;
    if (state.shouldRender) {
     XrViewLocateInfo li{XR_TYPE_VIEW_LOCATE_INFO}; li.viewConfigurationType = XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO; li.displayTime = state.predictedDisplayTime; li.space = local;
     XrViewState vs{XR_TYPE_VIEW_STATE}; uint32_t count = 0;
     std::vector<XrView> views(eyes.size(), {XR_TYPE_VIEW}); CHECK(a.xrLocateViews(session, &li, &vs, static_cast<uint32_t>(views.size()), &count, views.data()));
     if (count != eyes.size()) throw std::runtime_error("Unexpected located view count");
     if ((vs.viewStateFlags & (XR_VIEW_STATE_POSITION_VALID_BIT | XR_VIEW_STATE_ORIENTATION_VALID_BIT)) == (XR_VIEW_STATE_POSITION_VALID_BIT | XR_VIEW_STATE_ORIENTATION_VALID_BIT)) {
      for (size_t i = 0; i < eyes.size(); ++i) {
       auto& e = eyes[i]; uint32_t index = 0; XrSwapchainImageAcquireInfo ai{XR_TYPE_SWAPCHAIN_IMAGE_ACQUIRE_INFO}; CHECK(a.xrAcquireSwapchainImage(e.swap, &ai, &index));
       XrSwapchainImageWaitInfo wait{XR_TYPE_SWAPCHAIN_IMAGE_WAIT_INFO}; wait.timeout = 1000000000;
       XrResult wr = a.xrWaitSwapchainImage(e.swap, &wait);
       if (wr != XR_SUCCESS) throw std::runtime_error("xrWaitSwapchainImage failed or timed out result=" + std::to_string(wr));
       float t = std::chrono::duration<float>(Clock::now() - start).count(); float c = 0.18f + 0.025f * std::sin(t * 2.0f);
       float color[] = {c, c + 0.008f * static_cast<float>(i), c + 0.015f, 1};
       context->ClearRenderTargetView(e.targets.at(index).Get(), color); context->Flush();
       XrSwapchainImageReleaseInfo ri{XR_TYPE_SWAPCHAIN_IMAGE_RELEASE_INFO}; CHECK(a.xrReleaseSwapchainImage(e.swap, &ri));
       auto& p = projections[i]; p.pose = views[i].pose; p.fov = views[i].fov; p.subImage.swapchain = e.swap; p.subImage.imageRect.extent = {static_cast<int32_t>(e.width), static_cast<int32_t>(e.height)};
      }
      layer.viewCount = static_cast<uint32_t>(projections.size()); layer.views = projections.data(); end.layerCount = 1; end.layers = layers;
     }
    }
   } catch (...) { end.layerCount = 0; end.layers = nullptr; report(a.xrEndFrame(session, &end), "abort frame"); throw; }
   CHECK(a.xrEndFrame(session, &end)); if (end.layerCount) ++submitted; else ++zero;
   // Leave a small bounded window for the normal exit handshake.
   if (!exitRequested && Clock::now() >= deadline - std::chrono::milliseconds(250)) { CHECK(a.xrRequestExitSession(session)); exitRequested = true; }
  }
  if (!submitted) throw std::runtime_error("No actual projection frames submitted");
  if (!valid) throw std::runtime_error("No valid head tracking poses");
  std::cout << "SMOKE PASS: actual projection submissions accepted; downstream driver export must be checked separately\n";
 }
};
int main(int argc, char** argv) {
 std::cout.setf(std::ios::unitbuf); std::cerr.setf(std::ios::unitbuf);
 try {
  fs::path runtime, loader; int seconds = 0; bool mode = false;
  for (int i = 1; i < argc; ++i) {
   std::string arg = argv[i];
   if (arg == "--runtime" && i + 1 < argc && runtime.empty()) runtime = fs::u8path(argv[++i]);
   else if (arg == "--loader" && i + 1 < argc && loader.empty()) loader = fs::u8path(argv[++i]);
   else if (arg == "--probe" && !mode) mode = true;
   else if (arg == "--render-seconds" && i + 1 < argc && !mode) {
    std::string value = argv[++i]; if (value.empty() || value.size() > 2 || value.find_first_not_of("0123456789") != std::string::npos) throw std::runtime_error("Duration must be an integer 1..30");
    seconds = std::stoi(value); if (seconds < 1 || seconds > 30) throw std::runtime_error("Duration must be 1..30"); mode = true;
   } else throw std::runtime_error("Usage: openxr-probe --runtime <SteamVR/steamxr_win64.json> [--loader <SteamVR/bin/win64/openxr_loader.dll>] [--probe | --render-seconds <1..30>]");
  }
  if (runtime.empty() || runtime.native().size() > 32700) throw std::runtime_error("Required bounded --runtime path missing or too long");
  runtime = fs::canonical(runtime);
  if (runtime.filename() != L"steamxr_win64.json" || !fs::is_regular_file(runtime) || fs::file_size(runtime) > 1024*1024) throw std::runtime_error("Expected bounded SteamVR steamxr_win64.json manifest");
  // SteamVR's manifest is normally at the installation root; never search PATH or the current directory for the loader.
  fs::path expected = fs::canonical(runtime.parent_path() / "bin/win64/openxr_loader.dll");
  if (loader.empty()) loader = expected;
  if (loader.native().size() > 32700 || fs::canonical(loader) != expected) throw std::runtime_error("Loader must be the selected SteamVR installation's bin/win64/openxr_loader.dll");
  loader = expected;
  std::ifstream manifest(runtime); std::string contents((std::istreambuf_iterator<char>(manifest)), {});
  std::cout << "manifest bytes=" << contents.size() << "\n";
  // A broken runtime can block inside any synchronous API. Bound the entire process, including cleanup.
  HANDLE done = CreateEventW(nullptr, TRUE, FALSE, nullptr);
  if (!done) throw std::runtime_error("Cannot create watchdog event");
  DWORD budget = static_cast<DWORD>((seconds ? seconds : 0) * 1000 + 15000);
  struct Watch { HANDLE done; DWORD budget; } watch{done, budget};
  HANDLE thread = CreateThread(nullptr, 0, [](void* p)->DWORD { auto* w = static_cast<Watch*>(p); if (WaitForSingleObject(w->done, w->budget) == WAIT_TIMEOUT) { std::cerr << "ERROR: runtime process deadline exceeded; terminating stalled probe\n"; TerminateProcess(GetCurrentProcess(), 3); } return 0; }, &watch, 0, nullptr);
  if (!thread) { CloseHandle(done); throw std::runtime_error("Cannot create watchdog"); }
  int result = 0;
  try { Probe p; p.initialize(runtime, loader); auto system = p.inspect(); if (seconds) { p.setup(system); p.render(seconds); } else std::cout << "PROBE PASS: head system available; session/rendering not tested\n"; }
  catch (const std::exception& e) { std::cerr << "errors=1 ERROR: " << e.what() << '\n'; result = 1; }
  if (!result) std::cout << "errors=0\n";
  SetEvent(done); WaitForSingleObject(thread, INFINITE); CloseHandle(thread); CloseHandle(done); return result;
 } catch (const std::exception& e) { std::cerr << "ERROR: " << e.what() << '\n'; return 2; }
}
