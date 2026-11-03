#!/bin/sh
# Makes assets/tour.gif, the README's tour of the app: see src/core/gui/tests/tour.rs for what's in it. Needs python3,
# for the fake board and camera it starts (on ports of their own, so it leaves any already running alone). The frames
# are left in target/tour too, to look through.
set -e
cd "$(dirname "$0")/.."
cargo test --release make_tour -- --ignored --nocapture
ls -lh assets/tour.gif
