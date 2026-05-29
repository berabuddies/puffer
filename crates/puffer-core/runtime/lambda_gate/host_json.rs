use super::{
    LambdaFact, LambdaHostConcreteToolBinding, LambdaInputPattern, LambdaParam, LambdaToolSig,
};
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub(super) struct HostEnvJson {
    #[serde(default)]
    pub(super) effects: Vec<String>,
    #[serde(default)]
    pub(super) domains: Vec<String>,
    #[serde(default)]
    pub(super) tools: Vec<ToolSigJson>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ToolSigJson {
    name: String,
    #[serde(default)]
    params: Vec<ParamJson>,
    #[serde(default)]
    result: String,
    #[serde(default)]
    pub(super) effects: Vec<String>,
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
    #[serde(
        default,
        rename = "proofParams",
        alias = "proof_params",
        alias = "ghostParams",
        alias = "ghost_params"
    )]
    proof_params: Vec<String>,
}

impl ToolSigJson {
    pub(super) fn into_sig(self) -> Result<LambdaToolSig> {
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
            proof_params: self.proof_params.into_iter().collect(),
        })
    }
}

pub(super) fn concrete_tool_bindings_from_json_str(
    raw: &str,
) -> Result<Vec<LambdaHostConcreteToolBinding>> {
    let parsed: HostEnvJson =
        serde_json::from_str(raw).context("failed to parse Lambda Skill host catalogue")?;
    Ok(parsed
        .tools
        .into_iter()
        .map(|tool| LambdaHostConcreteToolBinding {
            host_tool: tool.name,
            concrete_tools: tool.concrete_tools,
        })
        .collect())
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
