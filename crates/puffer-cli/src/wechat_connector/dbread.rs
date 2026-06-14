//! Optional direct chat-DB reader (cheaper + more accurate than vision).
//!
//! WeChat 4.x stores messages in SQLCipher4 DBs under
//! `/config/xwechat_files/wxid_*/db_storage`. This reads them without touching
//! the UI or Tencent's servers: the 32-byte raw key is extracted ONCE from WeChat
//! process memory (`/proc/<pid>/mem`, needs the `SYS_PTRACE` cap the container
//! gets while the reader is enabled), cached (`~/.puffer/wechat/<slug>.dbkey`, 0600), and
//! reused (re-extracted only if a cached key stops working, e.g. after re-login).
//! The decrypt + SQLite query run as a Python helper inside the container
//! (stdlib `sqlite3`; pycryptodome for AES; zstandard for compressed text).
//!
//! Two reads:
//! - `read_session` (monitor): `session/session.db` → per-conversation latest
//!   summary + timestamp → new-message events.
//! - `read_history`: `message/message_*.db` shards → last N messages of a chat
//!   with sender / text / time / type / outgoing.
//!
//! On by default; disable with `WECHAT_ENABLE_DB_READ=0`. Validate with `db-probe`.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::docker::WechatInstance;

/// Whether the direct DB reader is enabled. ON by default; disable with
/// `WECHAT_ENABLE_DB_READ=0` (also `false`/`no`/`off`).
pub(crate) fn enabled() -> bool {
    match std::env::var("WECHAT_ENABLE_DB_READ") {
        Ok(v) => !matches!(v.trim(), "0" | "false" | "no" | "off"),
        Err(_) => true,
    }
}

/// One message read from the chat DB.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct DbMessage {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) sender: String,
    #[serde(default)]
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) create_time: f64,
    #[serde(default)]
    pub(crate) msg_type: String,
    #[serde(default)]
    pub(crate) outgoing: bool,
    /// wxids @-mentioned by this message (from the `source` XML `<atuserlist>`),
    /// empty for messages with no @-mentions.
    #[serde(default)]
    pub(crate) at_users: Vec<String>,
    /// Whether the logged-in user is among the @-mentioned (i.e. "someone @'d me").
    #[serde(default)]
    pub(crate) mentioned_me: bool,
}

/// Result of a session poll: new messages plus the advanced cursor.
pub(crate) struct DbRead {
    pub(crate) messages: Vec<DbMessage>,
    pub(crate) cursor: f64,
}

#[derive(Deserialize)]
struct RawResult {
    ok: bool,
    #[serde(default)]
    error: String,
    /// Set by the helper when none of the supplied keys decrypt the target DB.
    #[serde(default)]
    key_invalid: bool,
    #[serde(default)]
    messages: Vec<DbMessage>,
    #[serde(default)]
    cursor: f64,
    /// All candidate enc keys (hex), only in `extract-key` mode. WeChat 4.x uses
    /// a DIFFERENT key per DB, so we cache them all and pick per-DB at read time.
    #[serde(default)]
    keys: Vec<String>,
}

/// Monitor read: conversations whose latest message is newer than `since`.
pub(crate) async fn read_session(instance: &WechatInstance, since: f64) -> Result<DbRead> {
    let since_arg = format!("{since}");
    let raw = run_with_key(instance, "session", &[&since_arg]).await?;
    Ok(DbRead {
        messages: raw.messages,
        cursor: raw.cursor,
    })
}

/// On-demand read: the last `limit` messages of `chat` (a wxid / `*@chatroom`,
/// or a display name the helper resolves via contact.db).
pub(crate) async fn read_history(
    instance: &WechatInstance,
    chat: &str,
    limit: u32,
) -> Result<Vec<DbMessage>> {
    let limit_arg = limit.clamp(1, 1000).to_string();
    let raw = run_with_key(instance, "history", &[chat, &limit_arg]).await?;
    Ok(raw.messages)
}

/// Runs a keyed helper mode, passing ALL cached keys (the helper picks the one
/// that decrypts each DB) and re-extracting ONCE if none work (re-login).
async fn run_with_key(instance: &WechatInstance, mode: &str, extra: &[&str]) -> Result<RawResult> {
    instance
        .ensure_dbread_tools()
        .await
        .context("install DB-read deps")?;
    let mut keys = ensure_keys(instance).await?;
    for attempt in 0..2 {
        let csv = keys.join(",");
        let mut args: Vec<&str> = vec![mode];
        args.extend_from_slice(extra);
        // Keys travel via env (WECHAT_DBKEYS), not argv (see exec_python).
        let raw = exec(instance, &args, &[("WECHAT_DBKEYS", csv.as_str())]).await?;
        if raw.key_invalid && attempt == 0 {
            // Cached keys stopped working (likely a re-login) — re-extract once.
            keys = extract_keys(instance).await?;
            continue;
        }
        if !raw.ok {
            bail!("dbread {mode}: {}", raw.error);
        }
        return Ok(raw);
    }
    bail!("dbread {mode}: keys invalid even after re-extraction")
}

/// Returns the cached keys, extracting + caching them on first use.
async fn ensure_keys(instance: &WechatInstance) -> Result<Vec<String>> {
    let cached = cached_keys(instance);
    if !cached.is_empty() {
        return Ok(cached);
    }
    extract_keys(instance).await
}

/// Extracts all candidate keys from WeChat memory and caches them (0600).
async fn extract_keys(instance: &WechatInstance) -> Result<Vec<String>> {
    let raw = exec(instance, &["extract-key"], &[]).await?;
    if !raw.ok || raw.keys.is_empty() {
        bail!("dbread key extraction failed: {}", raw.error);
    }
    store_keys(instance, &raw.keys);
    Ok(raw.keys)
}

/// Runs the helper with `args` (+ `env`) and parses the single JSON line.
async fn exec(instance: &WechatInstance, args: &[&str], env: &[(&str, &str)]) -> Result<RawResult> {
    let (ok, stdout, stderr) = instance.exec_python(DBREAD_PY, args, env).await?;
    let line = stdout.trim();
    if line.is_empty() {
        bail!("dbread produced no output (exit ok={ok}): {}", stderr.trim());
    }
    serde_json::from_str(line).with_context(|| format!("parse dbread output: {line}"))
}

/// `~/.puffer/wechat/<instance>.dbkey` (honors WECHAT_STATE_DIR/PUFFER_HOME/HOME).
fn key_path(instance: &WechatInstance) -> Option<PathBuf> {
    let name = instance.name();
    if let Ok(dir) = std::env::var("WECHAT_STATE_DIR") {
        if !dir.trim().is_empty() {
            return Some(PathBuf::from(dir).join(format!("{name}.dbkey")));
        }
    }
    let home = std::env::var("PUFFER_HOME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| std::env::var("HOME").ok())?;
    Some(
        PathBuf::from(home)
            .join(".puffer")
            .join("wechat")
            .join(format!("{name}.dbkey")),
    )
}

fn cached_keys(instance: &WechatInstance) -> Vec<String> {
    key_path(instance)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|raw| {
            raw.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Persists the keys (one hex per line) with owner-only (0600) permissions,
/// created 0600 from the start (no world/group-readable window) under a 0700 dir.
fn store_keys(instance: &WechatInstance, keys: &[String]) {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let Some(path) = key_path(instance) else {
        return;
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
        // Restrict the containing dir too (best-effort).
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }
    // Create with mode 0600 atomically so the secret is never briefly readable.
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
    {
        let _ = file.write_all(keys.join("\n").as_bytes());
    }
}

/// The in-container Python helper (stdlib + pycryptodome [+ zstandard]).
/// Keys arrive via the `WECHAT_DBKEYS` env var (CSV of 64-hex keys), NOT argv.
/// Modes (argv[1]):
///   extract-key              -> {ok, keys}
///   session   <since>        -> {ok, messages, cursor}
///   history   <chat> <limit> -> {ok, messages}
/// Any keyed mode prints {ok:false, key_invalid:true} if no key decrypts.
const DBREAD_PY: &str = r#"
import sys, os, re, json, hashlib, hmac, struct, glob, sqlite3, tempfile
try:
    from Crypto.Cipher import AES
except Exception:
    try:
        from Cryptodome.Cipher import AES
    except Exception:
        AES = None
try:
    import zstandard as _zstd
    def zdec(b):
        try: return _zstd.ZstdDecompressor().decompress(b)
        except Exception: return None
except Exception:
    zdec = lambda b: None

PAGE=4096

def out(o):
    sys.stdout.write(json.dumps(o, ensure_ascii=False)); sys.exit(0)

def db_root():
    c=glob.glob('/config/xwechat_files/wxid_*/db_storage')
    return c[0] if c else None

def pids():
    r=[]
    for d in glob.glob('/proc/[0-9]*'):
        try: comm=open(d+'/comm').read().strip().lower()
        except Exception: continue
        if comm in ('weixin','wechat','wechatappex'):
            pid=int(d.rsplit('/',1)[-1])
            try: rss=int(open(d+'/statm').read().split()[1])
            except Exception: rss=0
            r.append((rss,pid))
    r.sort(reverse=True)
    return [p for _,p in r]

def regions(pid):
    res=[]
    try:
        for line in open('/proc/%d/maps'%pid):
            p=line.split()
            if len(p)<2: continue
            addr,perms=p[0],p[1]
            if 'r' not in perms: continue
            name=p[5] if len(p)>=6 else ''
            lo=name.lower()
            if name in ('[vdso]','[vsyscall]','[vvar]'): continue
            if any(name.startswith(x) for x in ('/usr/lib/','/lib/','/usr/share/')) and not any(k in lo for k in ('wcdb','wechat','weixin')): continue
            try: a,b=addr.split('-'); s=int(a,16); e=int(b,16)
            except Exception: continue
            if e-s<=0 or e-s>500*1024*1024: continue
            res.append((s,e-s))
    except Exception: pass
    return res

KEYRE=re.compile(rb"x'([0-9a-fA-F]{64,192})'")
def scan_keys(pid):
    cands=set()
    try: mem=open('/proc/%d/mem'%pid,'rb',0)
    except Exception: return cands
    for s,sz in regions(pid):
        try: mem.seek(s); buf=mem.read(sz)
        except Exception: continue
        for m in KEYRE.finditer(buf): cands.add(m.group(1).decode())
    mem.close()
    return cands

def cand_keys(hx):
    n=len(hx)
    if n==64: return [(bytes.fromhex(hx),None)]
    if n>=96 and n%2==0: return [(bytes.fromhex(hx[:64]),bytes.fromhex(hx[64:96]))]
    return []

def mac_key(enc,salt):
    ms=bytes(b^0x3a for b in salt)
    return hashlib.pbkdf2_hmac('sha512',enc,ms,2,dklen=32)

def validate(page1,enc):
    salt=page1[:16]; mk=mac_key(enc,salt)
    h=hmac.new(mk,page1[16:4032],hashlib.sha512); h.update(struct.pack('<I',1))
    return h.digest()==page1[4032:4096]

def decrypt(path,enc):
    data=open(path,'rb').read()
    if len(data)<PAGE: return None
    res=bytearray(); n=len(data)//PAGE
    for i in range(n):
        pg=data[i*PAGE:(i+1)*PAGE]; iv=pg[4016:4032]
        if i==0:
            pt=AES.new(enc,AES.MODE_CBC,iv).decrypt(pg[16:4016])
            res+=b"SQLite format 3\x00"+pt+b"\x00"*80
        else:
            pt=AES.new(enc,AES.MODE_CBC,iv).decrypt(pg[0:4016])
            res+=pt+b"\x00"*80
    return bytes(res)

def open_plain(plain):
    tf=tempfile.NamedTemporaryFile(delete=False,suffix='.db'); tf.write(plain); tf.close()
    con=sqlite3.connect(tf.name); con.row_factory=sqlite3.Row
    return con,tf.name

def has_table(con,name):
    return con.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",(name,)).fetchone() is not None

def cols(con,table):
    try: return [r[1] for r in con.execute('PRAGMA table_info("%s")'%table)]
    except Exception: return []

TYPEMAP={1:'text',3:'[image]',34:'[voice]',42:'[card]',43:'[video]',47:'[sticker]',48:'[location]',49:'[link/file]',50:'[call]',10000:'[system]',10002:'[recall]'}
def decode_content(blob,ct):
    if blob is None: return ''
    if isinstance(blob,str): return blob
    b=bytes(blob)
    try: ict=int(ct) if ct is not None else 0
    except Exception: ict=0
    if ict==4:
        d=zdec(b)
        if d is not None: b=d
    try: return b.decode('utf-8','ignore')
    except Exception: return ''

# ---- main ----
def need_session_key():
    root=db_root()
    if not root: out({"ok":False,"error":"db_storage not found (not logged in?)"})
    if AES is None: out({"ok":False,"error":"pycryptodome not installed"})
    sess=os.path.join(root,'session','session.db')
    if not os.path.exists(sess): out({"ok":False,"error":"session.db missing"})
    p1=open(sess,'rb').read(PAGE)
    if len(p1)<PAGE: out({"ok":False,"error":"session.db too small"})
    return root,sess,p1

def parse_keys(csv):
    r=[]
    for x in (csv or '').split(','):
        x=x.strip()
        if len(x)==64:
            try: r.append(bytes.fromhex(x))
            except Exception: pass
    return r

def pick(page1,keys):
    # WeChat 4.x uses a DIFFERENT key per DB; pick the one that validates this DB.
    for e in keys:
        if validate(page1,e): return e
    return None

def main():
    mode=sys.argv[1] if len(sys.argv)>1 else 'session'

    if mode=='extract-key':
        root,sess,p1=need_session_key()
        allkeys=set()
        for pid in pids():
            for hx in scan_keys(pid):
                for enc,_cs in cand_keys(hx):
                    allkeys.add(enc.hex())
        if not any(validate(p1,bytes.fromhex(k)) for k in allkeys):
            out({"ok":False,"error":"no key in memory decrypts session.db (logged in? SYS_PTRACE/root?)"})
        out({"ok":True,"keys":sorted(allkeys)})

    root,sess,p1=need_session_key()
    # Keys arrive via env (WECHAT_DBKEYS), not argv, so they are not exposed in
    # /proc/<pid>/cmdline to the untrusted WeChat process. argv carries only the
    # mode and non-secret params.
    keys=parse_keys(os.environ.get('WECHAT_DBKEYS',''))
    if not keys: out({"ok":False,"error":"no keys given"})
    enc=pick(p1,keys)
    if not enc: out({"ok":False,"key_invalid":True,"error":"no supplied key decrypts session.db"})

    if mode=='session':
        try: since=float(sys.argv[2]) if len(sys.argv)>2 and sys.argv[2] else 0.0
        except Exception: since=0.0
        plain=decrypt(sess,enc)
        if not plain: out({"ok":False,"error":"decrypt failed"})
        con,tmp=open_plain(plain); msgs=[]; cursor=since
        try:
            if has_table(con,'SessionTable'):
                c=set(cols(con,'SessionTable'))
                tcol='last_timestamp' if 'last_timestamp' in c else None
                for r in con.execute("SELECT * FROM SessionTable"):
                    d=dict(r); ts=d.get(tcol) if tcol else 0
                    try: ts=float(ts)
                    except Exception: ts=0.0
                    if ts>cursor: cursor=ts
                    # Inclusive (>=): a conversation that ties the cursor's whole-
                    # second timestamp must still be returned; the Rust side keeps a
                    # dedup set so the unchanged row isn't re-emitted.
                    if ts>=since and ts>0:
                        txt=d.get('summary') or ''
                        if isinstance(txt,(bytes,bytearray)):
                            try: txt=txt.decode('utf-8','ignore')
                            except Exception: txt=''
                        sender=d.get('last_sender_display_name') or d.get('username') or ''
                        msgs.append({"chat":d.get('username',''),"sender":sender,"text":str(txt),"create_time":ts})
        finally:
            con.close()
            try: os.unlink(tmp)
            except Exception: pass
        out({"ok":True,"messages":msgs,"cursor":cursor})

    if mode=='history':
        chat=sys.argv[2] if len(sys.argv)>2 else ''
        try: limit=int(sys.argv[3]) if len(sys.argv)>3 else 50
        except Exception: limit=50
        if not chat: out({"ok":False,"error":"history requires a chat"})
        # Resolve display name -> username via contact.db if needed.
        username=chat
        if not (chat.startswith('wxid_') or chat.endswith('@chatroom')):
            cpath=os.path.join(root,'contact','contact.db')
            if os.path.exists(cpath):
                cp1=open(cpath,'rb').read(PAGE)
                cenc=pick(cp1,keys)
                if cenc:
                    pl=decrypt(cpath,cenc)
                    if pl:
                        cc,ct=open_plain(pl)
                        try:
                            if has_table(cc,'contact'):
                                exact=[]; partial=[]
                                for r in cc.execute("SELECT username,remark,nick_name FROM contact"):
                                    d=dict(r)
                                    rk=d.get('remark') or ''; nk=d.get('nick_name') or ''
                                    un=d.get('username') or ''
                                    if not un: continue
                                    if chat==rk or chat==nk: exact.append(un)
                                    elif chat and (chat in rk or chat in nk): partial.append(un)
                                # Prefer an EXACT remark/nick match; fall back to a
                                # substring match ONLY when it is unambiguous (exactly
                                # one), else keep `chat` as-is rather than guessing and
                                # reading the wrong contact's history (confused deputy).
                                if len(set(exact))==1: username=exact[0]
                                elif not exact and len(set(partial))==1: username=partial[0]
                        except Exception: pass
                        finally:
                            cc.close()
                            try: os.unlink(ct)
                            except Exception: pass
        table='Msg_'+hashlib.md5(username.encode()).hexdigest()
        is_group=username.endswith('@chatroom')
        # Determine MY OWN wxid so `outgoing` is correct in GROUPS too (the 1-on-1
        # shortcut "sender != chat partner" can't identify self among group members).
        # The data dir is /config/xwechat_files/<self_wxid>[_NNNN]/db_storage: the
        # real wxid is the dir name, sometimes with a trailing _<digits> account
        # index. Strip ONLY a trailing _<digits> group (NOT any _segment — that
        # would turn a normal `wxid_abc` into `wxid`). Try the full name first,
        # then the stripped form; pick whichever actually appears as a sender, and
        # fall back to the FULL dir name (never the over-stripped form).
        selfdir=os.path.basename(os.path.dirname(root))
        self_cands=[selfdir]
        stripped=re.sub(r'_\d+$','',selfdir)
        if stripped and stripped!=selfdir: self_cands.append(stripped)
        rows=[]; senders=set()
        for shard in sorted(glob.glob(os.path.join(root,'message','message_*.db'))):
            sp1=open(shard,'rb').read(PAGE)
            senc=pick(sp1,keys)
            if len(sp1)<PAGE or not senc: continue
            pl=decrypt(shard,senc)
            if not pl: continue
            con,tmp=open_plain(pl)
            try:
                if not has_table(con,table): continue
                name2id={}
                if has_table(con,'Name2Id'):
                    for r in con.execute("SELECT rowid,user_name FROM Name2Id"):
                        name2id[r[0]]=r[1]
                cset=set(cols(con,table))
                ctcol='WCDB_CT_message_content' if 'WCDB_CT_message_content' in cset else ('ct' if 'ct' in cset else None)
                srccol='source' if 'source' in cset else None
                srcctcol='WCDB_CT_source' if 'WCDB_CT_source' in cset else None
                sel=['local_id','local_type','create_time','real_sender_id','message_content']
                sel=[x for x in sel if x in cset]
                if ctcol: sel.append(ctcol)
                if srccol: sel.append(srccol)
                if srcctcol: sel.append(srcctcol)
                if 'create_time' not in cset: continue
                q='SELECT %s FROM "%s" ORDER BY create_time DESC LIMIT %d'%(','.join('"%s"'%x for x in sel),table,limit)
                for r in con.execute(q):
                    d=dict(r)
                    t=d.get('local_type') or 0
                    try: base=int(t)&0xFFFFFFFF
                    except Exception: base=0
                    sid=d.get('real_sender_id')
                    suname=name2id.get(sid,'') if sid is not None else ''
                    if suname: senders.add(suname)
                    txt=''
                    if base==1:
                        txt=decode_content(d.get('message_content'), d.get(ctcol) if ctcol else None)
                        if is_group and ':\n' in txt[:64]:
                            txt=txt.split(':\n',1)[1]
                    else:
                        txt=TYPEMAP.get(base,'[type %d]'%base)
                    try: ct_=float(d.get('create_time') or 0)
                    except Exception: ct_=0.0
                    # @-mentions: the `source` XML carries <atuserlist>wxid,wxid</atuserlist>.
                    at_users=[]
                    if srccol:
                        srctxt=decode_content(d.get(srccol), d.get(srcctcol) if srcctcol else None)
                        m=re.search(r'<atuserlist>(.*?)</atuserlist>', srctxt, re.S)
                        if m:
                            at_users=[u.strip() for u in re.split(r'[,\s]+', m.group(1)) if u.strip()]
                    rows.append({"chat":username,"sender":suname or username,"text":txt,"create_time":ct_,"msg_type":TYPEMAP.get(base,str(base)),"at_users":at_users,"_su":suname})
            finally:
                con.close()
                try: os.unlink(tmp)
                except Exception: pass
        # Self = the candidate that shows up as an actual sender; else the FULL dir
        # name (self_cands[0]) — never the stripped form, which could be a prefix
        # that wrongly matches other senders.
        self_wxid=next((c for c in self_cands if c in senders), self_cands[0])
        for row in rows:
            su=row.pop("_su","")
            row["outgoing"]=bool(su) and su==self_wxid
            row["mentioned_me"]=self_wxid in row.get("at_users",[])
        rows.sort(key=lambda r:r["create_time"], reverse=True)
        out({"ok":True,"messages":rows[:limit]})

    out({"ok":False,"error":"unknown mode %r"%mode})

try:
    main()
except SystemExit:
    raise
except Exception as e:
    sys.stdout.write(json.dumps({"ok":False,"error":"unexpected: %r"%e}))
"#;
