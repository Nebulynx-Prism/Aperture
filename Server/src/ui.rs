//! User interface for OpenReality.
//!
//! Features:
//! - Default grayish theme (#252525 background)
//! - Top bar featuring the official logo image (`logo.png`)
//! - Window and taskbar icon set to `icon.png`
//! - Universal Virtual Desktop compatibility ("(VD Accounts)" manager + Auto-Accept mode)
//! - System Tray Icon support via Win32 Shell_NotifyIcon
//! - Image renderers via `egui_extras`
//! - Streaming telemetry, optical foveation, audio routing, and multi-monitor management

use crate::settings::{
    AudioStreaming, GamepadEmulation, OpenXRRuntime, StreamerSettings, VdAccount, VideoCodec,
};
use anyhow::Result;
use eframe::egui::{self, Color32, RichText, Sense, Stroke, Vec2};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub fn run(settings: StreamerSettings) -> Result<()> {
    // Load app icon from assets/icon.png
    let app_icon = load_app_icon();

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([720.0, 560.0])
        .with_min_inner_size([620.0, 480.0])
        .with_title("Prism Aperture Server")
        .with_decorations(true)
        .with_resizable(true);

    if let Some(icon) = app_icon {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Prism Aperture Server",
        options,
        Box::new(|cc| {
            // Install image loaders for PNG, JPEG, SVG, WebP, etc.
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(OpenRealityApp::new(settings)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("UI error: {e}"))
}

/// Load the application icon for titlebar and taskbar
fn load_app_icon() -> Option<egui::IconData> {
    let icon_bytes = include_bytes!("../assets/appv2.png");
    if let Ok(img) = image::load_from_memory(icon_bytes) {
        let rgba = img.to_rgba8();
        let (width, height) = rgba.dimensions();
        Some(egui::IconData {
            rgba: rgba.into_raw(),
            width,
            height,
        })
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppTab {
    Streaming,
    VideoOptics,
    AudioInput,
    Displays,
    VdAccountsModal,
}

pub struct OpenRealityApp {
    settings: StreamerSettings,
    current_tab: AppTab,

    // Status
    is_streaming: Arc<AtomicBool>,
    connected_clients: u32,
    current_fps: f32,
    current_bitrate_mbps: f32,
    current_latency_ms: f32,

    // Universal compatibility setting
    accept_any_client: bool,

    // Account modal inputs
    new_account_name: String,
    selected_platform: String,
    save_notification: Option<(String, std::time::Instant)>,

    // Active session handle
    session_handle: Option<crate::session::SessionHandle>,

    // Tray status
    tray_initialized: bool,
}

impl OpenRealityApp {
    pub fn new(settings: StreamerSettings) -> Self {
        let session_handle = Some(crate::session::start_session(settings.clone()));

        Self {
            settings,
            current_tab: AppTab::Streaming,
            is_streaming: Arc::new(AtomicBool::new(true)),
            connected_clients: 0,
            current_fps: 90.0,
            current_bitrate_mbps: 85.0,
            current_latency_ms: 18.5,
            accept_any_client: true, // Universal compatibility enabled by default
            new_account_name: String::new(),
            selected_platform: "OculusQuest".to_string(),
            save_notification: None,
            session_handle,
            tray_initialized: false,
        }
    }

    fn notify(&mut self, msg: &str) {
        self.save_notification = Some((msg.to_string(), std::time::Instant::now()));
    }
}

impl eframe::App for OpenRealityApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Sync real-time streaming telemetry
        if let Some(ref sh) = self.session_handle {
            self.connected_clients = sh.telemetry.connected_clients.load(Ordering::Relaxed);
            if let Ok(fps) = sh.telemetry.current_fps.try_read() {
                if *fps > 0.0 { self.current_fps = *fps; }
            }
            if let Ok(mbps) = sh.telemetry.current_bitrate_mbps.try_read() {
                if *mbps > 0.0 { self.current_bitrate_mbps = *mbps; }
            }
            if let Ok(lat) = sh.telemetry.current_latency_ms.try_read() {
                if *lat > 0.0 { self.current_latency_ms = *lat; }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }

        // Grayish Theme with #252525 background
        let mut visuals = egui::Visuals::dark();
        let bg_gray = Color32::from_rgb(0x25, 0x25, 0x25);       // #252525 background
        let panel_gray = Color32::from_rgb(0x28, 0x28, 0x28);    // #282828 slightly raised panels
        let widget_bg = Color32::from_rgb(0x32, 0x32, 0x32);     // #323232 widget fill
        let widget_hover = Color32::from_rgb(0x40, 0x40, 0x40);  // #404040 widget hover
        let widget_active = Color32::from_rgb(0x50, 0x50, 0x50); // #505050 widget active

        visuals.window_fill = bg_gray;
        visuals.panel_fill = bg_gray;
        visuals.extreme_bg_color = Color32::from_rgb(0x1c, 0x1c, 0x1c);
        visuals.faint_bg_color = panel_gray;

        visuals.widgets.noninteractive.bg_fill = panel_gray;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x3c, 0x3c, 0x3c));

        visuals.widgets.inactive.bg_fill = widget_bg;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x44, 0x44, 0x44));

        visuals.widgets.hovered.bg_fill = widget_hover;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x60, 0x60, 0x60));

        visuals.widgets.active.bg_fill = widget_active;
        visuals.widgets.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x80, 0x80, 0x80));

        ctx.set_visuals(visuals);

        // Top Navigation & Branding Bar
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                // Top bar image logo replacing text completely
                ui.add(
                    egui::Image::new(egui::include_image!("../assets/logo.png"))
                        .max_height(38.0)
                        .fit_to_original_size(0.24),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // (VD Accounts) - Quest Client Compatibility Button
                    let vd_btn = egui::Button::new(
                        RichText::new("⚡ (VD Accounts)")
                            .color(Color32::from_rgb(255, 230, 80))
                            .strong()
                            .size(13.0),
                    )
                    .fill(Color32::from_rgb(50, 45, 20))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(200, 180, 40)));

                    if ui.add(vd_btn).clicked() {
                        self.current_tab = AppTab::VdAccountsModal;
                    }

                    // Status Pill
                    let (status_text, status_color) = if self.connected_clients > 0 {
                        ("STREAMING", Color32::from_rgb(50, 220, 110))
                    } else if self.is_streaming.load(Ordering::Relaxed) {
                        ("READY", Color32::from_rgb(0, 190, 255))
                    } else {
                        ("IDLE", Color32::GRAY)
                    };

                    ui.label(
                        RichText::new(format!("● {status_text}"))
                            .color(status_color)
                            .strong()
                            .size(12.0),
                    );

                    ui.separator();

                    // Tray minimize shortcut
                    if ui.button("🗕 Tray").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                });
            });

            ui.add_space(8.0);

            // Tab bar
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.current_tab, AppTab::Streaming, "Streaming");
                ui.selectable_value(&mut self.current_tab, AppTab::VideoOptics, "Video & Optics");
                ui.selectable_value(&mut self.current_tab, AppTab::AudioInput, "Audio & Input");
                ui.selectable_value(&mut self.current_tab, AppTab::Displays, "Displays");
            });
            ui.add_space(4.0);
        });

        // Bottom Notification Bar
        egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("PC: {}", self.settings.computer_name))
                        .size(11.0)
                        .color(Color32::LIGHT_GRAY),
                );
                ui.separator();
                ui.label(
                    RichText::new(format!("ID: {}", &self.settings.computer_id[..8]))
                        .size(11.0)
                        .color(Color32::GRAY),
                );
                ui.separator();
                let (svc_text, svc_color) = if let Some(ref sh) = self.session_handle {
                    if sh.telemetry.service_connected.load(Ordering::Relaxed) {
                        ("Service: Active (Realtime Priority)", Color32::from_rgb(80, 230, 130))
                    } else {
                        ("Service: Standalone Mode", Color32::from_rgb(230, 190, 60))
                    }
                } else {
                    ("Service: Standalone Mode", Color32::GRAY)
                };
                ui.label(RichText::new(svc_text).size(11.0).color(svc_color));

                if let Some((msg, time)) = &self.save_notification {
                    if time.elapsed().as_secs() < 3 {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(msg).color(Color32::GREEN).size(11.0));
                        });
                    }
                }
            });
        });

        // Main Content Panel
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match self.current_tab {
                AppTab::Streaming => self.show_streaming_tab(ui),
                AppTab::VideoOptics => self.show_video_optics_tab(ui),
                AppTab::AudioInput => self.show_audio_input_tab(ui),
                AppTab::Displays => self.show_displays_tab(ui),
                AppTab::VdAccountsModal => self.show_vd_accounts_modal(ui),
            });
        });
    }
}

impl OpenRealityApp {
    fn show_streaming_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Stream Configuration");
        ui.add_space(6.0);

        egui::Grid::new("stream_grid").num_columns(2).spacing([20.0, 10.0]).show(ui, |ui| {
            ui.label("Preferred Codec:");
            egui::ComboBox::from_id_salt("codec_select")
                .selected_text(format!("{:?}", self.settings.preferred_codec))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.settings.preferred_codec, VideoCodec::Automatic, "Automatic (Recommended)");
                    ui.selectable_value(&mut self.settings.preferred_codec, VideoCodec::H264, "H.264 (Lowest Latency)");
                    ui.selectable_value(&mut self.settings.preferred_codec, VideoCodec::Hevc, "HEVC / H.265 (High Efficiency)");
                    ui.selectable_value(&mut self.settings.preferred_codec, VideoCodec::Hevc10bit, "HEVC 10-bit (Wide Color)");
                    ui.selectable_value(&mut self.settings.preferred_codec, VideoCodec::Av1, "AV1 (Next-Gen Visual Fidelity)");
                    ui.selectable_value(&mut self.settings.preferred_codec, VideoCodec::Av110bit, "AV1 10-bit (True HDR)");
                });
            ui.end_row();

            ui.label("Target Framerate:");
            egui::ComboBox::from_id_salt("fps_select")
                .selected_text(format!("{} Hz", self.settings.framerate))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.settings.framerate, 60, "60 Hz");
                    ui.selectable_value(&mut self.settings.framerate, 72, "72 Hz (Quest 1/2)");
                    ui.selectable_value(&mut self.settings.framerate, 80, "80 Hz (Rift S)");
                    ui.selectable_value(&mut self.settings.framerate, 90, "90 Hz (Native)");
                    ui.selectable_value(&mut self.settings.framerate, 120, "120 Hz (High Smoothness)");
                    ui.selectable_value(&mut self.settings.framerate, 144, "144 Hz (Ultra)");
                });
            ui.end_row();

            ui.label("Bitrate Limit:");
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut self.settings.bitrate_limit_mbps, 10..=200).suffix(" Mbps"));
                ui.checkbox(&mut self.settings.auto_adjust_bitrate, "Auto Adjust");
            });
            ui.end_row();

            ui.label("Hardware Encoder:");
            ui.label(RichText::new(&self.settings.device_name).color(Color32::from_rgb(0, 210, 160)).strong());
            ui.end_row();
        });

        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);

        ui.heading("Session Telemetry");
        ui.add_space(6.0);

        egui::Grid::new("telemetry_grid").num_columns(4).spacing([16.0, 8.0]).show(ui, |ui| {
            ui.label("Framerate:");
            ui.label(RichText::new(format!("{:.1} FPS", self.current_fps)).strong());

            ui.label("Bitrate:");
            ui.label(RichText::new(format!("{:.1} Mbps", self.current_bitrate_mbps)).strong());
            ui.end_row();

            ui.label("Total Latency:");
            ui.label(RichText::new(format!("{:.1} ms", self.current_latency_ms)).strong().color(Color32::from_rgb(100, 230, 100)));

            ui.label("Connected Clients:");
            ui.label(RichText::new(format!("{}", self.connected_clients)).strong());
            ui.end_row();
        });

        ui.add_space(16.0);
        if ui.button("💾 Save Settings").clicked() {
            let _ = self.settings.save();
            self.notify("Settings saved!");
        }
    }

    fn show_video_optics_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Foveated Streaming & Optics");
        ui.add_space(6.0);

        ui.checkbox(&mut self.settings.use_fov_stencil, "Enable FOV Stencil (Masks invisible peripheral pixels)");
        if self.settings.use_fov_stencil {
            ui.add(egui::Slider::new(&mut self.settings.fovea_size, 0.2..=1.0).text("Center Sharpness Radius"));
        }

        ui.add_space(10.0);
        ui.checkbox(&mut self.settings.use_sharpening, "Adaptive Edge Sharpening Filter");
        if self.settings.use_sharpening {
            ui.add(egui::Slider::new(&mut self.settings.sharpening, 0.0..=1.0).text("Sharpening Strength"));
        }

        ui.add_space(10.0);
        ui.checkbox(&mut self.settings.enable_aq, "Adaptive Quantization (AQ) for smoother color gradients");
        ui.checkbox(&mut self.settings.enable_yuv444, "YUV 4:4:4 Full Chroma (Higher bandwidth)");
        ui.checkbox(&mut self.settings.enable_2pass, "Two-Pass Encoding Pipeline");

        ui.add_space(14.0);
        if ui.button("💾 Save Settings").clicked() {
            let _ = self.settings.save();
            self.notify("Optics settings saved!");
        }
    }

    fn show_audio_input_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Audio Routing");
        ui.add_space(6.0);

        egui::Grid::new("audio_grid").num_columns(2).spacing([20.0, 10.0]).show(ui, |ui| {
            ui.label("Audio Output:");
            egui::ComboBox::from_id_salt("audio_stream_select")
                .selected_text(match self.settings.audio_streaming {
                    AudioStreaming::Disabled => "Disabled",
                    AudioStreaming::HeadsetOnly => "Headset Only",
                    AudioStreaming::HeadsetAndComputer => "Headset and Computer",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.settings.audio_streaming, AudioStreaming::HeadsetAndComputer, "Headset and Computer");
                    ui.selectable_value(&mut self.settings.audio_streaming, AudioStreaming::HeadsetOnly, "Headset Only");
                    ui.selectable_value(&mut self.settings.audio_streaming, AudioStreaming::Disabled, "Disabled");
                });
            ui.end_row();

            ui.label("Microphone:");
            ui.checkbox(&mut self.settings.auto_select_microphone, "Route VR Headset Microphone to PC");
            ui.end_row();

            ui.label("Virtual Audio Driver:");
            ui.checkbox(&mut self.settings.use_virtual_audio_driver, "Use Isolated Virtual Audio Endpoint");
            ui.end_row();
        });

        ui.add_space(14.0);
        ui.heading("Input Emulation");
        ui.add_space(6.0);

        egui::Grid::new("input_grid").num_columns(2).spacing([20.0, 10.0]).show(ui, |ui| {
            ui.label("Gamepad Emulation:");
            egui::ComboBox::from_id_salt("gamepad_select")
                .selected_text(match self.settings.gamepad_emulation {
                    GamepadEmulation::Automatic => "Automatic",
                    GamepadEmulation::Xbox => "Xbox 360 Controller",
                    GamepadEmulation::Dualshock => "DualShock 4",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.settings.gamepad_emulation, GamepadEmulation::Automatic, "Automatic");
                    ui.selectable_value(&mut self.settings.gamepad_emulation, GamepadEmulation::Xbox, "Xbox 360 Controller");
                    ui.selectable_value(&mut self.settings.gamepad_emulation, GamepadEmulation::Dualshock, "DualShock 4");
                });
            ui.end_row();

            ui.label("Touch Input:");
            ui.checkbox(&mut self.settings.use_touch_input, "Map VR controllers to native Windows Touch");
            ui.end_row();

            ui.label("Process Priority:");
            ui.checkbox(&mut self.settings.boost_game_priority, "Boost VR/Game Process Priority via Prism Aperture Service");
            ui.end_row();
        });

        ui.add_space(14.0);
        if ui.button("💾 Save Settings").clicked() {
            let _ = self.settings.save();
            self.notify("Audio/Input settings saved!");
        }
    }

    fn show_displays_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Monitors & Displays");
        ui.add_space(6.0);

        ui.label("Select active display output for VR streaming:");
        ui.add_space(4.0);

        egui::ComboBox::from_id_salt("monitor_select")
            .selected_text(format!("Display {} (Primary Desktop)", self.settings.selected_tab + 1))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.settings.selected_tab, 0, "Display 1 (1920x1080 @ 60Hz)");
                ui.selectable_value(&mut self.settings.selected_tab, 1, "Display 2 (Virtual Display)");
            });

        ui.add_space(10.0);
        ui.label("Prism Aperture supports switching between multiple virtual and physical monitors on the fly.");
    }

    /// (VD Accounts) modal for Virtual Desktop Client Compatibility
    fn show_vd_accounts_modal(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("◀ Back to Main").clicked() {
                self.current_tab = AppTab::Streaming;
            }
            ui.heading("Quest Client Compatibility (Accounts)");
        });

        ui.add_space(8.0);
        ui.label(
            RichText::new("Allows Prism Aperture and third-party Quest client apps to discover and connect to Prism Aperture Server.")
                .color(Color32::from_rgb(200, 200, 220)),
        );

        ui.add_space(10.0);

        // Universal Compatibility Toggle
        ui.group(|ui| {
            ui.checkbox(&mut self.accept_any_client, RichText::new("Universal Client Compatibility (Recommended)").strong());
            ui.label(
                RichText::new("Automatically accepts any incoming client connection without requiring pre-matched accounts or online entitlement verification.")
                    .size(11.0)
                    .color(Color32::from_rgb(180, 220, 180)),
            );
        });

        ui.add_space(8.0);
        ui.checkbox(&mut self.settings.vd_compatibility_enabled, "Enable Compatibility Layer");

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label(RichText::new("Configured User Accounts:").strong());

        let mut to_remove = None;
        for (i, account) in self.settings.vd_accounts.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.checkbox(&mut account.enabled, "");
                ui.label(RichText::new(&account.username).strong());
                ui.label(format!("({})", account.platform));

                if ui.button("🗑 Remove").clicked() {
                    to_remove = Some(i);
                }
            });
        }

        if let Some(idx) = to_remove {
            self.settings.vd_accounts.remove(idx);
            let _ = self.settings.save();
            self.notify("Account removed");
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label(RichText::new("Add Quest / Meta Username:").strong());
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.new_account_name).hint_text("Quest Username"));

            egui::ComboBox::from_id_salt("platform_select")
                .selected_text(&self.selected_platform)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.selected_platform, "OculusQuest".to_string(), "Oculus / Meta Quest");
                    ui.selectable_value(&mut self.selected_platform, "Pico".to_string(), "Pico");
                    ui.selectable_value(&mut self.selected_platform, "Vive".to_string(), "HTC Vive");
                    ui.selectable_value(&mut self.selected_platform, "Apple".to_string(), "Apple Vision");
                });

            if ui.button("➕ Add Account").clicked() && !self.new_account_name.trim().is_empty() {
                self.settings.vd_accounts.push(VdAccount {
                    username: self.new_account_name.trim().to_string(),
                    platform: self.selected_platform.clone(),
                    enabled: true,
                });
                self.new_account_name.clear();
                let _ = self.settings.save();
                self.notify("Account added!");
            }
        });

        ui.add_space(16.0);
        ui.horizontal(|ui| {
            if ui.button("💾 Save Accounts").clicked() {
                let _ = self.settings.save();
                self.notify("Accounts saved!");
            }
            if ui.button("Done").clicked() {
                self.current_tab = AppTab::Streaming;
            }
        });
    }
}
