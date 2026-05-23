use anyhow::{anyhow, Context, Result};
use puffer_resources::{SkillSpec, SkillVerificationSpec};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashMap};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) const LAMBDA_SKILL_COMPILER_ENV: &str = "PUFFER_LSKILLC";
pub(crate) const LAMBDA_SKILL_GATE_ENV: &str = "PUFFER_LAMBDA_SKILL_GATE";

/// One structured host fact tracked by the Lambda Skill call gate.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct LambdaFact {
    pred: String,
    args: Vec<String>,
}

impl LambdaFact {
    /// Creates a fact with the given predicate and payload arguments.
    pub(crate) fn new(pred: impl Into<String>, args: impl Into<Vec<String>>) -> Self {
        Self {
            pred: pred.into(),
            args: args.into(),
        }
    }

    /// Returns the predicate symbol for this fact.
    pub(crate) fn pred(&self) -> &str {
        &self.pred
    }

    /// Returns the fact payload arguments.
    pub(crate) fn args(&self) -> &[String] {
        &self.args
    }
}

/// One tool signature from a Lambda Skill host catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LambdaToolSig {
    name: String,
    params: Vec<LambdaParam>,
    result: String,
    effects: BTreeSet<String>,
    registers: Vec<LambdaFact>,
    context_req: Option<LambdaFact>,
}

impl LambdaToolSig {
    /// Returns the host tool name.
    #[cfg(test)]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Returns the declared effect row.
    #[cfg(test)]
    pub(crate) fn effects(&self) -> &BTreeSet<String> {
        &self.effects
    }

    /// Returns the required standing fact, if the tool has one.
    #[cfg(test)]
    pub(crate) fn context_req(&self) -> Option<&LambdaFact> {
        self.context_req.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LambdaParam {
    name: String,
    ty: String,
}

/// Parsed Lambda Skill host catalogue emitted by `lskillc export-json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LambdaHostEnv {
    effects: BTreeSet<String>,
    domains: Vec<String>,
    tools: HashMap<String, LambdaToolSig>,
}

impl LambdaHostEnv {
    /// Parses the JSON shape emitted by `lskillc export-json`.
    pub(crate) fn from_json_str(raw: &str) -> Result<Self> {
        let parsed: HostEnvJson =
            serde_json::from_str(raw).context("failed to parse Lambda Skill host catalogue")?;
        let mut tools = HashMap::new();
        for tool in parsed.tools {
            let sig = tool.into_sig()?;
            if tools.insert(sig.name.clone(), sig).is_some() {
                return Err(anyhow!("duplicate Lambda Skill host tool"));
            }
        }
        Ok(Self {
            effects: parsed.effects.into_iter().collect(),
            domains: parsed.domains,
            tools,
        })
    }

    /// Returns the declared host effect alphabet.
    #[cfg(test)]
    pub(crate) fn effects(&self) -> &BTreeSet<String> {
        &self.effects
    }

    /// Returns the declared host domains.
    #[cfg(test)]
    pub(crate) fn domains(&self) -> &[String] {
        &self.domains
    }

    /// Looks up a tool signature by host tool name.
    pub(crate) fn lookup_tool(&self, tool: &str) -> Option<&LambdaToolSig> {
        self.tools.get(tool)
    }
}

/// Stateful Lambda Skill gate, mirroring `LambdaW.Trace.GateState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LambdaGateState {
    host: LambdaHostEnv,
    caps: BTreeSet<String>,
    facts: BTreeSet<LambdaFact>,
}

impl LambdaGateState {
    /// Creates a gate with all host-declared effects available.
    pub(crate) fn with_host_caps(host: LambdaHostEnv) -> Self {
        let caps = host.effects.clone();
        Self {
            host,
            caps,
            facts: BTreeSet::new(),
        }
    }

    /// Creates a gate with explicit effect capabilities.
    #[cfg(test)]
    pub(crate) fn with_caps(host: LambdaHostEnv, caps: impl IntoIterator<Item = String>) -> Self {
        Self {
            host,
            caps: caps.into_iter().collect(),
            facts: BTreeSet::new(),
        }
    }

    /// Returns the current standing facts.
    #[cfg(test)]
    pub(crate) fn facts(&self) -> &BTreeSet<LambdaFact> {
        &self.facts
    }

    /// Adds an initial fact to the gate context.
    #[cfg(test)]
    pub(crate) fn add_fact(&mut self, fact: LambdaFact) {
        self.facts.insert(fact);
    }

    /// Admits or rejects one proposed host tool call without mutating state.
    pub(crate) fn admit_call(&self, tool: &str) -> LambdaGateVerdict {
        let Some(sig) = self.host.lookup_tool(tool) else {
            return LambdaGateVerdict::reject(format!("unknown tool: {tool}"));
        };
        if !sig.effects.is_subset(&self.caps) {
            return LambdaGateVerdict::reject(format!(
                "tool effects exceed gate capabilities: {tool}"
            ));
        }
        if let Some(required) = sig.context_req.as_ref() {
            if !self.facts.contains(required) {
                return LambdaGateVerdict::reject(format!(
                    "contextReq not satisfied for {tool}: ({})",
                    required.pred()
                ));
            }
        }
        LambdaGateVerdict::Accept
    }

    /// Gates one call and commits registered facts when accepted.
    pub(crate) fn step_call(&mut self, tool: &str) -> LambdaGateVerdict {
        let verdict = self.admit_call(tool);
        if verdict.is_accept() {
            if let Some(sig) = self.host.lookup_tool(tool) {
                for fact in &sig.registers {
                    self.facts.insert(fact.clone());
                }
            }
        }
        verdict
    }

    /// Builds trace metadata for a committed host call.
    pub(crate) fn committed_host_call_metadata(
        &self,
        host_tool: &str,
        concrete_tool: Option<&str>,
    ) -> Value {
        let registered_facts = self
            .host
            .lookup_tool(host_tool)
            .map(|sig| sig.registers.iter().map(lambda_fact_metadata).collect())
            .unwrap_or_default();
        lambda_skill_metadata(
            "host_call_committed",
            host_tool,
            concrete_tool,
            None,
            registered_facts,
        )
    }
}

/// Builds trace metadata for an admitted bridged host call.
pub(crate) fn admitted_host_call_metadata(
    host_tool: &str,
    concrete_tool: &str,
    concrete_input: Value,
) -> Value {
    lambda_skill_metadata(
        "host_call_admitted",
        host_tool,
        Some(concrete_tool),
        Some(concrete_input),
        Vec::new(),
    )
}

/// Merges Lambda trace metadata into an existing tool metadata value.
pub(crate) fn merge_tool_metadata(existing: &mut Value, addition: Value) {
    if addition.is_null() {
        return;
    }
    if existing.is_null() {
        *existing = addition;
        return;
    }
    match (existing, addition) {
        (Value::Object(target), Value::Object(source)) => target.extend(source),
        (target, Value::Object(source)) => {
            let previous = std::mem::replace(target, Value::Null);
            let mut merged = Map::new();
            merged.insert("tool".to_string(), previous);
            merged.extend(source);
            *target = Value::Object(merged);
        }
        (target, source) => {
            let previous = std::mem::replace(target, Value::Null);
            let mut merged = Map::new();
            merged.insert("tool".to_string(), previous);
            merged.insert("lambda_skill".to_string(), source);
            *target = Value::Object(merged);
        }
    }
}

fn lambda_skill_metadata(
    event: &str,
    host_tool: &str,
    concrete_tool: Option<&str>,
    concrete_input: Option<Value>,
    registered_facts: Vec<Value>,
) -> Value {
    let mut inner = Map::new();
    inner.insert("event".to_string(), Value::String(event.to_string()));
    inner.insert(
        "host_tool".to_string(),
        Value::String(host_tool.to_string()),
    );
    if let Some(tool) = concrete_tool {
        inner.insert("concrete_tool".to_string(), Value::String(tool.to_string()));
    }
    if let Some(input) = concrete_input {
        inner.insert("concrete_input".to_string(), input);
    }
    if !registered_facts.is_empty() {
        inner.insert(
            "registered_facts".to_string(),
            Value::Array(registered_facts),
        );
    }

    let mut outer = Map::new();
    outer.insert("lambda_skill".to_string(), Value::Object(inner));
    Value::Object(outer)
}

fn lambda_fact_metadata(fact: &LambdaFact) -> Value {
    let mut object = Map::new();
    object.insert("pred".to_string(), Value::String(fact.pred().to_string()));
    object.insert(
        "args".to_string(),
        Value::Array(fact.args().iter().cloned().map(Value::String).collect()),
    );
    Value::Object(object)
}

/// One admitted formal host call awaiting its concrete Puffer tool invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingLambdaHostCall {
    host_tool: String,
    concrete_tool: String,
    concrete_input: Value,
}

impl PendingLambdaHostCall {
    /// Creates a pending bridge from one formal host tool to one concrete tool call.
    pub(crate) fn new(
        host_tool: impl Into<String>,
        concrete_tool: impl Into<String>,
        concrete_input: Value,
    ) -> Self {
        Self {
            host_tool: host_tool.into(),
            concrete_tool: concrete_tool.into(),
            concrete_input,
        }
    }

    /// Returns the formal host tool name admitted by the Lambda gate.
    pub(crate) fn host_tool(&self) -> &str {
        &self.host_tool
    }

    /// Returns the concrete Puffer tool name this bridge permits next.
    pub(crate) fn concrete_tool(&self) -> &str {
        &self.concrete_tool
    }

    /// Returns true when the pending bridge permits this concrete call.
    pub(crate) fn permits_concrete_call(&self, tool_id: &str, input: &Value) -> bool {
        self.concrete_tool == tool_id && self.concrete_input == *input
    }
}

/// Builds a runtime gate for a verified Lambda Skill when catalogue data is available.
pub(crate) fn gate_for_verified_skill(skill: &SkillSpec) -> Result<Option<LambdaGateState>> {
    let Some(verification) = skill.verification.as_ref() else {
        return Ok(None);
    };
    if verification.system != "lambda-skill" {
        return Ok(None);
    }
    let Some(raw) = host_catalogue_json_for_verification(verification)? else {
        return Ok(None);
    };
    let host = LambdaHostEnv::from_json_str(&raw).context("failed to parse host catalogue")?;
    Ok(Some(LambdaGateState::with_host_caps(host)))
}

fn host_catalogue_json_for_verification(
    verification: &SkillVerificationSpec,
) -> Result<Option<String>> {
    if let Some(host_catalogue_path) = verification.host_catalogue_path.as_deref() {
        return fs::read_to_string(host_catalogue_path)
            .with_context(|| format!("failed to read {host_catalogue_path}"))
            .map(Some);
    }
    if !lambda_skill_compiler_gate_enabled() {
        return Ok(None);
    }
    let source_path = verification
        .source_path
        .as_deref()
        .ok_or_else(|| anyhow!("Lambda Skill gate requested but formal source path is missing"))?;
    export_host_catalogue_for_source(Path::new(source_path)).map(Some)
}

/// Returns true when Lambda Skill host catalogues should be compiled on demand.
pub(crate) fn lambda_skill_compiler_gate_enabled() -> bool {
    env::var(LAMBDA_SKILL_GATE_ENV)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on" | "compile" | "strict"
            )
        })
        .unwrap_or(false)
}

fn export_host_catalogue_for_source(source_path: &Path) -> Result<String> {
    let compiler = resolve_lskillc_for_source(source_path).ok_or_else(|| {
        anyhow!(
            "Lambda Skill gate requested but lskillc was not found; set {LAMBDA_SKILL_COMPILER_ENV}"
        )
    })?;
    export_host_catalogue_with_compiler(source_path, &compiler)
}

fn export_host_catalogue_with_compiler(source_path: &Path, compiler: &Path) -> Result<String> {
    let output = Command::new(compiler)
        .arg("export-json")
        .arg(source_path)
        .output()
        .with_context(|| format!("failed to run {}", compiler.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "{} export-json {} failed: {}",
            compiler.display(),
            source_path.display(),
            stderr.trim()
        ));
    }
    String::from_utf8(output.stdout).with_context(|| {
        format!(
            "{} export-json {} returned non-UTF-8 output",
            compiler.display(),
            source_path.display()
        )
    })
}

/// Resolves the Lambda Skill compiler available for a formal skill source.
pub(crate) fn resolve_lskillc_for_source(source_path: &Path) -> Option<PathBuf> {
    if let Some(path) = env::var_os(LAMBDA_SKILL_COMPILER_ENV)
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Some(path);
    }
    lskillc_workspace_candidates(source_path)
        .into_iter()
        .find(|path| path.is_file())
        .or_else(|| find_lskillc_in_path())
}

fn lskillc_workspace_candidates(source_path: &Path) -> Vec<PathBuf> {
    source_path
        .ancestors()
        .flat_map(|ancestor| {
            [
                ancestor.join("lean/LambdaW/.lake/build/bin/lskillc"),
                ancestor.join(".lake/build/bin/lskillc"),
            ]
        })
        .collect()
}

fn find_lskillc_in_path() -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|dir| dir.join("lskillc"))
        .find(|path| path.is_file())
}

/// Admission result for one Lambda Skill gate check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LambdaGateVerdict {
    Accept,
    Reject(String),
}

impl LambdaGateVerdict {
    /// Creates a rejection verdict with the provided reason.
    pub(crate) fn reject(reason: impl Into<String>) -> Self {
        Self::Reject(reason.into())
    }

    /// Returns true when the gate accepted the call.
    pub(crate) fn is_accept(&self) -> bool {
        matches!(self, Self::Accept)
    }

    /// Returns the rejection reason, if any.
    #[cfg(test)]
    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Accept => None,
            Self::Reject(reason) => Some(reason.as_str()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct HostEnvJson {
    #[serde(default)]
    effects: Vec<String>,
    #[serde(default)]
    domains: Vec<String>,
    #[serde(default)]
    tools: Vec<ToolSigJson>,
}

#[derive(Debug, Deserialize)]
struct ToolSigJson {
    name: String,
    #[serde(default)]
    params: Vec<ParamJson>,
    #[serde(default)]
    result: String,
    #[serde(default)]
    effects: Vec<String>,
    #[serde(default)]
    registers: Vec<FactJson>,
    #[serde(rename = "contextReq", default)]
    context_req: Option<FactJson>,
}

impl ToolSigJson {
    fn into_sig(self) -> Result<LambdaToolSig> {
        Ok(LambdaToolSig {
            name: self.name,
            params: self
                .params
                .into_iter()
                .map(|param| LambdaParam {
                    name: param.name,
                    ty: param.ty,
                })
                .collect(),
            result: self.result,
            effects: self.effects.into_iter().collect(),
            registers: self
                .registers
                .into_iter()
                .map(FactJson::into_fact)
                .collect(),
            context_req: self.context_req.map(FactJson::into_fact),
        })
    }
}

#[derive(Debug, Deserialize)]
struct ParamJson {
    name: String,
    ty: String,
}

#[derive(Debug, Deserialize)]
struct FactJson {
    pred: String,
    #[serde(default)]
    args: Vec<String>,
}

impl FactJson {
    fn into_fact(self) -> LambdaFact {
        LambdaFact::new(self.pred, self.args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SWAP_HOST_JSON: &str = r#"{
      "effects": ["net_r", "net_w", "sign"],
      "domains": ["TokenAddr"],
      "tools": [
        {
          "name": "get_quote",
          "params": [
            {"name": "from", "ty": "TokenAddr"},
            {"name": "to", "ty": "TokenAddr"}
          ],
          "result": "real{p > 0}",
          "effects": ["net_r"],
          "registers": [],
          "contextReq": null
        },
        {
          "name": "authenticate",
          "params": [],
          "result": "unit",
          "effects": [],
          "registers": [{"pred": "authed", "args": []}],
          "contextReq": null
        },
        {
          "name": "execute_swap",
          "params": [
            {"name": "from", "ty": "TokenAddr"},
            {"name": "to", "ty": "TokenAddr"},
            {"name": "amount", "ty": "real{a > 0}"}
          ],
          "result": "Result<Receipt, SwapErr>",
          "effects": ["net_w", "sign"],
          "registers": [],
          "contextReq": {"pred": "authed", "args": []}
        }
      ]
    }"#;

    #[test]
    fn parses_lskillc_host_catalogue_shape() {
        let host = LambdaHostEnv::from_json_str(SWAP_HOST_JSON).unwrap();
        assert!(host.effects().contains("net_w"));
        assert_eq!(host.domains(), &["TokenAddr".to_string()]);
        let execute = host.lookup_tool("execute_swap").unwrap();
        assert_eq!(execute.name(), "execute_swap");
        assert!(execute.effects().contains("sign"));
        assert_eq!(execute.context_req().unwrap().pred(), "authed");
    }

    #[test]
    fn gate_rejects_unknown_tools() {
        let host = LambdaHostEnv::from_json_str(SWAP_HOST_JSON).unwrap();
        let gate = LambdaGateState::with_host_caps(host);
        let verdict = gate.admit_call("missing_tool");
        assert_eq!(verdict.reason(), Some("unknown tool: missing_tool"));
    }

    #[test]
    fn gate_rejects_missing_capabilities() {
        let host = LambdaHostEnv::from_json_str(SWAP_HOST_JSON).unwrap();
        let gate = LambdaGateState::with_caps(host, ["net_r".to_string()]);
        let verdict = gate.admit_call("execute_swap");
        assert_eq!(
            verdict.reason(),
            Some("tool effects exceed gate capabilities: execute_swap")
        );
    }

    #[test]
    fn gate_tracks_registered_facts_for_context_requirements() {
        let host = LambdaHostEnv::from_json_str(SWAP_HOST_JSON).unwrap();
        let mut gate = LambdaGateState::with_host_caps(host);
        let rejected = gate.admit_call("execute_swap");
        assert_eq!(
            rejected.reason(),
            Some("contextReq not satisfied for execute_swap: (authed)")
        );

        assert!(gate.step_call("authenticate").is_accept());
        assert!(gate
            .facts()
            .contains(&LambdaFact::new("authed", Vec::new())));
        assert!(gate.step_call("execute_swap").is_accept());
    }

    #[test]
    fn gate_can_start_with_initial_facts() {
        let host = LambdaHostEnv::from_json_str(SWAP_HOST_JSON).unwrap();
        let mut gate = LambdaGateState::with_host_caps(host);
        gate.add_fact(LambdaFact::new("authed", Vec::new()));
        assert!(gate.admit_call("execute_swap").is_accept());
    }

    #[test]
    fn gate_for_verified_skill_reads_catalogue_file() {
        let root = tempfile::tempdir().unwrap();
        let catalogue = root.path().join("host.json");
        fs::write(&catalogue, SWAP_HOST_JSON).unwrap();
        let mut skill = SkillSpec::default();
        skill.verification = Some(SkillVerificationSpec {
            system: "lambda-skill".to_string(),
            source_path: None,
            generated_path: None,
            host_catalogue_path: Some(catalogue.display().to_string()),
            tools: None,
            actions: None,
        });

        let gate = gate_for_verified_skill(&skill)
            .unwrap()
            .expect("catalogue should create a gate");

        assert!(gate.admit_call("get_quote").is_accept());
    }

    #[test]
    fn workspace_candidates_include_ahl_popl_lake_binary() {
        let source = Path::new("/repo/skills/vendor/example/skill.lskill");
        let candidates = lskillc_workspace_candidates(source);

        assert!(candidates.contains(&PathBuf::from("/repo/lean/LambdaW/.lake/build/bin/lskillc")));
    }

    #[cfg(unix)]
    #[test]
    fn export_json_uses_external_lskillc_binary() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("skill.lskill");
        let compiler = root.path().join("lskillc");
        fs::write(&source, "host {}\n").unwrap();
        fs::write(
            &compiler,
            format!(
                "#!/bin/sh\nif [ \"$1\" != \"export-json\" ]; then exit 9; fi\nprintf '%s' '{}'\n",
                SWAP_HOST_JSON.replace('\'', "'\\''")
            ),
        )
        .unwrap();
        let mut perms = fs::metadata(&compiler).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&compiler, perms).unwrap();

        let raw = export_host_catalogue_with_compiler(&source, &compiler).unwrap();
        let host = LambdaHostEnv::from_json_str(&raw).unwrap();

        assert!(host.lookup_tool("execute_swap").is_some());
    }
}
