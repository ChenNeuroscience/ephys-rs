//! Makes the tour in the README, `assets/tour.gif`: the app's main features, a frame each, with a caption. Not a test,
//! but it's only here that the app can be driven and drawn without a window: `scripts/make_tour.sh` runs it.
//!
//! It runs the real thing, on the synthetic data, from its own fake board and camera (on ports of their own, so it
//! doesn't disturb any already running). Nothing's written: recording's shown with the writer disabled.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use eframe::egui::{Color32, FontId, Id, LayerId, Order, Rect};
use image::imageops::{self, FilterType};
use image::{Delay, Frame, RgbaImage};

use super::*;
use crate::core::annotation::Annotation;
use crate::standard_components::annotations::heading::Heading;
use crate::standard_components::annotations::sticky_note::StickyNote;
use crate::standard_components::ephys::board::Source as BoardSource;
use crate::standard_components::tracking::Crop;
use crate::standard_components::tracking::camera::Source as CameraSource;

/// The window, in points, drawn at `SCALE` pixels a point (for smooth text, once it's scaled to `WIDTH`).
const SIZE: egui::Vec2 = egui::vec2(1280.0, 780.0);
const SCALE: f32 = 1.5;
/// The GIF's width, in pixels.
const WIDTH: u32 = 960;
/// How long each frame's shown.
const SHOWN_MS: u32 = 3000;

/// What's written over a frame, and the part of the window the frame shows.
#[derive(Clone)]
struct Caption {
    text: String,
    area: Rect,
}

fn caption_id() -> Id {
    Id::NULL.with("tour caption")
}

/// The caption, if there is one, along the bottom of its area, the same size whatever the area's size.
fn caption_ui(ui: &egui::Ui) {
    let ctx = ui.ctx();
    let Some(caption) = ctx.data(|d| d.get_temp::<Caption>(caption_id())) else { return };
    let scale = caption.area.width() / SIZE.x;
    let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, caption_id()));
    let font = FontId::proportional(32.0 * scale);
    let text = painter.layout_no_wrap(caption.text, font, Color32::WHITE);
    let pad = egui::vec2(22.0, 11.0) * scale;
    let centre = caption.area.center_bottom() - egui::vec2(0.0, 62.0 * scale);
    let back = Rect::from_center_size(centre, text.size() + 2.0 * pad);
    painter.rect_filled(back, 10.0 * scale, Color32::from_black_alpha(225));
    painter.galley(back.min + pad, text, Color32::WHITE);
}

/// A fake source, stopped when it's dropped.
struct Fake(Child);

impl Fake {
    /// Runs `script`, from `synthetic_streams`, on a free port. Returns it, and the address to connect to.
    fn start(script: &str) -> (Self, String) {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let script = format!("{}/synthetic_streams/{script}", env!("CARGO_MANIFEST_DIR"));
        let child = Command::new("python3")
            .args([&script, "--port", &port.to_string()])
            .stdout(Stdio::null())
            .spawn()
            .expect("run python3");
        (Fake(child), format!("127.0.0.1:{port}"))
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.0.kill().ok();
        self.0.wait().ok();
    }
}

/// The README's tour as it's made: the app, and the frames taken so far.
struct Tour {
    harness: Harness<'static, TestApp>,
    frames: Vec<RgbaImage>,
}

impl Tour {
    /// Lets `seconds` pass, the GUI keeping up with the run.
    fn wait(&mut self, seconds: f32) {
        for _ in 0..(seconds * 20.0) as u32 {
            self.harness.run_steps(1);
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn app(&mut self) -> &mut TestApp {
        self.harness.state_mut()
    }

    fn id(&self, label: &str) -> NodeId {
        node_named(self.harness.state(), label)
    }

    /// Moves the pointer out of the way.
    fn park_pointer(&mut self) {
        self.harness.event(egui::Event::PointerMoved(egui::pos2(1200.0, 12.0)));
        self.harness.run_steps(3);
    }

    /// Shows `id`'s window, and nothing else.
    fn only(&mut self, id: NodeId) {
        let mode = self.app().layout.mode;
        let layout = &mut self.app().layout;
        *layout = Layout::default();
        layout.mode = mode;
        layout.open(id, None);
        layout.hide(Pane::Home);
        self.park_pointer();
    }

    /// Adds a frame of the whole window.
    fn shot(&mut self, caption: &str) {
        self.shot_of(caption, Rect::from_min_size(egui::Pos2::ZERO, SIZE));
    }

    /// Adds a frame of `area` of the window (of the window's shape), with `caption`.
    fn shot_of(&mut self, caption: &str, area: Rect) {
        let caption = Caption { text: caption.into(), area };
        self.harness.ctx.data_mut(|d| d.insert_temp(caption_id(), caption));
        self.harness.run_steps(3);
        let mut frame = self.harness.render().expect("drawn");
        self.harness.ctx.data_mut(|d| d.remove::<Caption>(caption_id()));
        let (x, y) = ((area.min.x * SCALE) as u32, (area.min.y * SCALE) as u32);
        let (w, h) = ((area.width() * SCALE) as u32, (area.height() * SCALE) as u32);
        let frame = imageops::crop(&mut frame, x, y, w, h).to_image();
        let height = WIDTH * h / w;
        self.frames.push(imageops::resize(&frame, WIDTH, height, FilterType::Lanczos3));
    }

    /// Writes the frames as the GIF, and as PNGs in `target/tour` to look through.
    fn write(self) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let pngs = root.join("target/tour");
        std::fs::create_dir_all(&pngs).unwrap();
        for (i, frame) in self.frames.iter().enumerate() {
            frame.save(pngs.join(format!("{:02}.png", i + 1))).unwrap();
        }
        let file = std::fs::File::create(root.join("assets/tour.gif")).unwrap();
        let mut gif = image::codecs::gif::GifEncoder::new_with_speed(file, 10);
        gif.set_repeat(image::codecs::gif::Repeat::Infinite).unwrap();
        let delay = Delay::from_numer_denom_ms(SHOWN_MS, 1);
        gif.encode_frames(self.frames.into_iter().map(|f| Frame::from_parts(f, 0, 0, delay))).unwrap();
    }
}

#[test]
#[ignore = "makes the README's tour: see scripts/make_tour.sh"]
fn make_tour() {
    let (_board, board) = Fake::start("fake_acquisition_board.py");
    let (_camera, camera) = Fake::start("fake_camera.py");
    let app = TestApp::open(None, THEMES)
        .with_quick_actions(crate::components::quick_actions::QUICK_ACTIONS)
        .with_base_directories(crate::components::BASE_DIRECTORIES);
    let harness = Harness::builder().with_size(SIZE).with_pixels_per_point(SCALE).wgpu().build_ui_state(
        |ui, app: &mut TestApp| {
            app.show(ui);
            caption_ui(ui);
        },
        app,
    );
    let mut tour = Tour { harness, frames: Vec::new() };
    let ctx = tour.harness.ctx.clone();
    let button = egui::accesskit::Role::Button;
    tour.harness.run_steps(3);
    tour.shot("Start from quick actions");

    // The standard graph, on the fake sources.
    tour.harness.get_by_role_and_label(button, "Standard LTX graph v0.5").click();
    tour.harness.run_steps(3);
    tour.app().layout.show(Pane::Home);
    tour.harness.run_steps(3);
    tour.harness.get_by_role_and_label(button, "Use synthetic data").click();
    tour.harness.run_steps(3);
    let tracker = tour.id("Tracker View");
    tour.app().layout.show(Pane::Node(tracker));
    let ids: Vec<NodeId> = tour.app().graph.iter().map(|(id, _)| id).collect();
    for id in ids {
        match tour.app().graph.get_mut(id).map(|e| &mut e.node) {
            Some(Nodes::AcquisitionBoard(b)) => b.source = BoardSource::Fake { address: board.clone() },
            Some(Nodes::Camera(c)) => c.source = CameraSource::Fake { address: camera.clone() },
            // The arena, so it's the mouse that's tracked, not the laptop beside it.
            Some(Nodes::PositionDetector(d)) => {
                d.detection.crop = Crop::Square { center: [320.0, 240.0], size: 440.0, angle: 0.0 };
            }
            _ => {}
        }
    }
    let saved = serde_json::json!({
        "chosen": "Subject, date and trial",
        "types": { "Subject, date and trial": { "directory": "~/recordings", "subject": "r2405" } },
    });
    let types = tour.app().base_directories.types();
    tour.app().base_directories = BaseDirectories::new(types, Some(&saved));
    tour.harness.run_steps(3);
    tour.wait(1.0);
    tour.app().perform(&ctx, Action::Play);
    tour.wait(5.0);
    tour.harness.get_all_by_label("Run").last().expect("the Spike Viewer's Run").click();
    tour.harness.get_all_by_label("Auto detect").last().expect("the LFP Viewer's").click();
    tour.wait(12.0);
    tour.park_pointer();
    tour.shot("Spikes, LFP and tracking, live");

    // The viewers.
    let layout = tour.app().layout.clone();
    let (spikes, lfp) = (tour.id("Spike Viewer"), tour.id("LFP Viewer"));
    tour.only(spikes);
    tour.wait(2.0);
    tour.shot("Tetrode spikes, overlaid live");
    tour.harness.event(egui::Event::PointerMoved(egui::pos2(80.0, 190.0)));
    tour.harness.run_steps(3);
    tour.harness.key_press(Key::T);
    tour.harness.run_steps(3);
    tour.shot("Set thresholds and gains here");
    tour.harness.key_press(Key::Escape);
    tour.harness.run_steps(3);
    tour.only(lfp);
    tour.wait(2.0);
    tour.shot("LFP traces and theta spectra");
    tour.harness.event(egui::Event::PointerMoved(egui::pos2(67.0, 190.0)));
    tour.wait(1.0);
    tour.shot("Pick reference channels per row");
    let ttl = tour.id("TTL Viewer");
    tour.only(ttl);
    tour.wait(2.0);
    tour.shot("Digital inputs at a glance");
    tour.only(tracker);
    tour.wait(2.0);
    tour.shot("LED tracking from video");

    // Recording, without writing anything.
    tour.app().layout = layout;
    tour.park_pointer();
    tour.shot_of("Files by subject, date and trial", Rect::from_min_size(egui::pos2(400.0, 30.0), SIZE * 0.5));
    let writer = tour.id("Write LTX");
    tour.app().graph.get_mut(writer).unwrap().disabled = true;
    tour.harness.run_steps(3);
    let place = tour.app().base_directories.place().unwrap();
    tour.app().set_recording(Some(Arc::new(engine::Recording::default())));
    tour.app().base_directories.current().unwrap().recording_started(&place);
    tour.wait(3.0);
    tour.shot("Record to LTX format");
    tour.app().set_recording(None);
    tour.app().graph.get_mut(writer).unwrap().disabled = false;
    tour.wait(1.0);

    // What's going on inside.
    let mode = tour.app().layout.mode;
    tour.app().layout = Layout::default();
    tour.app().layout.mode = mode;
    tour.app().layout.toggle_dev_tools();
    tour.app().layout.hide(Pane::Home);
    tour.wait(3.0);
    tour.harness.get_by_label("Performance").click();
    tour.harness.run_steps(3);
    tour.harness.get_by_label("Console").click();
    tour.harness.run_steps(3);
    tour.park_pointer();
    tour.wait(1.0);
    tour.shot("Every thread and buffer, visible");
    tour.harness.get_by_label("Threads").click();
    tour.harness.run_steps(3);
    tour.harness.get_by_label("Performance").click();
    tour.harness.run_steps(3);
    tour.park_pointer();
    tour.wait(1.0);
    tour.shot("Every pass and frame, timed");

    // The graph.
    tour.app().layout = Layout::default();
    tour.app().layout.mode = Mode::Advanced;
    tour.app().canvas.frame_all();
    tour.harness.run_steps(3);
    tour.shot("Build any graph you like");
    let start = on_node(&tour.harness, "Acquisition Board", FIRST_OUTPUT);
    pointer(&mut tour.harness, start, Some(true));
    pointer(&mut tour.harness, start + egui::vec2(80.0, -80.0), None);
    pointer(&mut tour.harness, start + egui::vec2(150.0, -170.0), None);
    tour.shot("Drag wires between nodes");
    pointer(&mut tour.harness, start + egui::vec2(150.0, -170.0), Some(false));
    tour.shot("Add what fits, right there");
    tour.app().menu = None;
    tour.park_pointer();
    let detector = tour.id("Spike Detector");
    tour.app().layout.open(detector, Some(Pane::Home));
    tour.app().layout.float(detector);
    tour.app().selection.only(detector);
    tour.wait(1.0);
    tour.shot("Settings beside the graph");
    let app = tour.app();
    let copied = paste_settings::copy(&app.graph, &[detector], &app.formats);
    if let Some(Nodes::SpikeDetector(d)) = tour.app().graph.get_mut(detector).map(|e| &mut e.node) {
        d.thresholds.set(0, 70.0);
    }
    let paste = paste_settings::PasteSettings::new(&copied, &tour.harness.state().graph, &tour.harness.state().formats)
        .unwrap();
    tour.app().paste = Some(paste);
    tour.harness.run_steps(3);
    tour.shot("Review settings before pasting");
    tour.app().paste = None;
    tour.app().layout.close(detector);
    tour.app().selection.clear();

    // Annotations, and a theme.
    let pos = tour.app().graph.get(detector).unwrap().pos;
    let heading = AnnotationTypes::Heading(Heading { text: "Tetrodes in CA1".into(), ..Default::default() });
    let note = StickyNote { text: "Check the reference after surgery".into(), ..Default::default() };
    let notes = [
        (heading, [pos[0], pos[1] - 110.0], [320.0, 50.0]),
        (AnnotationTypes::StickyNote(note), [pos[0] + 240.0, pos[1] - 200.0], [190.0, 120.0]),
    ];
    for (kind, pos, size) in notes {
        tour.app().annotations.add(Annotation { pos, size, kind });
    }
    tour.app().canvas.frame_all();
    tour.harness.run_steps(3);
    tour.shot("Annotate your rig");
    tour.app().theme = 1;
    THEMES[1].apply(&ctx);
    tour.harness.run_steps(3);
    tour.shot("Dark and light themes");
    tour.app().theme = 0;
    THEMES[0].apply(&ctx);
    let board_id = tour.id("Acquisition Board");
    tour.only(board_id);
    tour.wait(1.0);
    tour.shot("Open Ephys and Intan boards");

    tour.app().perform(&ctx, Action::Play);
    tour.harness.run_steps(3);
    println!("{} frames", tour.frames.len());
    tour.write();
}
