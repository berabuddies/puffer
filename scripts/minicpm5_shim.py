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
import ast, json, re, time, os, http.server, pathlib, threading
import xml.etree.ElementTree as ET
from mlx_lm import load, generate
from mlx_lm.sample_utils import make_sampler

MD = os.environ.get("MINICPM5_MODEL") or str(pathlib.Path(__file__).parent / "model")
HOST, PORT = "127.0.0.1", 8088

print("loading model ...", flush=True)
MODEL, TOK = load(MD)
LOCK = threading.Lock()  # mlx model is single-stream; serialize requests
print("model ready", flush=True)

# Tool-call parser ported from the official MiniCPM5 adapters
# (SGLang MiniCPM5Detector + vLLM minicpm5xml_tool_parser). The model emits
# XML-style calls: <function name="f"><param name="k">v</param></function>.
# Robustness this buys over a naive regex (all observed from small-model output):
#   - single OR double quotes, and trailing attributes on tags
#   - real XML parse (handles CDATA / multiline / nested), regex fallback
#   - tokenizer artefacts: U+0120 (space), U+010A (newline), collapsed tags
#   - schema-typed coercion: string args stay literal, others JSON-parsed
#   - OpenAI-style wrapper params (<param name="properties">{...}</param>) unwrapped
_FUNC_BLOCK_RE = re.compile(r"<function.*?</function>", re.S)
_FUNC_NAME_RE = re.compile(r"<function\s+name=['\"]([^'\"]+)['\"][^>]*>")
_PARAM_RE = re.compile(r"<param\s+name=['\"]([^'\"]+)['\"][^>]*>([\s\S]*?)</param>", re.S)
_PARAM_NO_NAME_RE = re.compile(r"<param(?![^>]*\bname=)[^>]*>", re.S)
_CDATA_RE = re.compile(r"^\s*<!\[CDATA\[([\s\S]*?)\]\]>\s*$")
_TOK_SPACE, _TOK_NL = "Ġ", "Ċ"

def _normalize_output(text):
    """Repair tokenizer artefacts before parsing (vLLM _normalize_model_output)."""
    if (_TOK_SPACE not in text and _TOK_NL not in text
            and "<functionname=" not in text and "<paramname=" not in text):
        return text
    return (text.replace(_TOK_SPACE, " ").replace(_TOK_NL, "\n")
                .replace("<functionname=", "<function name=")
                .replace("<paramname=", "<param name="))

def _unwrap_cdata(v):
    m = _CDATA_RE.match(v)
    return m.group(1) if m else v

def _coerce(val_text, ptype):
    """Coerce a param value by its declared JSON-schema type. string -> literal
    (never json.loads, so '0123'/'true'/'null' survive); others -> parsed."""
    val = _unwrap_cdata(val_text)
    if ptype == "string":
        return val.strip()
    val = val.strip()
    try:
        return json.loads(val)
    except Exception:
        try:
            return ast.literal_eval(val)
        except Exception:
            return val

def _params_from_block(block):
    """Yield (key, raw_value) pairs from a <function> block. Prefers real XML
    parsing; falls back to regex when the block isn't well-formed XML."""
    try:
        root = ET.fromstring(block)
        if root.tag == "function":
            for p in list(root):
                if p.tag != "param":
                    continue
                key = p.attrib.get("name")
                if key:
                    yield key, (p.text or "")
            return
    except Exception:
        pass
    for m in _PARAM_RE.finditer(block):
        yield m.group(1), m.group(2)

def parse_tool_calls(text, allow=None, schemas=None):
    """Parse XML tool calls into OpenAI tool_calls.
    allow   = set of advertised tool names; calls to unadvertised tools are
              dropped (untrusted model/tool text can't fabricate calls).
    schemas = {tool_name: {param_name: json_type}} used for typed coercion.
    """
    text = _normalize_output(text)
    schemas = schemas or {}
    calls = []
    for i, block in enumerate(_FUNC_BLOCK_RE.findall(text)):
        m = _FUNC_NAME_RE.search(block)
        if not m:
            continue
        name = m.group(1).strip()
        if allow is not None and name not in allow:
            continue
        ptypes = schemas.get(name, {})
        allowed = set(ptypes.keys())
        # A <param> with no name= means the model produced a malformed call;
        # match the official parsers and drop it rather than emit junk args.
        if _PARAM_NO_NAME_RE.search(block):
            continue
        args, seen, bad = {}, set(), False
        for key, raw in _params_from_block(block):
            # OpenAI-style wrapper: <param name="properties">{...}</param>.
            if key in ("properties", "arguments") and allowed and key not in allowed:
                wrapped = _coerce(raw, None)
                if isinstance(wrapped, dict):
                    for wk, wv in wrapped.items():
                        if allowed and wk not in allowed:
                            continue
                        args[wk] = wv if not isinstance(wv, str) else _coerce(wv, ptypes.get(wk))
                    continue
            if allowed and key not in allowed:
                continue  # ignore unknown params, keep the call
            if key in seen:
                bad = True
                break
            seen.add(key)
            args[key] = _coerce(raw, ptypes.get(key))
        if bad:
            continue
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

# Generous default so the model's thinking has room to finish AND still emit the
# tool call. A small budget truncates mid-reasoning -> the <function> call never
# lands -> the turn silently looks like "no tool call" (observed at 512 tokens).
DEFAULT_MAX_TOKENS = 4096

def _tool_fn(t):
    """Normalize an OpenAI tool entry to the bare {name,description,parameters}."""
    if isinstance(t, dict) and t.get("type") == "function":
        return t.get("function", t)
    return t

def _schemas_from_tools(tools):
    """Build {tool_name: {param_name: json_type}} for typed argument coercion."""
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

def run(messages, tools, temperature, max_tokens, enable_thinking=True):
    messages = _normalize(messages)
    kw = dict(add_generation_prompt=True, tokenize=False, enable_thinking=enable_thinking)
    if tools:
        kw["tools"] = [_tool_fn(t) for t in tools]
    text = TOK.apply_chat_template(messages, **kw)
    # Follow the model card's recommended sampling instead of greedy decoding.
    # MiniCPM5-1B is tuned for do_sample=True; temp=0 greedy collapses into
    # repetition loops (the model rambles and never emits the tool call). When
    # the caller doesn't pin a temperature, use the official defaults:
    #   think mode -> temp 0.9, top_p 0.95 ; no-think -> temp 0.7, top_p 0.95.
    temp = temperature if temperature is not None else (0.9 if enable_thinking else 0.7)
    sampler = make_sampler(temp=max(0.0, float(temp)), top_p=0.95)
    with LOCK:
        out = generate(MODEL, TOK, prompt=text,
                       max_tokens=int(max_tokens or DEFAULT_MAX_TOKENS),
                       sampler=sampler, verbose=False)
    # Strip thinking for the surfaced content/parse (keep the final segment).
    final = out.rsplit("</think>", 1)[-1].strip() if "</think>" in out else out.strip()
    allow, schemas = None, None
    if tools:
        schemas = _schemas_from_tools(tools)
        allow = set(schemas.keys())
    # Parse the post-think segment first; fall back to the full output in case a
    # call leaked into the thinking block.
    calls = parse_tool_calls(final, allow, schemas) or parse_tool_calls(out, allow, schemas)
    if calls:
        # Content before the first <function> tag (usually empty).
        content = _FUNC_BLOCK_RE.split(_normalize_output(final))[0].strip()
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
            # Pass temperature through as-is (None when unset) so run() can apply
            # the model card's recommended default per think/no-think mode.
            msg, finish = run(body.get("messages", []), body.get("tools"),
                              body.get("temperature"),
                              body.get("max_tokens") or DEFAULT_MAX_TOKENS,
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
