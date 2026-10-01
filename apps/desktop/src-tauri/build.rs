fn main() {
    // Embedded by the release workflow (ADR-0019): a change rebuilds.
    for var in [
        "ORCHESTRATOR_UPDATER_PUBKEY",
        "ORCHESTRATOR_UPDATER_ENDPOINT",
        "ORCHESTRATOR_COMMIT",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    tauri_build::build()
}
