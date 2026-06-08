#!/usr/bin/env python3
"""OpenAI-compatible shim for Qwen3.5-0.8B (8-bit MLX) with XML->tool_calls.

Why: Qwen3.5 emits tool calls in the Qwen3-Coder XML form
(<tool_call><function=name><parameter=k>v</parameter></function></tool_call>),
but OpenAI clients (pi-mono) require native `tool_calls`. This server runs the
mlx model and converts that XML into OpenAI tool_calls / finish_reason="tool_calls".
Supports stream + non-stream.

Sampling follows Qwen's official tech-report recommendation (greedy is
explicitly discouraged — it causes endless repetition):
  non-think (default here): temp 0.7, top_p 0.8, top_k 20
  think:                     temp 1.0, top_p 0.95, top_k 20

Run:  python3 qwen35_shim.py            # serves on 127.0.0.1:8088
"""
import ast, json, re, time, os, http.server, pathlib, threading
from mlx_lm import load, generate
from mlx_lm.sample_utils import make_sampler

MD = os.environ.get("QWEN35_MODEL") or str(pathlib.Path(__file__).parent / "model")
HOST, PORT = "127.0.0.1", 8088

print("loading model ...", flush=True)
MODEL, TOK = load(MD)
LOCK = threading.Lock()  # mlx model is single-stream; serialize requests
print("model ready", flush=True)

DEFAULT_MAX_TOKENS = 4096

# Tool-call parser for the Qwen3-Coder XML format (matches vLLM/SGLang
# qwen3_coder tool parsers): <function=NAME><parameter=KEY>VALUE</parameter></function>,
# usually wrapped in <tool_call>...</tool_call>. Values carry surrounding
# newlines that must be stripped. Tolerant of single/double-quoted attribute
# styles and trailing whitespace inside tags.
_FUNC_RE = re.compile(r"<function\s*=\s*['\"]?([^>'\"]+?)['\"]?\s*>(.*?)</function>", re.S)
_PARAM_RE = re.compile(r"<parameter\s*=\s*['\"]?([^>'\"]+?)['\"]?\s*>(.*?)</parameter>", re.S)


def _coerce(val_text, ptype):
    """Coerce by declared JSON-schema type. string stays literal (never
    json.loads), others parsed. Values are stripped of the wrapping whitespace
    Qwen inserts around <parameter> bodies."""
    val = val_text.strip()
    if ptype == "string":
        return val
    try:
        return json.loads(val)
    except Exception:
        try:
            return ast.literal_eval(val)
        except Exception:
            return val


def parse_tool_calls(text, allow=None, schemas=None):
    """Parse Qwen3-Coder XML tool calls into OpenAI tool_calls.
    allow   = advertised tool names; calls to others are dropped.
    schemas = {tool_name: {param_name: json_type}} for typed coercion.
    """
    schemas = schemas or {}
    calls = []
    for i, m in enumerate(_FUNC_RE.finditer(text)):
        name = m.group(1).strip()
        if allow is not None and name not in allow:
            continue
        ptypes = schemas.get(name, {})
        args = {}
        for pm in _PARAM_RE.finditer(m.group(2)):
            key = pm.group(1).strip()
            args[key] = _coerce(pm.group(2), ptypes.get(key))
        calls.append({"id": f"call_{int(time.time()*1000)}_{i}", "type": "function",
                      "function": {"name": name, "arguments": json.dumps(args, ensure_ascii=False)}})
    return calls


def _flatten_content(c):
    if isinstance(c, str):
        return c
    if isinstance(c, list):
        return "".join(p.get("text", "") if isinstance(p, dict) and p.get("type") == "text"
                       else (p if isinstance(p, str) else "") for p in c)
    return "" if c is None else str(c)


def _normalize(messages):
    """Adapt OpenAI message shapes to the chat template:
    - content list-of-parts -> string
    - assistant tool_calls: arguments JSON string -> dict
    - role 'tool' -> kept as-is (Qwen template handles tool role natively)
    """
    out = []
    for m in messages:
        nm = {**m, "content": _flatten_content(m.get("content"))}
        if nm.get("tool_calls"):
            tcs = []
            for tc in nm["tool_calls"]:
                fn = dict(tc.get("function", {}))
                a = fn.get("arguments")
                if isinstance(a, str):
                    try:
                        fn["arguments"] = json.loads(a)
                    except Exception:
                        fn["arguments"] = {}
                tcs.append({**tc, "function": fn})
            nm["tool_calls"] = tcs
            if nm.get("content") is None:
                nm["content"] = ""
        out.append(nm)
    return out


def _tool_fn(t):
    if isinstance(t, dict) and t.get("type") == "function":
        return t.get("function", t)
    return t


def _schemas_from_tools(tools):
    schemas = {}
    for t in tools or []:
        fn = _tool_fn(t)
        if not isinstance(fn, dict):
            continue
        name = fn.get("name")
        props = ((fn.get("parameters") or {}).get("properties") or {})
        if name:
            schemas[name] = {k: (v or {}).get("type") for k, v in props.items()}
    return schemas


def run(messages, tools, temperature, max_tokens, enable_thinking=False):
    messages = _normalize(messages)
    kw = dict(add_generation_prompt=True, tokenize=False)
    try:
        text = TOK.apply_chat_template(messages, enable_thinking=enable_thinking, **(
            {"tools": [_tool_fn(t) for t in tools]} if tools else {}), **kw)
    except TypeError:
        # template without enable_thinking kwarg
        text = TOK.apply_chat_template(messages, **(
            {"tools": [_tool_fn(t) for t in tools]} if tools else {}), **kw)
    # Qwen official sampling; greedy is discouraged. think vs non-think presets.
    if temperature is not None:
        temp, top_p = float(temperature), 0.8 if not enable_thinking else 0.95
    elif enable_thinking:
        temp, top_p = 1.0, 0.95
    else:
        temp, top_p = 0.7, 0.8
    sampler = make_sampler(temp=max(0.0, temp), top_p=top_p, top_k=20)
    with LOCK:
        out = generate(MODEL, TOK, prompt=text,
                       max_tokens=int(max_tokens or DEFAULT_MAX_TOKENS),
                       sampler=sampler, verbose=False)
    final = out.rsplit("</think>", 1)[-1].strip() if "</think>" in out else out.strip()
    allow, schemas = None, None
    if tools:
        schemas = _schemas_from_tools(tools)
        allow = set(schemas.keys())
    calls = parse_tool_calls(final, allow, schemas) or parse_tool_calls(out, allow, schemas)
    if calls:
        content = re.split(r"<tool_call>|<function\s*=", final)[0].strip()
        return {"role": "assistant", "content": content or None, "tool_calls": calls}, "tool_calls"
    return {"role": "assistant", "content": final}, "stop"


class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _json(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def do_GET(self):
        if self.path.startswith("/v1/models"):
            self._json(200, {"object": "list", "data": [{"id": "qwen3.5-0.8b", "object": "model"}]})
        else:
            self._json(404, {"error": "not found"})

    def do_POST(self):
        if not self.path.startswith("/v1/chat/completions"):
            return self._json(404, {"error": "not found"})
        try:
            raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            body = json.loads(raw)
        except Exception as e:
            return self._json(400, {"error": f"bad request: {e}"})
        try:
            # NOTE: do NOT log request messages — local, privacy-preserving model.
            think = body.get("enable_thinking")
            if think is None:
                think = (body.get("chat_template_kwargs") or {}).get("enable_thinking", False)
            msg, finish = run(body.get("messages", []), body.get("tools"),
                              body.get("temperature"),
                              body.get("max_tokens") or DEFAULT_MAX_TOKENS,
                              enable_thinking=bool(think))
        except Exception as e:
            import traceback
            traceback.print_exc()
            return self._json(500, {"error": f"generation failed: {e}"})
        cid = f"chatcmpl-{int(time.time()*1000)}"
        if body.get("stream"):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.end_headers()

            def send(d):
                self.wfile.write(f"data: {json.dumps(d)}\n\n".encode())
                self.wfile.flush()

            base = {"id": cid, "object": "chat.completion.chunk", "model": "qwen3.5-0.8b",
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
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        else:
            self._json(200, {"id": cid, "object": "chat.completion", "model": "qwen3.5-0.8b",
                "choices": [{"index": 0, "message": msg, "finish_reason": finish}],
                "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}})


if __name__ == "__main__":
    print(f"serving on http://{HOST}:{PORT}/v1", flush=True)
    http.server.ThreadingHTTPServer((HOST, PORT), H).serve_forever()
