//! `build` — the device daemon entry point.
//!
//! The full CLI (`start`/`stop`/`status`/...) lands once the task-spine is wired.
//! For now this is a placeholder so the binary target builds.

fn main() {
    println!("build-bridge {}", env!("CARGO_PKG_VERSION"));
}
