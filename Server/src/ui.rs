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
        .with_title("Aperture Server")
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
        "Aperture Server",
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
    Updates,
    Settings,
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
    update_checker: crate::updater::UpdateChecker,
    update_prompt: Option<crate::updater::Release>,
    update_seen: Option<String>,
    update_job: Option<crate::update_job::UpdateJob>,
    urgent_update_notice: bool,
    update_shutdown_started: Option<std::time::Instant>,
    runtime_switcher: crate::runtime_switch::RuntimeSwitcher,
}

impl OpenRealityApp {
    pub fn new(settings: StreamerSettings) -> Self {
        let session_handle = Some(crate::session::start_session(settings.clone()));
        let runtime_choice=settings.openxr_runtime;
        let arguments:Vec<String>=std::env::args().collect();
        let failed_update_key=arguments.windows(2).find(|pair|pair[0]=="--update-failed-key").map(|pair|pair[1].clone());

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
            update_checker: crate::updater::UpdateChecker::default(),
            update_prompt: None,
            update_seen: failed_update_key,
            update_job: None,
            urgent_update_notice: std::env::args().any(|a| a=="--updated-urgent"),
            update_shutdown_started: None,
            runtime_switcher: crate::runtime_switch::RuntimeSwitcher::new(runtime_choice),
        }
    }

    fn notify(&mut self, msg: &str) {
        self.save_notification = Some((msg.to_string(), std::time::Instant::now()));
    }
}

impl eframe::App for OpenRealityApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(choice)=self.runtime_switcher.poll() {
            self.settings.openxr_runtime=choice;
            if let Err(error)=self.settings.save(){self.notify(&format!("Runtime applied, but preference could not be saved: {error}"));}
        }
        if self.runtime_switcher.busy(){ctx.request_repaint_after(std::time::Duration::from_millis(100));}
        self.update_checker.poll();
        if self.settings.auto_check_updates && self.update_checker.due() { self.update_checker.start(); }
        if self.update_checker.checking() { ctx.request_repaint_after(std::time::Duration::from_millis(250)); }
        self.process_updates(ctx);
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
                    egui::Image::new(egui::include_image!("../assets/logo-aperture.png"))
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
                ui.selectable_value(&mut self.current_tab, AppTab::Updates, "Updates");
                ui.selectable_value(&mut self.current_tab, AppTab::Settings, "Settings");
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
                AppTab::Updates => self.show_updates_tab(ui),
                AppTab::Settings => self.show_settings_tab(ui),
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
            ui.checkbox(&mut self.settings.use_virtual_audio_driver, "Capture VB-CABLE (CABLE Input)");
            ui.end_row();
        });

        ui.add_space(14.0);
        ui.label("VB-CABLE: route apps to CABLE Input in Windows Sound settings. Save settings and restart Aperture Server after changing routing. Headset Only requires VB-CABLE to avoid speaker playback. Multichannel sources use front left/right.");
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
            ui.checkbox(&mut self.settings.boost_game_priority, "Boost VR/Game Process Priority via Aperture Service");
            ui.end_row();
        });

        ui.add_space(14.0);
        if ui.button("💾 Save Settings").clicked() {
            let _ = self.settings.save();
            self.notify("Audio/Input settings saved. Restart Aperture Server to apply audio routing.");
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
        ui.label("Aperture supports switching between multiple virtual and physical monitors on the fly.");
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
            RichText::new("Allows Aperture and third-party Quest client apps to discover and connect to Aperture Server.")
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



impl OpenRealityApp {
    fn show_updates_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Aperture Server Updates");
        ui.label(format!("Installed version: {}", env!("CARGO_PKG_VERSION")));
        if ui.checkbox(&mut self.settings.auto_check_updates, "Auto update checks").changed() {
            if let Err(error) = self.settings.save() { self.notify(&format!("Could not save update preference: {error}")); }
        }
        ui.label("Check at startup and every six hours when enabled. Stable releases only.");
        if ui.add_enabled(!self.update_checker.checking(), egui::Button::new("Check for updates")).clicked() {
            self.update_seen=None;
            self.settings.skipped_update_version=None;
            self.update_checker.start();
        }
        ui.label(&self.update_checker.message);
        if let Some(release) = &self.update_checker.release {
            if let Some(url) = &release.installer_url { ui.hyperlink_to("Download Aperture installer", url); }
            ui.hyperlink_to("Release notes", &release.page_url);
        }
        ui.hyperlink_to("All Aperture releases", "https://github.com/Nebulynx-Prism/Aperture/releases");
        ui.label("Normal ZIP updates ask before installation. Urgent ZIP updates install automatically when detected; Windows administrator approval may still be required.");
    }
}

impl OpenRealityApp {
    fn show_settings_tab(&mut self, ui:&mut egui::Ui) {
        ui.heading("Server Settings");
        ui.label("Diagnostics");
        if ui.checkbox(&mut self.settings.show_console,"Show diagnostic console window").changed(){
            match crate::console::set_visible(self.settings.show_console) {
                Ok(())=>if let Err(error)=self.settings.save(){self.notify(&format!("Could not save console preference: {error}"));},
                Err(error)=>{self.settings.show_console=!self.settings.show_console;self.notify(&format!("Console unavailable: {error}"));}
            }
        }
        ui.label("Off by default. Use this toggle to close the console without closing Aperture.");
        ui.add_space(16.0);
        ui.heading("OpenXR Runtime");
        ui.label("Close VR apps and SteamVR before switching. Changes apply to newly launched OpenXR applications.");
        egui::ComboBox::from_id_salt("openxr_runtime_choice").selected_text(match self.runtime_switcher.choice {
            OpenXRRuntime::SteamVR=>"SteamVR OpenXR (SteamVR)",
            OpenXRRuntime::ApertureXR=>"Aperture OpenXR (ApertureXR)",
            OpenXRRuntime::Automatic=>"Current system runtime",
        }).show_ui(ui,|ui|{
            ui.selectable_value(&mut self.runtime_switcher.choice,OpenXRRuntime::SteamVR,"SteamVR OpenXR (SteamVR)");
            ui.selectable_value(&mut self.runtime_switcher.choice,OpenXRRuntime::ApertureXR,"Aperture OpenXR (ApertureXR)");
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.runtime_switcher.busy() && self.runtime_switcher.choice!=OpenXRRuntime::Automatic,egui::Button::new("Apply runtime")).clicked(){self.runtime_switcher.apply(false);}
            if ui.add_enabled(!self.runtime_switcher.busy(),egui::Button::new("Restore previous runtime")).clicked(){self.runtime_switcher.apply(true);}
        });
        ui.label(&self.runtime_switcher.message);
        ui.label("Requires Aperture Service. ApertureXR is experimental and supports OpenXR apps; OpenVR apps require SteamVR. Never run both compositor backends together.");
    }
}

impl OpenRealityApp {
    fn begin_update(&mut self, release: &crate::updater::Release) {
        match crate::update_job::UpdateJob::start(release) {
            Ok(job)=>{self.update_job=Some(job);self.update_prompt=None;self.update_checker.message="Preparing update... Aperture stays open until verification finishes.".into();}
            Err(error)=>self.update_checker.message=format!("Could not start updater: {error:#}"),
        }
    }
    fn process_updates(&mut self,ctx:&egui::Context){
        if let Some(release)=self.update_checker.release.clone(){
            let key=format!("{}:{}",release.version,release.urgent);
            if release.package_url.is_some() && self.update_seen.as_ref()!=Some(&key) && self.update_job.is_none(){
                self.update_seen=Some(key);
                if release.urgent {self.begin_update(&release);}
                else if self.settings.skipped_update_version.as_ref()!=Some(&release.version){self.update_prompt=Some(release);}
            }
        }
        if let Some(job)=self.update_job.as_mut(){
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
            match job.poll(){
                crate::update_job::Progress::Ready=>{
                    if let Some(started)=self.update_shutdown_started {
                        if started.elapsed()>=std::time::Duration::from_secs(1){ctx.send_viewport_cmd(egui::ViewportCommand::Close);}
                    } else {
                        if let Some(session)=&self.session_handle{session.stop();}
                        self.update_shutdown_started=Some(std::time::Instant::now());
                    }
                }
                crate::update_job::Progress::Failed(error)=>{self.update_checker.message=format!("Update failed: {error}");self.update_job=None;}
                crate::update_job::Progress::Waiting=>{}
            }
        }
        if let Some(release)=self.update_prompt.clone(){
            egui::Window::new("Aperture Update").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER,egui::Vec2::ZERO).show(ctx,|ui|{
                ui.label(format!("v{} is available. View on GitHub for the changelog. Wanna update?",release.version));
                ui.hyperlink_to("View on GitHub",&release.page_url);
                ui.horizontal(|ui|{
                    if ui.button("Update").clicked(){self.begin_update(&release);}
                    if ui.button("Skip").clicked(){self.settings.skipped_update_version=Some(release.version.clone());let _=self.settings.save();self.update_prompt=None;}
                });
            });
        }
        if self.urgent_update_notice{
            egui::Window::new("Aperture updated").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER,egui::Vec2::ZERO).show(ctx,|ui|{
                ui.label("An urgent update was pushed and your software was auto-updated due to the urgent update.");
                if ui.button("OK").clicked(){self.urgent_update_notice=false;}
            });
        }
    }
}


