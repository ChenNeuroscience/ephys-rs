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
`Components::BASE_DIRECTORIES` (the [example's](../example/src/components/mod.rs), say). A `BaseDirectory`:

- `toolbar_ui`: draws its section of the main toolbar, as wide as it likes. `BaseDirectoryContext::recording` says
  whether it's recording, when what decides where a recording goes shouldn't change. To match the toolbar's
  buttons, use `core::gui::widgets::toolbar_button`.
- `place`: where the next recording goes, or the one going on: a directory, and perhaps a name for the recording,
  which writers then use rather than their own.
- `recording_started` and `recording_stopped`: `recording_started` is told where the recording went, as what's
  next may have changed by then (the writers' files may already be there, say).
- `save`: what it keeps, between runs of the program and in saved files, as JSON. Only what's chosen: a saved file
  counts as changed when this does, so a count of recordings, say, would mark it changed after each one.

The standard one, in [`base_directories.rs`](../src/standard_components/base_directories.rs), is **A directory**: a
button to open it, and the directory, clicked to choose another. The pieces are there for others to use:
`directory_ui`, `path_chip` (a pale piece of a path, saying what it is right below it when hovered), `path_separator`
(this system's) and `open_button` (opening a directory, or as much of it as there is, in the file manager).

The example adds another, in [`trials.rs`](../example/src/components/trials.rs):

- **Subject, date and trial**: a directory to keep them in, the subject (an animal's id, say), the day and the
  trial, shown as a flat path: `~/recordings/r2405/2026-11-03/2026-11-03 a`. The subject and the day are
  directories; the day and trial, again, are the recording's name, which writers name what they write after: Write
  LTX its files (`2026-11-03 a.set`...), Write Raw its folder. Trials are lettered a to z, then 1a to 1z, 2a, and so
  on. The next is the one after the last on disk in the day's directory, so an empty directory, or a new day, starts
  at a, gaps aren't filled, and trials stay in the order they were made. Nothing's counted or stored: what's on disk
  is looked at once a second at most, and after each recording. Before a recording the trial is the next one; during one, it's the current one, in red. The
  directory, subject and trial are clicked to change them: in place, for the subject and trial (Enter or clicking
  elsewhere applies it, Esc cancels, up and down step a trial), and a trial changed is for the next recording.

## Writers

When recording's asked for, the GUI asks the base directory in use where the recording goes. Every node then gets it as it
prepares to record (`Node::prepare_to_record`), and from its passes while recording (`Pass::base_directory`). Its
window gets it too (`Context::base_directory`), to show where it'll write. The standard writers have a
[`Location`](../src/standard_components/recording/mod.rs): the base directory, a subdirectory of it, or a directory
of their own. They name the recording as the base directory says, if it does, otherwise after their prefix and the
time.

The settings are kept with each recording too: as it starts, the graph (as File › Save writes it) is saved in the
base directory, as `<name>.ephys-rs.json`, named as the recording if the base directory names it (beside Write
LTX's files of that name, or Write Raw's folder), otherwise after when it started.
