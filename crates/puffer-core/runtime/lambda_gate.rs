use super::RequestToolFilter;
use anyhow::{anyhow, Context, Result};
use host_json::HostEnvJson;
use input_contract::LambdaInputPattern;
use puffer_resources::{SkillSpec, SkillVerificationSpec};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

mod fact_extraction;
mod host_json;
mod input_contract;
mod pending_call;
mod result_check;
mod semantic_predicate;
mod type_check;

pub(crate) use pending_call::PendingLambdaHostCall;

/// One host-to-concrete tool binding declared by a Lambda Skill host catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LambdaHostConcreteToolBinding {
    /// The formal host tool name.
    pub host_tool: String,
    /// The concrete Puffer tools allowed for the host tool.
    pub concrete_tools: Vec<String>,
}

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

    fn instantiate(&self, args: &Map<String, Value>) -> Self {
        self.instantiate_with_result(args, None)
    }

    fn instantiate_with_result(&self, args: &Map<String, Value>, result: Option<&Value>) -> Self {
        let resolved: Vec<String> = self
            .args
            .iter()
            .map(|arg| {
                args.get(arg)
                    .map(canonical_fact_arg)
                    .or_else(|| result.map(canonical_fact_arg))
                    .unwrap_or_else(|| arg.clone())
            })
            .collect();
        Self::new(self.pred.clone(), resolved)
    }
}

fn canonical_fact_arg(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
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
    proof_params: BTreeSet<String>,
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

    fn validate_args(&self, args: &Value, facts: &BTreeSet<LambdaFact>) -> Option<String> {
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

    fn validate_result(
        &self,
        args: &Map<String, Value>,
        result: &Value,
        facts: &BTreeSet<LambdaFact>,
    ) -> Option<String> {
        if result_check::lambda_result_matches_type_with_facts(result, args, &self.result, facts) {
            return None;
        }
        Some(format!(
            "result for {} does not match {}",
            self.name, self.result
        ))
    }

    fn required_context_satisfied(
        &self,
        args: &Map<String, Value>,
        facts: &BTreeSet<LambdaFact>,
    ) -> bool {
        self.context_req
            .as_ref()
            .map(|required| facts.contains(&required.instantiate(args)))
            .unwrap_or(true)
    }

    fn allows_concrete_tool(&self, concrete_tool: &str) -> bool {
        self.concrete_tools.contains(concrete_tool)
    }

    fn validate_runtime_contract(
        &self,
        dynamic_facts: &BTreeSet<(String, usize)>,
        available_facts: &BTreeSet<(String, usize)>,
    ) -> Result<()> {
        if self.concrete_tools.contains("LambdaInternal")
            && self.effects.iter().any(|effect| effect != "pure")
        {
            return Err(anyhow!(
                "Lambda Skill host tool {} binds LambdaInternal despite runtime effects [{}]",
                self.name,
                self.effects.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        let declared_params = self
            .params
            .iter()
            .map(|param| param.name.clone())
            .collect::<BTreeSet<_>>();
        for proof_param in &self.proof_params {
            let Some(param) = self.params.iter().find(|param| param.name == *proof_param) else {
                return Err(anyhow!(
                    "Lambda Skill host tool {} proof parameter {} is not declared",
                    self.name,
                    proof_param
                ));
            };
            if !type_check::has_refinement_in_type(&param.ty) {
                return Err(anyhow!(
                    "Lambda Skill host tool {} proof parameter {} must carry a runtime refinement",
                    self.name,
                    proof_param
                ));
            }
        }
        let concrete_bound_params = declared_params
            .difference(&self.proof_params)
            .cloned()
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
            for (pred, arity) in type_check::fact_refinement_shapes_in_type(&param.ty) {
                if !dynamic_facts.contains(&(pred.clone(), arity)) {
                    return Err(anyhow!(
                        "Lambda Skill host tool {} parameter {} uses fact refinement {}({} args) without a matching registered fact",
                        self.name,
                        param.name,
                        pred,
                        arity
                    ));
                }
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
        if let Some(required) = self.context_req.as_ref() {
            let shape = (required.pred().to_string(), required.args().len());
            if !available_facts.contains(&shape) {
                return Err(anyhow!(
                    "Lambda Skill host tool {} contextReq {}({} args) has no matching registered fact",
                    self.name,
                    required.pred(),
                    required.args().len()
                ));
            }
        }
        for concrete_tool in &self.concrete_tools {
            if let Some(unsupported_effects) =
                unsupported_effects_for_concrete_tool(concrete_tool, &self.effects)
            {
                return Err(anyhow!(
                    "Lambda Skill host tool {} binds {} despite unsupported effects [{}]",
                    self.name,
                    concrete_tool,
                    unsupported_effects.join(", ")
                ));
            }
            let Some(contract) = self.concrete_input_contracts.get(concrete_tool) else {
                return Err(anyhow!(
                    "Lambda Skill host tool {} lacks a concrete input contract for {}",
                    self.name,
                    concrete_tool
                ));
            };
            let mut refs = BTreeSet::new();
            contract.collect_arg_refs(&mut refs);
            if refs != concrete_bound_params {
                return Err(anyhow!(
                    "Lambda Skill host tool {} concrete input contract for {} must bind exactly the non-proof formal parameters [{}]",
                    self.name,
                    concrete_tool,
                    concrete_bound_params
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
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

    fn dynamic_fact_shapes(&self) -> impl Iterator<Item = (String, usize)> + '_ {
        self.registers
            .iter()
            .map(|fact| (fact.pred().to_string(), fact.args().len()))
            .chain(type_check::fact_refinement_shapes_in_type(&self.result))
    }

    fn instantiated_registers<'a>(
        &'a self,
        args: &'a Map<String, Value>,
    ) -> impl Iterator<Item = LambdaFact> + 'a {
        self.registers.iter().map(|fact| fact.instantiate(args))
    }

    fn instantiated_facts<'a>(
        &'a self,
        args: &'a Map<String, Value>,
        result: &'a Value,
    ) -> impl Iterator<Item = LambdaFact> + 'a {
        let registered = self
            .registers
            .iter()
            .map(|fact| fact.instantiate_with_result(args, Some(result)));
        let result_refinements =
            fact_extraction::facts_from_result_refinements(&self.result, result, args).into_iter();
        registered.chain(result_refinements)
    }

    fn validate_concrete_input(
        &self,
        concrete_tool: &str,
        args: &Value,
        input: &Value,
        skill_root: Option<&Path>,
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
        if contract.matches(object, skill_root, input) {
            return None;
        }
        Some(format!(
            "concrete input for {} does not match the precompiled {} contract",
            self.name, concrete_tool
        ))
    }

    fn materialize_concrete_input(
        &self,
        concrete_tool: &str,
        args: &Value,
        skill_root: Option<&Path>,
    ) -> Result<Value, String> {
        let Some(object) = args.as_object() else {
            return Err(format!(
                "formal args for {} must be a JSON object",
                self.name
            ));
        };
        let Some(contract) = self.concrete_input_contracts.get(concrete_tool) else {
            return Err(format!(
                "host tool {} lacks a concrete input contract for {}",
                self.name, concrete_tool
            ));
        };
        contract.render_value(object, skill_root).ok_or_else(|| {
            format!(
                "concrete input for {} could not be materialized from the precompiled {} contract",
                self.name, concrete_tool
            )
        })
    }
}

fn unsupported_effects_for_concrete_tool(
    concrete_tool: &str,
    effects: &BTreeSet<String>,
) -> Option<Vec<String>> {
    let allowed_effects = match concrete_tool {
        "Read" => &["fs_r", "proc"][..],
        "WebFetch" | "WebSearch" => &["net_r", "proc"][..],
        _ => return None,
    };
    let unsupported = effects
        .iter()
        .filter(|effect| !allowed_effects.contains(&effect.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if unsupported.is_empty() {
        None
    } else {
        Some(unsupported)
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
    skill_root: Option<PathBuf>,
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
            skill_root: None,
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
        let dynamic_facts = self
            .tools
            .values()
            .flat_map(LambdaToolSig::dynamic_fact_shapes)
            .collect::<BTreeSet<_>>();
        let available_facts = dynamic_facts.clone();
        for sig in self.tools.values() {
            sig.validate_runtime_contract(&dynamic_facts, &available_facts)?;
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
    require_concrete_tool_approval: bool,
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
            require_concrete_tool_approval: false,
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
            require_concrete_tool_approval: false,
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

    /// Sets whether verified concrete calls should still use normal approval.
    pub(crate) fn set_require_concrete_tool_approval(&mut self, require_approval: bool) {
        self.require_concrete_tool_approval = require_approval;
    }

    /// Returns true when verified concrete calls should still use normal approval.
    pub(crate) fn require_concrete_tool_approval(&self) -> bool {
        self.require_concrete_tool_approval
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
            if !required.args().is_empty() {
                return LambdaGateVerdict::reject(format!(
                    "contextReq for {tool} requires formal args: ({})",
                    required.pred()
                ));
            }
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
        let Some(sig) = self.host.lookup_tool(tool) else {
            return LambdaGateVerdict::reject(format!("unknown tool: {tool}"));
        };
        if !sig.effects.is_subset(&self.caps) {
            return LambdaGateVerdict::reject(format!(
                "tool effects exceed gate capabilities: {tool}"
            ));
        }
        let Some(object) = args.as_object() else {
            return LambdaGateVerdict::reject(format!(
                "formal args for {} must be a JSON object",
                sig.name
            ));
        };
        if !sig.required_context_satisfied(object, &self.facts) {
            if let Some(required) = sig.context_req.as_ref() {
                return LambdaGateVerdict::reject(format!(
                    "contextReq not satisfied for {tool}: ({})",
                    required.pred()
                ));
            }
        }
        if let Some(reason) = sig.validate_args(args, &self.facts) {
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
        if let Some(reason) = sig.validate_concrete_input(
            concrete_tool,
            args,
            concrete_input,
            self.host.skill_root.as_deref(),
        ) {
            return LambdaGateVerdict::reject(reason);
        }
        LambdaGateVerdict::Accept
    }

    /// Materializes the exact concrete input declared for a host call.
    pub(crate) fn materialize_concrete_input_binding(
        &self,
        host_tool: &str,
        args: &Value,
        concrete_tool: &str,
    ) -> std::result::Result<Value, String> {
        let Some(sig) = self.host.lookup_tool(host_tool) else {
            return Err(format!("unknown tool: {host_tool}"));
        };
        sig.materialize_concrete_input(concrete_tool, args, self.host.skill_root.as_deref())
    }

    /// Gates one no-argument call and commits registered facts when accepted.
    pub(crate) fn step_call(&mut self, tool: &str) -> LambdaGateVerdict {
        self.step_call_with_args(tool, &Value::Object(Map::new()))
    }

    /// Gates one call and commits instantiated registered facts when accepted.
    pub(crate) fn step_call_with_args(&mut self, tool: &str, args: &Value) -> LambdaGateVerdict {
        self.step_call_with_args_and_result(tool, args, &Value::Null)
    }

    /// Gates one call and commits facts instantiated from formal args and result.
    pub(crate) fn step_call_with_args_and_result(
        &mut self,
        tool: &str,
        args: &Value,
        result: &Value,
    ) -> LambdaGateVerdict {
        let verdict = self.admit_call_with_args(tool, args);
        if verdict.is_accept() {
            if let Some(sig) = self.host.lookup_tool(tool) {
                let object = args.as_object().cloned().unwrap_or_default();
                if let Some(reason) = sig.validate_result(&object, result, &self.facts) {
                    return LambdaGateVerdict::Reject(reason);
                }
                for fact in sig.instantiated_facts(&object, result) {
                    self.facts.insert(fact);
                }
            }
        }
        verdict
    }

    /// Builds trace metadata for a committed host call.
    pub(crate) fn committed_host_call_metadata(
        &self,
        host_tool: &str,
        host_args: Option<&Value>,
        metadata_host_args: Option<&Value>,
        concrete_tool: Option<&str>,
        result: Option<&Value>,
    ) -> Value {
        let host_arg_object = host_args.and_then(Value::as_object);
        let registered_facts = self
            .host
            .lookup_tool(host_tool)
            .map(|sig| match (host_arg_object, result) {
                (Some(args), Some(result)) => sig
                    .instantiated_facts(args, result)
                    .map(|fact| lambda_fact_metadata(&fact))
                    .collect(),
                (Some(args), None) => sig
                    .instantiated_registers(args)
                    .map(|fact| lambda_fact_metadata(&fact))
                    .collect(),
                (None, _) => sig.registers.iter().map(lambda_fact_metadata).collect(),
            })
            .unwrap_or_default();
        lambda_skill_metadata(
            "host_call_committed",
            host_tool,
            metadata_host_args.cloned().or_else(|| host_args.cloned()),
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
    let mut host = LambdaHostEnv::from_json_str(&raw)
        .context("failed to parse host catalogue")?
        .apply_concrete_tool_bindings(&verification.host_tool_bindings)
        .context("failed to apply host tool bindings")?;
    host.skill_root = skill_root_for_verification(verification);
    host.validate_concrete_tool_bindings()
        .context("failed to validate host tool bindings")?;
    host.validate_runtime_contracts()
        .context("failed to validate Lambda Skill runtime contracts")?;
    Ok(Some(LambdaGateState::with_host_caps(host)))
}

fn skill_root_for_verification(verification: &SkillVerificationSpec) -> Option<PathBuf> {
    verification
        .source_path
        .as_deref()
        .and_then(|path| Path::new(path).parent())
        .map(Path::to_path_buf)
}

/// Validates a host catalogue against the runtime harness requirements.
pub(crate) fn validate_host_catalogue_runtime(raw: &str) -> Result<()> {
    let host = LambdaHostEnv::from_json_str(raw).context("failed to parse host catalogue")?;
    host.validate_concrete_tool_bindings()
        .context("failed to validate host tool bindings")?;
    host.validate_runtime_contracts()
        .context("failed to validate Lambda Skill runtime contracts")
}

/// Returns the concrete tool bindings declared in a Lambda Skill host catalogue.
pub(crate) fn host_catalogue_concrete_tool_bindings(
    raw: &str,
) -> Result<Vec<LambdaHostConcreteToolBinding>> {
    host_json::concrete_tool_bindings_from_json_str(raw)
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

#[cfg(test)]
mod tests;
