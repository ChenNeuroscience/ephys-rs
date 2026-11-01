"""
Fake acquisition board for ephys-rs: streams headstage channels, auxiliary (accelerometer) inputs, analogue
inputs and digital inputs over TCP, as an Open Ephys acquisition board would deliver them. Standard library only.

    python3 synthetic_streams/fake_acquisition_board.py [--port 27031] [--headstages A:32,B:32]

Then press Play in ephys-rs: Acquisition Board nodes use this board by default. The node connects as a client;
when it disconnects we wait for the next one.

What it sends:
  * each headstage's channels are tetrodes (groups of 4), each picking up 2 or 3 units: spikes seen on all 4
    channels at different sizes, firing at a few Hz, most often at a preferred phase of the theta rhythm
  * LFP: theta (8 Hz) and a little gamma (40 Hz), much the same across a headstage
  * noise, about 8 uV rms
  * auxiliary inputs (3 per headstage): an accelerometer on a moving head, with gravity on the third
  * analogue inputs: a 1 Hz sine of +-1 V on the first, a 0.5 Hz square of 0 to 3.3 V on the second,
    millivolts of noise on the rest
  * digital inputs: a 10 ms pulse each second on line 1 (a sync pulse), half a second high every 3.7 s on
    line 2 (a stimulus, say), and line 3 changing at random every 2 s or so

Each channel's signal is worked out once, as a loop a second or so long (a whole number of theta cycles, so it
joins up), and sent from there: so starting takes a moment for each sample rate asked for.

Wire format (must match src/standard_components/ephys/board/fake.rs): on connecting we send a line of JSON
describing the board, and read a line of JSON back saying the sample rate, e.g. {"sample_rate": 30000}. Then
blocks, each little-endian:
    b"RHD1", u32 samples, u32 headstage channels, u32 aux channels, u32 adc channels, u64 first sample number,
    then i16 for each headstage channel's samples in turn (0.195 uV per bit), each aux channel's (37.4 uV per
    bit), each adc channel's (152.6 uV per bit), then u16 per sample of the digital inputs (a bit per line)
"""
import argparse
import array
import json
import math
import random
import socket
import struct
import sys
import time

HEADER = struct.Struct("<4sIIIIQ")
EPHYS_UV_PER_BIT = 0.195
AUX_V_PER_BIT = 0.0000374
ADC_V_PER_BIT = 0.00015258789
ADC_CHANNELS = 8
TTL_LINES = 8
THETA_HZ = 8.0
BLOCKS_PER_SECOND = 100


def to_i16(values, per_bit):
    """An array of i16 from values (in the units of per_bit), clamped."""
    return array.array("h", (max(-32767, min(32767, int(v / per_bit))) for v in values))


def spike_shape(t_ms):
    """A spike waveform at t_ms milliseconds in: a sharp trough then a slower rebound."""
    return -math.exp(-(((t_ms - 0.3) / 0.08) ** 2)) + 0.35 * math.exp(-(((t_ms - 0.55) / 0.18) ** 2))


class Signals:
    """Every channel's loop, at one sample rate."""

    def __init__(self, headstages, rate, seed=1):
        rng = random.Random(seed)
        self.rate = rate
        cycle = rate / THETA_HZ  # samples per theta cycle (needn't be whole)
        self.ephys = []  # (loop, length) per channel
        self.aux = []
        for h, (_, channels) in enumerate(headstages):
            lfp_gain = rng.uniform(0.7, 1.3)
            for group in range(0, channels, 4):
                width = min(4, channels - group)
                cycles = 8 + (h * 7 + group // 4) % 5  # 1 to 1.5 s, differing from group to group
                length = round(cycles * cycle)
                lfp = [lfp_gain * (80.0 * math.sin(2 * math.pi * cycles * i / length)
                                   + 15.0 * math.sin(2 * math.pi * 5 * cycles * i / length + 1.0))
                       for i in range(length)]
                loops = [[lfp[i] * rng.uniform(0.9, 1.1) + rng.gauss(0.0, 8.0) for i in range(length)]
                         for _ in range(width)]
                self.add_units(loops, length, cycles, rng)
                self.ephys.extend((to_i16(loop, EPHYS_UV_PER_BIT), length) for loop in loops)
            self.aux.extend(self.accelerometer(rate, rng))
        self.adc = self.analogue(rate, rng)

    @staticmethod
    def add_units(loops, length, cycles, rng):
        """Adds the spikes of 2 or 3 units to a tetrode's loops."""
        rate = length * THETA_HZ / cycles  # samples per second
        samples = max(4, round(1.2e-3 * rate))  # 1.2 ms of waveform
        for _ in range(rng.choice((2, 3))):
            sizes = [rng.uniform(40.0, 220.0) for _ in loops]
            hz = rng.uniform(2.0, 12.0)
            preferred = rng.uniform(0, 2 * math.pi)
            t = 0
            while True:
                t += max(samples, int(rng.expovariate(2 * hz / rate)))  # twice as often, then half kept
                if t + samples >= length:
                    break
                phase = 2 * math.pi * cycles * t / length
                if rng.random() > (1 + math.cos(phase - preferred)) / 2:
                    continue
                for loop, size in zip(loops, sizes):
                    for k in range(samples):
                        loop[t + k] += size * spike_shape(k * 1000.0 / rate)

    @staticmethod
    def accelerometer(rate, rng):
        """A head moving about: x and y swaying, z holding gravity (in volts, as the chip gives it)."""
        length = round(4 * rate)  # 4 s, joining up
        loops = []
        for axis in range(3):
            offset = 1.5 if axis < 2 else 1.8
            amplitude = rng.uniform(0.05, 0.15)
            loop = [offset + amplitude * math.sin(2 * math.pi * (axis + 1) * i / length + axis)
                    for i in range(length)]
            loops.append((to_i16(loop, AUX_V_PER_BIT), length))
        return loops

    @staticmethod
    def analogue(rate, rng):
        length = round(2 * rate)  # 2 s, joining up
        sine = [math.sin(2 * math.pi * 2 * i / length) for i in range(length)]
        square = [3.3 if i < length // 2 else 0.0 for i in range(length)]
        loops = [(to_i16(sine, ADC_V_PER_BIT), length), (to_i16(square, ADC_V_PER_BIT), length)]
        for _ in range(ADC_CHANNELS - 2):
            loops.append((to_i16([rng.gauss(0.0, 0.003) for _ in range(length)], ADC_V_PER_BIT), length))
        return loops


def take(loop, length, first, n):
    """n samples of a loop from sample `first`, as bytes."""
    start = first % length
    if start + n <= length:
        part = loop[start:start + n]
    else:
        part = loop[start:] + loop[:n - (length - start)]
    if sys.byteorder == "big":
        part.byteswap()
    return part.tobytes()


class Digital:
    """The digital inputs, worked out sample by sample."""

    def __init__(self, rate, seed=2):
        self.rate = rate
        self.rng = random.Random(seed)
        self.random_line = 0
        self.next_toggle = self.rng.expovariate(0.5) * rate

    def words(self, first, n):
        rate = self.rate
        out = array.array("H", bytes(2 * n))
        for i in range(n):
            s = first + i
            word = 0
            if s % rate < 0.01 * rate:
                word |= 1
            if s % (3.7 * rate) < 0.5 * rate:
                word |= 2
            if s >= self.next_toggle:
                self.random_line ^= 1
                self.next_toggle += self.rng.expovariate(0.5) * rate
            word |= self.random_line << 2
            out[i] = word
        if sys.byteorder == "big":
            out.byteswap()
        return out.tobytes()


def serve(conn, headstages, cache):
    description = {
        "description": "Fake acquisition board (synthetic_streams/fake_acquisition_board.py)",
        "headstages": [{"port": port, "channels": channels} for port, channels in headstages],
        "adc_channels": ADC_CHANNELS,
        "ttl_lines": TTL_LINES,
    }
    conn.sendall((json.dumps(description) + "\n").encode())
    line = b""
    while not line.endswith(b"\n"):
        chunk = conn.recv(1024)
        if not chunk:
            return
        line += chunk
    rate = int(json.loads(line)["sample_rate"])
    if rate not in cache:
        print(f"working out the signals at {rate} Hz...", flush=True)
        cache[rate] = Signals(headstages, rate)
    signals = cache[rate]
    digital = Digital(rate)
    n = max(1, rate // BLOCKS_PER_SECOND)
    print(f"streaming {len(signals.ephys)} channels at {rate} Hz", flush=True)
    start = time.monotonic()
    first = 0
    while True:
        due = start + (first + n) / rate
        delay = due - time.monotonic()
        if delay > 0:
            time.sleep(delay)
        parts = [HEADER.pack(b"RHD1", n, len(signals.ephys), len(signals.aux), len(signals.adc), first)]
        parts.extend(take(loop, length, first, n) for loop, length in signals.ephys)
        parts.extend(take(loop, length, first, n) for loop, length in signals.aux)
        parts.extend(take(loop, length, first, n) for loop, length in signals.adc)
        parts.append(digital.words(first, n))
        conn.sendall(b"".join(parts))
        first += n


def parse_headstages(text):
    headstages = []
    for item in text.split(","):
        port, channels = item.split(":")
        headstages.append((port.strip().upper(), int(channels)))
    return headstages


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--port", type=int, default=27031)
    parser.add_argument("--headstages", default="A:32,B:32", help="port:channels, comma separated")
    args = parser.parse_args()
    headstages = parse_headstages(args.headstages)

    cache = {}
    print("working out the signals at 30000 Hz...", flush=True)
    cache[30000] = Signals(headstages, 30000)
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", args.port))
    listener.listen(4)
    print(f"fake acquisition board on 127.0.0.1:{args.port}, headstages {args.headstages}", flush=True)
    while True:
        conn, _ = listener.accept()
        conn.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        try:
            serve(conn, headstages, cache)
        except (BrokenPipeError, ConnectionResetError, ValueError, KeyError) as e:
            print(f"client went: {e}", flush=True)
        finally:
            conn.close()


if __name__ == "__main__":
    main()
