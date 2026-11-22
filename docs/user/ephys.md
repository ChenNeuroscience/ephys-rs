# Electrophysiology

Continuous voltages and digital inputs from an acquisition board: re-referenced, filtered, downsampled, scaled and
viewed. For spikes, see [Spikes](spikes.md).

Each component below starts with what kind of node it is and its ports. Data ports are written `like this`, and
settings ports are marked ◇.

- [Acquisition Board](#acquisition-board)
- [Reference](#reference)
- [Channel Selector](#channel-selector)
- [Band-pass Filter](#band-pass-filter)
- [Downsample](#downsample)
- [Continuous Gain](#continuous-gain)
- [Spectrum](#spectrum)
- [Hilbert Transform](#hilbert-transform)
- [Continuous Data Viewer](#continuous-data-viewer)
- [Phase Viewer](#phase-viewer)
- [TTL Viewer](#ttl-viewer)

## Acquisition Board

> **Source.** Out: `headstages` (continuous, µV), `digital` (events), `auxiliary` (each headstage's three auxiliary
> inputs: accelerometers, say) and `analogue` (the board's 8 analogue inputs), both continuous.

<img src="../../tests/snapshots/board_settings.png" alt="The Acquisition Board's settings">

Samples from an acquisition board's headstages, and its digital (TTL) inputs, all sampled together. The board can be:

- an **Open Ephys Acquisition Board** (generations 1 and 2, on Opal Kelly FPGAs, or generation 3, over USB 3);
- an **Intan RHD USB Interface Board**;
- the **fake board** in `synthetic_streams/`, for trying things without hardware.

The real boards use the libraries and bitfiles of the Open Ephys GUI's acquisition board plugin: install the Open
Ephys GUI and the plugin (and its Intan RHD USB plugin, for Intan's board), and ephys-rs uses them from the GUI's
folder. File › Settings… › **Open Ephys** shows which file each board uses, lets you choose another folder, and links
to each file, to download on its own instead (see [Settings](main-window.md#settings)). The generation 3 board needs
the plugin's liboni as a library of its own, which it is up to version 1.1.

**Settings**

- **Sample rate**.
- **Headstages**: the channels on each port. **Detect** asks the board which headstages are plugged in. The board
  only ever acquires the headstages set: if they aren't plugged in, Play asks what to do (see
  [Playing and recording](main-window.md#playing-and-recording)), and while running it acquires none of them, saying
  so, until **Use the board's headstages** fixes it.
- **Bandwidth**: the headstages' own analogue filters (low and high cut), and their DSP high-pass. The chips can only
  set them to certain values, which depend on the sample rate: as the board starts, if the settings aren't values
  it can do, Play asks whether to use the nearest it can.

Below the settings is what the board's doing: samples per second, how many have come, and how many were dropped or
didn't arrive in time.

## Reference

> **Processing.** In: `continuous`. Out: `referenced` (the channels, each less its reference), `reference` (the
> references themselves, a channel each). ◇ `reference`.

<img src="../../tests/snapshots/reference_settings_uses.png" width="300" alt="Two references, and which each channel uses">

Re-references channels: as Open Ephys's common average reference, but with any number of references. Each reference
is the **mean** or **median** of some channels. The median is barely moved by a spike on one of its channels. Each
channel then has subtracted from it the reference it uses, if any. A reference made of one channel is that channel.

**Settings**

- For each reference: mean or median, and which channels it's made from.
- **Which each channel uses**: a reference, or none.

The references can also be chosen from an [Continuous Data Viewer](#continuous-data-viewer), connected to the ◇ `reference` setting, while
looking at the traces.

## Channel Selector

> **Processing.** In: `continuous`. Out: `selected`. ◇ `selection`.

<img src="../../tests/snapshots/channel_selector_settings.png" alt="Selecting 4 of 16 channels">

Passes on some of a stream's channels (which channels are kept for the EEG, say). It passes on all of them while the
selection's off (the power button), so you can look through them all, then choose. The selection can be chosen from an
[Continuous Data Viewer](#continuous-data-viewer) connected to ◇ `selection`.

## Band-pass Filter

> **Processing.** In: `continuous`. Out: `filtered`.

A Butterworth band-pass filter on every channel, as Open Ephys's filter has. Typically 600–6000 Hz before spike
detection, or a lower band for the LFP.

**Settings**: **Low cut-off**, **High cut-off** (Hz), and **Order**.

## Downsample

> **Processing.** In: `continuous`. Out: `downsampled`.

Continuous data at a lower **Rate**, keeping every so many samples: every 6th, from 30 kHz to 5 kHz, say, for Write
LTX's EEG. It doesn't filter, so filter out what's above half the new rate first, with a Band-pass Filter. Otherwise
it aliases.

## Continuous Gain

> **Processing.** In: `continuous` (µV). Out: `scaled` (amplified µV). ◇ `gains`.

Amplifies each channel by a gain, as an amplifier does: ×1 leaves it as it is, ×2 doubles it. That's all a gain does:
what's recorded is up to whatever records it (Write LTX's EEG [scale](recording.md#write-ltx) maps ±250 µV to its −128
to 127, to begin with). Gains go from ×¼ to ×4, and can be **one for** all channels, or one each.

**Auto detect** chooses gains from the data: it samples each channel **for** a while, then sets each gain so that the
mean plus a multiple of a percentile's distance from it (the **Level**) maps to the upper limit of the **Fit range**
(what's recorded: ±250 µV, Write LTX's EEG scale, to begin with). Run it from here, or from an [Continuous Data Viewer](#continuous-data-viewer) connected to ◇ `gains`.
It needs data, so it only runs while acquiring, and not while recording: **Stop**, undoing it, stopping acquisition or
starting to record leaves the gains as they were. Its result is an edit like any other, which undo takes back.

## Spectrum

> **Processing.** In: `continuous`. Out: `spectra`.

Each channel's power spectrum over a **Band** of interest (theta, 4–12 Hz, to begin with), updated every few passes,
for a Continuous Data Viewer to show beside its traces. It only runs while the viewer's showing them.

## Hilbert Transform

> **Processing.** In: `continuous`. Out: `analytic` (complex continuous).

Makes the analytic signal of each channel: complex values whose angle is the signal's phase and whose magnitude is its
envelope, for a [Phase Viewer](#phase-viewer). Phase only means something for a narrow band, so band-pass the data
first (to theta, say), then downsample it to about 1 kHz: Band-pass Filter → Downsample → Hilbert Transform.

The transform has to work as the data arrives, so it uses a filter that needs some data after each sample, and its
output comes that much later. The **Lowest frequency** (4 Hz, to begin with) sets how long the filter is: the lower the
frequency, the longer the **Delay** (375 ms for 4 Hz). The filter's length grows with the sample rate, so at higher
rates it's capped, and the node warns that it's only accurate above a higher frequency until the data's downsampled.

## Continuous Data Viewer

> **Viewer.** In: `continuous`, `spectra` (from a Spectrum), `reference` (from a Reference's `reference` output).
> ◇ `gains` (a Continuous Gain's), ◇ `selection` (a Channel Selector's), ◇ `reference` (a Reference's).

![Traces, with the references in rows of their own](../../tests/snapshots/continuous_viewer_rows.png)

Scrolling traces of continuous data, a row per channel, with the time along the bottom. Each row keeps the highest and
lowest sample in each pixel's worth of time, so spikes show however far it's zoomed out.

**Toolbar**: **Span** (seconds shown), **Scale** (±µV, the rows' y axes), **Row height**. With a gain connected, the
gain (one for all, ×) and their auto detect: the traces are what comes in, in µV, before it. With a reference
connected, the references.

Connected to other nodes' settings, it sets them while you look at the traces:

**Choosing references** (◇ `reference`): a column of Rs for each reference sits left of the channels' numbers.
Click a channel's R to put it in that reference and have it use it. Connected to the Reference's `reference` output too,
it shows each reference's own trace, in its colour, at the top. A dimmed row of Rs is a channel that settings say
otherwise for.

![Choosing references](../../tests/snapshots/continuous_viewer_references.png)

**Choosing channels** (◇ `selection`): while the selection's off, all channels show. Click the numbers of those to
keep, then **Apply selection**. **Disable selection** shows them all again.

<img src="../../tests/snapshots/continuous_viewer_toggles.png" alt="Channels being chosen, before the selection's applied"> <img src="../../tests/snapshots/continuous_viewer_references_applied.png" alt="With the selection applied">

## Phase Viewer

> **Viewer.** In: `analytic` (complex continuous, from a Hilbert Transform).

![Each channel's phase around the anchor's](../../tests/snapshots/phase_viewer.png)

How each channel's oscillation relates to one channel, the **Anchor**, over the last few seconds (**Average over**).
For each channel, it shows:

- **Frequency**: how fast the channel's phase turns, on average.
- **Phase**: the channel's phase minus the anchor's, on average. Positive means the channel leads the anchor.
- **Consistency**: how steady that difference is, from 0% (none) to 100% (always the same).
- **Envelope**: the channel's average amplitude.

The polar plot shows them all at once: the anchor is at 0°, on the right, and each channel is at the angle of its
phase (anticlockwise leads, clockwise lags), as far out as it's consistent.

## TTL Viewer

> **Viewer.** In: `events`.

![The TTL Viewer](../../tests/snapshots/ttl_viewer_time_axis.png)

Each digital input line's level over the last few seconds, a row per line. On the right is how many times it's gone
up, since acquisition started and since recording last started. Use it to check that TTLs from other equipment (a
timing LED's, a VR computer's) are arriving. **Settings**: **Span**, **Line height**.
