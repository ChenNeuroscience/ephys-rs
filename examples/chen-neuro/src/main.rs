//! chen-neuro: ephys-rs as the Chen lab's LTX rig uses it, and what this repository builds.
//!
//!     python3 synthetic_streams/fake_acquisition_board.py   # in other terminals: a fake board...
//!     python3 synthetic_streams/fake_camera.py              # ...and camera
//!     cargo run --release                                   # an empty graph, or name a graph file
//!
//! Its tests are with the library's (see src/lib.rs), which can drive the whole app.

// No console window alongside the app in Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod components;

fn main() -> eframe::Result {
    ephys_rs::core::gui::run::<components::ChenNeuro>(std::env::args().nth(1).map(Into::into))
}
