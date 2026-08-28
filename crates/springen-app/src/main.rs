//! Springen — the desktop map design tool.
//!
//! `--screenshot <dir>` renders each screen once and writes PNGs instead of
//! opening a window, so the interface can be checked in CI on a machine with
//! only a software rasteriser.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod graph_view;
mod panels;
mod theme;
mod view3d;

use std::path::PathBuf;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut screenshot: Option<PathBuf> = None;
    let mut screen: Option<String> = None;
    let mut starter: Option<String> = None;
    let mut smoke: Option<PathBuf> = None;
    let mut view: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--screenshot" => {
                screenshot = args.get(i + 1).map(PathBuf::from);
                i += 1;
            }
            "--screen" => {
                screen = args.get(i + 1).cloned();
                i += 1;
            }
            "--view" => {
                view = args.get(i + 1).cloned();
                i += 1;
            }
            "--smoke" => {
                smoke = args.get(i + 1).map(PathBuf::from);
                i += 1;
            }
            "--starter" => {
                starter = args.get(i + 1).cloned();
                i += 1;
            }
            "--help" | "-h" => {
                println!(
                    "springen [--starter ridge|islands|textured] [--screen splash|projects|workspace|terrain|floating]\n         [--screenshot <file.png>] [--smoke <out.sd7>]"
                );
                return Ok(());
            }
            _ => {}
        }
        i += 1;
    }

    // The preload window is a placeholder, so it opens small and undecorated
    // and grows into the real window once priming is done.
    let preload = screen.is_none() && smoke.is_none();
    let viewport = if preload {
        egui::ViewportBuilder::default()
            .with_inner_size([480.0, 300.0])
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(true)
            .with_title("Springen")
    } else {
        egui::ViewportBuilder::default()
            .with_inner_size([1600.0, 1000.0])
            .with_min_inner_size([1100.0, 700.0])
            .with_title("Springen")
    };
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let starter_for_app = starter.clone();
    eframe::run_native(
        "Springen",
        options,
        Box::new(move |cc| {
            let mut a = app::SpringenApp::new(cc, starter_for_app.as_deref());
            if let Some(s) = &screen {
                a.goto(s);
            }
            if let Some(v) = &view {
                a.set_view(v);
            }
            if let Some(out) = &smoke {
                a.smoke_test(out.clone());
            }
            match &screenshot {
                Some(path) => Ok(Box::new(Shot::new(a, path.clone()))),
                None => Ok(Box::new(a)),
            }
        }),
    )
}

/// Renders a few frames so layout and textures settle, asks for a screenshot,
/// writes it and quits.
struct Shot {
    inner: app::SpringenApp,
    path: PathBuf,
    asked: bool,
    /// Frames to let settle first. The preload needs one per stage.
    wait_frames: u64,
}

impl Shot {
    fn new(inner: app::SpringenApp, path: PathBuf) -> Self {
        Shot {
            inner,
            path,
            asked: false,
            wait_frames: 12,
        }
    }
}

impl eframe::App for Shot {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        self.inner.ui(ui, frame);
        ctx.request_repaint();

        if self.inner.frames >= self.wait_frames && !self.asked {
            self.asked = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }

        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = shot {
            let [w, h] = image.size;
            let mut samples = Vec::with_capacity(w * h * 3);
            for px in image.pixels.iter() {
                samples.push(u16::from(px.r()));
                samples.push(u16::from(px.g()));
                samples.push(u16::from(px.b()));
            }
            let png = springen_core::png::encode(
                w,
                h,
                springen_core::png::PngColor::Rgb,
                8,
                &samples,
                springen_core::png::Compression::Deflate,
            );
            if let Some(parent) = self.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&self.path, &png) {
                Ok(()) => println!("Wrote {} — {} × {}", self.path.display(), w, h),
                Err(e) => eprintln!("{}: {e}", self.path.display()),
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
