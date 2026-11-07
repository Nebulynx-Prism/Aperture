// Reuse the reviewed D3D11 app probe without changing its SteamVR-only entrypoint.
#define main prism_steamvr_probe_main
#include "obj/probe_app.inc"
#undef main
#include "../prism_openvr_driver/prism_ipc.h"
#include <atomic>
#include <thread>

struct Fixture {
    HANDLE mapping{}, mutex{}; PrismSharedHeader* header{};
    std::atomic<bool> stop{false}; std::thread writer;
    void start() {
        mapping=CreateFileMappingW(INVALID_HANDLE_VALUE,nullptr,PAGE_READWRITE,0,PRISM_MAPPING_BYTES,PRISM_MAPPING_NAME);
        if(!mapping || GetLastError()==ERROR_ALREADY_EXISTS) throw std::runtime_error("Fixture refuses to replace a live Prism mapping");
        mutex=CreateMutexW(nullptr,FALSE,PRISM_MUTEX_NAME);
        if(!mutex) throw std::runtime_error("Fixture mutex creation failed");
        header=static_cast<PrismSharedHeader*>(MapViewOfFile(mapping,FILE_MAP_ALL_ACCESS,0,0,PRISM_MAPPING_BYTES));
        if(!header) throw std::runtime_error("Fixture mapping failed");
        *header={}; header->magic=PRISM_MAGIC; header->version=1; header->client_connected=1;
        header->tracking.version=1; header->tracking.ipd=.064f; header->tracking.refresh_rate=72;
        header->tracking.head.valid=1; header->tracking.head.orientation[3]=1;
        for(int e=0;e<2;++e) {
            header->tracking.fov[4*e]=-.7f; header->tracking.fov[4*e+1]=.7f;
            header->tracking.fov[4*e+2]=.8f; header->tracking.fov[4*e+3]=-.8f;
        }
        header->server_heartbeat_ms=header->tracking_received_ms=GetTickCount64();
        writer=std::thread([this] {
            while(!stop) {
                if(WaitForSingleObject(mutex,100)==WAIT_OBJECT_0) {
                    header->server_heartbeat_ms=header->tracking_received_ms=GetTickCount64();
                    ReleaseMutex(mutex);
                }
                Sleep(10);
            }
        });
        std::cout << "TEST FIXTURE tracking (not a real headset); production mapping is exclusively owned by this probe\n";
    }
    ~Fixture() {
        stop=true; if(writer.joinable()) writer.join();
        if(header) UnmapViewOfFile(header); if(mutex) CloseHandle(mutex); if(mapping) CloseHandle(mapping);
    }
};
int main(int argc,char** argv) {
    bool fixtureMode=false; int seconds=3;
    for(int i=1;i<argc;++i) {
        if(std::string(argv[i])=="--fixture") fixtureMode=true;
        else if(std::string(argv[i])=="--live") fixtureMode=false;
        else { std::cerr<<"Usage: runtime_probe [--fixture|--live]\n"; return 2; }
    }
    HANDLE watchdogDone=CreateEventW(nullptr,TRUE,FALSE,nullptr);
    if(!watchdogDone) return 2;
    std::thread watchdog([&] {
        if(WaitForSingleObject(watchdogDone,30000)==WAIT_TIMEOUT) {
            std::cerr<<"Runtime probe exceeded 30-second deadline\n";
            TerminateProcess(GetCurrentProcess(),3);
        }
    });
    int exitCode=0;
    try {
        Fixture fixture; if(fixtureMode) fixture.start();
        auto folder=fs::canonical(fs::u8path(argv[0])).parent_path();
        auto manifest=folder/"bin/prism-vdxr.json";
        auto loader=fs::path(L"C:/Program Files (x86)/Steam/steamapps/common/SteamVR/bin/win64/openxr_loader.dll");
        { Probe probe; probe.initialize(manifest,loader); auto system=probe.inspect(); probe.setup(system); probe.render(seconds); }
        if(fixtureMode) {
            if(!fixture.header->frame_sequence) throw std::runtime_error("xrEndFrame accepted but no shared SBS frame was published");
            std::cout<<"Verified exported SBS frames="<<fixture.header->frame_sequence<<" dimensions="<<fixture.header->frame_width<<'x'<<fixture.header->frame_height<<'\n';
        }
    } catch(const std::exception& e) { std::cerr<<"ERROR: "<<e.what()<<'\n'; exitCode=1; }
    SetEvent(watchdogDone); watchdog.join(); CloseHandle(watchdogDone); return exitCode;
}
