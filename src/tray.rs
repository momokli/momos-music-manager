//! macOS menu bar tray icon.
//!
//! Uses `[NSApp run]` for the full Cocoa event loop — required so
//! NSStatusItem button clicks actually fire. Works with LSUIElement=true.

use std::sync::mpsc;
use std::time::Duration;

use objc2::rc::Retained;
use objc2_app_kit::NSApplication;
use objc2_foundation::MainThreadMarker;
use tray_icon::TrayIconBuilder;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};

pub struct ServerShutdown;

fn build_tray_icon() -> tray_icon::Icon {
    let size: u32 = 22;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let cx = (size / 2) as f32;
    let cy = (size / 2) as f32;
    let radius = size as f32 / 2.0 - 2.0;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= radius {
                rgba.extend_from_slice(&[29, 185, 84, 255]);
            } else if dist <= radius + 1.0 {
                let a = ((radius + 1.0 - dist) * 255.0) as u8;
                rgba.extend_from_slice(&[29, 185, 84, a]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    tray_icon::Icon::from_rgba(rgba, size, size).expect("tray icon from_rgba")
}

pub fn run(_host: String, port: u16, server_rx: mpsc::Receiver<ServerShutdown>) {
    tracing::info!("Starting tray icon (macOS menu bar)");
    let url = format!("http://127.0.0.1:{}", port);

    // ── Initialize NSApplication ────────────────────────────────────────
    let _mtm = MainThreadMarker::new().expect("must be on main thread");
    let app: Retained<NSApplication> =
        unsafe { objc2::msg_send![objc2::class!(NSApplication), sharedApplication] };

    // ── Build menu + tray ───────────────────────────────────────────────
    let open = MenuItem::new("Open Dashboard", true, None);
    let quit = MenuItem::new("Quit Momo's Music Manager", true, None);
    let menu = Menu::new();
    menu.append_items(&[&open, &quit]).expect("menu items");

    let menu_receiver = MenuEvent::receiver();
    let open_id = open.id().clone();
    let quit_id = quit.id().clone();

    let tray_result = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Momo's Music Manager")
        .with_icon(build_tray_icon())
        .build();

    let tray = match tray_result {
        Ok(t) => {
            tracing::info!("Tray icon created!");
            let _ = webbrowser::open(&url);
            t
        }
        Err(e) => {
            tracing::error!("Tray icon FAILED: {}", e);
            let _ = webbrowser::open(&url);
            loop {
                std::thread::sleep(Duration::from_millis(500));
                if server_rx.try_recv().is_ok() {
                    break;
                }
            }
            return;
        }
    };

    // ── Monitor menu events + server death from a bg thread ─────────────
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(200));
            if let Ok(event) = menu_receiver.try_recv() {
                if event.id == open_id {
                    let _ = webbrowser::open(&url);
                } else if event.id == quit_id {
                    unsafe {
                        let app: Retained<NSApplication> =
                            objc2::msg_send![objc2::class!(NSApplication), sharedApplication];
                        let _: () = objc2::msg_send![&app, terminate: std::ptr::null::<objc2::runtime::NSObject>()];
                    }
                    break;
                }
            }
            if server_rx.try_recv().is_ok() {
                unsafe {
                    let app: Retained<NSApplication> =
                        objc2::msg_send![objc2::class!(NSApplication), sharedApplication];
                    let _: () = objc2::msg_send![&app, terminate: std::ptr::null::<objc2::runtime::NSObject>()];
                }
                break;
            }
        }
    });

    // ── Run the Cocoa event loop ────────────────────────────────────────
    // This processes ALL events including NSStatusItem button clicks.
    // Blocks until [NSApp terminate:] is called.
    unsafe {
        let _: () = objc2::msg_send![&app, run];
    }

    // Cleanup
    drop(tray);
    tracing::info!("Tray icon shut down");
}
