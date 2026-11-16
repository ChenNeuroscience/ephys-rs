//! A node's window can draw on the GPU with a shader of its own (an `egui_wgpu::Callback`, painted in egui's render
//! pass). A render pipeline needs to know some things to draw there, which the app puts where callbacks find them
//! (`install`) as it starts. A window checks `available` first, and draws some other way if the GPU isn't (a test
//! harness without it, say).

use eframe::{egui, egui_wgpu, wgpu};

/// A pipeline drawing in egui's render pass needs to know what the pass draws into: its format. The app doesn't use
/// multisampling or a depth buffer, so a pipeline mustn't either.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub format: wgpu::TextureFormat,
}

/// As it starts, the app makes the GPU available to windows drawing with `ctx`, rendering with `render_state`.
pub fn install(ctx: &egui::Context, render_state: &egui_wgpu::RenderState) {
    render_state.renderer.write().callback_resources.insert(Target { format: render_state.target_format });
    ctx.data_mut(|d| d.insert_temp(available_id(), true));
}

/// A window checks whether it can draw on the GPU, drawing with `ctx` (see `install`), with this.
pub fn available(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp(available_id())).unwrap_or(false)
}

fn available_id() -> egui::Id {
    egui::Id::unique("ephys-rs gpu available")
}

/// Test harnesses render with this renderer, and its render state, which they `install` once built.
#[cfg(test)]
pub fn test_renderer() -> (egui_kittest::wgpu::WgpuTestRenderer, egui_wgpu::RenderState) {
    let render_state = egui_kittest::wgpu::create_render_state(
        egui_kittest::wgpu::default_wgpu_setup(),
        egui_wgpu::RendererOptions::PREDICTABLE,
    );
    (egui_kittest::wgpu::WgpuTestRenderer::from_render_state(render_state.clone()), render_state)
}
