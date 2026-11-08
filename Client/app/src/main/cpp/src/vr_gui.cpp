#include "vr_gui.hpp"
#include "outfit_font.hpp"
#include <android/log.h>
#include <cmath>
#include <cstring>
#include <algorithm>

#define LOG_TAG "PrismAperture_GUI"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)

namespace openreality {

// Clean Gray & White Theme (#252525 background requested by user)
static constexpr uint32_t COL_BG_MAIN     = 0xFF252525; // #252525 Main UI background
static constexpr uint32_t COL_PANEL_BG    = 0xFF303030; // #303030 Container panel
static constexpr uint32_t COL_INNER_CARD  = 0xFF1E1E1E; // #1E1E1E Nested item cards
static constexpr uint32_t COL_BORDER      = 0xFF454545; // #454545 Clean subtle gray border
static constexpr uint32_t COL_BORDER_HI   = 0xFF656565; // #656565 Selected border
static constexpr uint32_t COL_HOVER       = 0xFF404040; // #404040 Hover state
static constexpr uint32_t COL_WHITE       = 0xFFFFFFFF; // Pure crisp white
static constexpr uint32_t COL_SILVER      = 0xFFCCCCCC; // Light silver text
static constexpr uint32_t COL_GRAY_MUTED  = 0xFF888888; // Muted gray for captions
static constexpr uint32_t COL_DARK_TEXT   = 0xFF252525; // Dark text for white pills
static constexpr uint32_t COL_GREEN       = 0xFF4CAF50; // Connected green
static constexpr uint32_t COL_YELLOW      = 0xFFFFB300; // Standby / searching yellow
static constexpr uint32_t COL_RED         = 0xFFEF5350; // Error / disconnected red

VRGui::VRGui() {
    m_pixelBuffer.resize(UI_WIDTH * UI_HEIGHT, COL_BG_MAIN);
}

VRGui::~VRGui() {
    release();
}

bool VRGui::init() {
    glGenTextures(1, &m_textureId);
    glBindTexture(GL_TEXTURE_2D, m_textureId);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);

    redraw();
    LOGI("VRGui initialized with Outfit font and smooth rounded gray/white #252525 theme (%dx%d)",
         UI_WIDTH, UI_HEIGHT);
    return true;
}

void VRGui::release() {
    if (m_textureId != 0) {
        glDeleteTextures(1, &m_textureId);
        m_textureId = 0;
    }
}

void VRGui::setServerInfo(const std::string& name, const std::string& host, int port, bool isWiredUsb) {
    if (isWiredUsb) {
        m_usbServerHost = host;
        m_usbServerPort = port;
        m_usbFound = true;
    } else {
        m_wifiServerName = name;
        m_wifiServerHost = host;
        m_wifiServerPort = port;
        m_wifiFound = true;
    }
    m_dirty = true;
}

void VRGui::setConnectionStatus(bool isConnected, bool isConnecting, bool isUsb) {
    if (m_isConnected == isConnected && m_isConnecting == isConnecting &&
        m_isUsbConnected == isUsb) return;
    m_isConnected = isConnected;
    m_isConnecting = isConnecting;
    m_isUsbConnected = isUsb;
    m_dirty = true;
}

void VRGui::setTelemetry(float latencyMs, float fps, float bitrateMbps) {
    m_latencyMs = latencyMs;
    m_fps = fps;
    m_bitrateMbps = bitrateMbps;
    if (m_activeTab == 3) {
        m_dirty = true;
    }
}

void VRGui::setStreamResolution(int width, int height) {
    if (m_streamWidth == width && m_streamHeight == height) return;
    m_streamWidth = width;
    m_streamHeight = height;
    if (m_activeTab == 2 || m_activeTab == 3) m_dirty = true;
}

void VRGui::drawRect(int x, int y, int w, int h, uint32_t color) {
    int x1 = std::max(0, x);
    int y1 = std::max(0, y);
    int x2 = std::min(UI_WIDTH, x + w);
    int y2 = std::min(UI_HEIGHT, y + h);

    for (int cy = y1; cy < y2; ++cy) {
        uint32_t* row = &m_pixelBuffer[cy * UI_WIDTH + x1];
        std::fill_n(row, x2 - x1, color);
    }
}

// Smooth anti-aliased rounded rectangle with border
void VRGui::drawRoundRect(int x, int y, int w, int h, int radius, uint32_t fillColor, uint32_t borderColor) {
    int x1 = std::max(0, x);
    int y1 = std::max(0, y);
    int x2 = std::min(UI_WIDTH, x + w);
    int y2 = std::min(UI_HEIGHT, y + h);
    if (x1 >= x2 || y1 >= y2) return;

    float r = static_cast<float>(radius);
    float rInner = r - 1.2f;

    for (int py = y1; py < y2; ++py) {
        float dy = 0.0f;
        bool inCornerY = false;
        if (py < y + radius) {
            dy = (y + radius) - (py + 0.5f);
            inCornerY = true;
        } else if (py >= y + h - radius) {
            dy = (py + 0.5f) - (y + h - radius);
            inCornerY = true;
        }

        for (int px = x1; px < x2; ++px) {
            float dx = 0.0f;
            bool inCornerX = false;
            if (px < x + radius) {
                dx = (x + radius) - (px + 0.5f);
                inCornerX = true;
            } else if (px >= x + w - radius) {
                dx = (px + 0.5f) - (x + w - radius);
                inCornerX = true;
            }

            float dist = 0.0f;
            float alpha = 1.0f;
            bool isBorder = false;

            if (inCornerX && inCornerY) {
                dist = std::sqrt(dx * dx + dy * dy);
                if (dist > r + 0.5f) {
                    continue; // Outside rounded corner
                } else if (dist > r - 0.5f) {
                    alpha = (r + 0.5f - dist); // Subpixel anti-aliased edge
                }
                if (borderColor != 0 && dist >= rInner) {
                    isBorder = true;
                }
            } else {
                if (borderColor != 0) {
                    if (px == x || px == x + w - 1 || py == y || py == y + h - 1) {
                        isBorder = true;
                    }
                }
            }

            uint32_t col = isBorder ? borderColor : fillColor;
            if (alpha >= 0.99f) {
                m_pixelBuffer[py * UI_WIDTH + px] = col;
            } else if (alpha > 0.01f) {
                uint32_t bg = m_pixelBuffer[py * UI_WIDTH + px];
                uint8_t cr = static_cast<uint8_t>(((col >> 16) & 0xFF) * alpha + ((bg >> 16) & 0xFF) * (1.0f - alpha));
                uint8_t cg = static_cast<uint8_t>(((col >> 8) & 0xFF) * alpha + ((bg >> 8) & 0xFF) * (1.0f - alpha));
                uint8_t cb = static_cast<uint8_t>((col & 0xFF) * alpha + (bg & 0xFF) * (1.0f - alpha));
                m_pixelBuffer[py * UI_WIDTH + px] = 0xFF000000 | (cr << 16) | (cg << 8) | cb;
            }
        }
    }
}

// Proportional anti-aliased Outfit font glyph rendering
void VRGui::drawGlyph(int x, int y, char c, uint32_t color, float scale) {
    if (c < 32 || c > 126) c = '?';
    const OutfitGlyph& g = OUTFIT_GLYPHS[c - 32];
    if (g.w <= 0 || g.h <= 0) return;

    int dstW = static_cast<int>(g.w * scale + 0.99f);
    int dstH = static_cast<int>(g.h * scale + 0.99f);

    uint8_t r = (color >> 16) & 0xFF;
    uint8_t gCol = (color >> 8) & 0xFF;
    uint8_t b = color & 0xFF;

    for (int dy = 0; dy < dstH; ++dy) {
        int py = y + dy;
        if (py < 0 || py >= UI_HEIGHT) continue;

        int srcY = static_cast<int>(dy / scale);
        if (srcY >= g.h) srcY = g.h - 1;
        int atlasY = g.y + srcY;

        for (int dx = 0; dx < dstW; ++dx) {
            int px = x + dx;
            if (px < 0 || px >= UI_WIDTH) continue;

            int srcX = static_cast<int>(dx / scale);
            if (srcX >= g.w) srcX = g.w - 1;
            int atlasX = g.x + srcX;

            uint8_t aByte = OUTFIT_ATLAS_ALPHA[atlasY * OUTFIT_ATLAS_W + atlasX];
            if (aByte == 0) continue;

            float a = aByte / 255.0f;
            if (a >= 0.99f) {
                m_pixelBuffer[py * UI_WIDTH + px] = 0xFF000000 | (r << 16) | (gCol << 8) | b;
            } else {
                uint32_t bg = m_pixelBuffer[py * UI_WIDTH + px];
                uint8_t bgR = (bg >> 16) & 0xFF;
                uint8_t bgG = (bg >> 8) & 0xFF;
                uint8_t bgB = bg & 0xFF;
                uint8_t outR = static_cast<uint8_t>(r * a + bgR * (1.0f - a));
                uint8_t outG = static_cast<uint8_t>(gCol * a + bgG * (1.0f - a));
                uint8_t outB = static_cast<uint8_t>(b * a + bgB * (1.0f - a));
                m_pixelBuffer[py * UI_WIDTH + px] = 0xFF000000 | (outR << 16) | (outG << 8) | outB;
            }
        }
    }
}

int VRGui::getTextWidth(const std::string& text, float scale) {
    int width = 0;
    for (char c : text) {
        if (c == ' ') {
            width += static_cast<int>(9.0f * scale);
            continue;
        }
        if (c < 32 || c > 126) c = '?';
        const OutfitGlyph& g = OUTFIT_GLYPHS[c - 32];
        int adv = g.advance > 0 ? g.advance : g.w + 1;
        width += static_cast<int>(adv * scale);
    }
    return width;
}

void VRGui::drawText(int x, int y, const std::string& text, uint32_t color, float scale) {
    int curX = x;
    for (char c : text) {
        if (c == ' ') {
            curX += static_cast<int>(9.0f * scale);
            continue;
        }
        if (c < 32 || c > 126) c = '?';
        const OutfitGlyph& g = OUTFIT_GLYPHS[c - 32];
        drawGlyph(curX, y, c, color, scale);
        int adv = g.advance > 0 ? g.advance : g.w + 1;
        curX += static_cast<int>(adv * scale);
    }
}

void VRGui::drawTextCentered(int x, int y, int w, const std::string& text, uint32_t color, float scale) {
    int tw = getTextWidth(text, scale);
    int sx = x + (w - tw) / 2;
    drawText(sx, y, text, color, scale);
}

void VRGui::redraw() {
    // Fill with sleek #252525 gray
    std::fill(m_pixelBuffer.begin(), m_pixelBuffer.end(), COL_BG_MAIN);

    // 1. Top Header Banner (Rounded 12px)
    drawRoundRect(20, 16, UI_WIDTH - 40, 76, 12, COL_PANEL_BG, COL_BORDER);

    // Title: PRISM APERTURE in Outfit Font
    drawText(40, 34, "PRISM APERTURE", COL_WHITE, 1.25f);
    drawText(295, 42, "v0.1.0", COL_GRAY_MUTED, 0.75f);

    // Header Action: SteamVR Quick Launch (ID: 15)
    uint32_t svrFill = (m_hoveredButtonId == 15) ? COL_HOVER : COL_INNER_CARD;
    drawRoundRect(UI_WIDTH - 530, 28, 170, 52, 10, svrFill, (m_hoveredButtonId == 15) ? COL_WHITE : COL_BORDER);
    drawTextCentered(UI_WIDTH - 530, 44, 170, "LAUNCH STEAMVR", COL_WHITE, 0.82f);

    // Readiness requires a negotiated stereo stream with decoder output.
    drawRoundRect(UI_WIDTH - 350, 28, 155, 52, 10, COL_INNER_CARD, COL_BORDER);
    drawTextCentered(UI_WIDTH - 350, 44, 155, (m_vrMode ? "PCVR READY" : (m_startRequested ? "START REQUESTED" : "PCVR STANDBY")), COL_YELLOW, 0.70f);

    // Connection Status Pill
    if (m_isConnected) {
        drawRoundRect(UI_WIDTH - 185, 28, 155, 52, 10, 0xFF1B3320, COL_GREEN);
        const char* linkLabel = m_isUsbConnected ? "WIRED (USB)" : "WI-FI ACTIVE";
        drawTextCentered(UI_WIDTH - 185, 44, 155, linkLabel, COL_GREEN, 0.80f);
    } else if (m_isConnecting) {
        drawRoundRect(UI_WIDTH - 185, 28, 155, 52, 10, 0xFF332B12, COL_YELLOW);
        drawTextCentered(UI_WIDTH - 185, 44, 155, "CONNECTING...", COL_YELLOW, 0.80f);
    } else {
        drawRoundRect(UI_WIDTH - 185, 28, 155, 10, 10, 0xFF331A1A, COL_RED);
        drawTextCentered(UI_WIDTH - 185, 44, 155, "DISCONNECTED", COL_RED, 0.80f);
    }

    // 2. Navigation Tabs (IDs: 1, 2, 3, 4)
    int tabW = 230;
    int tabH = 46;
    int tabY = 104;

    auto drawNavTab = [&](int id, int tabIndex, const char* label, int x) {
        bool active = (m_activeTab == tabIndex);
        uint32_t fill = active ? COL_WHITE : (m_hoveredButtonId == id ? COL_HOVER : COL_PANEL_BG);
        uint32_t border = active ? COL_WHITE : COL_BORDER;
        uint32_t textCol = active ? COL_DARK_TEXT : (m_hoveredButtonId == id ? COL_WHITE : COL_SILVER);
        drawRoundRect(x, tabY, tabW, tabH, 10, fill, border);
        drawTextCentered(x, tabY + 14, tabW, label, textCol, 0.88f);
    };

    drawNavTab(1, 0, "COMPUTERS", 20);
    drawNavTab(2, 1, "GAMES", 265);
    drawNavTab(3, 2, "SETTINGS", 510);
    drawNavTab(4, 3, "STATS", 755);

    // 3. Main Content Container
    int contY = 162;
    int contH = 410;
    drawRoundRect(20, contY, UI_WIDTH - 40, contH, 14, COL_PANEL_BG, COL_BORDER);

    if (m_activeTab == 0) {
        // --- TAB 0: COMPUTERS (Wired USB + Wireless Wi-Fi) ---
        drawText(42, contY + 20, "AVAILABLE COMPUTERS & CONNECTION MODES", COL_WHITE, 0.95f);

        // Card 1: Wired USB-C Direct Link (ID: 11)
        int usbY = contY + 54;
        drawRoundRect(40, usbY, UI_WIDTH - 80, 116, 12, COL_INNER_CARD, COL_BORDER);
        drawText(65, usbY + 20, "Aperture PC (USB-C Wired Link)", COL_WHITE, 1.05f);
        drawText(65, usbY + 54, "Endpoint: 127.0.0.1:38810 (USB forwarding)", COL_SILVER, 0.80f);
        drawText(65, usbY + 80, m_usbFound ? "Status: USB Link Active & Ready" : "Status: Ready (Connect USB-C Cable)",
                 m_usbFound ? COL_GREEN : COL_YELLOW, 0.78f);

        int btnW = 180, btnH = 50;
        int btnX = UI_WIDTH - 250;
        uint32_t uBtnFill = (m_hoveredButtonId == 11) ? COL_WHITE : 0xFF353535;
        uint32_t uBtnText = (m_hoveredButtonId == 11) ? COL_DARK_TEXT : COL_WHITE;
        drawRoundRect(btnX, usbY + 32, btnW, btnH, 10, uBtnFill, COL_BORDER_HI);
        drawTextCentered(btnX, usbY + 48, btnW, "CONNECT USB", uBtnText, 0.85f);

        // Card 2: Wi-Fi Wireless LAN (ID: 13)
        int wifiY = usbY + 128;
        drawRoundRect(40, wifiY, UI_WIDTH - 80, 116, 12, COL_INNER_CARD, COL_BORDER);
        std::string wTitle = m_wifiFound ? m_wifiServerName : "Aperture PC (Wi-Fi Wireless)";
        drawText(65, wifiY + 20, wTitle, COL_WHITE, 1.05f);
        std::string wEp = "Endpoint: " + m_wifiServerHost + ":" + std::to_string(m_wifiServerPort) + " (High-Speed Local Wi-Fi)";
        drawText(65, wifiY + 54, wEp, COL_SILVER, 0.80f);
        drawText(65, wifiY + 80, m_wifiFound ? "Status: Discovered on Local Network" : "Status: Scanning Network...",
                 m_wifiFound ? COL_GREEN : COL_YELLOW, 0.78f);

        uint32_t wBtnFill = (m_hoveredButtonId == 13) ? COL_WHITE : 0xFF353535;
        uint32_t wBtnText = (m_hoveredButtonId == 13) ? COL_DARK_TEXT : COL_WHITE;
        drawRoundRect(btnX, wifiY + 32, btnW, btnH, 10, wBtnFill, COL_BORDER_HI);
        drawTextCentered(btnX, wifiY + 48, btnW, "CONNECT WI-FI", wBtnText, 0.85f);

        // Disconnect Button (ID: 14)
        if (m_isConnected) {
            int dY = wifiY + 128;
            uint32_t dFill = (m_hoveredButtonId == 14) ? COL_RED : 0xFF3A1E1E;
            drawRoundRect(40, dY, 240, 48, 10, dFill, COL_RED);
            drawTextCentered(40, dY + 15, 240, "DISCONNECT SESSION", COL_WHITE, 0.82f);
        }

    } else if (m_activeTab == 1) {
        // --- TAB 1: GAMES & STEAMVR LAUNCHER ---
        drawText(42, contY + 20, "PC VR GAMES & STEAMVR RUNTIME", COL_WHITE, 0.95f);

        // Big SteamVR Card (ID: 15)
        int svrY = contY + 52;
        uint32_t scFill = (m_hoveredButtonId == 15) ? COL_HOVER : COL_INNER_CARD;
        drawRoundRect(40, svrY, UI_WIDTH - 80, 84, 12, scFill, (m_hoveredButtonId == 15) ? COL_WHITE : COL_BORDER);
        drawText(65, svrY + 20, "SteamVR Runtime", COL_WHITE, 1.15f);
        drawText(65, svrY + 52, "Launch a game or SteamVR. Stereo starts when the PC stream is ready.", COL_SILVER, 0.80f);
        
        int sBtnW = 160, sBtnH = 48;
        int sBtnX = UI_WIDTH - 230;
        drawRoundRect(sBtnX, svrY + 18, sBtnW, sBtnH, 10, COL_WHITE, COL_WHITE);
        drawTextCentered(sBtnX, svrY + 32, sBtnW, "LAUNCH VR", COL_DARK_TEXT, 0.85f);

        // Game Library Cards
        struct GameItem {
            int btnId;
            const char* title;
            const char* dev;
            const char* appId;
        };
        GameItem games[4] = {
            {101, "Half-Life: Alyx", "Valve Software - VR Action", "546560"},
            {102, "Beat Saber", "Beat Games - VR Rhythm", "620980"},
            {103, "Blade and Sorcery", "WarpFrog - VR Combat", "629730"},
            {104, "VRChat", "VRChat Inc. - Social VR", "438100"}
        };

        for (int i = 0; i < 4; ++i) {
            int gy = svrY + 96 + i * 62;
            drawRoundRect(40, gy, UI_WIDTH - 80, 54, 10, COL_INNER_CARD, COL_BORDER);
            drawText(65, gy + 14, games[i].title, COL_WHITE, 0.92f);
            drawText(65, gy + 34, games[i].dev, COL_GRAY_MUTED, 0.72f);

            int pBtnW = 110, pBtnH = 38;
            int pBtnX = UI_WIDTH - 170;
            int pBtnY = gy + 8;
            uint32_t pFill = (m_hoveredButtonId == games[i].btnId) ? COL_WHITE : 0xFF353535;
            uint32_t pText = (m_hoveredButtonId == games[i].btnId) ? COL_DARK_TEXT : COL_WHITE;
            drawRoundRect(pBtnX, pBtnY, pBtnW, pBtnH, 8, pFill, COL_BORDER_HI);
            drawTextCentered(pBtnX, pBtnY + 12, pBtnW, "PLAY", pText, 0.82f);
        }

    } else if (m_activeTab == 2) {
        // --- TAB 2: STREAMING SETTINGS ---
        int rowY = contY + 28;

        // Refresh Rate: 72, 80, 90, 120 (IDs: 20..23)
        drawText(50, rowY, "Display Refresh Rate:", COL_WHITE, 0.88f);
        int rates[4] = {72, 80, 90, 120};
        for (int i = 0; i < 4; ++i) {
            int bx = 380 + i * 135;
            bool sel = (m_selectedRefreshRate == rates[i]);
            uint32_t fill = sel ? COL_WHITE : (m_hoveredButtonId == (20 + i) ? COL_HOVER : COL_INNER_CARD);
            uint32_t textCol = sel ? COL_DARK_TEXT : COL_SILVER;
            drawRoundRect(bx, rowY - 10, 115, 42, 8, fill, sel ? COL_WHITE : COL_BORDER);
            drawTextCentered(bx, rowY + 3, 115, std::to_string(rates[i]) + " Hz", textCol, 0.82f);
        }

        // Codec: HEVC, AV1, H264 (IDs: 30..32)
        rowY += 66;
        drawText(50, rowY, "Video Codec:", COL_WHITE, 0.88f);
        const char* codecs[3] = {"HEVC (H.265)", "AV1 (Quest 3)", "H.264 (AVC)"};
        for (int i = 0; i < 3; ++i) {
            int bx = 380 + i * 180;
            const char* codeTag = (i == 0) ? "HEVC" : ((i == 1) ? "AV1" : "H264");
            bool sel = (m_selectedCodec == codeTag);
            uint32_t fill = sel ? COL_WHITE : (m_hoveredButtonId == (30 + i) ? COL_HOVER : COL_INNER_CARD);
            uint32_t textCol = sel ? COL_DARK_TEXT : COL_SILVER;
            drawRoundRect(bx, rowY - 10, 165, 42, 8, fill, sel ? COL_WHITE : COL_BORDER);
            drawTextCentered(bx, rowY + 3, 165, codecs[i], textCol, 0.80f);
        }

        // Bitrate: 50, 100, 150, 200 (IDs: 40..43)
        rowY += 66;
        drawText(50, rowY, "Target Bitrate:", COL_WHITE, 0.88f);
        int bitrates[4] = {50, 100, 150, 200};
        for (int i = 0; i < 4; ++i) {
            int bx = 380 + i * 135;
            bool sel = (m_selectedBitrate == bitrates[i]);
            uint32_t fill = sel ? COL_WHITE : (m_hoveredButtonId == (40 + i) ? COL_HOVER : COL_INNER_CARD);
            uint32_t textCol = sel ? COL_DARK_TEXT : COL_SILVER;
            drawRoundRect(bx, rowY - 10, 115, 42, 8, fill, sel ? COL_WHITE : COL_BORDER);
            drawTextCentered(bx, rowY + 3, 115, std::to_string(bitrates[i]) + " Mbps", textCol, 0.82f);
        }

        // Geometry: Curved vs Flat (IDs: 50, 51)
        rowY += 66;
        drawText(50, rowY, "Screen Geometry:", COL_WHITE, 0.88f);
        const char* geos[2] = {"CURVED (60 deg)", "FLAT SCREEN"};
        for (int i = 0; i < 2; ++i) {
            int bx = 380 + i * 270;
            bool sel = (i == 0 ? m_curvedScreen : !m_curvedScreen);
            uint32_t fill = sel ? COL_WHITE : (m_hoveredButtonId == (50 + i) ? COL_HOVER : COL_INNER_CARD);
            uint32_t textCol = sel ? COL_DARK_TEXT : COL_SILVER;
            drawRoundRect(bx, rowY - 10, 240, 42, 8, fill, sel ? COL_WHITE : COL_BORDER);
            drawTextCentered(bx, rowY + 3, 240, geos[i], textCol, 0.80f);
        }

        // Passthrough vs Dark Space (IDs: 60, 61)
        rowY += 66;
        drawText(50, rowY, "Environment Mode:", COL_WHITE, 0.88f);
        const char* envs[2] = {"DARK SPACE", "PASSTHROUGH MR"};
        for (int i = 0; i < 2; ++i) {
            int bx = 380 + i * 270;
            bool sel = (i == 0 ? !m_passthroughEnabled : m_passthroughEnabled);
            uint32_t fill = sel ? COL_WHITE : (m_hoveredButtonId == (60 + i) ? COL_HOVER : COL_INNER_CARD);
            uint32_t textCol = sel ? COL_DARK_TEXT : COL_SILVER;
            drawRoundRect(bx, rowY - 10, 240, 42, 8, fill, sel ? COL_WHITE : COL_BORDER);
            drawTextCentered(bx, rowY + 3, 240, envs[i], textCol, 0.80f);
        }
        rowY += 58;
        char resolutionText[128];
        if (m_streamWidth > 0 && m_streamHeight > 0)
            std::snprintf(resolutionText, sizeof(resolutionText), "Live SBS resolution: %d x %d", m_streamWidth, m_streamHeight);
        else
            std::snprintf(resolutionText, sizeof(resolutionText), "Live SBS resolution: waiting for video");
        drawText(50, rowY, resolutionText, COL_WHITE, 0.78f);
        drawText(285, rowY, "Per-eye stream resolution", COL_SILVER, 0.68f);
        drawText(665, rowY, "Desktop size", COL_SILVER, 0.68f);
        rowY += 24;
        const float scales[3] = {0.70f, 0.85f, 1.0f};
        const char* scaleLabels[3] = {"70%", "85%", "Native"};
        for (int i = 0; i < 3; ++i) {
            const int bx = 265 + i * 112;
            const bool selected = std::abs(m_resolutionScale - scales[i]) < 0.01f;
            const uint32_t fill = selected ? COL_WHITE : (m_hoveredButtonId == 70 + i ? COL_HOVER : COL_INNER_CARD);
            drawRoundRect(bx, rowY - 10, 100, 42, 8, fill, selected ? COL_WHITE : COL_BORDER);
            drawTextCentered(bx, rowY + 3, 100, scaleLabels[i], selected ? COL_DARK_TEXT : COL_SILVER, 0.78f);
        }
        const float desktopScales[3] = {0.75f, 1.0f, 1.25f};
        const char* desktopLabels[3] = {"75%", "100%", "125%"};
        for (int i = 0; i < 3; ++i) {
            const int bx = 650 + i * 112;
            const bool selected = std::abs(m_desktopScale - desktopScales[i]) < 0.01f;
            const uint32_t fill = selected ? COL_WHITE : (m_hoveredButtonId == 80 + i ? COL_HOVER : COL_INNER_CARD);
            drawRoundRect(bx, rowY - 10, 100, 42, 8, fill, selected ? COL_WHITE : COL_BORDER);
            drawTextCentered(bx, rowY + 3, 100, desktopLabels[i], selected ? COL_DARK_TEXT : COL_SILVER, 0.78f);
        }

    } else if (m_activeTab == 3) {
        // --- TAB 3: PERFORMANCE STATS ---
        int statY = contY + 28;
        drawText(50, statY, "REAL-TIME STREAMING TELEMETRY & HARDWARE METRICS", COL_WHITE, 0.95f);

        statY += 48;
        char buf[128];
        snprintf(buf, sizeof(buf), "Active Link Type:            %s",
                 m_isConnected ? (m_isUsbConnected ? "USB Forwarding" : "Wi-Fi High-Speed Wireless") : "Disconnected");
        drawText(80, statY, buf, m_isConnected ? COL_GREEN : COL_RED, 0.85f);

        statY += 44;
        if (m_latencyMs >= 0)
            snprintf(buf, sizeof(buf), "Round-Trip Network Latency:  %.1f ms", m_latencyMs);
        else
            snprintf(buf, sizeof(buf), "Round-Trip Network Latency:  Unknown");
        drawText(80, statY, buf, COL_WHITE, 0.85f);

        statY += 44;
        snprintf(buf, sizeof(buf), "Render Loop FPS:             %.1f FPS (Configured: %d Hz)", m_fps, m_selectedRefreshRate);
        drawText(80, statY, buf, COL_WHITE, 0.85f);

        statY += 44;
        if (m_bitrateMbps >= 0)
            snprintf(buf, sizeof(buf), "Video Stream Bitrate:        %.1f Mbps", m_bitrateMbps);
        else
            snprintf(buf, sizeof(buf), "Video Stream Bitrate:        Unknown");
        drawText(80, statY, buf, COL_WHITE, 0.85f);

        statY += 44;
        snprintf(buf, sizeof(buf), "Preferred Video Codec:      %s (server selects stream)", m_selectedCodec.c_str());
        drawText(80, statY, buf, COL_WHITE, 0.85f);

        statY += 44;
        drawText(80, statY, "Audio Pipeline:              AAudio Low-Latency Real-Time Engine (48kHz Stereo)", COL_WHITE, 0.85f);
    }

    // 4. Bottom Hide Overlay Button (ID: 99)
    int closeBtnW = 260;
    int closeBtnH = 44;
    int closeBtnX = (UI_WIDTH - closeBtnW) / 2;
    int closeBtnY = UI_HEIGHT - 56;
    uint32_t clsFill = (m_hoveredButtonId == 99) ? COL_HOVER : COL_PANEL_BG;
    drawRoundRect(closeBtnX, closeBtnY, closeBtnW, closeBtnH, 10, clsFill, COL_BORDER);
    drawTextCentered(closeBtnX, closeBtnY + 12, closeBtnW, "HIDE MENU (MENU / B)", COL_WHITE, 0.82f);

    // Upload to OpenGL Texture
    glBindTexture(GL_TEXTURE_2D, m_textureId);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, UI_WIDTH, UI_HEIGHT, 0, GL_RGBA, GL_UNSIGNED_BYTE, m_pixelBuffer.data());
    m_dirty = false;
}

bool VRGui::updateRay(const float rayOrigin[3], const float rayDir[3], bool triggerDown) {
    if (!m_visible) {
        m_rayHit = false;
        return false;
    }

    if (std::abs(rayDir[2]) < 0.001f) {
        m_rayHit = false;
        return false;
    }

    float t = (PLANE_Z - rayOrigin[2]) / rayDir[2];
    if (t <= 0.0f || t > 5.0f) {
        m_rayHit = false;
        return false;
    }

    float hitX = rayOrigin[0] + rayDir[0] * t;
    float hitY = rayOrigin[1] + rayDir[1] * t;

    float halfW = PLANE_WIDTH * 0.5f;
    float halfH = PLANE_HEIGHT * 0.5f;
    float relY = hitY - PLANE_Y;

    if (hitX < -halfW || hitX > halfW || relY < -halfH || relY > halfH) {
        m_rayHit = false;
        m_hoveredButtonId = -1;
        return false;
    }

    m_rayHit = true;
    m_hitDistance = t;
    m_hitPos[0] = hitX;
    m_hitPos[1] = hitY;
    m_hitPos[2] = PLANE_Z;

    float normU = (hitX + halfW) / PLANE_WIDTH;
    float normV = (halfH - relY) / PLANE_HEIGHT;

    int px = static_cast<int>(normU * UI_WIDTH);
    int py = static_cast<int>(normV * UI_HEIGHT);

    int prevHover = m_hoveredButtonId;
    m_hoveredButtonId = -1;

    // Header SteamVR button (ID: 15)
    if (px >= UI_WIDTH - 530 && px <= UI_WIDTH - 360 && py >= 28 && py <= 80) {
        m_hoveredButtonId = 15;
    }
    // Header VR Mode Toggle (ID: 16)
    else if (px >= UI_WIDTH - 350 && px <= UI_WIDTH - 195 && py >= 28 && py <= 80) {
        m_hoveredButtonId = 16;
    }
    // Navigation Tabs (IDs: 1..4)
    else if (py >= 104 && py <= 150) {
        if (px >= 20 && px <= 250) m_hoveredButtonId = 1;
        else if (px >= 265 && px <= 495) m_hoveredButtonId = 2;
        else if (px >= 510 && px <= 740) m_hoveredButtonId = 3;
        else if (px >= 755 && px <= 985) m_hoveredButtonId = 4;
    }
    // TAB 0: COMPUTERS
    else if (m_activeTab == 0) {
        if (px >= UI_WIDTH - 250 && px <= UI_WIDTH - 70 && py >= 248 && py <= 302) {
            m_hoveredButtonId = 11; // Connect USB
        } else if (px >= UI_WIDTH - 250 && px <= UI_WIDTH - 70 && py >= 376 && py <= 430) {
            m_hoveredButtonId = 13; // Connect Wi-Fi
        } else if (px >= 40 && px <= 280 && py >= 504 && py <= 552) {
            m_hoveredButtonId = 14; // Disconnect
        }
    }
    // TAB 1: GAMES
    else if (m_activeTab == 1) {
        if (py >= 214 && py <= 298) {
            m_hoveredButtonId = 15; // Launch SteamVR
        } else if (px >= UI_WIDTH - 170 && px <= UI_WIDTH - 60) {
            if (py >= 318 && py <= 356) m_hoveredButtonId = 101; // Alyx
            else if (py >= 380 && py <= 418) m_hoveredButtonId = 102; // Beat Saber
            else if (py >= 442 && py <= 480) m_hoveredButtonId = 103; // Blade & Sorcery
            else if (py >= 504 && py <= 542) m_hoveredButtonId = 104; // VRChat
        }
    }
    // TAB 2: SETTINGS
    else if (m_activeTab == 2) {
        if (py >= 180 && py <= 226) {
            for (int i = 0; i < 4; ++i) {
                if (px >= 380 + i * 135 && px <= 380 + i * 135 + 115) m_hoveredButtonId = 20 + i;
            }
        } else if (py >= 246 && py <= 292) {
            for (int i = 0; i < 3; ++i) {
                if (px >= 380 + i * 180 && px <= 380 + i * 180 + 165) m_hoveredButtonId = 30 + i;
            }
        } else if (py >= 312 && py <= 358) {
            for (int i = 0; i < 4; ++i) {
                if (px >= 380 + i * 135 && px <= 380 + i * 135 + 115) m_hoveredButtonId = 40 + i;
            }
        } else if (py >= 378 && py <= 424) {
            if (px >= 380 && px <= 620) m_hoveredButtonId = 50;
            else if (px >= 650 && px <= 890) m_hoveredButtonId = 51;
        } else if (py >= 444 && py <= 490) {
            if (px >= 380 && px <= 620) m_hoveredButtonId = 60;
            else if (px >= 650 && px <= 890) m_hoveredButtonId = 61;
        } else if (py >= 520 && py <= 570) {
            for (int i = 0; i < 3; ++i) {
                const int streamBx = 265 + i * 112;
                const int desktopBx = 650 + i * 112;
                if (px >= streamBx && px <= streamBx + 100) m_hoveredButtonId = 70 + i;
                else if (px >= desktopBx && px <= desktopBx + 100) m_hoveredButtonId = 80 + i;
            }
        }
    }
    // Hide Menu Button
    if (px >= (UI_WIDTH - 260) / 2 && px <= (UI_WIDTH + 260) / 2 && py >= UI_HEIGHT - 56 && py <= UI_HEIGHT - 12) {
        m_hoveredButtonId = 99;
    }

    if (m_hoveredButtonId != prevHover) {
        m_dirty = true;
    }

    if (triggerDown && !m_prevTriggerDown && m_hoveredButtonId != -1) {
        handleClick(m_hoveredButtonId);
    }
    m_prevTriggerDown = triggerDown;

    if (m_dirty) {
        redraw();
    }
    return true;
}

void VRGui::handleClick(int buttonId) {
    LOGI("VRGui clicked button %d", buttonId);

    // Tab switching
    if (buttonId == 1) { m_activeTab = 0; m_dirty = true; }
    else if (buttonId == 2) { m_activeTab = 1; m_dirty = true; }
    else if (buttonId == 3) { m_activeTab = 2; m_dirty = true; }
    else if (buttonId == 4) { m_activeTab = 3; m_dirty = true; }

    // Connect USB (Wired)
    else if (buttonId == 11) {
        if (m_onConnect) m_onConnect("127.0.0.1", m_usbServerPort);
    }
    // Connect Wi-Fi (Wireless)
    else if (buttonId == 13) {
        if (m_onConnect) m_onConnect(m_wifiServerHost, m_wifiServerPort);
    }
    // Disconnect
    else if (buttonId == 14) {
        if (m_onDisconnect) m_onDisconnect();
    }
    // Launch requests preserve the current display until the server switches streams.
    else if (buttonId == 15) {
        m_startRequested = true;
        m_visible = true;
        if (m_onLaunchSteamVR) m_onLaunchSteamVR();
        m_dirty = true;
    }
    else if (buttonId == 16) {
        if (m_onVrModeChanged) m_onVrModeChanged(false);
        m_startRequested = false;
        m_visible = true;
        m_dirty = true;
    }
    else if (buttonId >= 101 && buttonId <= 104) {
        static const char* gameIds[] = {"546560", "620980", "629730", "438100"};
        m_startRequested = true;
        m_visible = true;
        if (m_onLaunchGame) m_onLaunchGame(gameIds[buttonId - 101]);
        m_dirty = true;
    }

    // Refresh rate
    else if (buttonId >= 20 && buttonId <= 23) {
        int rates[4] = {72, 80, 90, 120};
        m_selectedRefreshRate = rates[buttonId - 20];
        if (m_onRefreshRateChanged) m_onRefreshRateChanged(m_selectedRefreshRate);
        m_dirty = true;
    }
    // Codec
    else if (buttonId >= 30 && buttonId <= 32) {
        const char* codecs[3] = {"HEVC", "AV1", "H264"};
        m_selectedCodec = codecs[buttonId - 30];
        m_dirty = true;
    }
    // Bitrate
    else if (buttonId >= 40 && buttonId <= 43) {
        int bitrates[4] = {50, 100, 150, 200};
        m_selectedBitrate = bitrates[buttonId - 40];
        m_dirty = true;
    }
    else if (buttonId >= 70 && buttonId <= 72) {
        static constexpr float scales[3] = {0.70f, 0.85f, 1.0f};
        m_resolutionScale = scales[buttonId - 70];
        if (m_onResolutionScaleChanged) m_onResolutionScaleChanged(m_resolutionScale);
        m_dirty = true;
    }
    else if (buttonId >= 80 && buttonId <= 82) {
        static constexpr float scales[3] = {0.75f, 1.0f, 1.25f};
        m_desktopScale = scales[buttonId - 80];
        m_dirty = true;
    }
    // Geometry
    else if (buttonId == 50) {
        m_curvedScreen = true;
        if (m_onCurvedChanged) m_onCurvedChanged(true);
        m_dirty = true;
    } else if (buttonId == 51) {
        m_curvedScreen = false;
        if (m_onCurvedChanged) m_onCurvedChanged(false);
        m_dirty = true;
    }
    // Passthrough
    else if (buttonId == 60) {
        m_passthroughEnabled = false;
        if (m_onPassthroughChanged) m_onPassthroughChanged(false);
        m_dirty = true;
    } else if (buttonId == 61) {
        m_passthroughEnabled = true;
        if (m_onPassthroughChanged) m_onPassthroughChanged(true);
        m_dirty = true;
    }
    // Hide Menu
    else if (buttonId == 99) {
        m_visible = false;
    }
}

} // namespace openreality
