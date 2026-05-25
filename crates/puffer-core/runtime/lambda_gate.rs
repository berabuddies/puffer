use super::RequestToolFilter;
use anyhow::{anyhow, Context, Result};
use input_contract::LambdaInputPattern;
use puffer_resources::{SkillSpec, SkillVerificationSpec};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;

mod input_contract;
mod type_check;

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
    concrete_tools: BTreeSet<String>,
    concrete_input_contracts: BTreeMap<String, LambdaInputPattern>,
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

    fn validate_args(&self, args: &Value, facts: &BTreeSet<String>) -> Option<String> {
        let Some(object) = args.as_object() else {
            return Some(format!(
                "formal args for {} must be a JSON object",
                self.name
            ));
        };
        for param in &self.params {
            let Some(value) = object.get(&param.name) else {
                return Some(format!(
                    "formal args for {} missing parameter {}",
                    self.name, param.name
                ));
            };
            if !type_check::lambda_arg_matches_type_with_facts(
                value,
                &param.name,
                object,
                &param.ty,
                facts,
            ) {
                return Some(format!(
                    "formal arg {} for {} does not match {}",
                    param.name, self.name, param.ty
                ));
            }
        }
        for key in object.keys() {
            if !self.params.iter().any(|param| param.name == *key) {
                return Some(format!(
                    "formal args for {} include undeclared parameter {}",
                    self.name, key
                ));
            }
        }
        None
    }

    fn allows_concrete_tool(&self, concrete_tool: &str) -> bool {
        self.concrete_tools.contains(concrete_tool)
    }

    fn validate_runtime_contract(&self) -> Result<()> {
        let declared_params = self
            .params
            .iter()
            .map(|param| param.name.clone())
            .collect::<BTreeSet<_>>();
        for param in &self.params {
            let unsupported = type_check::unsupported_refinements_in_type(&param.ty);
            if !unsupported.is_empty() {
                return Err(anyhow!(
                    "Lambda Skill host tool {} parameter {} uses unsupported runtime refinement {}",
                    self.name,
                    param.name,
                    unsupported.join(", ")
                ));
            }
        }
        let unsupported_result = type_check::unsupported_refinements_in_type(&self.result);
        if !unsupported_result.is_empty() {
            return Err(anyhow!(
                "Lambda Skill host tool {} result uses unsupported runtime refinement {}",
                self.name,
                unsupported_result.join(", ")
            ));
        }
        for concrete_tool in &self.concrete_tools {
            let Some(contract) = self.concrete_input_contracts.get(concrete_tool) else {
                return Err(anyhow!(
                    "Lambda Skill host tool {} lacks a concrete input contract for {}",
                    self.name,
                    concrete_tool
                ));
            };
            let mut refs = BTreeSet::new();
            contract.collect_arg_refs(&mut refs);
            if refs != declared_params {
                return Err(anyhow!(
                    "Lambda Skill host tool {} concrete input contract for {} must bind exactly the formal parameters [{}]",
                    self.name,
                    concrete_tool,
                    declared_params.iter().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
        }
        for concrete_tool in self.concrete_input_contracts.keys() {
            if !self.concrete_tools.contains(concrete_tool) {
                return Err(anyhow!(
                    "Lambda Skill host tool {} declares a concrete input contract for unbound concrete tool {}",
                    self.name,
                    concrete_tool
                ));
            }
        }
        Ok(())
    }

    fn result_predicate_facts(&self) -> impl Iterator<Item = LambdaFact> + '_ {
        type_check::predicate_names_in_type(&self.result)
            .into_iter()
            .map(|pred| LambdaFact::new(pred, Vec::<String>::new()))
    }

    fn validate_concrete_input(
        &self,
        concrete_tool: &str,
        args: &Value,
        input: &Value,
    ) -> Option<String> {
        let Some(object) = args.as_object() else {
            return Some(format!(
                "formal args for {} must be a JSON object",
                self.name
            ));
        };
        let Some(contract) = self.concrete_input_contracts.get(concrete_tool) else {
            return Some(format!(
                "host tool {} lacks a concrete input contract for {}",
                self.name, concrete_tool
            ));
        };
        if contract.matches(object, input) {
            return None;
        }
        Some(format!(
            "concrete input for {} does not match the precompiled {} contract",
            self.name, concrete_tool
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LambdaParam {
    name: String,
    ty: String,
}

/// Parsed precompiled Lambda Skill host catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LambdaHostEnv {
    effects: BTreeSet<String>,
    domains: Vec<String>,
    tools: HashMap<String, LambdaToolSig>,
}

impl LambdaHostEnv {
    /// Parses the precompiled host catalogue JSON shape.
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

    fn apply_concrete_tool_bindings(
        mut self,
        bindings: &BTreeMap<String, Vec<String>>,
    ) -> Result<Self> {
        for (host_tool, concrete_tools) in bindings {
            let Some(sig) = self.tools.get_mut(host_tool) else {
                return Err(anyhow!(
                    "host_tool_bindings references unknown Lambda Skill host tool {host_tool}"
                ));
            };
            sig.concrete_tools
                .extend(concrete_tools.iter().filter_map(|tool| {
                    let trimmed = tool.trim();
                    (!trimmed.is_empty()).then(|| trimmed.to_string())
                }));
        }
        Ok(self)
    }

    fn validate_concrete_tool_bindings(&self) -> Result<()> {
        for sig in self.tools.values() {
            if sig.concrete_tools.is_empty() {
                return Err(anyhow!(
                    "Lambda Skill host tool {} lacks a concrete tool binding; add concreteTools to the host catalogue or host_tool_bindings to the lambda_skill_libraries manifest",
                    sig.name
                ));
            }
        }
        Ok(())
    }

    fn validate_runtime_contracts(&self) -> Result<()> {
        for sig in self.tools.values() {
            sig.validate_runtime_contract()?;
        }
        Ok(())
    }
}

/// Stateful Lambda Skill gate, mirroring `LambdaW.Trace.GateState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LambdaGateState {
    host: LambdaHostEnv,
    caps: BTreeSet<String>,
    facts: BTreeSet<LambdaFact>,
    request_tool_filter: Option<RequestToolFilter>,
}

impl LambdaGateState {
    /// Creates a gate with all host-declared effects available.
    pub(crate) fn with_host_caps(host: LambdaHostEnv) -> Self {
        let caps = host.effects.clone();
        Self {
            host,
            caps,
            facts: BTreeSet::new(),
            request_tool_filter: None,
        }
    }

    /// Creates a gate with explicit effect capabilities.
    #[cfg(test)]
    pub(crate) fn with_caps(host: LambdaHostEnv, caps: impl IntoIterator<Item = String>) -> Self {
        Self {
            host,
            caps: caps.into_iter().collect(),
            facts: BTreeSet::new(),
            request_tool_filter: None,
        }
    }

    /// Attaches the concrete Puffer tool scope active for this Lambda Skill.
    pub(crate) fn set_request_tool_filter(&mut self, filter: RequestToolFilter) {
        self.request_tool_filter = Some(filter);
    }

    /// Returns the concrete Puffer tool scope active for this Lambda Skill.
    pub(crate) fn request_tool_filter(&self) -> Option<&RequestToolFilter> {
        self.request_tool_filter.as_ref()
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

    /// Admits one proposed host tool call and validates its formal arguments.
    pub(crate) fn admit_call_with_args(&self, tool: &str, args: &Value) -> LambdaGateVerdict {
        let verdict = self.admit_call(tool);
        if !verdict.is_accept() {
            return verdict;
        }
        let Some(sig) = self.host.lookup_tool(tool) else {
            return LambdaGateVerdict::reject(format!("unknown tool: {tool}"));
        };
        let predicates = self.available_predicates();
        if let Some(reason) = sig.validate_args(args, &predicates) {
            return LambdaGateVerdict::reject(reason);
        }
        LambdaGateVerdict::Accept
    }

    /// Admits or rejects the concrete Puffer tool bound to a host tool.
    pub(crate) fn admit_concrete_tool_binding(
        &self,
        host_tool: &str,
        concrete_tool: &str,
    ) -> LambdaGateVerdict {
        let Some(sig) = self.host.lookup_tool(host_tool) else {
            return LambdaGateVerdict::reject(format!("unknown tool: {host_tool}"));
        };
        if sig.allows_concrete_tool(concrete_tool) {
            return LambdaGateVerdict::Accept;
        }
        LambdaGateVerdict::reject(format!(
            "host tool {host_tool} is not bound to concrete tool {concrete_tool}"
        ))
    }

    /// Admits or rejects the concrete input bound to a host tool call.
    pub(crate) fn admit_concrete_input_binding(
        &self,
        host_tool: &str,
        args: &Value,
        concrete_tool: &str,
        concrete_input: &Value,
    ) -> LambdaGateVerdict {
        let Some(sig) = self.host.lookup_tool(host_tool) else {
            return LambdaGateVerdict::reject(format!("unknown tool: {host_tool}"));
        };
        if let Some(reason) = sig.validate_concrete_input(concrete_tool, args, concrete_input) {
            return LambdaGateVerdict::reject(reason);
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
                for fact in sig.result_predicate_facts() {
                    self.facts.insert(fact);
                }
            }
        }
        verdict
    }

    fn available_predicates(&self) -> BTreeSet<String> {
        self.facts.iter().map(|fact| fact.pred.clone()).collect()
    }

    /// Builds trace metadata for a committed host call.
    pub(crate) fn committed_host_call_metadata(
        &self,
        host_tool: &str,
        host_args: Option<&Value>,
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
            host_args.cloned(),
            concrete_tool,
            None,
            registered_facts,
        )
    }
}

/// Builds trace metadata for an admitted bridged host call.
pub(crate) fn admitted_host_call_metadata(
    host_tool: &str,
    host_args: Value,
    concrete_tool: &str,
    concrete_input: Value,
) -> Value {
    lambda_skill_metadata(
        "host_call_admitted",
        host_tool,
        Some(host_args),
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
    host_args: Option<Value>,
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
    if let Some(args) = host_args {
        inner.insert("host_args".to_string(), args);
    }
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
    host_args: Value,
    concrete_tool: String,
    concrete_input: Value,
}

impl PendingLambdaHostCall {
    /// Creates a pending bridge from one formal host tool to one concrete tool call.
    pub(crate) fn new(
        host_tool: impl Into<String>,
        host_args: Value,
        concrete_tool: impl Into<String>,
        concrete_input: Value,
    ) -> Self {
        Self {
            host_tool: host_tool.into(),
            host_args,
            concrete_tool: concrete_tool.into(),
            concrete_input,
        }
    }

    /// Returns the formal host tool name admitted by the Lambda gate.
    pub(crate) fn host_tool(&self) -> &str {
        &self.host_tool
    }

    /// Returns the formal host arguments admitted by the Lambda gate.
    pub(crate) fn host_args(&self) -> &Value {
        &self.host_args
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
    let host = LambdaHostEnv::from_json_str(&raw)
        .context("failed to parse host catalogue")?
        .apply_concrete_tool_bindings(&verification.host_tool_bindings)
        .context("failed to apply host tool bindings")?;
    host.validate_concrete_tool_bindings()
        .context("failed to validate host tool bindings")?;
    host.validate_runtime_contracts()
        .context("failed to validate Lambda Skill runtime contracts")?;
    Ok(Some(LambdaGateState::with_host_caps(host)))
}

/// Validates a host catalogue against the runtime harness requirements.
pub(crate) fn validate_host_catalogue_runtime(raw: &str) -> Result<()> {
    let host = LambdaHostEnv::from_json_str(raw).context("failed to parse host catalogue")?;
    host.validate_concrete_tool_bindings()
        .context("failed to validate host tool bindings")?;
    host.validate_runtime_contracts()
        .context("failed to validate Lambda Skill runtime contracts")
}

fn host_catalogue_json_for_verification(
    verification: &SkillVerificationSpec,
) -> Result<Option<String>> {
    if let Some(host_catalogue_path) = verification.host_catalogue_path.as_deref() {
        return fs::read_to_string(host_catalogue_path)
            .with_context(|| format!("failed to read {host_catalogue_path}"))
            .map(Some);
    }
    Ok(None)
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
    #[serde(default, rename = "concreteTools", alias = "concrete_tools")]
    concrete_tools: Vec<String>,
    #[serde(
        default,
        rename = "concreteInputContracts",
        alias = "concrete_input_contracts"
    )]
    concrete_input_contracts: Value,
}

impl ToolSigJson {
    fn into_sig(self) -> Result<LambdaToolSig> {
        let concrete_input_contracts =
            parse_concrete_input_contracts(self.concrete_input_contracts).with_context(|| {
                format!("failed to parse concrete input contracts for {}", self.name)
            })?;
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
            concrete_tools: self.concrete_tools.into_iter().collect(),
            concrete_input_contracts,
        })
    }
}

fn parse_concrete_input_contracts(value: Value) -> Result<BTreeMap<String, LambdaInputPattern>> {
    match value {
        Value::Null => Ok(BTreeMap::new()),
        Value::Object(object) => object
            .into_iter()
            .map(|(tool, pattern)| Ok((tool, LambdaInputPattern::from_json(pattern)?)))
            .collect(),
        Value::Array(items) => {
            let mut contracts = BTreeMap::new();
            for item in items {
                let Value::Object(mut object) = item else {
                    return Err(anyhow!("concrete input contract must be an object"));
                };
                let Some(tool) = object
                    .remove("tool")
                    .and_then(|value| value.as_str().map(str::to_string))
                else {
                    return Err(anyhow!("concrete input contract missing tool"));
                };
                let Some(input) = object.remove("input") else {
                    return Err(anyhow!("concrete input contract for {tool} missing input"));
                };
                if contracts
                    .insert(tool.clone(), LambdaInputPattern::from_json(input)?)
                    .is_some()
                {
                    return Err(anyhow!("duplicate concrete input contract for {tool}"));
                }
            }
            Ok(contracts)
        }
        _ => Err(anyhow!("concreteInputContracts must be an object or array")),
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
      "effects": ["net_r", "net_w", "sign"], "domains": ["TokenAddr"],
      "tools": [
        {"name": "get_quote", "params": [{"name": "from", "ty": "TokenAddr"}, {"name": "to", "ty": "TokenAddr"}], "result": "real{p > 0}", "effects": ["net_r"], "concreteTools": ["ToolSearch"], "concreteInputContracts": {"ToolSearch": {"query": {"$template": "quote ${from} ${to}"}}}, "registers": [], "contextReq": null},
        {"name": "authenticate", "params": [], "result": "unit", "effects": [], "concreteTools": ["ToolSearch"], "concreteInputContracts": {"ToolSearch": {"query": "authenticate"}}, "registers": [{"pred": "authed", "args": []}], "contextReq": null},
        {"name": "execute_swap", "params": [{"name": "from", "ty": "TokenAddr"}, {"name": "to", "ty": "TokenAddr"}, {"name": "amount", "ty": "real{a > 0}"}], "result": "Result<Receipt, SwapErr>", "effects": ["net_w", "sign"], "concreteTools": ["Bash"], "concreteInputContracts": {"Bash": {"command": {"$template": "swap ${from} ${to} ${amount}"}}}, "registers": [], "contextReq": {"pred": "authed", "args": []}}
      ]
    }"#;

    #[test]
    fn parses_precompiled_host_catalogue_shape() {
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
    fn gate_validates_formal_host_arguments() {
        let host = LambdaHostEnv::from_json_str(SWAP_HOST_JSON).unwrap();
        let mut gate = LambdaGateState::with_host_caps(host);
        gate.add_fact(LambdaFact::new("authed", Vec::new()));

        assert!(gate
            .admit_call_with_args(
                "execute_swap",
                &serde_json::json!({
                    "from": "ETH",
                    "to": "USDC",
                    "amount": 10.5
                })
            )
            .is_accept());
        assert_eq!(
            gate.admit_call_with_args(
                "execute_swap",
                &serde_json::json!({
                    "from": "ETH",
                    "to": "USDC",
                    "amount": "ten"
                })
            )
            .reason(),
            Some("formal arg amount for execute_swap does not match real{a > 0}")
        );
        assert_eq!(
            gate.admit_call_with_args(
                "execute_swap",
                &serde_json::json!({
                    "from": "ETH",
                    "amount": 10.5
                })
            )
            .reason(),
            Some("formal args for execute_swap missing parameter to")
        );
    }

    #[test]
    fn gate_validates_precompiled_concrete_input_contract() {
        let host = LambdaHostEnv::from_json_str(SWAP_HOST_JSON).unwrap();
        let gate = LambdaGateState::with_host_caps(host);

        assert!(gate
            .admit_concrete_input_binding(
                "get_quote",
                &serde_json::json!({"from": "ETH", "to": "USDC"}),
                "ToolSearch",
                &serde_json::json!({"query": "quote ETH USDC"})
            )
            .is_accept());
        assert_eq!(
            gate.admit_concrete_input_binding(
                "get_quote",
                &serde_json::json!({"from": "ETH", "to": "USDC"}),
                "ToolSearch",
                &serde_json::json!({"query": "quote BTC USDC"})
            )
            .reason(),
            Some("concrete input for get_quote does not match the precompiled ToolSearch contract")
        );
    }

    #[test]
    fn host_catalogue_runtime_validation_rejects_missing_input_contract() {
        let error = validate_host_catalogue_runtime(
            r#"{"effects":[],"domains":[],"tools":[{"name":"formal_search","effects":[],"concreteTools":["ToolSearch"],"params":[{"name":"query","ty":"str"}]}]}"#,
        )
        .expect_err("missing concrete input contract must fail");

        assert!(format!("{error:#}").contains("lacks a concrete input contract"));
    }

    #[test]
    fn host_catalogue_runtime_validation_rejects_malformed_refinement() {
        let error = validate_host_catalogue_runtime(
            r#"{"effects":[],"domains":[],"tools":[{"name":"custom_fetch","effects":[],"concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$arg":"id"}}},"params":[{"name":"id","ty":"str{host_custom_rule id}"}]}]}"#,
        )
        .expect_err("malformed runtime refinement must fail");

        assert!(format!("{error:#}").contains("unsupported runtime refinement host_custom_rule id"));
    }

    #[test]
    fn host_catalogue_runtime_validation_rejects_malformed_result_refinement() {
        let error = validate_host_catalogue_runtime(
            r#"{"effects":[],"domains":[],"tools":[{"name":"custom_parse","effects":[],"concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":"parse"}},"result":"Paper{host_custom_rule p}"}]}"#,
        )
        .expect_err("malformed result refinement must fail");

        assert!(format!("{error:#}").contains("unsupported runtime refinement host_custom_rule p"));
    }

    #[test]
    fn gate_for_verified_skill_reads_catalogue_file() {
        let root = tempfile::tempdir().unwrap();
        let catalogue = root.path().join("host.json");
        fs::write(
            &catalogue,
            r#"{"effects":[],"domains":[],"tools":[{"name":"formal_search","effects":[],"concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":"formal"}}}]}"#,
        )
        .unwrap();
        let mut skill = SkillSpec::default();
        skill.verification = Some(SkillVerificationSpec {
            system: "lambda-skill".to_string(),
            source_path: None,
            generated_path: None,
            host_catalogue_path: Some(catalogue.display().to_string()),
            compiler_path: None,
            host_tool_bindings: Default::default(),
            tools: None,
            actions: None,
        });

        let gate = gate_for_verified_skill(&skill)
            .unwrap()
            .expect("catalogue should create a gate");

        assert!(gate.admit_call("formal_search").is_accept());
        assert!(gate
            .admit_concrete_tool_binding("formal_search", "ToolSearch")
            .is_accept());
    }

    #[test]
    fn gate_for_verified_skill_ignores_compiler_path_without_host_catalogue() {
        let root = tempfile::tempdir().unwrap();
        let compiler = root.path().join("lskillc");
        fs::write(&compiler, "").unwrap();
        let mut skill = SkillSpec::default();
        skill.verification = Some(SkillVerificationSpec {
            system: "lambda-skill".to_string(),
            source_path: Some(root.path().join("skill.lskill").display().to_string()),
            generated_path: None,
            host_catalogue_path: None,
            compiler_path: Some(compiler.display().to_string()),
            host_tool_bindings: Default::default(),
            tools: None,
            actions: None,
        });

        assert!(gate_for_verified_skill(&skill).unwrap().is_none());
    }
}
