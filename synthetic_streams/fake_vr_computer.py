"""
Fake VR computer for ephys-rs: the fake camera's mouse exploring its 1 x 1 m arena, reported as x, y in made-up pixels,
with a TTL and a log line every few seconds. Standard library only.

    python3 synthetic_streams/fake_vr_computer.py [--port 27032] [--fps 50]

Then use the "Use VR computer" quick action, or add a VR Computer node, and press Play in ephys-rs. The node connects as a
client; when it disconnects we wait for the next one.

Each message is a JSON object on a line of its own (must match examples/chen-neuro/src/components/vr_computer.rs).
Settings are sent on being connected to, and again whenever they change: press Enter here to change them.
"""
import argparse
import json
import math
import random
import socket
import sys
import threading
import time

ARENA_HALF = 0.5  # metres
PIXELS_PER_METRE = 800
TTL_HIGH = 0.3  # seconds
TTL_GAP = (3.0, 6.0)  # seconds between trials


class Mouse:
    """Moves as fake_camera.py's does."""

    MARGIN = 0.05  # keep the body centre this far from the walls

    def __init__(self, fps):
        self.dt = 1 / fps
        self.x, self.y = random.uniform(-0.3, 0.3), random.uniform(-0.3, 0.3)
        self.heading = random.uniform(0, 2 * math.pi)
        self.speed = 0.0
        self.target_speed = 0.1
        self.bout_left = 0.0
        self.turn = 0.0

    def step(self):
        self.bout_left -= self.dt
        if self.bout_left <= 0:  # alternate pauses and runs of varying speed
            self.target_speed = 0.0 if random.random() < 0.3 else random.uniform(0.05, 0.3)
            self.bout_left = random.uniform(0.4, 2.0)
        self.speed += (self.target_speed - self.speed) * min(1.0, 4 * self.dt)
        self.turn = 0.9 * self.turn + random.gauss(0, 0.6)
        self.heading += self.turn * self.dt * 3

        lim = ARENA_HALF - self.MARGIN
        nx = self.x + self.speed * math.cos(self.heading) * self.dt
        ny = self.y + self.speed * math.sin(self.heading) * self.dt
        if abs(nx) > lim or abs(ny) > lim:
            # Turn away from the wall: steer towards a point near the centre.
            desired = math.atan2(-self.y + random.uniform(-0.2, 0.2), -self.x + random.uniform(-0.2, 0.2))
            diff = (desired - self.heading + math.pi) % (2 * math.pi) - math.pi
            self.heading += max(-0.4, min(0.4, diff))
        else:
            self.x, self.y = nx, ny

    def position(self, now):
        x, y = (self.x + ARENA_HALF) * PIXELS_PER_METRE, (self.y + ARENA_HALF) * PIXELS_PER_METRE
        return {"type": "pos", "time": now, "x": x, "y": y}


def send(connection, message):
    connection.sendall((json.dumps(message) + "\n").encode())


def settings(changes):
    size = 2 * ARENA_HALF * PIXELS_PER_METRE
    required = {
        "window_min_x": 0,
        "window_max_x": size,
        "window_min_y": 0,
        "window_max_y": size,
        "pixels_per_metre": PIXELS_PER_METRE,
    }
    return {"type": "settings", "settings": {**required, "task": "open field", "arena": {"changes": changes}}}


def count_enters(enters):
    for _ in sys.stdin:
        enters.append(1)


def serve(connection, fps, enters):
    changes = len(enters)
    send(connection, settings(changes))
    mouse = Mouse(fps)
    start = time.monotonic()
    frame, trial = 0, 0
    next_trial, ttl_off = random.uniform(*TTL_GAP), None
    while True:
        now = time.monotonic() - start
        mouse.step()
        send(connection, mouse.position(now))
        if now >= next_trial:
            trial += 1
            send(connection, {"type": "log", "time": now, "line": f"trial {trial} started"})
            send(connection, {"type": "ttl", "time": now, "value": 1})
            ttl_off = now + TTL_HIGH
            next_trial = ttl_off + random.uniform(*TTL_GAP)
        elif ttl_off is not None and now >= ttl_off:
            send(connection, {"type": "ttl", "time": now, "value": 0})
            ttl_off = None
        if changes != len(enters):
            changes = len(enters)
            send(connection, settings(changes))
        frame += 1
        time.sleep(max(0.0, start + frame / fps - time.monotonic()))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--port", type=int, default=27032)
    parser.add_argument("--fps", type=float, default=50.0)
    args = parser.parse_args()
    server = socket.create_server(("127.0.0.1", args.port))
    print(f"Fake VR computer on port {args.port}")
    enters = []
    threading.Thread(target=count_enters, args=(enters,), daemon=True).start()
    while True:
        connection, _ = server.accept()
        connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        print("Connected")
        try:
            serve(connection, args.fps, enters)
        except OSError:
            print("Disconnected")
        connection.close()


if __name__ == "__main__":
    main()
