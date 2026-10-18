//! The computer: what a "GPU acceleration doesn't work" report needs first.

use serde_json::json;

/// Records the `system` event: the OS, processor, memory, the build's
/// features, and every device llama.cpp finds (loading its backends, as a
/// cleanup model's first load would).
pub fn record_system() {
    std::thread::Builder::new()
        .name("bd-telemetry-system".into())
        .spawn(|| {
            let sys = sysinfo::System::new_with_specifics(
                sysinfo::RefreshKind::nothing()
                    .with_memory(sysinfo::MemoryRefreshKind::nothing().with_ram())
                    .with_cpu(sysinfo::CpuRefreshKind::nothing()),
            );
            let cpu = sys.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default();
            let backend = bd_cleanup::shared_backend().err().map(|e| e.to_string());
            let gpus: Vec<_> = bd_cleanup::devices()
                .into_iter()
                .map(|d| json!({"name": d.name, "description": d.description, "backend": d.backend, "free_mb": d.free_mb, "total_mb": d.total_mb}))
                .collect();
            let mut features = Vec::new();
            for (on, name) in [
                (cfg!(feature = "vulkan"), "vulkan"),
                (cfg!(feature = "vulkan-dl"), "vulkan-dl"),
                (cfg!(feature = "pipewire"), "pipewire"),
                (cfg!(feature = "nemotron-hotwords"), "nemotron-hotwords"),
            ] {
                if on {
                    features.push(name);
                }
            }
            super::record("system", None, || {
                json!({
                    "os_version": sysinfo::System::long_os_version(),
                    "arch": std::env::consts::ARCH,
                    "cpu": cpu,
                    "cores": sys.cpus().len(),
                    "ram_mb": sys.total_memory() >> 20,
                    "features": features,
                    "gpus": gpus,
                    "llama_backend_error": backend,
                })
            });
        })
        .ok();
}
