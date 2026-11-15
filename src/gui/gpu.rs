//! Drawing on the GPU from a node's window, with a shader of its own (an `egui_wgpu::Callback`, painted in egui's
//! render pass): what a render pipeline needs to know to draw there, which the app puts where callbacks find it
//! (`install`) as it starts. A window checks `available` first, and draws some other way if it isn't (a test harness
//! without the GPU, say).

use eframe::{egui, egui_wgpu, wgpu};

/// What egui's render pass draws into, for a pipeline drawing in it: its format. The app doesn't use multisampling or
/// a depth buffer, so a pipeline mustn't either.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub format: wgpu::TextureFormat,
}

/// Makes the GPU available to windows drawing with `ctx`, rendering with `render_state`.
pub fn install(ctx: &egui::Context, render_state: &egui_wgpu::RenderState) {
    render_state.renderer.write().callback_resources.insert(Target { format: render_state.target_format });
    ctx.data_mut(|d| d.insert_temp(available_id(), true));
}

/// Whether windows drawing with `ctx` can draw on the GPU (see `install`).
pub fn available(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp(available_id())).unwrap_or(false)
}

fn available_id() -> egui::Id {
    egui::Id::unique("ephys-rs gpu available")
}

/// A test harness's renderer, and its render state, to `install` once the harness is built.
#[cfg(test)]
pub fn test_renderer() -> (egui_kittest::wgpu::WgpuTestRenderer, egui_wgpu::RenderState) {
    let render_state = egui_kittest::wgpu::create_render_state(
        egui_kittest::wgpu::default_wgpu_setup(),
        egui_wgpu::RendererOptions::PREDICTABLE,
    );
    (egui_kittest::wgpu::WgpuTestRenderer::from_render_state(render_state.clone()), render_state)
}
