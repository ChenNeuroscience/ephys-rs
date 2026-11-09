use std::sync::atomic::{AtomicU64, Ordering};

use super::files::{POS_RECORD, SYNCED_FROM};
use super::*;
use crate::engine::{Graph, NodesEnum};
use crate::standard::themes::THEMES;
use crate::{NodeType, Nodes};

/// A folder of its own, for a test's recording.
fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let number = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ephys-rs-sync-{}-{number}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A recording, as Write LTX writes one, with `positions` a tenth of a second apart, in `dir`; its `.set` file.
fn recording(dir: &Path, positions: u32) -> PathBuf {
    let set = dir.join("session.set");
    let header = |key: &str, value: &str| (key.to_string(), value.to_string());
    LtxFile { headers: vec![header("trial_date", "Monday, 09 Nov 2026")], data: Vec::new() }.write(&set).unwrap();
    let mut data = Vec::new();
    for i in 0..positions {
        data.extend_from_slice(&(i * 9600).to_be_bytes());
        data.extend_from_slice(&[0; POS_RECORD - 4]);
    }
    let headers = vec![header("timestamp_timebase", "96000 hz"), header("num_pos_samples", &positions.to_string())];
    LtxFile { headers, data }.write(&dir.join("session.pos")).unwrap();
    let ttl = |lines: &str| LtxFile {
        headers: vec![header("ttl_format", "ttl_<line> <seconds> <state>")],
        data: lines.into(),
    };
    ttl("ttl_1 0.5 1\r\nttl_2 0.6 1\r\nttl_1 1.5 0\r\n").write(&dir.join("session.ttl")).unwrap();
    ttl("ttl_1 0.4 1\r\nttl_1 1.4 0\r\n").write(&dir.join("session.pos.ttl")).unwrap();
    set
}

fn wait_for(job: &Job) -> Result<Report, String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while matches!(job.status(), Status::Running { .. }) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    match job.status() {
        Status::Done { result, .. } => result,
        status => panic!("not done: {status:?}"),
    }
}

/// Sends a Sync Positions `paths`, as Write LTX does once it's finished, or its Run button does; what it ran.
fn send(paths: Vec<PathBuf>) -> Job {
    let mut graph = Graph::<Nodes>::default();
    let id = graph.add(Nodes::create(NodeType::SyncPositions), [0.0, 0.0]);
    let state: crate::engine::UiStateCell = std::cell::RefCell::new(Box::new(SyncState::default()));
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
    let synced = LtxFile::read(&dir.join("session.pos")).unwrap();
    assert_eq!(synced.header(SYNCED_FROM), Some("session.pos.original"));
    let stats = &report.stats;
    assert_eq!((stats.positions, stats.board_changes, stats.led_changes), (3, 2, 2), "only the board's line 1");
    assert!((stats.positions_seconds - 0.2).abs() < 1e-9);
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
    assert_eq!(report.stats.positions, 3, "the original's");
    assert!(!dir.join("session (2).pos.original").exists(), "nothing more kept");
    // Recorded again over the synced ones, they're the positions as recorded: kept, beside the first.
    recording(&dir, 4);
    let report = wait_for(&send(vec![set])).unwrap();
    assert_eq!(report.plan, Plan::Keep { original: dir.join("session (2).pos.original") });
    assert_eq!(report.stats.positions, 4);
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
    let state: crate::engine::UiStateCell = std::cell::RefCell::new(Box::new(SyncState { job, ..Default::default() }));
    let inputs: [Option<DataStream>; 0] = [];
    let mut harness = egui_kittest::Harness::builder().with_size([620.0, 380.0]).wgpu().build_ui_state(
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
    let set = recording(&dir, 3);
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
    use crate::engine::{Port, PortRole};
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
    let ltx = crate::engine::NodeState::unconnected(&Nodes::create(NodeType::WriteLtx));
    let new = crate::engine::NodeState::unconnected(&Nodes::create(NodeType::SyncPositions));
    assert_eq!(new.find(sync, &ltx, (PortRole::Control, 4)), Some(setting), "offered, to add from it");
    let raw_state = crate::engine::NodeState::unconnected(&Nodes::create(NodeType::WriteRaw));
    let from_setting = raw_state.find(raw, &new, (PortRole::Setting, 0));
    assert_eq!(from_setting, Some(port(raw, PortRole::Control, 0)), "and from it, to what can send it files");
}
