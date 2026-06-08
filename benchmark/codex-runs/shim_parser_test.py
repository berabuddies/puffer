#!/usr/bin/env python3
"""Offline edge-case suite for the REAL shim tool-call parser.

Imports scripts/minicpm5_shim.py with mlx_lm.load monkeypatched out (so no model
download/load), then drives shim.parse_tool_calls directly. Covers every case
the official SGLang/vLLM MiniCPM5 parsers handle, including the ones the old
naive regex silently dropped or mis-typed.
"""
import importlib.util, pathlib, sys, types, json

# --- stub mlx_lm so importing the shim doesn't load a model ---
for name in ("mlx_lm", "mlx_lm.sample_utils"):
    if name not in sys.modules:
        sys.modules[name] = types.ModuleType(name)
sys.modules["mlx_lm"].load = lambda *a, **k: (None, None)
sys.modules["mlx_lm"].generate = lambda *a, **k: ""
sys.modules["mlx_lm.sample_utils"].make_sampler = lambda *a, **k: None

SHIM = pathlib.Path(__file__).resolve().parents[2] / "scripts" / "minicpm5_shim.py"
spec = importlib.util.spec_from_file_location("minicpm5_shim", SHIM)
shim = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shim)

CT = {"create_task": {"title": "string", "due": "string"}}      # both string-typed
NT = {"f": {"zip": "string", "flag": "string", "note": "string", "n": "integer"}}

def call(name, args):
    return {"name": name, "args": args}

# (id, raw_output, schemas, allow, expected_calls)  expected = list of {name,args} or []
CASES = [
    ("a clean double-quote",
     '<function name="create_task"><param name="title">Q3 report</param></function>',
     CT, {"create_task"}, [call("create_task", {"title": "Q3 report"})]),

    ("b single-quote tag",
     "<function name='create_task'><param name='title'>Q3 report</param></function>",
     CT, {"create_task"}, [call("create_task", {"title": "Q3 report"})]),

    ("c trailing attr on <function>",
     '<function name="create_task" idx="0"><param name="title">Q3</param></function>',
     CT, {"create_task"}, [call("create_task", {"title": "Q3"})]),

    ("d CDATA value",
     '<function name="create_task"><param name="title"><![CDATA[multi\nline]]></param></function>',
     CT, {"create_task"}, [call("create_task", {"title": "multi\nline"})]),

    ("e string args stay literal (true/null/0123)",
     '<function name="f"><param name="flag">true</param><param name="note">null</param><param name="zip">0123</param></function>',
     NT, {"f"}, [call("f", {"flag": "true", "note": "null", "zip": "0123"})]),

    ("f integer arg coerced",
     '<function name="f"><param name="n">42</param></function>',
     NT, {"f"}, [call("f", {"n": 42})]),

    ("g extra whitespace in <param> tag",
     '<function name="create_task"><param  name="title" >Q3</param></function>',
     CT, {"create_task"}, [call("create_task", {"title": "Q3"})]),

    ("h truncated, no closing tag",
     '<function name="create_task"><param name="title">Q3',
     CT, {"create_task"}, []),

    ("i tokenizer artefacts (space/newline glyphs)",
     'ĠĠ<function name="create_task"><param name="title">QĠ3</param></function>',
     CT, {"create_task"}, [call("create_task", {"title": "Q 3"})]),

    ("j collapsed tags <functionname=/<paramname=",
     '<functionname="create_task"><paramname="title">Q3</param></function>',
     CT, {"create_task"}, [call("create_task", {"title": "Q3"})]),

    ("k <param> missing name -> drop malformed call",
     '<function name="create_task"><param>Q3</param></function>',
     CT, {"create_task"}, []),

    ("l CDATA containing </param>",
     '<function name="create_task"><param name="title"><![CDATA[a</param>b]]></param></function>',
     CT, {"create_task"}, [call("create_task", {"title": "a</param>b"})]),

    ("m two calls in one output",
     '<function name="create_task"><param name="title">A</param></function>'
     '<function name="create_task"><param name="title">B</param></function>',
     CT, {"create_task"}, [call("create_task", {"title": "A"}), call("create_task", {"title": "B"})]),

    ("n unadvertised tool dropped by allow",
     '<function name="rm_rf"><param name="path">/</param></function>',
     {}, {"create_task"}, []),
]

def normalize(parsed):
    return [{"name": c["function"]["name"], "args": json.loads(c["function"]["arguments"])} for c in parsed]

npass = 0
print(f"{'ID':<48} {'RESULT'}")
print("-" * 70)
for cid, raw, schemas, allow, expected in CASES:
    got = normalize(shim.parse_tool_calls(raw, allow, schemas))
    ok = got == expected
    npass += ok
    print(f"{cid:<48} {'PASS' if ok else 'FAIL'}")
    if not ok:
        print(f"    expected: {expected}")
        print(f"    got:      {got}")

print("-" * 70)
print(f"{npass}/{len(CASES)} passed")
sys.exit(0 if npass == len(CASES) else 1)
