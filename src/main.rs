//! Legacy egui interface (feature `legacy-egui`): `cargo run --features legacy-egui --bin typelune-egui`.
//! Not bundled; the shipping app is the SwiftUI shell in `macos/`.

mod app;

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--version") {
        println!("typelune-egui {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    // Developer fallback only: it must use an explicit, isolated data folder so it can never
    // create or edit the real library (or pre-empt the one-time migration).
    let data_dir = match typelune_core::store::resolve_data_location() {
        typelune_core::store::DataLocation::Default { dir, legacy_dir } => {
            typelune_core::store::developer_fallback_dir(
                std::env::var_os(typelune_core::store::DATA_DIR_ENV),
                &dir,
                &legacy_dir,
            )
        }
        _ => {
            let base = dirs::data_dir().unwrap_or_default();
            typelune_core::store::developer_fallback_dir(
                std::env::var_os(typelune_core::store::DATA_DIR_ENV),
                &base.join(typelune_core::store::APP_DIR_NAME),
                &base.join(typelune_core::store::LEGACY_APP_DIR_NAME),
            )
        }
    };
    let data_dir = match data_dir {
        Ok(d) => d,
        Err(e) => {
            eprintln!("typelune-egui: {e}");
            std::process::exit(2);
        }
    };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(concat!(
                "Typelune (legacy egui) v",
                env!("CARGO_PKG_VERSION")
            ))
            .with_app_id("io.github.tranxuanliem.typelune.egui")
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([920.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Typelune",
        options,
        Box::new(move |cc| Ok(Box::new(app::TypeluneApp::new(cc, data_dir)))),
    )
}
