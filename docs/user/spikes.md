# Spikes

Spikes are detected in groups of channels (tetrodes, say) by a Spike Detector, scaled for recording by a Spike Gain,
and viewed, and tuned, in a Spike Viewer. A typical chain is: Acquisition Board → Reference → Band-pass Filter
(600–6000 Hz) → Spike Detector → Spike Gain → Write LTX, with a Spike Viewer after the gain.

Each component starts with what kind of node it is and its ports: data ports `like this`, settings ports marked ◇.

## Spike Detector

> **Processing.** In: `continuous` (filtered, µV). Out: `spikes`. ◇ `thresholds`.

![A Spike Detector's settings, floating over a Spike Viewer](../../tests/snapshots/graph_windows.png)

Finds spikes: a crossing of a channel's threshold, on any channel of a group, then the waveform on all the group's
channels around the peak that follows. A group can't spike again until its last spike's waveform has ended.

**Settings**

- **Polarity**: whether spikes go negative, positive, or both ways.
- **Group**: single channels, stereotrodes (2), tetrodes (4) or octrodes (8).
- **Before peak**, **After peak**: how many samples of waveform to keep around the peak.
- **Thresholds**: each channel's, in µV past zero, the way the spikes go. Most easily set from a Spike Viewer.
- **Auto detect**: chooses thresholds from the data. It samples each channel **for** a while, then sets each threshold
  to the mean plus a multiple (the **×**) of a percentile's (the **Level**'s) distance from it. Run it here, or from a
  Spike Viewer. **Each channel** shows what it chose for each.

## Spike Gain

> **Processing.** In: `spikes` (µV). Out: `spikes` (scaled). ◇ `gains`.

<img src="../../tests/snapshots/spike_gain_settings.png" alt="A gain for each tetrode, and auto detect">

Multiplies each channel's spike waveforms by a gain, into the units whatever records them needs: Write LTX wants
values from −128 to 127. A spike bigger than that is clipped in the recording. Gains can be one for all channels, one
for each group (tetrode), or one each. Beside each gain is the range in µV it **fits**.

**Auto detect** samples each channel's spikes **for** a while, then sets each gain so that a range just fits, as
its **Algorithm** says:

- **Threshold × factor**: the channel's threshold, times the **Threshold factor**.
- **Peak percentile × factor**: a percentile of its spikes' peaks (95 %, say, or 100 % for the biggest), times its
  **factor**.
- The **larger**, **mean** or **geometric mean** of the two, each times its factor. The geometric mean is the default.

The range is never less than 1.5 × the threshold, so the threshold's always in view, and a channel with no spikes goes
by its threshold. If the thresholds change while it's sampling (moved by hand, say), it starts sampling again, so the
gains follow the new thresholds and the spikes they find.

Connect Write LTX's ◇ `gains` to it, and the gains are recorded in the `.set` file.

## Spike Viewer

> **Viewer.** In: `spikes`. ◇ `thresholds` (a Spike Detector's), ◇ `gains` (a Spike Gain's).

![Spike Viewers, as a quick action opens them](../../tests/snapshots/standard_ltx_layout.png)

Each group's recent spikes, overlaid: a box per tetrode, with a plot per channel and its threshold. Spikes always go up
the plot, whichever way they go (a negative-going spike is shown upside down), so the threshold is always above the
middle. A plot shows just the range its gain fits, so a spike reaching the edge is clipped in the recording.

Connected to a detector's thresholds and a gain's gains, it sets them:

- Drag a threshold's line to move it.
- Over a channel, press **T** to type its threshold, or **G** its gain. Up and down step it, Enter applies it, Esc
  cancels.
- **Auto detect** (gains, thresholds, or both) and **Run** triggers the detector's and gain's own auto detect. Both
  are done one after the other, the thresholds first, as the gains can go by them: it says which it's on, and what's
  next. The cogs beside **Gains** and **Thresholds** open their nodes' settings, which say how.

**Plot height** sets the size of each box.
