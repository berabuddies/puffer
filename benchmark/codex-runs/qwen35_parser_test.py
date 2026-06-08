#!/usr/bin/env python3
"""Offline edge-case suite for the qwen35 shim tool-call parser.

Imports scripts/qwen35_shim.py with mlx_lm stubbed (no model load) and drives
parse_tool_calls over the Qwen3-Coder XML tool-call format
(<tool_call><function=NAME><parameter=KEY>VALUE</parameter></function></tool_call>).
"""
import importlib.util, pathlib, sys, types, json

for name in ("mlx_lm", "mlx_lm.sample_utils"):
    sys.modules.setdefault(name, types.ModuleType(name))
sys.modules["mlx_lm"].load = lambda *a, **k: (None, None)
sys.modules["mlx_lm"].generate = lambda *a, **k: ""
sys.modules["mlx_lm.sample_utils"].make_sampler = lambda *a, **k: None

SHIM = pathlib.Path(__file__).resolve().parents[2] / "scripts" / "qwen35_shim.py"
spec = importlib.util.spec_from_file_location("qwen35_shim", SHIM)
q = importlib.util.module_from_spec(spec); spec.loader.exec_module(q)

CT = {"create_task": {"title": "string", "due": "string", "n": "integer"}}

def c(name, args):
    return {"name": name, "args": args}

CASES = [
    ("a wrapped + newlines (real Qwen output)",
     "<tool_call>\n<function=create_task>\n<parameter=title>\nQ3报告\n</parameter>\n<parameter=due>\n明天早上\n</parameter>\n</function>\n</tool_call>",
     [c("create_task", {"title": "Q3报告", "due": "明天早上"})]),

    ("b no wrapper",
     "<function=create_task><parameter=title>买菜</parameter></function>",
     [c("create_task", {"title": "买菜"})]),

    ("c integer arg coerced",
     "<function=create_task><parameter=n>42</parameter></function>",
     [c("create_task", {"n": 42})]),

    ("d string arg stays literal",
     "<function=create_task><parameter=title>0123</parameter></function>",
     [c("create_task", {"title": "0123"})]),

    ("e whitespace in tags",
     "<function = create_task ><parameter = title >x</parameter></function>",
     [c("create_task", {"title": "x"})]),

    ("f two calls",
     "<function=create_task><parameter=title>A</parameter></function>"
     "<function=create_task><parameter=title>B</parameter></function>",
     [c("create_task", {"title": "A"}), c("create_task", {"title": "B"})]),

    ("g unadvertised tool dropped",
     "<function=rm_rf><parameter=path>/</parameter></function>",
     []),

    ("h prose before call (content split safe)",
     "我来帮你建任务。\n<tool_call><function=create_task><parameter=title>报税</parameter></function></tool_call>",
     [c("create_task", {"title": "报税"})]),
]

def norm(parsed):
    return [{"name": x["function"]["name"], "args": json.loads(x["function"]["arguments"])} for x in parsed]

npass = 0
print(f"{'ID':<44} RESULT")
print("-" * 64)
for cid, raw, expected in CASES:
    got = norm(q.parse_tool_calls(raw, {"create_task"}, CT))
    ok = got == expected
    npass += ok
    print(f"{cid:<44} {'PASS' if ok else 'FAIL'}")
    if not ok:
        print(f"    expected: {expected}")
        print(f"    got:      {got}")
print("-" * 64)
print(f"{npass}/{len(CASES)} passed")
sys.exit(0 if npass == len(CASES) else 1)
