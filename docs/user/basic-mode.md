# Basic mode

Basic mode is for running experiments without thinking about the graph. It's where ephys-rs starts.

![The basic view, with nothing set up yet](../../tests/snapshots/basic_empty.png)

## Starting out

With nothing set up, the home pane offers your recent files, and the *quick actions* your build has. A quick action
is a ready-made change to the setup. Chen-neuro's "Standard LTX graph" adds a whole tetrode rig in one click: an
acquisition board and a camera, with spikes, EEG, digital inputs and positions viewed and recorded, and each
recording's positions put on the board's clock once it's written; and an Arduino, setting its pin 8 high while
acquiring, once its port is chosen in its settings (it never guesses which Arduino). Your lab's build will have its
own.

![Recent files in the basic view](../../tests/snapshots/basic_recent_files.png)

## Once it's set up

![The basic view after a quick action: the viewers, beside the windows they opened](../../tests/snapshots/basic_after_quick_actions.png)

Once there's a setup, the home pane lists its components, each as a large button: by default the viewers (green).
Click one to open its window beside the home pane. A quick action usually opens the ones you need for you:

![The windows a quick action opened](../../tests/snapshots/standard_ltx_layout.png)

Below them are the quick actions that still apply, which change the setup from here. Chen-neuro's "Use VR computer",
for example, swaps the camera for positions from a VR computer. Each can be undone (Ctrl/⌘ Z) in one step.

You can make your own quick actions, and choose which are offered, in Settings › Quick actions. **New from the
graph** makes one from the setup as it is (or from the nodes selected). The editor shows its nodes as a graph, much
as advanced mode does, with a note under each saying what happens if it isn't in your setup, and what it is in your setup now; select one to change that, and which of its settings to set. Quick actions
are files, so you can share them: copy one (the copy button) and paste it in on another computer, or copy the files
themselves (the folder icon opens their folder). The ones the app comes with are files too, so you can change or
delete them like your own.

![After using a quick action](../../tests/snapshots/basic_using_vr.png)

Then use **Play** and **Record** in the toolbar, as in [a typical session](README.md#a-typical-session).

## What basic mode leaves out

In basic mode, windows don't have the buttons that change the graph (deleting a node, disabling it, or choosing what
shows in basic mode), so the setup can't be broken by accident. Settings can still be adjusted, and copied and
pasted. Everything else is in [advanced mode](graph.md), a click away on the toolbar's graph button (Ctrl/⌘ 1).

To have a node that isn't a viewer show in basic mode (an Alarm Processor, say, to adjust its alarms), right-click it
on the graph and tick **Show in basic view**.
