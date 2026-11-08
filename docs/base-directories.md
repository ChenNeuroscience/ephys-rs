# Base directories

Where recordings go is chosen in a section of the main toolbar, for all the writers at once: the **base directory**.
A program offers kinds of base directory, and one is in use. That's the first, unless you choose another from the
section's settings in advanced mode. Which one, and what each has (a directory chosen, say), is kept with the
program's preferences and in saved files, so with each recording too. Opening a file brings back what it has (one
saved before files kept this leaves things as they are), and changing it counts as a change to the file.

![The subject, date and trial base directory, being chosen](../tests/snapshots/toolbar_base_directory.png)

Beside it, the toolbar shows how full the disk it's on is: a bar, and what's free, amber from 90% full and red from
97%, with the size in its tooltip. That's whichever base directory's in use, and needs nothing from it: only
`place`. (A writer with a directory of its own could be on another disk, which this doesn't show.) It's looked at
once a minute and when the directory changes, on a thread of its own, as a disk asleep or on the network can take a
while to answer ([`disk_space.rs`](../src/core/gui/disk_space.rs)).

## Writing one

A kind of base directory is a [`BaseDirectoryType`](../src/core/base_directory.rs): a name, a description, and
`make`, which makes a `BaseDirectory` from whatever it saved last time (or nothing). The program lists them in
`Components::BASE_DIRECTORIES` ([chen-neuro's](../examples/chen-neuro/src/components/mod.rs), say). A `BaseDirectory`:

- `toolbar_ui`: draws its section of the main toolbar, as wide as it likes. `BaseDirectoryContext::recording` says
  whether it's recording, when what decides where a recording goes shouldn't change. To match the toolbar's
  buttons, use `core::gui::widgets::toolbar_button`.
- `place`: where the next recording goes, or the one going on: a directory, and perhaps a prefix, which writers then
  use rather than their own. It's the whole of the name of what's written, but the extension, so it should be
  different for each recording: writers don't check, and would write over the last.
- `recording_started` and `recording_stopped`: `recording_started` is told where the recording went, as what's
  next may have changed by then (the writers' files may already be there, say).
- `save`: what it keeps, between runs of the program and in saved files, as JSON. Only what's chosen: a saved file
  counts as changed when this does, so a count of recordings, say, would mark it changed after each one.

The standard one, in [`base_directories.rs`](../src/standard_components/base_directories.rs), is **A directory**: a
button to open it, the directory, clicked to choose another, and a file prefix, if you want one, clicked to change it
in place. Recordings are named after the prefix and when they start (`mouse1_2026-11-03_143005`), so one doesn't write
over the last; with no prefix, the writers name them. The pieces are there for others to use: `directory_ui`,
`path_chip` (a pale piece of a path, saying what it is right below it when hovered), `PathEdit` (typing in place of a
chip), `name_piece` (text made safe for a file's name), `path_separator` (this system's) and `open_button` (opening a
directory, or as much of it as there is, in the file manager).

chen-neuro adds another, in [`trials.rs`](../examples/chen-neuro/src/components/trials.rs):

- **Subject, date and trial**: a directory to keep them in, the subject (an animal's id, say), the day and the
  trial, shown as a flat path: `~/recordings/r2405/2026-11-03/r2405 2026-11-03 a`. The subject and the day are
  directories; the prefix mirrors them, with the trial: Write LTX starts its files with it
  (`r2405 2026-11-03 a.set`...), and Write Raw names its folder it. Trials are lettered a to z, then 1a to 1z, 2a, and so
  on. The next is the one after the last on disk in the day's directory, so an empty directory, or a new day, starts
  at a, gaps aren't filled, and trials stay in the order they were made. Nothing's counted or stored: what's on disk
  is looked at once a second at most, and after each recording. Before a recording the trial is the next one; during one, it's the current one, in red. The
  directory, subject and trial are clicked to change them: in place, for the subject and trial (Enter or clicking
  elsewhere applies it, Esc cancels, up and down step a trial), and a trial changed is for the next recording. The
  subject's two pieces are the same subject: hovering one lights up the other, and typing in one changes both.

## Writers

When recording's asked for, the GUI asks the base directory in use where the recording goes. Every node then gets it as it
prepares to record (`Node::prepare_to_record`), and from its passes while recording (`Pass::base_directory`). Its
window gets it too (`UiContext::base_directory`), to show where it'll write. The standard writers have a
[`Location`](../src/standard_components/recording/mod.rs): the base directory, a subdirectory of it, or a directory
of their own. They use the base directory's prefix, if it has one, otherwise their own and the time. A prefix is only
text: what it's the start of is up to the writer, so Write LTX's files and Write Raw's folder share it, and the
toolbar calls it a file prefix as that's the usual case.

The settings are kept with each recording too: as it starts, the graph (as File › Save writes it) is saved in the
base directory, as `<prefix>.ephys-rs.json`, with the base directory's prefix if it has one (beside Write LTX's files,
or Write Raw's folder), otherwise named after when it started.
