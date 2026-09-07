fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().plugin(
        "model-metadata",
        tauri_build::InlinedPlugin::new().commands(&["enrich_model_metadata"]),
    ))
    .expect("failed to build Tauri application")
}
