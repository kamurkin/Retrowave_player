mod app;
mod media;
mod playback;
mod playlist;
mod skin;

fn main() -> eframe::Result<()> {
    playback::configure_vlc_plugin_path();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([430.0, 250.0])
            .with_min_inner_size([390.0, 220.0])
            .with_title("RetroWave Player"),
        ..Default::default()
    };
    eframe::run_native(
        "RetroWave Player",
        options,
        Box::new(|context| Ok(Box::new(app::RetroWaveApp::new(context)))),
    )
}
