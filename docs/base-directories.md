# Base directories

Where recordings go is chosen in a section of the main toolbar, for all the writers at once: the **base directory**.
A program offers kinds of base directory, and one is in use. That's the first, unless you choose another from the
section's settings in advanced mode. The program remembers which one, and what each has (a directory chosen, say),
with its preferences.

![The subject, date and trial base directory, being chosen](../tests/snapshots/toolbar_base_directory.png)

## Writing one

A kind of base directory is a [`BaseDirectoryType`](../src/core/base_directory.rs): a name, a description, and
`make`, which makes a `BaseDirectory` from whatever it saved last time (or nothing). The program lists them in
`Components::BASE_DIRECTORIES` ([`main.rs`](../src/main.rs)). A `BaseDirectory`:

- `toolbar_ui`: draws its section of the main toolbar, as wide as it likes. `BaseDirectoryContext::recording` says
  whether it's recording, when what decides where a recording goes shouldn't change. To match the toolbar's
  buttons, use `core::gui::widgets::toolbar_button`.
- `place`: where the next recording goes, or the one going on: a directory, and perhaps a name for the recording,
  which writers then use rather than their own.
- `recording_started` and `recording_stopped`: to count recordings, say.
- `save`: what it keeps between runs of the program, as JSON.

The standard ones are in [`base_directories.rs`](../src/standard_components/base_directories.rs):

- **A directory**: a button to choose a directory, and the directory.
- **Subject, date and trial**: a directory to keep them in, the subject (an animal's id, say), and the trial,
  shown as a path: `~/recordings / r2405 / 2026-11-03 a`. The subject is a directory; the day and trial are the
  recording's name, which writers name what they write after: Write LTX its files (`2026-11-03 a.set`...), Write
  Raw its folder. Trials are lettered a to z, then 1a to 1z, 2a, and so on, counting up after each recording, and
  starting again each day from the first not yet on disk. Before a recording the trial is the next one, which you
  can change; during one, it's the current one, in red.

## Writers

When recording's asked for, the GUI asks the base directory in use where the recording goes. Every node then gets it as it
prepares to record (`Node::prepare_to_record`), and from its passes while recording (`Pass::base_directory`). Its
window gets it too (`Context::base_directory`), to show where it'll write. The standard writers have a
[`Location`](../src/standard_components/recording/mod.rs): the base directory, a subdirectory of it, or a directory
of their own. They name the recording as the base directory says, if it does, otherwise after their prefix and the
time.
