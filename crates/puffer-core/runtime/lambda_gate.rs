use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};

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
}
