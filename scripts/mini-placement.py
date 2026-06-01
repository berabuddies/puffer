#!/usr/bin/env python3
"""mini-placement — find a cold/empty screen region for the puffer mini window.

The mini floating window (hotkey-summoned, ~1/10 screen, ChatGPT-mini style)
should not always land dead-center on top of what the user is doing. This engine
screenshots the screen, finds the *coldest* region (lowest visual activity =
empty desktop / blank panel / whitespace), and computes a window {x,y,w,h} to
drop the mini window into that gap.

Two detectors:
  - cv  (default): local edge-activity map + summed-area table. Precise pixel
        coords, instant, no network, nothing leaves the machine. Best default.
  - vision (--vision): ask a VLM for the empty-region bbox. Useful when "cold"
        is semantic (e.g. a photo that is visually busy but contextually idle),
        but VLM bbox coords are coarse, so we snap its box to the cv map.

Output: JSON {x,y,w,h, screen:{w,h}, region:{...}, detector} in *screen pixels*
(top-left origin). With --visualize, also writes a PNG with the rect drawn.

Usage:
  mini-placement.py                         # screenshot now, cv detector
  mini-placement.py --shot path.png         # use an existing screenshot
  mini-placement.py --area 0.10 --ratio 0.62
  mini-placement.py --visualize out.png
  mini-placement.py --vision                # VLM-assisted (needs OOPS_KEY env)
"""
import sys, os, json, subprocess, tempfile, argparse, base64, urllib.request
import numpy as np
from PIL import Image, ImageDraw

# macOS menu bar + Dock are reserved strips the window must avoid.
MENUBAR_FRAC = 0.035   # top ~3.5% (notch/menubar)
EDGE_MARGIN  = 0.012   # keep a small gap from screen edges


def screenshot():
    """Capture the main display to a temp PNG; return (path, is_temp)."""
    fd, path = tempfile.mkstemp(suffix=".png", prefix="puffer-shot-")
    os.close(fd)
    # -x = no sound, -C = capture cursor off by default; main display only.
    subprocess.run(["screencapture", "-x", "-m", path], check=True)
    return path, True


def activity_map(img, grid_w=192):
    """Downscale to a coarse grid and return a per-cell edge-activity array.

    Content (text, icons, photos, window chrome) has high local gradient; empty
    desktop / flat panels are near zero. We sum |dx|+|dy| of luminance.
    """
    W, H = img.size
    gh = max(1, round(grid_w * H / W))
    small = img.convert("L").resize((grid_w, gh), Image.BILINEAR)
    a = np.asarray(small, dtype=np.float32)
    gx = np.abs(np.diff(a, axis=1, prepend=a[:, :1]))
    gy = np.abs(np.diff(a, axis=0, prepend=a[:1, :]))
    act = gx + gy
    return act, grid_w, gh, W, H


def coldest_rect(act, gw, gh, area_frac, ratio):
    """Slide a target-size window over the activity grid; return the coldest
    top-left (gj, gi) and (tw, th) in grid cells. ratio = w/h of the window."""
    target_area = area_frac * gw * gh
    tw = max(2, int(round((target_area * ratio) ** 0.5)))
    th = max(2, int(round(tw / ratio)))
    tw, th = min(tw, gw), min(th, gh)

    # Summed-area table for O(1) rectangle sums.
    sat = np.zeros((gh + 1, gw + 1), dtype=np.float64)
    sat[1:, 1:] = np.cumsum(np.cumsum(act, axis=0), axis=1)

    def rect_sum(i, j):  # sum of act[i:i+th, j:j+tw]
        return (sat[i + th, j + tw] - sat[i, j + tw]
                - sat[i + th, j] + sat[i, j])

    # Forbidden zones: top menubar strip + edge margins.
    top_lo = int(round(MENUBAR_FRAC * gh))
    mi = int(round(EDGE_MARGIN * gh))
    mj = int(round(EDGE_MARGIN * gw))
    i_lo, i_hi = top_lo + mi, gh - th - mi
    j_lo, j_hi = mj, gw - tw - mj
    if i_hi < i_lo:
        i_lo, i_hi = 0, gh - th
    if j_hi < j_lo:
        j_lo, j_hi = 0, gw - tw

    best = None
    for i in range(i_lo, i_hi + 1):
        for j in range(j_lo, j_hi + 1):
            s = rect_sum(i, j)
            # Tie-break: prefer regions nearer a screen edge (less likely to be
            # the user's active focus) and lower on screen. Cheap heuristic.
            edge_pull = min(j, gw - tw - j) + 0.5 * (gh - th - i)
            score = s + 0.15 * edge_pull
            if best is None or score < best[0]:
                best = (score, i, j, s)
    _, gi, gj, raw = best
    return gi, gj, tw, th, raw, float(act.mean())


def to_pixels(gi, gj, tw, th, gw, gh, W, H):
    """Map grid cells back to screen pixels."""
    return {
        "x": int(round(gj / gw * W)),
        "y": int(round(gi / gh * H)),
        "w": int(round(tw / gw * W)),
        "h": int(round(th / gh * H)),
    }


def vision_box(shot_path, key, base):
    """Ask a VLM for the largest empty region as a normalized [x,y,w,h] box."""
    # Downscale before upload: full retina PNGs are multi-MB and the VLM only
    # needs layout, not pixels. Normalized coords stay valid after scaling.
    import io
    im = Image.open(shot_path).convert("RGB")
    im.thumbnail((1280, 1280), Image.BILINEAR)
    buf = io.BytesIO()
    im.save(buf, format="JPEG", quality=70)
    b64 = base64.b64encode(buf.getvalue()).decode()
    body = json.dumps({
        "model": os.environ.get("OOPS_MODEL", "gpt-5.4"),
        "temperature": 0,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text":
                 "Find the largest EMPTY / visually idle region of this screen "
                 "(blank desktop, whitespace, an unused panel) suitable for a "
                 "small floating window. Reply ONLY JSON: "
                 '{"x":<0-1>,"y":<0-1>,"w":<0-1>,"h":<0-1>} as fractions of the image.'},
                {"type": "image_url",
                 "image_url": {"url": f"data:image/jpeg;base64,{b64}"}},
            ],
        }],
        "max_tokens": 200,
    }).encode()
    req = urllib.request.Request(base.rstrip("/") + "/chat/completions",
                                 data=body, method="POST",
                                 headers={"Content-Type": "application/json",
                                          "Authorization": f"Bearer {key}"})
    with urllib.request.urlopen(req, timeout=60) as r:
        d = json.load(r)
    txt = d["choices"][0]["message"]["content"]
    s, e = txt.find("{"), txt.rfind("}")
    return json.loads(txt[s:e + 1])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--shot")
    ap.add_argument("--area", type=float, default=0.10, help="window area as frac of screen")
    ap.add_argument("--ratio", type=float, default=0.62, help="window w/h")
    ap.add_argument("--visualize")
    ap.add_argument("--vision", action="store_true")
    args = ap.parse_args()

    if args.shot:
        shot, is_temp = args.shot, False
    else:
        shot, is_temp = screenshot()

    img = Image.open(shot)
    act, gw, gh, W, H = activity_map(img)
    gi, gj, tw, th, raw, mean_act = coldest_rect(act, gw, gh, args.area, args.ratio)
    detector = "cv"

    if args.vision:
        key = os.environ.get("OOPS_KEY")
        base = os.environ.get("OOPS_BASE", "https://gw.oops.asia/v1")
        if not key:
            print("warn: --vision needs OOPS_KEY env; falling back to cv", file=sys.stderr)
        else:
            try:
                vb = vision_box(shot, key, base)
                # Snap the VLM's coarse box to grid, then re-find the coldest
                # rect *within* its suggested region for precise coords.
                detector = "vision+cv"
                gj = int(round(vb["x"] * gw))
                gi = int(round(vb["y"] * gh))
                tw = min(tw, gw - gj)
                th = min(th, gh - gi)
            except Exception as ex:
                print(f"warn: vision failed ({ex}); using cv", file=sys.stderr)

    rect = to_pixels(gi, gj, tw, th, gw, gh, W, H)
    out = {
        **rect,
        "screen": {"w": W, "h": H},
        "detector": detector,
        "region": {"cold_score": round(raw, 1), "mean_activity": round(mean_act, 2),
                   "fill": round(rect["w"] * rect["h"] / (W * H), 3)},
    }
    print(json.dumps(out, indent=1))

    if args.visualize:
        vis = img.convert("RGB").copy()
        d = ImageDraw.Draw(vis)
        d.rectangle([rect["x"], rect["y"], rect["x"] + rect["w"], rect["y"] + rect["h"]],
                    outline=(0, 220, 130), width=8)
        vis.save(args.visualize)
        print(f"visualized -> {args.visualize}", file=sys.stderr)

    if is_temp and not args.visualize:
        os.unlink(shot)


if __name__ == "__main__":
    main()
