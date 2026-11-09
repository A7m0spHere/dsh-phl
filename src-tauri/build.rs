fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .plugin(
                "model-metadata",
                tauri_build::InlinedPlugin::new().commands(&["enrich_model_metadata"]),
            )
            .plugin(
                "process-monitor",
                tauri_build::InlinedPlugin::new().commands(&["sample_instance_processes"]),
            ),
    )
    .expect("failed to build Tauri application")
}
