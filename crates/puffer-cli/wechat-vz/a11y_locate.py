#!/usr/bin/env python3
"""AT-SPI locator for the WeChat desktop client.

Reads the live OS accessibility tree (role / name / states / bounds) and answers
structured queries, so the connector can locate the search box, contact/chat
rows, the message input, and the Send button by *role+name* and click their real
pixel bounds — instead of reading the screen with the vision model.

Requires an AT-SPI-capable WeChat (see guest-setup.sh: a real session D-Bus,
at-spi-bus-launcher, and GTK_MODULES=gail:atk-bridge / QT_ACCESSIBILITY=1 set
*before* WeChat launches). Run inside the guest/container.

Usage (all emit one JSON object/array on stdout):
  a11y_locate.py dump [--max-depth N]          # whole tree (debug)
  a11y_locate.py find --role ROLE [--name S] [--contains] [--app NAME] [--state S]
  a11y_locate.py center --role ROLE --name S    # {x,y} click point of first match
  a11y_locate.py state --role ROLE --name S     # states of first match (e.g. is Send DISABLED)
  a11y_locate.py wait --role ROLE --name S [--timeout SECS]  # poll until present

Exit code 0 with a result, 3 when nothing matches, 2 on a11y/runtime error.
"""

import argparse
import json
import sys
import time

# AT-SPI via GObject-introspection (gir1.2-atspi-2.0 + python3-gi). This is the
# binding the WeChat client actually registers against (verified on WeChat
# 4.1.1.4); pyatspi is not required.
try:
    import gi

    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi

    Atspi.init()
except Exception as e:  # pragma: no cover - environment guard
    print(json.dumps({"error": f"Atspi (gi) unavailable: {e}"}))
    sys.exit(2)


def _bounds(acc):
    """Screen bounds (x, y, w, h) of an accessible, or None."""
    try:
        ext = acc.get_component_iface().get_extents(Atspi.CoordType.SCREEN)
        return {"x": ext.x, "y": ext.y, "w": ext.width, "h": ext.height}
    except Exception:
        return None


def _states(acc):
    try:
        ss = acc.get_state_set()
        return sorted(
            s.value_nick.upper().replace("-", "_")
            for s in Atspi.StateType.__enum_values__.values()
            if ss.contains(s)
        )
    except Exception:
        return []


def _name(acc):
    try:
        return acc.get_name() or ""
    except Exception:
        return ""


def _role(acc):
    # AT-SPI returns role names with spaces ("push button"); normalize to the
    # conventional hyphenated form ("push-button") used in queries/dumps.
    try:
        return (acc.get_role_name() or "").replace(" ", "-")
    except Exception:
        return ""


def _matches(acc, role, name, contains, want_state, max_x=None, min_x=None):
    if role and _role(acc) != role:
        return False
    if name:
        # Strip whitespace: WeChat conversation rows carry trailing spaces (and a
        # last-message preview) that must still match the bare recipient name.
        n = (_name(acc) or "").strip()
        want = name.strip()
        if contains:
            if want not in n:
                return False
        elif n != want:
            return False
    if want_state and want_state not in _states(acc):
        return False
    if max_x is not None or min_x is not None:
        b = _bounds(acc)
        if not b:
            return False
        if max_x is not None and b["x"] >= max_x:
            return False
        if min_x is not None and b["x"] < min_x:
            return False
    return True


def _walk(root, visit, max_depth=40, depth=0):
    """Depth-first walk; `visit(acc, depth)` may return True to stop early."""
    if visit(root, depth):
        return True
    if depth >= max_depth:
        return False
    try:
        n = root.get_child_count()
    except Exception:
        return False
    for i in range(n):
        try:
            child = root.get_child_at_index(i)
        except Exception:
            continue
        if child is None:
            continue
        if _walk(child, visit, max_depth, depth + 1):
            return True
    return False


def _apps(app_filter):
    desktop = Atspi.get_desktop(0)
    out = []
    for i in range(desktop.get_child_count()):
        try:
            app = desktop.get_child_at_index(i)
        except Exception:
            continue
        if app is None:
            continue
        if app_filter and app_filter.lower() not in (_name(app) or "").lower():
            continue
        out.append(app)
    return out


def find_first(role, name, contains, app_filter, want_state, max_x=None, min_x=None):
    hit = {"acc": None}

    def visit(acc, _depth):
        if _matches(acc, role, name, contains, want_state, max_x, min_x):
            hit["acc"] = acc
            return True
        return False

    for app in _apps(app_filter):
        if _walk(app, visit):
            break
    return hit["acc"]


def describe(acc):
    b = _bounds(acc)
    d = {"role": _role(acc), "name": _name(acc), "states": _states(acc), "bounds": b}
    if b and b["w"] > 0 and b["h"] > 0:
        d["center"] = {"x": b["x"] + b["w"] // 2, "y": b["y"] + b["h"] // 2}
    return d


def cmd_dump(args):
    rows = []

    def visit(acc, depth):
        rows.append({"depth": depth, **describe(acc)})
        return False

    apps = _apps(args.app)
    for app in apps:
        _walk(app, visit, max_depth=args.max_depth)
    print(json.dumps({"apps": len(apps), "nodes": rows}, ensure_ascii=False))
    return 0


def cmd_find(args):
    acc = find_first(args.role, args.name, args.contains, args.app, args.state,
                     getattr(args, "max_x", None), getattr(args, "min_x", None))
    if acc is None:
        print(json.dumps({"found": False}))
        return 3
    print(json.dumps({"found": True, **describe(acc)}, ensure_ascii=False))
    return 0


def cmd_center(args):
    acc = find_first(args.role, args.name, args.contains, args.app, args.state,
                     getattr(args, "max_x", None), getattr(args, "min_x", None))
    if acc is None:
        print(json.dumps({"found": False}))
        return 3
    d = describe(acc)
    if "center" not in d:
        print(json.dumps({"found": True, "center": None, "reason": "no bounds"}))
        return 3
    print(json.dumps({"found": True, **d["center"]}))
    return 0


def cmd_state(args):
    acc = find_first(args.role, args.name, args.contains, args.app, args.state,
                     getattr(args, "max_x", None), getattr(args, "min_x", None))
    if acc is None:
        print(json.dumps({"found": False}))
        return 3
    print(json.dumps({"found": True, "states": _states(acc)}, ensure_ascii=False))
    return 0


def cmd_wait(args):
    start = time.monotonic()
    while True:
        acc = find_first(args.role, args.name, args.contains, args.app, args.state,
                     getattr(args, "max_x", None), getattr(args, "min_x", None))
        if acc is not None:
            print(json.dumps({"found": True, **describe(acc)}, ensure_ascii=False))
            return 0
        if time.monotonic() - start >= args.timeout:
            print(json.dumps({"found": False, "timeout": args.timeout}))
            return 3
        time.sleep(0.4)


def main(argv):
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="cmd", required=True)

    def add_query(sp):
        sp.add_argument("--role", default="")
        sp.add_argument("--name", default="")
        sp.add_argument("--contains", action="store_true", help="substring name match")
        sp.add_argument("--app", default="", help="restrict to app whose name contains this")
        sp.add_argument("--state", default="", help="require this AT-SPI state")
        sp.add_argument("--max-x", type=int, default=None,
                        help="only match elements whose x < this (e.g. the left "
                             "conversation panel, to avoid the web-search result rows)")
        sp.add_argument("--min-x", type=int, default=None,
                        help="only match elements whose x >= this (e.g. the right "
                             "chat pane, for the open-chat title / message input)")

    sd = sub.add_parser("dump")
    sd.add_argument("--app", default="")
    sd.add_argument("--max-depth", type=int, default=40)

    add_query(sub.add_parser("find"))
    add_query(sub.add_parser("center"))
    add_query(sub.add_parser("state"))
    w = sub.add_parser("wait")
    add_query(w)
    w.add_argument("--timeout", type=float, default=10.0)

    args = p.parse_args(argv)
    try:
        return {
            "dump": cmd_dump,
            "find": cmd_find,
            "center": cmd_center,
            "state": cmd_state,
            "wait": cmd_wait,
        }[args.cmd](args)
    except Exception as e:  # pragma: no cover
        print(json.dumps({"error": str(e)}))
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
