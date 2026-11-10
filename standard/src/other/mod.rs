//! Nodes that aren't about data as it's acquired: running a program on what's been recorded (`run_command`), or
//! putting its positions on the board's clock (`sync_positions`), say; or that go by it to control the app, as
//! starting and stopping recording by a TTL line does (`ttl_recording_control`).

pub mod run_command;
pub mod sync_positions;
pub mod ttl_recording_control;
