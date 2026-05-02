use super::model_proposal_violations;
use super::parse::parse_candidate_list_json;
use crate::contract::{
    ActionContract, ApprovalSpec, CapabilityContract, ContractStatus, Idempotency, Reversibility,
    RiskLevel, SemanticIntentSpec, SideEffectClass, TrustLevel, VerificationSpec,
};
use crate::model_policy::{MAX_MODEL_EDIT_REPLACEMENT_ARG_LINES, MAX_MODEL_EXECUTABLE_ARG_CHARS};
use serde_json::json;
use std::collections::BTreeMap;

fn tool_contract() -> CapabilityContract {
    let mut slots = BTreeMap::new();
    slots.insert("command".to_string(), "command".to_string());
    CapabilityContract {
        contract_id: "puffer.tools".to_string(),
        version: "0.1.0".to_string(),
        status: ContractStatus::Active,
        trust_level: TrustLevel::Sandboxed,
        description: "tools".to_string(),
        actions: vec![ActionContract {
            name: "Bash".to_string(),
            description: "run shell".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"],
                "additionalProperties": false
            }),
            output_schema: json!({"type": "object"}),
            side_effect_class: SideEffectClass::Unknown,
            reversibility: Reversibility::Unknown,
            idempotency: Idempotency::Unknown,
            risk_level: RiskLevel::High,
            preconditions: Vec::new(),
            postconditions: Vec::new(),
            verification: VerificationSpec {
                methods: Vec::new(),
                observation_checks: Vec::new(),
                method_templates: Vec::new(),
                templates: Vec::new(),
                required_before_completion: false,
                confidence: 0.5,
            },
            approval: ApprovalSpec {
                required: false,
                reason: None,
            },
            failure_modes: Vec::new(),
            forbidden_uses: Vec::new(),
            argument_safety: Vec::new(),
            structured_argument_safety: Vec::new(),
            semantic_intents: vec![SemanticIntentSpec {
                intent: "shell_command".to_string(),
                slots,
                optional_slots: BTreeMap::new(),
                defaults: BTreeMap::new(),
                side_effect_class: Some(SideEffectClass::Unknown),
                slot_kinds: Default::default(),
            }],
            intent_extractors: Vec::new(),
            repair_rules: Vec::new(),
            cost_estimate: None,
            latency_estimate: None,
        }],
        global_constraints: Vec::new(),
        forbidden_uses: Vec::new(),
        local_rules: Vec::new(),
        examples: Vec::new(),
        contract_hash: None,
    }
}

fn non_shell_action(name: &str, side_effect_class: SideEffectClass) -> ActionContract {
    ActionContract {
        name: name.to_string(),
        description: name.to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {"file_path": {"type": "string"}},
            "required": ["file_path"],
            "additionalProperties": false
        }),
        output_schema: json!({"type": "object"}),
        side_effect_class,
        reversibility: Reversibility::Reversible,
        idempotency: Idempotency::Idempotent,
        risk_level: RiskLevel::Low,
        preconditions: Vec::new(),
        postconditions: Vec::new(),
        verification: VerificationSpec {
            methods: Vec::new(),
            observation_checks: Vec::new(),
            method_templates: Vec::new(),
            templates: Vec::new(),
            required_before_completion: false,
            confidence: 0.5,
        },
        approval: ApprovalSpec {
            required: false,
            reason: None,
        },
        failure_modes: Vec::new(),
        forbidden_uses: Vec::new(),
        argument_safety: Vec::new(),
        structured_argument_safety: Vec::new(),
        semantic_intents: Vec::new(),
        intent_extractors: Vec::new(),
        repair_rules: Vec::new(),
        cost_estimate: None,
        latency_estimate: None,
    }
}

fn edit_action() -> ActionContract {
    let mut action = non_shell_action("Edit", SideEffectClass::LocalWrite);
    action.input_schema = json!({
        "type": "object",
        "properties": {
            "file_path": {"type": "string"},
            "old_string": {"type": "string"},
            "new_string": {"type": "string"}
        },
        "required": ["file_path", "old_string", "new_string"],
        "additionalProperties": false
    });
    action
}

#[test]
fn candidate_policy_error_exposes_structured_violation() {
    let text = json!({
        "candidates": [{
            "id": "oversized",
            "tool_id": "Bash",
            "args": {"command": "x".repeat(MAX_MODEL_EXECUTABLE_ARG_CHARS + 1)},
            "completion_role": "repair",
            "depends_on": [],
            "rationale": "oversized generated command"
        }]
    })
    .to_string();

    let error = parse_candidate_list_json(&text, &tool_contract()).unwrap_err();
    let violations = model_proposal_violations(&error);

    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].code, "executable_arg_char_limit_exceeded");
    assert_eq!(violations[0].path, "candidates[0].args.command");
    assert_eq!(violations[0].tool_id.as_deref(), Some("Bash"));
    assert_eq!(
        violations[0].limit,
        Some(MAX_MODEL_EXECUTABLE_ARG_CHARS as u64)
    );
}

#[test]
fn multiline_executable_payload_under_char_limit_is_allowed() {
    let command = std::iter::repeat("printf x")
        .take(128)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(command.chars().count() < MAX_MODEL_EXECUTABLE_ARG_CHARS);
    let text = json!({
        "candidates": [{
            "id": "multiline",
            "tool_id": "Bash",
            "args": {"command": command},
            "completion_role": "repair",
            "depends_on": [],
            "rationale": "bounded multiline generated command"
        }]
    })
    .to_string();

    let parsed = parse_candidate_list_json(&text, &tool_contract()).unwrap();

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id.as_deref(), Some("multiline"));
}

#[test]
fn unknown_side_effect_support_candidate_is_normalized_to_verification() {
    let text = json!({
        "candidates": [{
            "id": "cat-file",
            "tool_id": "Bash",
            "args": {"command": "cat /tmp/input.txt"},
            "completion_role": "support",
            "depends_on": [],
            "rationale": "read data with shell"
        }]
    })
    .to_string();

    let parsed = parse_candidate_list_json(&text, &tool_contract()).unwrap();

    assert_eq!(parsed.len(), 1);
    assert_eq!(
        parsed[0].completion_role,
        crate::planner::CompletionRole::Verification
    );
}

#[test]
fn local_write_support_candidate_is_normalized_to_repair() {
    let mut contract = tool_contract();
    contract
        .actions
        .push(non_shell_action("Write", SideEffectClass::LocalWrite));
    let text = json!({
        "candidates": [{
            "id": "write-support",
            "tool_id": "Write",
            "args": {"file_path": "/tmp/output.txt"},
            "completion_role": "support",
            "depends_on": [],
            "rationale": "write as observation"
        }]
    })
    .to_string();

    let parsed = parse_candidate_list_json(&text, &contract).unwrap();

    assert_eq!(parsed.len(), 1);
    assert_eq!(
        parsed[0].completion_role,
        crate::planner::CompletionRole::Repair
    );
}

#[test]
fn local_write_verification_candidate_is_normalized_to_repair() {
    let mut contract = tool_contract();
    contract
        .actions
        .push(non_shell_action("Write", SideEffectClass::LocalWrite));
    let text = json!({
        "candidates": [{
            "id": "write-verify",
            "tool_id": "Write",
            "args": {"file_path": "/tmp/output.txt"},
            "completion_role": "verification",
            "depends_on": [],
            "rationale": "write as verification"
        }]
    })
    .to_string();

    let parsed = parse_candidate_list_json(&text, &contract).unwrap();

    assert_eq!(parsed.len(), 1);
    assert_eq!(
        parsed[0].completion_role,
        crate::planner::CompletionRole::Repair
    );
}

#[test]
fn communication_verification_candidate_is_normalized_to_repair() {
    let mut contract = tool_contract();
    contract.actions.push(non_shell_action(
        "SendEmail",
        SideEffectClass::Communication,
    ));
    let text = json!({
        "candidates": [{
            "id": "email-verify",
            "tool_id": "SendEmail",
            "args": {"file_path": "/tmp/body.txt"},
            "completion_role": "verification",
            "depends_on": [],
            "rationale": "send as verification"
        }]
    })
    .to_string();

    let parsed = parse_candidate_list_json(&text, &contract).unwrap();

    assert_eq!(parsed.len(), 1);
    assert_eq!(
        parsed[0].completion_role,
        crate::planner::CompletionRole::Repair
    );
}

#[test]
fn read_only_repair_candidate_is_normalized_to_support() {
    let mut contract = tool_contract();
    contract
        .actions
        .push(non_shell_action("Read", SideEffectClass::LocalRead));
    let text = json!({
        "candidates": [{
            "id": "read-repair",
            "tool_id": "Read",
            "args": {"file_path": "/tmp/input.txt"},
            "completion_role": "repair",
            "depends_on": [],
            "rationale": "read as repair"
        }]
    })
    .to_string();

    let parsed = parse_candidate_list_json(&text, &contract).unwrap();

    assert_eq!(parsed.len(), 1);
    assert_eq!(
        parsed[0].completion_role,
        crate::planner::CompletionRole::Support
    );
}

#[test]
fn large_exact_replacement_edit_is_structurally_rejected() {
    let mut contract = tool_contract();
    contract.actions.push(edit_action());
    let new_string = std::iter::repeat("line")
        .take(MAX_MODEL_EDIT_REPLACEMENT_ARG_LINES + 1)
        .collect::<Vec<_>>()
        .join("\n");
    let text = json!({
        "candidates": [{
            "id": "rewrite-with-edit",
            "tool_id": "Edit",
            "args": {
                "file_path": "/tmp/output.py",
                "old_string": "old",
                "new_string": new_string
            },
            "completion_role": "repair",
            "depends_on": [],
            "rationale": "large edit"
        }]
    })
    .to_string();

    let error = parse_candidate_list_json(&text, &contract).unwrap_err();
    let violations = model_proposal_violations(&error);

    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].code, "edit_replacement_line_limit_exceeded");
    assert_eq!(violations[0].path, "candidates[0].args.new_string");
    assert_eq!(
        violations[0].limit,
        Some(MAX_MODEL_EDIT_REPLACEMENT_ARG_LINES as u64)
    );
}

#[test]
fn opaque_shell_text_is_not_parsed_for_file_write_syntax() {
    let text = json!({
        "candidates": [{
            "id": "heredoc",
            "tool_id": "Bash",
            "args": {
                "command": "cat > /tmp/solve.py << 'PY'\nprint(1)\nPY\npython3 /tmp/solve.py"
            },
            "completion_role": "repair",
            "depends_on": [],
            "rationale": "write and run generated file"
        }]
    })
    .to_string();

    let parsed = parse_candidate_list_json(&text, &tool_contract()).unwrap();

    assert_eq!(parsed.len(), 1);
}

#[test]
fn shell_stderr_redirect_to_stdout_remains_allowed() {
    let text = json!({
        "candidates": [{
            "id": "compile",
            "tool_id": "Bash",
            "args": {"command": "gcc /tmp/main.c 2>&1"},
            "completion_role": "verification",
            "depends_on": [],
            "rationale": "compile and capture diagnostics"
        }]
    })
    .to_string();

    let parsed = parse_candidate_list_json(&text, &tool_contract()).unwrap();

    assert_eq!(parsed.len(), 1);
}
