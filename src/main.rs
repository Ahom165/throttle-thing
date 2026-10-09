//! Point d'entrée de l'application graphique (egui).
//! Sous Windows en release, pas de console : sous-système fenêtré.

#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[cfg(feature = "gui")]
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([640.0, 680.0])
            .with_min_inner_size([520.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Limiteur de débit — dossier",
        options,
        Box::new(|cc| Ok(Box::new(throttle_folder::app::ThrottleApp::new(cc)))),
    )
}

#[cfg(not(feature = "gui"))]
fn main() {
    eprintln!(
        "throttle-folder : compilé sans l'interface graphique. \
         Utilise les features par défaut (cargo build --release)."
    );
}
