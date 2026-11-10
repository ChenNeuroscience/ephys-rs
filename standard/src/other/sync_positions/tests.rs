use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::files::{POS_RECORD, POST_PROCESSING_INFO, SYNCED_FROM};
use super::*;
use crate::standard::themes::THEMES;
use crate::{NodeType, Nodes};
use ephys_rs::engine::{Graph, NodesEnum};

/// A folder of its own, for a test's recording.
fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let number = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ephys-rs-sync-{}-{number}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A recording, as Write LTX writes one, with `positions` at 10 Hz, in `dir`, the camera's clock 100 ms behind the
/// board's; its `.set` file.
fn recording(dir: &Path, positions: u32) -> PathBuf {
    let set = dir.join("session.set");
    let header = |key: &str, value: &str| (key.to_string(), value.to_string());
    LtxFile { headers: vec![header("trial_date", "Monday, 09 Nov 2026")], data: Vec::new() }.write(&set).unwrap();
    let mut data = Vec::new();
    for i in 0..positions {
        data.extend_from_slice(&(i * 9600).to_be_bytes());
        data.extend_from_slice(&[0; POS_RECORD - 4]);
    }
    let headers = vec![
        header("timestamp_timebase", "96000 hz"),
        header("sample_rate", "10.000000 hz"),
        header("num_pos_samples", &positions.to_string()),
    ];
    LtxFile { headers, data }.write(&dir.join("session.pos")).unwrap();
    let ttl = |lines: &str| LtxFile {
        headers: vec![header("ttl_format", "ttl_<line> <seconds> <state>")],
        data: lines.into(),
    };
    ttl("ttl_1 0.5 1\r\nttl_2 0.6 1\r\nttl_1 1.5 0\r\n").write(&dir.join("session.ttl")).unwrap();
    ttl("ttl_1 0.4 1\r\nttl_1 1.4 0\r\n").write(&dir.join("session.pos.ttl")).unwrap();
    set
}

/// A ten-minute recording, as a rig might make one, in `dir`: positions at 50 Hz, and the timing LED pulsing every 3 to
/// 7 s, seen by the camera at the first frame after each change, its clock 5 ms behind the board's at the start and 30
/// ppm slower. The board misses one pulse, and has one more on its line; its `.set` file.
fn long_recording(dir: &Path) -> PathBuf {
    let set = recording(dir, 0);
    let (fps, seconds) = (50.0, 600.0);
    let camera = |board: f64| (board - 0.005) / (1.0 + 30e-6);
    let mut data = Vec::new();
    for frame in 0..(seconds * fps) as u32 {
        data.extend_from_slice(&((frame as f64 / fps * 96000.0).round() as u32).to_be_bytes());
        data.extend_from_slice(&[0, 100, 0, 100, 0, 120, 0, 120, 0, 9, 0, 6, 0, 0, 0, 0]);
    }
    let header = |key: &str, value: &str| (key.to_string(), value.to_string());
    let headers = vec![
        header("timestamp_timebase", "96000 hz"),
        header("sample_rate", "50.000000 hz"),
        header("num_pos_samples", &(seconds * fps).to_string()),
    ];
    LtxFile { headers, data }.write(&dir.join("session.pos")).unwrap();
    let (mut board, mut led) = (String::new(), String::new());
    let mut t = 1.0;
    for pulse in 0.. {
        t += 3.0 + 4.0 * crate::standard::plots::noise(pulse, 1).abs() as f64;
        if t > seconds - 2.0 {
            break;
        }
        for (at, state) in [(t, 1), (t + 0.5, 0)] {
            if pulse != 40 {
                board += &format!("ttl_1 {at:.6} {state}\r\n");
            }
            led += &format!("ttl_1 {:.6} {state}\r\n", (camera(at) * fps).ceil() / fps);
        }
        if pulse == 70 {
            board += &format!("ttl_1 {:.6} 1\r\nttl_1 {:.6} 0\r\n", t + 1.2, t + 1.3);
            // Well before the next.
            t += 4.0;
        }
    }
    let ttl = |data: String| LtxFile {
        headers: vec![header("ttl_format", "ttl_<line> <seconds> <state>")],
        data: data.into(),
    };
    ttl(board).write(&dir.join("session.ttl")).unwrap();
    ttl(led).write(&dir.join("session.pos.ttl")).unwrap();
    set
}

#[test]
fn a_long_recording_is_put_on_the_board_s_clock() {
    let dir = scratch("long");
    let report = wait_for(&send(vec![long_recording(&dir)])).unwrap();
    let synced = &report.synced;
    assert!(synced.warnings.is_empty(), "{:?}", synced.warnings);
    // The extra pulse's up has none within 1 s, but it makes the real pulse's up ambiguous, 1.2 s before it, as its
    // down does the real one's, and itself.
    assert_eq!((synced.pairing.board.unmatched, synced.pairing.board.ambiguous), (1, 3));
    assert_eq!((synced.pairing.led.unmatched, synced.pairing.led.ambiguous), (2, 2), "and the missed pulse's");
    let ppm = synced.drift_ppm.unwrap();
    assert!((ppm - 30.0).abs() < 5.0, "{ppm} ppm");
    // The LED's seen up to a frame late, so the offset's about half a frame less than the clocks'.
    let start = synced.knots[0].offset.unwrap();
    assert!((start - (0.005 - 0.010)).abs() < 0.003, "{start}");
    let written = LtxFile::read(&dir.join("session.pos")).unwrap();
    let records = written.data.len() / POS_RECORD;
    assert_eq!(records, synced.edits.positions);
    for (i, record) in written.data.chunks_exact(POS_RECORD).enumerate().step_by(997) {
        assert_eq!(files::timestamp(record), (i as f64 / 50.0 * 96000.0).round() as u32, "on the 50 Hz grid");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

fn wait_for(job: &Job) -> Result<Report, String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while matches!(job.status(), Status::Running { .. }) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    match job.status() {
        Status::Done { result, .. } => result.map(|report| *report),
        status => panic!("not done: {status:?}"),
    }
}

/// Sends a Sync Positions `paths`, as Write LTX does once it's finished, or its Run button does; what it ran.
fn send(paths: Vec<PathBuf>) -> Job {
    let mut graph = Graph::<Nodes>::default();
    let id = graph.add(Nodes::create(NodeType::SyncPositions), [0.0, 0.0]);
    let state: ephys_rs::engine::UiStateCell = std::cell::RefCell::new(Box::new(SyncState::default()));
    let trigger = Trigger::RunOnFiles(RunOnFiles(paths));
    graph.edit(id, &[], &THEMES[0].colours, None, Some(&state), None, false, false, None, |node, context| {
        node.on_trigger(trigger, context, &ControlTriggers::none())
    });
    state.borrow().downcast_ref::<SyncState>().unwrap().job.clone()
}

#[test]
fn the_positions_as_recorded_are_kept_and_the_synced_ones_put_in_their_place() {
    let dir = scratch("keep");
    let set = recording(&dir, 3);
    let recorded = std::fs::read(dir.join("session.pos")).unwrap();
    let others = vec![dir.join("session.1"), set.clone(), dir.join("session.egf")];
    let report = wait_for(&send(others)).unwrap();
    assert_eq!(report.plan, Plan::Keep { original: dir.join("session.pos.original") });
    assert_eq!(std::fs::read(dir.join("session.pos.original")).unwrap(), recorded, "kept as it was");
    let written = LtxFile::read(&dir.join("session.pos")).unwrap();
    assert_eq!(written.header(SYNCED_FROM), Some("session.pos.original"));
    let info = written.header(POST_PROCESSING_INFO).unwrap();
    assert!(info.contains("board's line 1: changes paired within +/-1 s, if no other was within +/-2 s"), "{info}");
    assert!(info.contains("within +/-60 s of every 30 s"), "{info}");
    assert_eq!(written.header("num_pos_samples"), Some("4"));
    assert_eq!(written.data.len(), 4 * POS_RECORD);
    let synced = &report.synced;
    assert_eq!(synced.pairing.board.changes(), 2, "only the board's line 1");
    assert_eq!(synced.pairing.pairs.len(), 2);
    // 0, 0.1 and 0.2 s by the camera are 0.1, 0.2 and 0.3 s by the board, after a slot with none.
    assert_eq!((synced.recorded, synced.edits.positions, synced.edits.added), (3, 4, 1));
    assert!(!dir.join("session.pos.writing").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn synced_again_its_from_the_original_not_itself() {
    let dir = scratch("again");
    let set = recording(&dir, 3);
    wait_for(&send(vec![set.clone()])).unwrap();
    // What's in the synced positions is no matter: they're synced again from the original.
    let mut synced = LtxFile::read(&dir.join("session.pos")).unwrap();
    synced.data.truncate(POS_RECORD);
    synced.write(&dir.join("session.pos")).unwrap();
    let report = wait_for(&send(vec![set.clone()])).unwrap();
    assert_eq!(report.plan, Plan::Resync { original: dir.join("session.pos.original") });
    assert_eq!(report.synced.recorded, 3, "the original's");
    assert!(!dir.join("session (2).pos.original").exists(), "nothing more kept");
    // Recorded again over the synced ones, they're the positions as recorded: kept, beside the first.
    recording(&dir, 4);
    let report = wait_for(&send(vec![set])).unwrap();
    assert_eq!(report.plan, Plan::Keep { original: dir.join("session (2).pos.original") });
    assert_eq!(report.synced.recorded, 4);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn it_says_what_it_cant_sync() {
    let dir = scratch("missing");
    let set = recording(&dir, 3);
    std::fs::remove_file(dir.join("session.pos.ttl")).unwrap();
    assert_eq!(wait_for(&send(vec![set])).unwrap_err(), "There's no session.pos.ttl");
    assert!(!dir.join("session.pos.original").exists(), "nothing touched");
    assert_eq!(
        wait_for(&send(vec![dir.join("session.pos")])).unwrap_err(),
        "There's no .set file among the files sent"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn what_cant_be_synced_is_left_as_it_is() {
    let dir = scratch("unsynced");
    let set = recording(&dir, 3);
    let recorded = std::fs::read(dir.join("session.pos")).unwrap();
    let header = vec![("ttl_format".to_string(), "ttl_<line> <seconds> <state>".to_string())];
    LtxFile { headers: header.clone(), data: b"ttl_3 0.5 1\r\n".to_vec() }.write(&dir.join("session.ttl")).unwrap();
    let problem = wait_for(&send(vec![set.clone()])).unwrap_err();
    assert_eq!(problem, "The board's line 1 never changed: is the timing LED on another?");
    assert!(!dir.join("session.pos.original").exists(), "nothing kept, as nothing's changed");
    assert_eq!(std::fs::read(dir.join("session.pos")).unwrap(), recorded);
    // A second later than the LED's: too far to pair.
    LtxFile { headers: header, data: b"ttl_1 1.5 1\r\nttl_1 2.5 0\r\n".to_vec() }
        .write(&dir.join("session.ttl"))
        .unwrap();
    let problem = wait_for(&send(vec![set])).unwrap_err();
    assert!(
        problem.starts_with(
            "None of the timing LED's 2 changes paired with the board's line 1's 2 (0 ambiguous, 2 unmatched)"
        ),
        "{problem}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

/// A harness showing a Sync Positions' window, with `/recordings/session.set` picked (wherever it is, for the
/// snapshots), having synced `ran`'s recording, if given.
fn window_harness(ran: Option<&Path>) -> egui_kittest::Harness<'static, Graph<Nodes>> {
    let mut graph = Graph::<Nodes>::default();
    let id = graph.add(Nodes::create(NodeType::SyncPositions), [0.0, 0.0]);
    if let Some(Nodes::SyncPositions(node)) = graph.get_mut(id).map(|e| &mut e.node) {
        node.settings.set_file = PathBuf::from("/recordings/session.set");
    }
    let job = match ran {
        Some(set) => {
            let job = send(vec![set.to_path_buf()]);
            wait_for(&job).unwrap();
            job
        }
        None => Job::default(),
    };
    let state: ephys_rs::engine::UiStateCell = std::cell::RefCell::new(Box::new(SyncState::with(job)));
    let inputs: [Option<DataStream>; 0] = [];
    let mut harness = egui_kittest::Harness::builder().with_size([680.0, 780.0]).wgpu().build_ui_state(
        move |ui, graph: &mut Graph<Nodes>| {
            graph.edit(
                id,
                &inputs,
                &THEMES[0].colours,
                None,
                Some(&state),
                None,
                false,
                false,
                None,
                |node, context| node.gui_ui(ui, context),
            );
        },
        graph,
    );
    THEMES[0].apply(&harness.ctx);
    harness.run_steps(3);
    harness
}

#[test]
fn window() {
    use egui_kittest::kittest::Queryable;
    let mut harness = window_harness(None);
    assert!(harness.query_by_label("session.pos · missing").is_some(), "the files it'll use, if they're there");
    harness.snapshot("sync_positions");
}

/// Synced, then synced again, from the original: see tests/snapshots/.
#[test]
fn it_shows_what_it_did() {
    use egui_kittest::kittest::Queryable;
    let dir = scratch("window");
    let set = long_recording(&dir);
    let mut results = egui_kittest::SnapshotResults::new();
    for again in [false, true] {
        let mut harness = window_harness(Some(&set));
        assert!(harness.query_by_label("✔ Synced session.pos").is_some());
        assert!(harness.query_by_label("session.pos.ttl").is_some(), "the files it used");
        if again {
            assert!(harness.query_by_label_contains("had been synced before").is_some(), "from the original");
            results.add(harness.try_snapshot("sync_positions_synced_again"));
        } else {
            assert!(harness.query_by_label_contains("kept as session.pos.original").is_some());
            results.add(harness.try_snapshot("sync_positions_synced"));
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

/// What sends files (a writer, once it's finished, or a Run Command, once it's run) connects to it, though it's not a
/// command: what's sent is all that passes between them.
#[test]
fn what_sends_files_connects_to_it() {
    use ephys_rs::engine::{Port, PortRole};
    let mut graph = Graph::<Nodes>::default();
    let [sync, writer, raw, command] =
        [NodeType::SyncPositions, NodeType::WriteLtx, NodeType::WriteRaw, NodeType::RunCommand]
            .map(|node_type| graph.add(Nodes::create(node_type), [0.0, 0.0]));
    let port = |node, role, index| Port { node, role, index };
    let setting = port(sync, PortRole::Setting, 0);
    assert!(graph.check(port(writer, PortRole::Control, 4), setting).is_ok(), "Write LTX, after recording");
    assert!(graph.check(setting, port(raw, PortRole::Control, 0)).is_ok(), "Write Raw, either way round");
    assert!(graph.check(port(command, PortRole::Control, 0), setting).is_ok(), "Run Command, after running");
    assert!(graph.check(port(writer, PortRole::Control, 0), setting).is_err(), "not what's recorded with positions");
    let command_setting = port(command, PortRole::Setting, 0);
    assert!(graph.check(port(writer, PortRole::Control, 4), command_setting).is_ok(), "and a command, still");
    assert!(graph.check(port(writer, PortRole::Control, 0), command_setting).is_err());
    let edge = graph.connect(port(writer, PortRole::Control, 4), setting).unwrap();
    graph.prune_edges();
    assert!(graph.edges().contains(&edge), "kept, as it fits");
    let ltx = ephys_rs::engine::NodeState::unconnected(&Nodes::create(NodeType::WriteLtx));
    let new = ephys_rs::engine::NodeState::unconnected(&Nodes::create(NodeType::SyncPositions));
    assert_eq!(new.find(sync, &ltx, (PortRole::Control, 4)), Some(setting), "offered, to add from it");
    let raw_state = ephys_rs::engine::NodeState::unconnected(&Nodes::create(NodeType::WriteRaw));
    let from_setting = raw_state.find(raw, &new, (PortRole::Setting, 0));
    assert_eq!(from_setting, Some(port(raw, PortRole::Control, 0)), "and from it, to what can send it files");
}
