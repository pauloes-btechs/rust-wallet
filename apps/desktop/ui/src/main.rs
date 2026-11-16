//! rust-wallet UI: Leptos (client-side rendered) compiled to WebAssembly.
//!
//! The same bundle is the desktop app's frontend (inside Tauri, talking to
//! the Rust shell over IPC) and the hosted demo (in any browser, answering
//! from a recorded litd regtest session). See `api.rs`.

mod api;
mod app;
mod components;
mod fmt;
mod pages;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
