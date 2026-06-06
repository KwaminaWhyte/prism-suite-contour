//! Contour — vector graphics editor, app #2 of the Prism creative suite.
//! v0 scaffold entry point.

// egui 0.34 deprecates several menu/panel aliases mid-cycle; silence the churn.
#![allow(deprecated)]

mod align;
mod app;
mod arrange;
mod artboard;
mod boolean;
mod canvas;
mod document;
mod export;
mod gradient;
mod group;
mod history;
mod icons;
mod snap;
mod theme;
mod transform;

use app::ContourApp;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_title("Contour"),
        ..Default::default()
    };

    eframe::run_native(
        "Contour",
        options,
        Box::new(|cc| Ok(Box::new(ContourApp::new(cc)))),
    )
}
