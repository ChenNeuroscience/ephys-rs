//! ephys-rs: electrophysiology acquisition, processing and recording, built around a node graph.
//!
//!     cargo run --release                  # starts with an example graph
//!     cargo run --release -- rig.json      # opens a graph file (or starts one there)

// No console window alongside the app in Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod edges;
mod engine;
mod gui;
mod nodes;
mod theme;

fn main() -> eframe::Result {
    gui::run(std::env::args().nth(1).map(Into::into), nodes::example)
}
