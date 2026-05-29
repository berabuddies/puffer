#!/usr/bin/env python3
"""Real-time user-behavior analysis with the local MiniCPM5-1B.

Taps a puffer session transcript (user messages + tool invocations = what the
user is doing) and asks the on-device model to infer what's going on. Cheap +
private enough to run continuously. Presentation is out of scope — this just
produces the rolling analysis (stdout + ~/.puffer/behavior/<session>.jsonl).

Usage:
  behavior_analyzer.py <session.jsonl>            # one-shot
  behavior_analyzer.py --watch <session.jsonl>    # tail + analyze on new activity
"""
import json, sys, os, time, urllib.request, pathlib

SHIM = "http://127.0.0.1:8088/v1/chat/completions"
WINDOW = 12  # recent behavior events to consider

SYS = (
    "You observe a developer working inside a coding agent. Read their recent "
    "actions and infer what is actually happening. Output ONLY one JSON object "
    "(no prose, no markdown) with these keys, filled from the actions:\n"
    "  intent: string — what they are trying to accomplish\n"
    "  state: one of exploring | implementing | debugging | stuck | reviewing | idle\n"
    "  activity: string — one-line summary of the recent actions\n"
    "  signals: array of short strings — notable behavior you observed\n"
    "  suggestion: string — one short proactive suggestion, or \"\"\n\n"
    "Example (for a DIFFERENT, unrelated session):\n"
    "Recent actions:\n"
    "USER: add a dark mode toggle to settings\n"
    "TOOL Read(settings.tsx) -> ok\n"
    "TOOL Edit(settings.tsx) -> ok\n"
    "TOOL Edit(theme.css) -> ok\n"
    'Output: {"intent":"Add a dark mode toggle to the settings screen",'
    '"state":"implementing","activity":"Read settings, edited settings.tsx and theme.css",'
    '"signals":["new feature work across UI + styles"],"suggestion":"Add a test for the toggle state"}\n\n'
    "Now analyze the actual session below the same way."
)

def behavior_line(ev):
    t = ev.get("type")
    if t == "user_message":
        return "USER: " + (ev.get("text") or "")[:200]
    if t == "tool_invocation":
        tid = ev.get("tool_id", "?")
        inp = ev.get("input")
        arg = ""
        try:
            o = inp if isinstance(inp, dict) else json.loads(inp)
            v = o.get("file_path") or o.get("path") or o.get("command") or o.get("pattern") or o.get("query") or ""
            arg = str(v).split("/")[-1] if o.get("file_path") or o.get("path") else str(v)
        except Exception:
            arg = str(inp)
        ok = ev.get("success")
        return f"TOOL {tid}({arg[:40]}) -> {'ok' if ok else 'FAIL'}"
    if t == "assistant_message":
        return "AGENT: " + (ev.get("text") or "")[:120]
    return None

def window(events):
    raw = [l for l in (behavior_line(e) for e in events) if l]
    # collapse consecutive duplicates into "line (xN)" — repetition is itself a
    # behavior signal (e.g. repeated reads/test runs) and stops the model parroting.
    coll = []
    for l in raw:
        if coll and coll[-1][0] == l:
            coll[-1][1] += 1
        else:
            coll.append([l, 1])
    lines = [f"{l} (x{n})" if n > 1 else l for l, n in coll]
    return "\n".join(lines[-WINDOW:])

def analyze(behavior):
    body = json.dumps({
        "model": "minicpm5-1b", "temperature": 0,
        "enable_thinking": False,  # structured analysis: direct JSON, no thinking-loop
        "messages": [{"role": "system", "content": SYS},
                     {"role": "user", "content": "Recent actions:\n" + behavior}],
        "max_tokens": 512,
    }).encode()
    req = urllib.request.Request(SHIM, data=body, method="POST",
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=120) as r:
        d = json.load(r)
    txt = d["choices"][0]["message"].get("content") or ""
    if "</think>" in txt:
        txt = txt.split("</think>")[-1]
    # extract last {...}
    s, e = txt.find("{"), txt.rfind("}")
    if s >= 0 and e > s:
        try:
            return json.loads(txt[s:e+1])
        except Exception:
            pass
    return {"raw": txt.strip()[:200]}

def load(path):
    out = []
    for l in open(path):
        l = l.strip()
        if l:
            try: out.append(json.loads(l))
            except: pass
    return out

def run_once(path, outdir):
    events = load(path)
    beh = window(events)
    if not beh.strip():
        return None
    t0 = time.time()
    a = analyze(beh)
    a["_secs"] = round(time.time() - t0, 1)
    a["_ts"] = int(time.time())
    sid = pathlib.Path(path).stem.replace(".session", "")
    outdir.mkdir(parents=True, exist_ok=True)
    with open(outdir / f"{sid}.jsonl", "a") as f:
        f.write(json.dumps(a, ensure_ascii=False) + "\n")
    print(json.dumps(a, ensure_ascii=False, indent=1))
    return a

def main():
    args = sys.argv[1:]
    watch = "--watch" in args
    args = [a for a in args if a != "--watch"]
    path = args[0]
    outdir = pathlib.Path(os.path.expanduser("~/.puffer/behavior"))
    if not watch:
        run_once(path, outdir); return
    print(f"[watch] {path} — analyzing on new activity", flush=True)
    last = -1
    while True:
        try:
            sz = os.path.getsize(path)
        except OSError:
            sz = -1
        if sz != last:
            time.sleep(0.8)  # debounce
            run_once(path, outdir)
            last = os.path.getsize(path)
        time.sleep(2)

if __name__ == "__main__":
    main()
