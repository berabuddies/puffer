#!/usr/bin/env python3
"""OpenAI-compatible shim for MiniCPM5-1B (4-bit MLX) with XML->tool_calls.

Why: MiniCPM5 emits tool calls in its native XML form
(<function name="..."><param name="...">v</param></function>), but OpenAI clients
(pi-mono) require native `tool_calls`. This server runs the mlx model with
enable_thinking=True (required for tool reasoning) and converts XML output into
OpenAI tool_calls / finish_reason="tool_calls". Supports stream + non-stream.

Run:  python3 minicpm_shim.py            # serves on 127.0.0.1:8088
Test: curl 127.0.0.1:8088/v1/chat/completions -d '{...}'
"""
import json, re, time, os, http.server, pathlib, threading
from mlx_lm import load, generate
from mlx_lm.sample_utils import make_sampler

MD = os.environ.get("MINICPM5_MODEL") or str(pathlib.Path(__file__).parent / "model")
HOST, PORT = "127.0.0.1", 8088

print("loading model ...", flush=True)
MODEL, TOK = load(MD)
LOCK = threading.Lock()  # mlx model is single-stream; serialize requests
print("model ready", flush=True)

CALL_RE = re.compile(r'<function\s+name="([^"]+)">(.*?)</function>', re.S)
PARAM_RE = re.compile(r'<param\s+name="([^"]+)">(.*?)</param>', re.S)

def _coerce(v):
    v = v.strip()
    try: return json.loads(v)            # numbers/bools/json
    except Exception: return v           # plain string

def parse_tool_calls(text, allow=None):
    # allow = set of advertised tool names; drop any call the model invents that
    # the request didn't offer (untrusted model/tool-output text can't fabricate
    # calls to unadvertised tools).
    calls = []
    for i, m in enumerate(CALL_RE.finditer(text)):
        name = m.group(1)
        if allow is not None and name not in allow:
            continue
        args = {pn: _coerce(pv) for pn, pv in PARAM_RE.findall(m.group(2))}
        calls.append({"id": f"call_{int(time.time()*1000)}_{i}", "type": "function",
                      "function": {"name": name, "arguments": json.dumps(args, ensure_ascii=False)}})
    return calls

def _flatten_content(c):
    """OpenAI clients (pi) send content as a list of parts; MiniCPM's chat
    template requires plain string content. Join text parts into a string."""
    if isinstance(c, str):
        return c
    if isinstance(c, list):
        return "".join(p.get("text", "") if isinstance(p, dict) and p.get("type") == "text"
                       else (p if isinstance(p, str) else "") for p in c)
    return "" if c is None else str(c)

def _normalize(messages):
    """Adapt OpenAI message shapes to MiniCPM's chat template expectations:
    - content: list-of-parts -> string
    - assistant tool_calls: function.arguments JSON string -> dict (template calls .items())
    - role 'tool' result -> role 'user' wrapped in <tool_response>...</tool_response>
    """
    out = []
    for m in messages:
        role = m.get("role")
        if role == "tool":
            out.append({"role": "user",
                        "content": f"<tool_response>{_flatten_content(m.get('content'))}</tool_response>"})
            continue
        nm = {**m, "content": _flatten_content(m.get("content"))}
        if nm.get("tool_calls"):
            tcs = []
            for tc in nm["tool_calls"]:
                fn = dict(tc.get("function", {}))
                a = fn.get("arguments")
                if isinstance(a, str):
                    try: fn["arguments"] = json.loads(a)
                    except Exception: fn["arguments"] = {}
                tcs.append({**tc, "function": fn})
            nm["tool_calls"] = tcs
            if nm.get("content") is None:
                nm["content"] = ""
        out.append(nm)
    return out

def run(messages, tools, temperature, max_tokens, enable_thinking=True):
    messages = _normalize(messages)
    kw = dict(add_generation_prompt=True, tokenize=False, enable_thinking=enable_thinking)
    if tools:
        kw["tools"] = [t.get("function", t) if t.get("type") == "function" else t for t in tools]
        # pi sends tools as {type:function, function:{name,description,parameters}};
        # the template expects each tool object to have name/description/parameters
        kw["tools"] = [(t["function"] if isinstance(t, dict) and t.get("type") == "function" else t) for t in tools]
    text = TOK.apply_chat_template(messages, **kw)
    sampler = make_sampler(temp=max(0.0, float(temperature or 0.0)))
    with LOCK:
        out = generate(MODEL, TOK, prompt=text, max_tokens=int(max_tokens or 512),
                       sampler=sampler, verbose=False)
    # strip thinking for the surfaced content/parse (keep final segment)
    final = out.split("</think>")[-1].strip() if "</think>" in out else out.strip()
    allow = None
    if tools:
        allow = {(t.get("function", t) if isinstance(t, dict) else {}).get("name")
                 for t in tools}
        allow.discard(None)
    calls = parse_tool_calls(final, allow) or parse_tool_calls(out, allow)
    if calls:
        # content before the first <function> (often empty)
        content = CALL_RE.split(final)[0].strip()
        return {"role": "assistant", "content": content or None, "tool_calls": calls}, "tool_calls"
    return {"role": "assistant", "content": final}, "stop"

class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def _json(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code); self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b))); self.end_headers(); self.wfile.write(b)

    def do_GET(self):
        if self.path.startswith("/v1/models"):
            self._json(200, {"object": "list", "data": [{"id": "minicpm5-1b", "object": "model"}]})
        else:
            self._json(404, {"error": "not found"})

    def do_POST(self):
        if not self.path.startswith("/v1/chat/completions"):
            return self._json(404, {"error": "not found"})
        try:
            raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            body = json.loads(raw)
        except Exception as e:
            return self._json(400, {"error": f"bad request: {e}", "got_bytes": len(raw) if 'raw' in dir() else 0})
        try:
            # NOTE: do NOT log request messages anywhere — this is a local,
            # privacy-preserving model; prompts/tool outputs/code stay in memory.
            think = body.get("enable_thinking")
            if think is None:
                think = (body.get("chat_template_kwargs") or {}).get("enable_thinking", True)
            msg, finish = run(body.get("messages", []), body.get("tools"),
                              body.get("temperature", 0.0), body.get("max_tokens") or 2048,
                              enable_thinking=bool(think))
        except Exception as e:
            import traceback; traceback.print_exc()
            return self._json(500, {"error": f"generation failed: {e}"})
        cid = f"chatcmpl-{int(time.time()*1000)}"
        if body.get("stream"):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache"); self.end_headers()
            def send(d): self.wfile.write(f"data: {json.dumps(d)}\n\n".encode()); self.wfile.flush()
            base = {"id": cid, "object": "chat.completion.chunk", "model": "minicpm5-1b",
                    "choices": [{"index": 0, "delta": {}, "finish_reason": None}]}
            send({**base, "choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": None}]})
            if msg.get("tool_calls"):
                for j, tc in enumerate(msg["tool_calls"]):
                    delta = {"tool_calls": [{"index": j, "id": tc["id"], "type": "function",
                              "function": {"name": tc["function"]["name"], "arguments": tc["function"]["arguments"]}}]}
                    send({**base, "choices": [{"index": 0, "delta": delta, "finish_reason": None}]})
            elif msg.get("content"):
                send({**base, "choices": [{"index": 0, "delta": {"content": msg["content"]}, "finish_reason": None}]})
            send({**base, "choices": [{"index": 0, "delta": {}, "finish_reason": finish}]})
            self.wfile.write(b"data: [DONE]\n\n"); self.wfile.flush()
        else:
            self._json(200, {"id": cid, "object": "chat.completion", "model": "minicpm5-1b",
                "choices": [{"index": 0, "message": msg, "finish_reason": finish}],
                "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}})

if __name__ == "__main__":
    print(f"serving on http://{HOST}:{PORT}/v1", flush=True)
    http.server.ThreadingHTTPServer((HOST, PORT), H).serve_forever()
