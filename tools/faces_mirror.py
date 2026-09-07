#!/usr/bin/env python3
"""Mirror the EXACT integer math from the Rust clock faces and render ASCII.

Lives in `tools/` so it can be previewed offline on a laptop — it mirrors the
13 scene faces in `src/clock_faces/` (Palm, Lighthouse, Cat, Balloon, Beach,
Windmill, Hourglass, Firefly, Aquarium, Waterfall, Forest, Torii, Island), in
the same order as the `clock::FACES` table in `src/ui/clock.rs`.

One 128x64 canvas per face (the readout area is not drawn; full-width elements
like full-width sand / drifting clouds ARE drawn so we can check wrap/clip
behaviour).

Usage:
    python3 tools/faces_mirror.py

The faces that animate from the wall clock by themselves (Palm frond sway,
Balloon/Balloon-drift and Windmill clouds) sample EspInstant milliseconds on
the firmware; this tool takes the drift/sway as parameters so you can pick
snapshots.  The helper functions at the bottom (`tick`, `palm_sway`, `drift`,
`balloon_sway`) reproduce the exact firmware formulas if you want realistic
values for a given millisecond instant.

NOTE: `tdiv()` reproduces Rust's `i32 /` (truncate toward zero).  Python's `//`
floors, which differs by 1px whenever the numerator is negative (e.g. pier
directions past 180 degrees), so all math mirrored from Rust goes through
`tdiv`.
"""
W, H = 128, 64


def new_canvas():
    return [[0] * W for _ in range(H)]


def tdiv(n, d):  # Rust i32 '/' — truncate toward zero (Python '//' floors)
    q = abs(n) // d
    return q if n >= 0 else -q


def setp(px, x, y):
    x, y = int(x), int(y)
    if 0 <= x < W and 0 <= y < H:
        px[y][x] = 1


def clr(px, x, y):
    x, y = int(x), int(y)
    if 0 <= x < W and 0 <= y < H:
        px[y][x] = 0


def fl(px, x0, y0, x1, y1, on=True):
    n = int(max(abs(x1 - x0), abs(y1 - y0), 1))
    for i in range(n + 1):
        t = i / n
        if on:
            setp(px, x0 + (x1 - x0) * t, y0 + (y1 - y0) * t)
        else:
            clr(px, x0 + (x1 - x0) * t, y0 + (y1 - y0) * t)


def rect(px, x, y, w, h, on=True):
    for yy in range(int(y), int(y + h)):
        for xx in range(int(x), int(x + w)):
            if on:
                setp(px, xx, yy)
            else:
                clr(px, xx, yy)


def vline(px, x, y, h, on=True):
    for yy in range(int(y), int(y + h)):
        if on:
            setp(px, x, yy)
        else:
            clr(px, x, yy)


def hl(px, x, y, w, on=True):
    for xx in range(int(x), int(x + w)):
        if on:
            setp(px, xx, y)
        else:
            clr(px, xx, y)


def circ(px, x, y, r, on=True):
    r2 = r * r
    for yy in range(int(y - r), int(y + r) + 1):
        for xx in range(int(x - r), int(x + r) + 1):
            if (xx - x) ** 2 + (yy - y) ** 2 <= r2:
                if on:
                    setp(px, xx, yy)
                else:
                    clr(px, xx, yy)


def tri(px, x0, y0, x1, y1, x2, y2, on=True):
    (xx0, yy0), (xx1, yy1), (xx2, yy2) = (x0, y0), (x1, y1), (x2, y2)
    for yy in range(int(min(yy0, yy1, yy2)), int(max(yy0, yy1, yy2)) + 1):
        for xx in range(int(min(xx0, xx1, xx2)), int(max(xx0, xx1, xx2)) + 1):
            d1 = (xx - xx1) * (yy0 - yy1) - (xx0 - xx1) * (yy - yy1)
            d2 = (xx - xx2) * (yy1 - yy2) - (xx1 - xx2) * (yy - yy2)
            d3 = (xx - xx0) * (yy2 - yy0) - (xx2 - xx0) * (yy - yy0)
            neg = (d1 < 0) or (d2 < 0) or (d3 < 0)
            pos = (d1 > 0) or (d2 > 0) or (d3 > 0)
            if not (neg and pos) and 0 <= xx < W and 0 <= yy < H:
                if on:
                    px[yy][xx] = 1
                else:
                    px[yy][xx] = 0


# --- Exact port of geom::sin_fp / cos_fp / ray (Bhaskara, *1024 fixed point) ---
def sin_fp(deg):
    d = deg % 360
    neg, x = (True, d - 180) if d >= 180 else (False, d)
    x_half = 180 - x
    num = 4 * x * x_half
    den = 40500 - x * x_half
    v = 1024 * num // den
    return -v if neg else v


def cos_fp(deg):
    return sin_fp(deg + 90)


def ray(ox, oy, ln, deg):  # clockwise from 12 o'clock
    dx = sin_fp(deg) * ln // 1024
    dy = -cos_fp(deg) * ln // 1024
    return (ox + dx, oy + dy)


def show(px, name):
    print(f"===== {name} =====")
    for y in range(H):
        print("%02d %s" % (y, "".join("#" if px[y][x] else "." for x in range(W))))


# ---------------------------------------------------------------------------
# palm  (src/clock_faces/palm.rs)
# ---------------------------------------------------------------------------
def palm(sway):
    """Coconut palm with a fan of arched, blade-shaped fronds swaying ±8°.

    `sway` is the current frond sway in degrees (firmware: ±8, from
    `palm::sway_now()`)."""
    px = new_canvas()
    CROWN = (22, 17)

    # Rising sun behind the crown.
    circ(px, 10, 8, 5)

    # The fan of fronds: each arch bends over (right blades clockwise, left
    # counter-clockwise) and is drawn as a tapered 5-point strip.
    FROND_DEG = [10, 65, 120, 275, 310, 345, 355]
    FROND_SCALE = [10, 10, 8, 8, 10, 10, 10]
    steps = [(2, 0, 1), (7, 25, 3), (13, 55, 3), (19, 85, 2), (26, 100, 0)]
    for a, k in zip(FROND_DEG, FROND_SCALE):
        a0 = a + sway
        s = a0 - 15
        e = s + (52 if a0 <= 180 else -52)
        up = [None] * 5
        lo = [None] * 5
        for i, (ln, tt, wd) in enumerate(steps):
            ln = ln * k // 10
            ang = s + tdiv((e - s) * tt, 100)
            x, y = ray(CROWN[0], CROWN[1], ln, ang)
            up[i] = ray(x, y, wd, ang - 90)
            lo[i] = ray(x, y, wd, ang + 90)
        for i in range(4):
            tri(px, up[i][0], up[i][1], lo[i][0], lo[i][1], up[i + 1][0], up[i + 1][1])
            tri(px, lo[i][0], lo[i][1], lo[i + 1][0], lo[i + 1][1], up[i + 1][0], up[i + 1][1])

    # Trunk: a gently curving triple line from sand to crown.
    for (x0, y0, x1, y1) in [
        (26, 60, 24, 50), (24, 50, 23, 40), (23, 40, 22, 30),
        (22, 30, 21, 21), (21, 21, 22, 17),
    ]:
        for dx in (0, 1, 2):
            fl(px, x0 + dx, y0, x1 + dx, y1)

    # One coconut nestled at the upper-right of the crown.
    circ(px, 29, 19, 2)

    # A band of sand across the bottom.
    fl(px, 0, 61, 127, 61)
    for (x0, x1) in [(0, 24), (44, 60), (76, 92), (106, 127)]:
        fl(px, x0, 63, x1, 63)
    return px


# ---------------------------------------------------------------------------
# lighthouse  (src/clock_faces/lighthouse.rs)
# ---------------------------------------------------------------------------
def lighthouse(m, s):
    """Striped lighthouse on a rocky islet; the lamp blinks each second, the
    beam pushes through a 100..140 sweep with the minutes and the full-width
    wave band rolls every eight seconds."""
    px = new_canvas()
    # Sea: horizon and rolling wave dashes across the whole panel width.
    fl(px, 0, 46, 57, 46)
    roll = (s // 2) % 4 == 3
    drift = 3 if roll else 0
    for (y, a, b) in [
        (55, 37, 57), (57, 62, 80), (57, 98, 114), (58, 40, 57), (58, 66, 84),
        (58, 104, 124), (59, 4, 18), (59, 30, 44), (59, 60, 76), (59, 90, 106),
        (59, 118, 126), (61, 12, 28), (61, 48, 62), (61, 78, 94), (61, 110, 124),
        (63, 6, 22), (63, 34, 50), (63, 70, 84), (63, 100, 116),
    ]:
        fl(px, a + drift, y, min(b + drift, 127), y)
    if roll:
        fl(px, 70, 60, 92, 60)  # a crest sweeping through the water band

    # The islet: a rock mound peaking under the tower.
    tri(px, 0, 46, 22, 43, 30, 50)
    tri(px, 0, 46, 30, 50, 40, 58)
    tri(px, 0, 46, 40, 58, 0, 58)

    # Striped tower: eight alternating bands, tapering from 12 to 5 px wide.
    for k in range(8):
        w = 12 - k
        if k % 2 == 0:
            rect(px, 20 - w // 2, 44 - 4 * k, w, 4)

    # Lantern room + the lamp (glow blinks every other second).
    rect(px, 15, 12, 10, 4)
    circ(px, 20, 12, 2)
    if s % 2 == 0:
        circ(px, 20, 12, 5)

    # The beam: a long smooth wedge swinging 100..140 degrees with the minutes.
    a = 120 + tdiv(sin_fp(m * 6) * 20, 1024)
    b0 = ray(20, 12, 42, a - 5)
    b1 = ray(20, 12, 42, a + 5)
    tri(px, 20, 12, b0[0], b0[1], b1[0], b1[1])
    return px


# ---------------------------------------------------------------------------
# cat  (src/clock_faces/cat.rs)
# ---------------------------------------------------------------------------
def cat(h, m, s):
    """Kawaii cat whose two eyes are miniature clock dials: hours in the left,
    minutes in the right; the tail wags between two curls each second."""
    px = new_canvas()
    tri(px, 6, 15, 16, 2, 27, 11)
    tri(px, 12, 12, 16, 7, 21, 11, False)
    tri(px, 52, 15, 42, 2, 31, 11)
    tri(px, 46, 12, 42, 7, 37, 11, False)
    circ(px, 29, 30, 24)
    fl(px, 24, 13, 22, 18, False)
    fl(px, 29, 13, 29, 18, False)
    fl(px, 34, 13, 36, 18, False)
    l = (20, 31)
    r = (38, 31)
    for (ex, ey) in [l, r]:
        circ(px, ex, ey, 8, False)
        fl(px, ex - 1, ey - 7, ex + 1, ey - 7, True)
        fl(px, ex - 1, ey + 7, ex + 1, ey + 7, True)
        fl(px, ex - 7, ey - 1, ex - 7, ey + 1, True)
        fl(px, ex + 7, ey - 1, ex + 7, ey + 1, True)
    ha = (h % 12) * 30 + m // 2
    ma = m * 6
    hx, hy = ray(l[0], l[1], 6, ha)
    mx, my = ray(r[0], r[1], 7, ma)
    fl(px, l[0], l[1], hx, hy, True)
    fl(px, r[0], r[1], mx, my, True)
    tri(px, 27, 40, 31, 40, 29, 43, False)
    fl(px, 29, 43, 22, 48, False)
    fl(px, 29, 43, 36, 48, False)
    for (y0, y1) in [(26, 23), (29, 29), (32, 35)]:
        fl(px, 9, y0, 2, y1, False)
        fl(px, 49, y0, 56, y1, False)
    rect(px, 10, 52, 39, 6)
    circ(px, 15, 58, 4)
    circ(px, 42, 58, 4)
    pts = [(46, 60), (52, 55), (48, 48), (52, 42)] if s % 2 == 0 else [
        (46, 60), (54, 54), (50, 46), (45, 42)]
    for i in range(3):
        fl(px, pts[i][0], pts[i][1], pts[i + 1][0], pts[i + 1][1])
    return px


# ---------------------------------------------------------------------------
# balloon  (src/clock_faces/balloon.rs)
# ---------------------------------------------------------------------------
def balloon(h, drift, sway):
    """Hot-air balloon whose altitude follows the hour, with clouds drifting
    across the whole panel and a gentle horizontal sway."""
    px = new_canvas()
    circ(px, 8, 5, 3)
    for (cbase, cy, rr) in [(98, 4, 2), (66, 28, 2), (54, 9, 4), (26, 22, 3)]:
        cx = (cbase - drift) % 144
        circ(px, cx, cy, rr)
        circ(px, cx + rr, cy, rr - 1)
        circ(px, cx - rr + 1, cy, rr - 1)
    deg = h * 15
    yb = 34 - ((1024 - cos_fp(deg)) * 8) // 1024
    bx = 26 + sway
    circ(px, bx, yb, 10)
    vline(px, bx - 4, yb - 7, 14)
    vline(px, bx + 4, yb - 7, 14)
    fl(px, bx - 4, yb + 7, bx - 3, yb + 14)
    fl(px, bx + 4, yb + 7, bx + 3, yb + 14)
    rect(px, bx - 4, yb + 14, 8, 5)
    tri(px, 0, 63, 14, 52, 28, 63)
    tri(px, 24, 63, 44, 57, 57, 63)
    return px


# ---------------------------------------------------------------------------
# beach  (src/clock_faces/beach.rs)
# ---------------------------------------------------------------------------
def beach(h, m, s):
    """Beach day/night: by day the sun arcs 6..18 with rays; by night a
    crescent moon, twinkling stars and a moon-lit reflection; full-width sand
    with a scalloped surf foam line."""
    px = new_canvas()
    fl(px, 0, 42, 57, 42)
    for (y, a, b) in [
        (44, 2, 12), (44, 20, 30), (46, 8, 18), (46, 32, 44),
        (48, 0, 10), (48, 24, 36), (50, 6, 16), (50, 40, 50),
        (52, 14, 26), (52, 50, 57), (54, 4, 18), (54, 30, 42),
    ]:
        fl(px, a, y, b, y)
    night = h >= 18 or h < 6
    if night:
        circ(px, 16, 14, 5)
        circ(px, 20, 12, 4, False)
        stars = [(30, 8), (41, 5), (49, 10), (55, 17), (33, 18), (24, 20)]
        for i, (sx, sy) in enumerate(stars):
            if (s + i) % 2 == 0:
                rect(px, sx - 1, sy, 3, 1)
                rect(px, sx, sy - 1, 1, 3)
        rect(px, 16, 44, 1, 1)
        rect(px, 15, 48, 3, 1)
        rect(px, 14, 52, 5, 1)
        rect(px, 13, 56, 7, 1)
    tri(px, 19, 44, 19, 49, 26, 49)
    tri(px, 19, 44, 19, 49, 14, 49)
    tri(px, 11, 49, 28, 49, 19, 53)
    rect(px, 0, 57, 128, 7)
    for s_ in range(0, 113, 16):
        fl(px, s_, 55, s_ + 9, 55)
        fl(px, s_ + 10, 56, s_ + 16, 56)
    for (x, y) in [(10, 59), (24, 58), (45, 59), (64, 60), (82, 59), (100, 61), (116, 58)]:
        rect(px, x, y, 1, 1, False)
    if not night:
        th = (h - 6) * 15
        sx = 12 + ((1024 - cos_fp(th)) * 21) // 1024
        sy = 36 - tdiv(sin_fp(th) * 18, 1024)
        circ(px, sx, sy, 6)
        for d in [0, 45, 90, 135, 180, 225, 270, 315]:
            ex = sx + tdiv(cos_fp(d) * 10, 1024)
            ey = sy - tdiv(sin_fp(d) * 10, 1024)
            if ey <= 44:
                fl(px, sx, sy, ex, ey)
        if sy > 30:
            rect(px, sx - 1, 46, 2, 1)
            rect(px, sx - 2, 50, 2, 1)
            rect(px, sx - 1, 54, 2, 1)
        fl(px, 4, 19, 8, 15)
        fl(px, 8, 15, 12, 19)
        fl(px, 42, 9, 46, 5)
        fl(px, 46, 5, 50, 9)
    return px


# ---------------------------------------------------------------------------
# windmill  (src/clock_faces/windmill.rs)
# ---------------------------------------------------------------------------
def windmill(m, drift):
    """Dutch windmill whose four rectangular sails turn one full round per
    hour, with clouds drifting across the panel and grass tufts swaying."""
    px = new_canvas()
    for (cbase, cy, rr) in [(98, 5, 2), (58, 15, 3), (28, 22, 2)]:
        cloud(px, (cbase - drift) % 144, cy, rr)
    circ(px, 52, 6, 3)

    # Tower: solid stone body, door + window kept off the rotor column.
    tri(px, 23, 38, 35, 38, 43, 63)
    tri(px, 23, 38, 43, 63, 15, 63)
    rect(px, 21, 57, 5, 6, False)
    rect(px, 33, 47, 3, 3, False)

    a = m * 6 + 45
    circ(px, 29, 34, 3)
    for i in range(4):
        bpt_sail(px, 29, 34, a + i * 90)

    lean = 1 if drift % 2 == 0 else -1
    for x in [8, 14, 44, 50]:
        fl(px, x, 63, x - 2 + lean, 58)
        fl(px, x, 63, x, 58)
        fl(px, x, 63, x + 2 + lean, 58)
    return px


def cloud(px, cx, cy, rr):
    circ(px, cx, cy, rr)
    circ(px, cx - rr - 2, cy, rr - 1)
    circ(px, cx + rr + 2, cy, rr - 1)
    fl(px, cx - rr - 2, cy + 1, cx + rr + 2, cy + 1)


def bpt_sail(px, ox, oy, deg):  # one rotor blade: base, tip, boom
    hw = 3
    b1 = bpt(ox, oy, 2, deg, hw)
    b2 = bpt(ox, oy, 2, deg, -hw)
    t1 = bpt(ox, oy, 20, deg, hw)
    t2 = bpt(ox, oy, 20, deg, -hw)
    tri(px, b1[0], b1[1], b2[0], b2[1], t1[0], t1[1])
    tri(px, b2[0], b2[1], t1[0], t1[1], t2[0], t2[1])
    l4 = 15  # 20 * 3 // 4
    k1 = bpt(ox, oy, l4, deg, hw + 1)
    k2 = bpt(ox, oy, l4, deg, -(hw + 1))
    fl(px, k1[0], k1[1], k2[0], k2[1])


def bpt(ox, oy, ln, deg, off):  # blade point: ray then perpendicular offset
    bx, by = ray(ox, oy, ln, deg)
    pxv = cos_fp(deg)
    pyv = sin_fp(deg)
    return (bx + tdiv(pxv * off, 1024), by + tdiv(pyv * off, 1024))


# ---------------------------------------------------------------------------
# hourglass  (src/clock_faces/hourglass.rs)
# ---------------------------------------------------------------------------
def hourglass(m, s):
    """Hourglass whose sand drains from the upper to the lower bulb with the
    minutes; grains keep trickling through the waist."""
    px = new_canvas()
    rect(px, 17, 9, 26, 3)
    rect(px, 17, 53, 26, 3)
    rect(px, 16, 56, 9, 4)
    rect(px, 35, 56, 9, 4)
    fl(px, 20, 12, 28, 30)
    fl(px, 28, 30, 20, 52)
    fl(px, 40, 12, 32, 30)
    fl(px, 32, 30, 40, 52)
    fl(px, 20, 12, 40, 12)
    fl(px, 20, 52, 40, 52)
    fl(px, 30, 26, 30, 35)
    sy = 30 - ((60 - m) * 18) // 60
    hw = 10 - ((sy - 12) * 8) // 18
    tri(px, 30, 30, 30 - hw, sy, 30 + hw, sy)
    py = 52 - (m * 22) // 60
    hw2 = 2 + ((py - 30) * 8) // 22
    tri(px, 30, 52, 30 - hw2, py, 30 + hw2, py)
    gy0 = min(36 + (s * 2) % 20, py)
    gy1 = min(36 + ((s * 2 + 9) % 20), py)
    setp(px, 30, gy0)
    setp(px, 29, gy1)
    return px


# ---------------------------------------------------------------------------
# firefly  (src/clock_faces/firefly.rs)
# ---------------------------------------------------------------------------
def firefly(h, t):
    """Jar of fireflies: each hour of the 12-hour cycle lights one more
    firefly, twinkling and wandering on the 500 ms sub-second tick `t`."""
    px = new_canvas()
    circ(px, 9, 7, 3)
    circ(px, 12, 6, 2, False)
    if (t // 4) % 2 == 0:
        for (sx, sy) in [(22, 8), (40, 6)]:
            rect(px, sx - 1, sy, 3, 1)
            rect(px, sx, sy - 1, 1, 3)
    rect(px, 23, 12, 14, 3)
    rect(px, 24, 16, 12, 3)
    fl(px, 24, 20, 18, 28)
    fl(px, 18, 28, 18, 49)
    fl(px, 18, 49, 42, 49)
    fl(px, 42, 49, 42, 28)
    fl(px, 42, 28, 36, 20)
    rect(px, 14, 53, 32, 2)
    lit = h % 12
    if lit == 0:
        lit = 12
    spots = [
        (21, 44), (27, 25), (33, 46), (38, 31), (24, 37), (31, 41),
        (36, 26), (25, 29), (30, 34), (35, 44), (22, 31), (39, 39)]
    for k, (fx, fy) in enumerate(spots):
        if k >= lit:
            break
        if (t + k * 5) % 8 < 3:
            rect(px, fx, fy, 2, 2)
        else:
            rect(px, fx, fy, 1, 1)
    for k in range(3):
        fx = (t * 2 + k * 47) % 128
        fy = 18 + tdiv(sin_fp((t * 11 + k * 97) % 360) * 12, 1024)
        if (t + k * 5) % 8 < 3:
            rect(px, fx, fy, 2, 2)
        else:
            rect(px, fx, fy, 1, 1)
    return px


# ---------------------------------------------------------------------------
# aquarium  (src/clock_faces/aquarium.rs)
# ---------------------------------------------------------------------------
def aquarium(m, t):
    """Goldfish swimming one full trip across the tank per hour (position =
    minute), tail wiggling and bubbles rising on the sub-second tick `t`."""
    px = new_canvas()
    rect(px, 4, 23, 50, 1)
    fl(px, 6, 24, 6, 55)
    fl(px, 52, 24, 52, 55)
    fl(px, 6, 55, 52, 55)
    for (gx, gy) in [
        (9, 53), (12, 52), (16, 53), (20, 52), (24, 53), (28, 52),
        (32, 53), (36, 52), (40, 53), (44, 52), (48, 53)]:
        rect(px, gx, gy, 1, 1)
    for sx in [12, 45]:
        fl(px, sx, 54, sx - 2, 49)
        fl(px, sx - 2, 49, sx, 44)
        fl(px, sx, 44, sx - 1, 40)
    fx = 16 + (m * 30) // 60
    circ(px, fx + 1, 42, 3)
    rect(px, fx - 5, 40, 6, 5)
    if t % 2 == 0:
        tri(px, fx - 6, 42, fx - 10, 37, fx - 10, 47)
    else:
        tri(px, fx - 6, 42, fx - 9, 36, fx - 9, 48)
    rect(px, fx + 2, 41, 1, 1, False)
    for i in range(2):
        by = 44 - ((t * 2 + i * 9) % 20)
        bx = fx + 4 + (i % 2)
        rect(px, bx, by, 1, 1)
    return px


# ---------------------------------------------------------------------------
# waterfall  (src/clock_faces/waterfall.rs)
# ---------------------------------------------------------------------------
def waterfall(m, t):
    """Waterfall: the plunge pool is the minute hand (its surface climbs from
    the bed), with falling flecks, mist and ripples on the 500 ms tick `t`."""
    px = new_canvas()
    sy = 63 - (m * 10) // 60
    circ(px, 52, 4, 2)
    for k in range(2):
        bx = 16 + ((t * 2 + k * 33) % 36)
        by = 5 + k * 4
        fl(px, bx, by, bx + 2, by + 2)
        fl(px, bx + 2, by + 2, bx + 4, by)
    rect(px, 0, 15, 20, 4)
    tri(px, 13, 6, 7, 13, 17, 13)
    tri(px, 13, 10, 6, 18, 18, 18)
    tri(px, 13, 14, 5, 23, 19, 23)
    tri(px, 0, 19, 20, 19, 0, 63)
    rect(px, 2, 42, 3, 1, False)
    tri(px, 51, 18, 46, 30, 56, 30)
    tri(px, 51, 25, 42, 38, 56, 38)
    tri(px, 51, 31, 40, 45, 56, 45)
    rect(px, 50, 40, 2, 10)
    tri(px, 36, 28, 32, 36, 40, 36)
    tri(px, 36, 33, 30, 42, 42, 42)
    rect(px, 35, 41, 2, 5)
    tri(px, 48, 56, 57, 56, 52, 63)
    rect(px, 4, sy, 54, 64 - sy)
    for k in range(3):
        cx = 10 + ((t * 2 + k * 13) % 30)
        rect(px, cx, sy - 1, 1, 1)
    for k in range(3):
        rx = 8 + ((t * 2 + k * 11) % 32)
        rect(px, rx, sy + 2, 4, 1, False)
        rect(px, 8 + ((rx + 7) % 32), sy + 4, 3, 1, False)
    for k in range(8):
        mx = 16 + k * 2
        my = sy - 4 + (k % 3)
        rect(px, mx, my, 1, 1)
    if t % 2 == 0:
        rect(px, 19, sy - 6, 1, 1)
        rect(px, 29, sy - 6, 1, 1)
    drop = sy - 19
    rect(px, 20, 19, 9, drop)
    rect(px, 24, 19, 1, drop, False)
    for i in range(4):
        y = 19 + ((t * 3 + i * 9) % (sy - 20))
        cx = 22 if i % 2 == 0 else 26
        rect(px, cx, y, 1, 2, False)
    rect(px, 19, sy - 2, 1, 1)
    rect(px, 29, sy - 2, 1, 1)
    return px


# ---------------------------------------------------------------------------
# forest  (src/clock_faces/forest.rs)
# ---------------------------------------------------------------------------
def forest(h, m, t):
    """Layers of pines with a squirrel climbing the big foreground tree as the
    minute hand; sun/moon sky and pulsing flowers on the 500 ms tick `t`."""
    px = new_canvas()
    if 6 <= h < 18:
        tt = h - 6
        sx = 30 + tt * 2
        sy = 18 - tt
        circ(px, sx, sy, 3)
        hl(px, sx - 2, sy - 4, 5)
        hl(px, sx - 2, sy + 4, 5)
        vline(px, sx - 4, sy - 1, 3)
        vline(px, sx + 4, sy - 1, 3)
    else:
        circ(px, 50, 7, 3)
        circ(px, 53, 6, 2, False)
        for k, (sx, sy) in enumerate([(16, 5), (40, 3), (31, 12), (48, 15), (57, 4), (24, 2)]):
            if (t + k * 3) % 8 < 4:
                rect(px, sx - 1, sy, 3, 1)
                rect(px, sx, sy - 1, 1, 3)
    for x in [31, 54]:
        tri(px, x + 3, 20, x - 1, 30, x + 7, 30)
        tri(px, x + 3, 26, x - 2, 36, x + 8, 36)
        rect(px, x + 2, 36, 2, 4)
    x = 43
    tri(px, x, 26, x - 4, 33, x + 4, 33)
    tri(px, x, 30, x - 6, 40, x + 6, 40)
    tri(px, x, 34, x - 7, 46, x + 7, 46)
    rect(px, x - 1, 46, 2, 3)
    hl(px, 0, 46, 58)
    rect(px, 14, 20, 3, 36)
    tri(px, 15, 6, 6, 20, 24, 20)
    tri(px, 15, 15, 4, 30, 26, 30)
    tri(px, 15, 24, 3, 46, 27, 46)
    sq_y = 53 - (m * 38) // 60
    rect(px, 14, sq_y, 2, 3, False)
    rect(px, 16, sq_y, 1, 2, False)
    circ(px, 44, 57, 4)
    circ(px, 52, 59, 3)
    rect(px, 24, 55, 1, 3)
    rect(px, 22, 53, 5, 2)
    tri(px, 6, 62, 2, 55, 10, 55)
    for k, (fx, fy) in enumerate([(36, 55), (40, 58)]):
        if (t + k * 2) % 4 < 2:
            rect(px, fx, fy, 1, 1)
    return px


# ---------------------------------------------------------------------------
# torii  (src/clock_faces/torii.rs)
# ---------------------------------------------------------------------------
def torii(h, m, t):
    """Red torii gate in the water: the rising tide is the minute hand; sun by
    day (moon + stars by night), reflection shimmers on the 500 ms tick `t`."""
    px = new_canvas()
    tide = 58 - (m * 12) // 60
    if 6 <= h < 18:
        circ(px, 30, 34, 4)
    else:
        circ(px, 30, 33, 3)
        circ(px, 33, 32, 2, False)
        for k, (sx, sy) in enumerate([(6, 4), (15, 9), (27, 4), (37, 8), (49, 5), (24, 3)]):
            if (t + k * 3) % 8 < 4:
                rect(px, sx - 1, sy, 3, 1)
                rect(px, sx, sy - 1, 1, 3)
    rect(px, 13, 16, 3, 48)
    rect(px, 43, 16, 3, 48)
    rect(px, 10, 14, 40, 3)
    rect(px, 8, 11, 2, 3)
    rect(px, 50, 11, 2, 3)
    rect(px, 13, 26, 33, 2)
    rect(px, 5, 51, 4, 3)
    rect(px, 6, 46, 2, 5)
    tri(px, 7, 41, 3, 46, 11, 46)
    if 6 <= h < 18:
        rect(px, 6, 43, 2, 2, False)
    else:
        rect(px, 6, 43, 2, 2)
    rect(px, 49, 51, 4, 3)
    rect(px, 50, 46, 2, 5)
    tri(px, 50, 41, 46, 46, 54, 46)
    if 6 <= h < 18:
        rect(px, 50, 43, 2, 2, False)
    else:
        rect(px, 50, 43, 2, 2)
    rect(px, 0, tide, 58, 64 - tide)
    for k in range(3):
        rx = (t * 2 + k * 19) % 52
        rect(px, rx, tide + 2, 3, 1, False)
        rect(px, (rx + 7) % 52, tide + 4, 2, 1, False)
        rect(px, (rx + 20) % 52, tide + 8, 3, 1, False)
        rect(px, (rx + 33) % 52, tide + 12, 2, 1, False)
    rect(px, 14, tide + 3, 1, 3, False)
    rect(px, 44, tide + 2, 1, 4, False)
    rect(px, 14, tide + 6, 1, 2, False)
    rect(px, 44, tide + 6, 1, 2, False)
    return px


# ---------------------------------------------------------------------------
# island  (src/clock_faces/island.rs)
# ---------------------------------------------------------------------------
def island(h, m, t):
    """Island house: a tiny boat circles the island once per hour (the minute
    hand); window glows by night, smoke and ripples move on the 500 ms tick."""
    px = new_canvas()
    if 6 <= h < 18:
        tt = h - 6
        circ(px, 8 + tt * 3, 30 - tt * 2, 3)
    else:
        circ(px, 42, 8, 3)
        circ(px, 45, 7, 2, False)
        if (t // 4) % 2 == 0:
            for (sx, sy) in [(16, 5), (30, 10)]:
                rect(px, sx - 1, sy, 3, 1)
                rect(px, sx, sy - 1, 1, 3)
    hl(px, 0, 44, 58)
    for (ry, rx) in [(46, 8), (50, 26), (54, 40), (58, 14), (60, 44)]:
        hl(px, rx, ry, 5)
    ang = (m * 6) % 360
    bx = 29 + tdiv(cos_fp(ang) * 22, 1024)
    by = 53 + tdiv(sin_fp(ang) * 6, 1024)
    rect(px, bx - 2, by, 4, 1)
    rect(px, bx, by - 3, 1, 3)
    rect(px, bx - 5, by + 1, 3, 1)
    rect(px, 16, 42, 26, 8)
    circ(px, 16, 50, 3)
    circ(px, 42, 50, 3)
    tri(px, 13, 27, 6, 41, 20, 41)
    rect(px, 12, 34, 2, 8)
    fl(px, 12, 41, 9, 51)
    fl(px, 13, 41, 13, 53)
    fl(px, 13, 41, 17, 52)
    circ(px, 47, 30, 4)
    rect(px, 46, 34, 2, 8)
    fl(px, 46, 41, 43, 52)
    fl(px, 47, 41, 47, 54)
    fl(px, 47, 41, 51, 51)
    rect(px, 27, 35, 8, 1)
    rect(px, 27, 41, 8, 1)
    rect(px, 27, 35, 1, 7)
    rect(px, 34, 35, 1, 7)
    rect(px, 29, 37, 2, 4, False)
    if not (6 <= h < 18):
        rect(px, 31, 36, 2, 2)
    tri(px, 31, 30, 22, 38, 40, 38)
    rect(px, 33, 26, 2, 4)
    for k in range(2):
        sy = 23 - ((t * 2 + k * 5) % 10)
        rect(px, 34 + k * 2, sy, 1, 1)
    for k in range(3):
        rx = (t * 2 + k * 21) % 52
        hl(px, rx, 48 + k * 4, 3)
    return px


# ---------------------------------------------------------------------------
# Firmware wall-clock sampling helpers (EspInstant milliseconds -> draw params)
# ---------------------------------------------------------------------------
def tick(ms):
    """clock_faces::tick_now(): the shared 500 ms redraw tick."""
    return (ms // 500) % 3600


def palm_sway(ms):
    """palm::sway_now(): ±8 degrees, 24 deg/s -> full cycle every 15 s."""
    deg = ((ms // 500) * 12) % 360
    return tdiv(sin_fp(deg) * 8, 1024)


def drift(ms):
    """balloon::drift_now() / windmill::drift_now(): 1 px per 500 ms, wrap 144."""
    return (ms // 500) % 144


def balloon_sway(ms):
    """balloon::sway_now(): ±4 px on a 64 s cycle."""
    ph = (ms // 1000) % 64
    return tdiv(sin_fp(ph * 6) * 4, 1024)


# ---------------------------------------------------------------------------
if __name__ == "__main__":
    show(palm(palm_sway(0)), "palm ms=0 sway=%+d" % palm_sway(0))
    show(palm(palm_sway(6250)), "palm ms=6250 sway=%+d (max lean)" % palm_sway(6250))
    show(lighthouse(0, 0), "lighthouse m=0 s=0")
    show(lighthouse(15, 1), "lighthouse m=15 s=1 (beam east)")
    show(lighthouse(45, 6), "lighthouse m=45 s=6 (wave roll)")
    show(lighthouse(59, 4), "lighthouse m=59 s=4")
    show(cat(14, 27, 3), "cat 14:27")
    show(cat(22, 5, 2), "cat 22:05")
    show(balloon(14, drift(6000), balloon_sway(6000)), "balloon 14h ms=6000")
    show(balloon(23, drift(72000), balloon_sway(72000)), "balloon 23h ms=72000")
    show(beach(14, 27, 3), "beach 14h day")
    show(beach(2, 12, 3), "beach 02h night")
    show(windmill(0, drift(6000)), "windmill m=0")
    show(windmill(15, drift(6000)), "windmill m=15")
    show(windmill(59, drift(72000)), "windmill m=59 drift=%d" % drift(72000))
    show(hourglass(0, 3), "hourglass m=0 s=3")
    show(hourglass(20, 14), "hourglass m=20 s=14")
    show(hourglass(40, 27), "hourglass m=40 s=27")
    show(hourglass(59, 5), "hourglass m=59 s=5")
    show(firefly(1, 0), "firefly h=1 t=0")
    show(firefly(6, 0), "firefly h=6 t=0")
    show(firefly(12, 11), "firefly h=12 t=11")
    show(aquarium(0, 0), "aquarium m=0 t=0")
    show(aquarium(30, 1), "aquarium m=30 t=1")
    show(aquarium(59, 5), "aquarium m=59 t=5")
    show(waterfall(0, 0), "waterfall m=0 t=0")
    show(waterfall(30, 11), "waterfall m=30 t=11")
    show(waterfall(59, 23), "waterfall m=59 t=23")
    show(forest(12, 0, 3), "forest 12h m=0")
    show(forest(2, 30, 3), "forest 02h m=30")
    show(torii(10, 0, 3), "torii 10h low tide")
    show(torii(2, 59, 7), "torii 02h high tide")
    show(island(14, 0, 3), "island 14h m=0")
    show(island(2, 45, 11), "island 02h m=45")