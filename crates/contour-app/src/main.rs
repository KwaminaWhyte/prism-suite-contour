//! Contour — vector graphics editor, app #2 of the Prism creative suite.
//! v0 scaffold entry point.

// egui 0.34 deprecates several menu/panel aliases mid-cycle; silence the churn.
#![allow(deprecated)]

mod align;
mod app;
mod appearance;
mod arrange;
mod artboard;
mod blend;
mod boolean;
mod canvas;
mod clip;
mod clipboard;
mod document;
mod effects;
mod export;
mod eyedropper;
mod fonts;
mod gradient;
mod graphic_styles;
mod group;
mod history;
mod icons;
mod layers;
mod liveshape;
mod opacity_mask;
mod pathedit;
mod shapebuilder;
mod snap;
mod stroke;
mod swatches;
mod text;
mod theme;
mod trace;
mod transform;
mod workspace;

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
