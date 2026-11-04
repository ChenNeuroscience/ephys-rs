"""
Fake camera for ephys-rs: renders a mouse with two head LEDs exploring a 1x1 m arena, as seen by a
camera looking straight down from 2.5 m, and streams raw grayscale frames over TCP. Standard library only.

    python3 synthetic_streams/fake_camera.py [--port 27030] [--fps 50] [--width 640] [--height 480]

Then set a Camera node's source to the fake camera (at the same address) and press Play in ephys-rs.
The Camera node connects as a client; when it disconnects we wait for the next one.

Scene (world coordinates in metres, origin at the centre of the arena floor, x right, y down the image, z up):
  * black arena floor, 1 x 1 m, on a dark grey room floor
  * grey walls 45 cm high; with perspective their inner faces appear to splay outward
  * a mouse with a headstage carrying a big and a small LED
  * a tether cable rising from the headstage towards a commutator 2 m up, off to the left of the frame;
    it gets thicker as it approaches the camera
  * a laptop on the room floor outside the arena, seen from above with its bright screen tilted back (something for
    the crop to exclude)
  * a timing LED on the room floor just outside the arena, which flashes for 300 ms every 3-6 s
  * a little noise on every frame, and a tiny shimmer on a couple of the walls

Wire format per frame, little-endian (must match src/standard_components/tracking/camera/fake.rs):
    b"FRM1", u32 width, u32 height, u64 frame_id, f64 timestamp_seconds, width*height uint8 pixels
"""
import argparse
import math
import os
import random
import socket
import struct
import time

HEADER = struct.Struct("<4sIIQd")

CAMERA_HEIGHT = 2.5
ARENA_HALF = 0.5
WALL_HEIGHT = 0.45
WALL_THICKNESS = 0.015
COMMUTATOR = (-1.1, -0.15, 2.0)
CABLE_DIAMETER = 0.005
NOISE_VARIANTS = 8
WALL_JITTER = 0.6  # pixels, on the walls that shimmer
SHIMMERING_WALLS = (0, 2)
LAPTOP = (0.73, 0.22, 0.12)  # x, y and angle
TIMING_LED = (-0.70, 0.30, 0.02)
TIMING_LED_ON = 0.3  # seconds
TIMING_LED_GAP = (3.0, 6.0)  # seconds between flashes


class Camera:
    def __init__(self, w, h):
        self.w, self.h = w, h
        # Focal length (px) chosen so the outside of the wall tops just fits in the frame.
        outer = ARENA_HALF + WALL_THICKNESS
        self.f = 0.95 * (min(w, h) / 2) / (outer / (CAMERA_HEIGHT - WALL_HEIGHT))

    def scale(self, z):
        """Pixels per metre for something at height z."""
        return self.f / (CAMERA_HEIGHT - z)

    def project(self, x, y, z):
        k = self.scale(z)
        return (self.w / 2 + x * k, self.h / 2 + y * k)


# ---- rasterising into a bytearray, coordinates are continuous with pixel i covering [i, i+1) ----


def fill_polygon(buf, w, h, pts, value):
    ys = [p[1] for p in pts]
    y0, y1 = max(0, int(math.floor(min(ys)))), min(h - 1, int(math.ceil(max(ys))))
    n = len(pts)
    for y in range(y0, y1 + 1):
        yc = y + 0.5
        xs = []
        for i in range(n):
            (xa, ya), (xb, yb) = pts[i], pts[(i + 1) % n]
            if (ya <= yc < yb) or (yb <= yc < ya):
                xs.append(xa + (yc - ya) * (xb - xa) / (yb - ya))
        xs.sort()
        for j in range(0, len(xs) - 1, 2):
            xa = max(0, math.ceil(xs[j] - 0.5))
            xb = min(w - 1, math.ceil(xs[j + 1] - 0.5) - 1)
            if xa <= xb:
                buf[y * w + xa : y * w + xb + 1] = bytes([value]) * (xb - xa + 1)


def draw_disc(buf, w, h, cx, cy, r, value, blend_max=False):
    for y in range(max(0, int(cy - r)), min(h, int(cy + r) + 1)):
        dy = y + 0.5 - cy
        if dy * dy > r * r:
            continue
        dx = math.sqrt(r * r - dy * dy)
        x0, x1 = max(0, math.ceil(cx - dx - 0.5)), min(w - 1, math.ceil(cx + dx - 0.5) - 1)
        if x0 > x1:
            continue
        if blend_max:
            for i in range(y * w + x0, y * w + x1 + 1):
                if buf[i] < value:
                    buf[i] = value
        else:
            buf[y * w + x0 : y * w + x1 + 1] = bytes([value]) * (x1 - x0 + 1)


def draw_ellipse(buf, w, h, cx, cy, a, b, angle, value):
    """Filled ellipse with semi-axes a (along `angle`) and b, in pixels."""
    ca, sa = math.cos(angle), math.sin(angle)
    r = max(a, b)
    for y in range(max(0, int(cy - r)), min(h, int(cy + r) + 1)):
        for x in range(max(0, int(cx - r)), min(w, int(cx + r) + 1)):
            dx, dy = x + 0.5 - cx, y + 0.5 - cy
            u, v = dx * ca + dy * sa, -dx * sa + dy * ca
            if (u / a) ** 2 + (v / b) ** 2 <= 1:
                buf[y * w + x] = value


def draw_glow(buf, w, h, cx, cy, core, value):
    """An LED as the camera sees it: a saturated core and a dimmer bloom around it."""
    draw_disc(buf, w, h, cx, cy, core * 2.2, value // 2, blend_max=True)
    draw_disc(buf, w, h, cx, cy, core * 1.5, value * 3 // 4, blend_max=True)
    draw_disc(buf, w, h, cx, cy, core, value)


# ---- the static scene ----


def draw_laptop(buf, cam):
    """A laptop from above: its base, keys and trackpad, and past them its screen, tilted back and lit."""
    w, h = cam.w, cam.h
    x, y, angle = LAPTOP
    ca, sa = math.cos(angle), math.sin(angle)

    def box(x0, y0, x1, y1, value, z=0.02):
        corners = ((x0, y0), (x1, y0), (x1, y1), (x0, y1))
        fill_polygon(buf, w, h, [cam.project(x + u * ca - v * sa, y + u * sa + v * ca, z) for u, v in corners], value)

    box(-0.085, -0.098, 0.085, -0.058, 225, 0.1)  # screen
    box(-0.085, -0.06, 0.085, 0.06, 58)  # base
    columns, rows, pitch = 13, 5, 0.0125
    for row in range(rows):
        for column in range(columns):
            u, v = -0.08 + column * pitch, -0.052 + row * pitch
            box(u, v, u + pitch - 0.003, v + pitch - 0.003, 88)
    box(-0.03, 0.017, 0.03, 0.054, 72)  # trackpad


def draw_timing_led_housing(buf, cam):
    w, h = cam.w, cam.h
    k = cam.scale(TIMING_LED[2])
    u, v = cam.project(*TIMING_LED)
    draw_disc(buf, w, h, u, v, 0.014 * k, 50)
    draw_disc(buf, w, h, u, v, 0.006 * k, 95)


def render_background(cam, jitter=0.0):
    """The room, with the shimmering walls `jitter` pixels or so from where they belong."""
    w, h = cam.w, cam.h

    # Each wall moves as one, so its parts stay joined.
    nudges = [(random.uniform(-jitter, jitter), random.uniform(-jitter, jitter)) for _ in range(4)]

    def nudged(points, side):
        if side not in SHIMMERING_WALLS:
            return points
        dx, dy = nudges[side]
        return [(u + dx, v + dy) for u, v in points]

    buf = bytearray(b"\x26" * (w * h))  # room floor

    def floor_square(half, z):
        return [cam.project(sx * half, sy * half, z) for sx, sy in ((-1, -1), (1, -1), (1, 1), (-1, 1))]

    # Inner wall faces, drawn as horizontal bands so they get darker towards the floor.
    bands = 15
    side_tone = (0, 12, -8, 6)  # each wall is lit slightly differently
    corners = ((-1, -1), (1, -1), (1, 1), (-1, 1))
    for side in range(4):
        (ax, ay), (bx, by) = corners[side], corners[(side + 1) % 4]
        for i in range(bands):
            z0, z1 = WALL_HEIGHT * i / bands, WALL_HEIGHT * (i + 1) / bands
            tone = int(85 + 50 * (i + 0.5) / bands) + side_tone[side]
            quad = [
                cam.project(ax * ARENA_HALF, ay * ARENA_HALF, z0),
                cam.project(bx * ARENA_HALF, by * ARENA_HALF, z0),
                cam.project(bx * ARENA_HALF, by * ARENA_HALF, z1),
                cam.project(ax * ARENA_HALF, ay * ARENA_HALF, z1),
            ]
            fill_polygon(buf, w, h, nudged(quad, side), tone)

    # Wall tops: a light ring between the inner and outer top edges.
    outer = floor_square(ARENA_HALF + WALL_THICKNESS, WALL_HEIGHT)
    inner = floor_square(ARENA_HALF, WALL_HEIGHT)
    for side in range(4):
        ring = [inner[side], inner[(side + 1) % 4], outer[(side + 1) % 4], outer[side]]
        fill_polygon(buf, w, h, nudged(ring, side), 175)

    fill_polygon(buf, w, h, floor_square(ARENA_HALF, 0), 0x0C)  # arena floor

    draw_laptop(buf, cam)
    draw_timing_led_housing(buf, cam)
    return buf


def with_noise(base):
    noise = os.urandom(len(base))
    return bytearray(min(255, max(0, v + (n % 7) - 3)) for v, n in zip(base, noise))


# ---- the timing LED ----


class TimingLed:
    def __init__(self, fps):
        self.dt = 1 / fps
        self.t = 0.0
        self.off_at = 0.0
        self.on_at = random.uniform(*TIMING_LED_GAP)

    def step(self):
        self.t += self.dt
        if self.t >= self.on_at:
            self.off_at = self.t + TIMING_LED_ON
            self.on_at = self.off_at + random.uniform(*TIMING_LED_GAP)

    def draw(self, buf, cam):
        if self.t < self.off_at:
            u, v = cam.project(*TIMING_LED)
            draw_glow(buf, cam.w, cam.h, u, v, 0.007 * cam.scale(TIMING_LED[2]), 255)


# ---- the mouse ----


class Mouse:
    LED_HEIGHT = 0.07
    MARGIN = 0.05  # keep the body centre this far from the walls

    def __init__(self, fps):
        self.dt = 1 / fps
        self.x, self.y = random.uniform(-0.3, 0.3), random.uniform(-0.3, 0.3)
        self.heading = random.uniform(0, 2 * math.pi)
        self.speed = 0.0
        self.target_speed = 0.1
        self.bout_left = 0.0
        self.turn = 0.0
        self.led_dropout = 0  # frames for which the LEDs are hidden (e.g. head down, grooming)
        self.t = 0.0

    def step(self):
        self.t += self.dt
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

        if self.led_dropout > 0:
            self.led_dropout -= 1
        elif random.random() < 0.003:
            self.led_dropout = random.randint(2, 15)

    def draw(self, buf, cam):
        w, h = cam.w, cam.h
        ch, sh = math.cos(self.heading), math.sin(self.heading)

        def at(dist, z):
            return cam.project(self.x + dist * ch, self.y + dist * sh, z)

        # Tail: a gently waving curve behind the body.
        k = cam.scale(0.005)
        for i in range(30):
            s = i / 29
            wobble = 0.012 * math.sin(self.t * 5 + s * 4) * s
            px = self.x + (-0.05 - 0.08 * s) * ch - wobble * sh
            py = self.y + (-0.05 - 0.08 * s) * sh + wobble * ch
            u, v = cam.project(px, py, 0.005)
            draw_disc(buf, w, h, u, v, max(0.8, 0.004 * k * (1 - 0.6 * s)), 55)

        k = cam.scale(0.02)
        bx, by = at(-0.015, 0.02)
        draw_ellipse(buf, w, h, bx, by, 0.042 * k, 0.019 * k, self.heading, 80)  # body
        hx, hy = at(0.03, 0.025)
        draw_ellipse(buf, w, h, hx, hy, 0.016 * k, 0.012 * k, self.heading, 92)  # head
        for side in (-1, 1):  # ears
            ex, ey = cam.project(self.x + 0.022 * ch - side * 0.011 * sh, self.y + 0.022 * sh + side * 0.011 * ch, 0.03)
            draw_disc(buf, w, h, ex, ey, 0.005 * k, 105)

        # Headstage, then the tether, then the LEDs on top.
        sx, sy = at(0.025, 0.06)
        draw_ellipse(buf, w, h, sx, sy, 0.012 * cam.scale(0.06), 0.008 * cam.scale(0.06), self.heading, 40)
        self.draw_cable(buf, cam, (self.x + 0.02 * ch, self.y + 0.02 * sh, 0.075))

        if self.led_dropout == 0:
            k = cam.scale(self.LED_HEIGHT)
            u, v = at(0.035, self.LED_HEIGHT)
            draw_glow(buf, w, h, u, v, 0.008 * k, 255)  # big LED, front
            u, v = at(0.005, self.LED_HEIGHT)
            draw_glow(buf, w, h, u, v, 0.005 * k, 235)  # small LED, back

    def draw_cable(self, buf, cam, start):
        """Tether from the headstage up to the commutator, sagging and swaying slightly. Seen from above it runs
        off to the left, getting thicker as it rises towards the camera."""
        sway = 0.04 * math.sin(self.t * 0.7)
        n = 120
        points = []  # (u, v, radius) in pixels
        for i in range(n + 1):
            s = i / n
            x = start[0] + (COMMUTATOR[0] - start[0]) * s
            y = start[1] + (COMMUTATOR[1] - start[1]) * s + sway * math.sin(math.pi * s)
            # rises steeply off the head, then flattens out under the commutator
            z = start[2] + (COMMUTATOR[2] - start[2]) * math.sin(s * math.pi / 2)
            u, v = cam.project(x, y, z)
            points.append((u, v, max(0.8, CABLE_DIAMETER / 2 * cam.scale(z))))
        # Stamp discs densely enough along each segment that the cable is continuous.
        for (u0, v0, r0), (u1, v1, r1) in zip(points, points[1:]):
            if max(u0, u1) < -r1 or min(u0, u1) >= cam.w + r1 or max(v0, v1) < -r1 or min(v0, v1) >= cam.h + r1:
                continue
            steps = max(1, int(math.hypot(u1 - u0, v1 - v0) / (0.5 * min(r0, r1))))
            for j in range(steps):
                t = j / steps
                draw_disc(buf, cam.w, cam.h, u0 + (u1 - u0) * t, v0 + (v1 - v0) * t, r0 + (r1 - r0) * t, 62)


def serve_client(conn, args, backgrounds, cam):
    mouse = Mouse(args.fps)
    timing_led = TimingLed(args.fps)
    interval = 1.0 / args.fps
    next_due = time.monotonic()
    frame_id = 0
    sent_at = time.monotonic()
    report_every = int(args.fps) * 5
    while True:
        mouse.step()
        timing_led.step()
        frame = bytearray(random.choice(backgrounds))
        mouse.draw(frame, cam)
        timing_led.draw(frame, cam)
        conn.sendall(HEADER.pack(b"FRM1", cam.w, cam.h, frame_id, time.time()) + frame)
        frame_id += 1
        if frame_id % report_every == 0:
            now = time.monotonic()
            print(f"  sent {frame_id} frames ({report_every / (now - sent_at):.1f} fps)")
            sent_at = now
        next_due += interval
        delay = next_due - time.monotonic()
        if delay > 0:
            time.sleep(delay)
        else:
            next_due = time.monotonic()  # fell behind; don't try to catch up in a burst


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--port", type=int, default=27030)
    p.add_argument("--fps", type=float, default=50)
    p.add_argument("--width", type=int, default=640)
    p.add_argument("--height", type=int, default=480)
    args = p.parse_args()

    cam = Camera(args.width, args.height)
    print("rendering background...")
    backgrounds = [with_noise(render_background(cam, WALL_JITTER)) for _ in range(NOISE_VARIANTS)]
    print(f"floor: {cam.scale(0):.1f} px/m, wall tops: {cam.scale(WALL_HEIGHT):.1f} px/m "
          f"(so a ruler along the 1 m floor edge should read {cam.scale(0):.0f} px)")

    with socket.create_server((args.host, args.port)) as server:
        while True:
            print(f"fake camera {cam.w}x{cam.h}@{args.fps}fps waiting on {args.host}:{args.port}")
            conn, addr = server.accept()
            print(f"client connected from {addr}")
            with conn:
                conn.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                try:
                    serve_client(conn, args, backgrounds, cam)
                except (BrokenPipeError, ConnectionResetError):
                    print("client disconnected")


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
