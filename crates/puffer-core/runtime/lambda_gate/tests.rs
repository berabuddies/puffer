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
    assert!(gate.admit_call("execute_swap").is_accept());
    assert!(gate
        .step_call_with_args(
            "execute_swap",
            &serde_json::json!({
                "from": "ETH",
                "to": "USDC",
                "amount": 10.5
            })
        )
        .is_accept());
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
fn gate_context_requirements_bind_to_registered_argument_values() {
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["net_w"],"domains":[],"tools":[
          {"name":"login","params":[{"name":"app","ty":"str"}],"result":"unit","effects":[],"concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$template":"login ${app}"}}},"registers":[{"pred":"authed","args":["app"]}]},
          {"name":"send","params":[{"name":"app","ty":"str"}],"result":"unit","effects":["net_w"],"concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$template":"send ${app}"}}},"registers":[],"contextReq":{"pred":"authed","args":["app"]}}
        ]}"#,
    )
    .unwrap();
    let mut gate = LambdaGateState::with_host_caps(host);

    assert!(gate
        .step_call_with_args("login", &serde_json::json!({"app": "github"}))
        .is_accept());
    assert!(gate.facts().contains(&LambdaFact::new(
        "authed",
        vec![serde_json::to_string(&serde_json::json!("github")).unwrap()]
    )));
    assert!(gate
        .admit_call_with_args("send", &serde_json::json!({"app": "github"}))
        .is_accept());
    assert_eq!(
        gate.admit_call_with_args("send", &serde_json::json!({"app": "slack"}))
            .reason(),
        Some("contextReq not satisfied for send: (authed)")
    );
}

#[test]
fn gate_custom_refinements_bind_to_registered_argument_values() {
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["fs_w"],"domains":[],"tools":[
          {"name":"approve_plan","params":[{"name":"plan","ty":"str"}],"result":"unit","effects":[],"concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$template":"approve ${plan}"}}},"registers":[{"pred":"plan_approved","args":["plan"]}]},
          {"name":"apply_plan","params":[{"name":"plan","ty":"Plan{plan_approved(p)}"}],"result":"unit","effects":["fs_w"],"concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":{"$template":"apply ${shell:plan}"}}},"registers":[]}
        ]}"#,
    )
    .unwrap();
    let mut gate = LambdaGateState::with_host_caps(host);

    assert!(!gate
        .admit_call_with_args("apply_plan", &serde_json::json!({"plan": "move files"}))
        .is_accept());
    assert!(gate
        .step_call_with_args("approve_plan", &serde_json::json!({"plan": "move files"}))
        .is_accept());
    assert!(gate
        .admit_call_with_args("apply_plan", &serde_json::json!({"plan": "move files"}))
        .is_accept());
    assert!(!gate
        .admit_call_with_args("apply_plan", &serde_json::json!({"plan": "delete files"}))
        .is_accept());
}

#[test]
fn gate_custom_refinements_bind_to_result_values() {
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["net_r"],"domains":[],"tools":[
          {"name":"resolve_pr","effects":["net_r"],"concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":"resolve pr"}},"result":"PRRef{pr_resolved(p)}"},
          {"name":"read_pr","effects":["net_r"],"concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$template":"read ${pr}"}}},"params":[{"name":"pr","ty":"PRRef{pr_resolved(p)}"}],"result":"str"}
        ]}"#,
    )
    .unwrap();
    let mut gate = LambdaGateState::with_host_caps(host);

    assert!(!gate
        .admit_call_with_args("read_pr", &serde_json::json!({"pr": "91"}))
        .is_accept());
    assert!(gate
        .step_call_with_args_and_result(
            "resolve_pr",
            &serde_json::json!({}),
            &serde_json::json!("91")
        )
        .is_accept());
    assert!(gate.facts().contains(&LambdaFact::new(
        "pr_resolved",
        vec![serde_json::to_string(&serde_json::json!("91")).unwrap()]
    )));
    assert!(gate
        .admit_call_with_args("read_pr", &serde_json::json!({"pr": "91"}))
        .is_accept());
    assert!(!gate
        .admit_call_with_args("read_pr", &serde_json::json!({"pr": "92"}))
        .is_accept());
}

#[test]
fn gate_rejects_result_refinement_failures_before_committing_facts() {
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["fs_w"],"domains":[],"tools":[
          {"name":"save_report","effects":["fs_w"],"concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":"save"}},"result":"str{ends_with_pdf(p) && report_saved(p)}"},
          {"name":"publish_report","effects":["fs_w"],"concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":{"$template":"publish ${shell:path}"}}},"params":[{"name":"path","ty":"ReportPath{report_saved(p)}"}],"result":"unit"}
        ]}"#,
    )
    .unwrap();
    let mut gate = LambdaGateState::with_host_caps(host);

    assert_eq!(
        gate.step_call_with_args_and_result(
            "save_report",
            &serde_json::json!({}),
            &serde_json::json!("report.txt")
        )
        .reason(),
        Some("result for save_report does not match str{ends_with_pdf(p) && report_saved(p)}")
    );
    assert!(gate.facts().is_empty());
    assert_eq!(
        gate.admit_call_with_args("publish_report", &serde_json::json!({"path": "report.txt"}))
            .reason(),
        Some("formal arg path for publish_report does not match ReportPath{report_saved(p)}")
    );

    assert!(gate
        .step_call_with_args_and_result(
            "save_report",
            &serde_json::json!({}),
            &serde_json::json!("report.pdf")
        )
        .is_accept());
    assert!(gate
        .admit_call_with_args("publish_report", &serde_json::json!({"path": "report.pdf"}))
        .is_accept());
}

#[test]
fn gate_enforces_record_result_refinements() {
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["proc"],"domains":[],"tools":[
          {"name":"resolve_server","effects":["proc"],"concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":"server"}},"result":"{host: BindHost{loopback_only(h)}, port: BindPort{ephemeral_port(p)}}"}
        ]}"#,
    )
    .unwrap();
    let mut gate = LambdaGateState::with_host_caps(host);

    assert!(gate
        .step_call_with_args_and_result(
            "resolve_server",
            &serde_json::json!({}),
            &serde_json::json!({"host": "127.0.0.1", "port": 3000})
        )
        .is_accept());
    assert_eq!(
        gate.step_call_with_args_and_result(
            "resolve_server",
            &serde_json::json!({}),
            &serde_json::json!({"host": "example.com", "port": 3000})
        )
        .reason(),
        Some(
            "result for resolve_server does not match {host: BindHost{loopback_only(h)}, port: BindPort{ephemeral_port(p)}}"
        )
    );
}

#[test]
fn gate_registers_can_bind_to_result_values() {
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["net_r"],"domains":[],"tools":[
          {"name":"resolve_pr","effects":["net_r"],"concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":"resolve pr"}},"result":"PRRef","registers":[{"pred":"pr_resolved","args":["pr"]}]},
          {"name":"read_pr","effects":["net_r"],"concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$template":"read ${pr}"}}},"params":[{"name":"pr","ty":"PRRef{pr_resolved(p)}"}],"result":"str"}
        ]}"#,
    )
    .unwrap();
    let mut gate = LambdaGateState::with_host_caps(host);

    assert!(gate
        .step_call_with_args_and_result(
            "resolve_pr",
            &serde_json::json!({}),
            &serde_json::json!("91")
        )
        .is_accept());
    assert!(gate
        .admit_call_with_args("read_pr", &serde_json::json!({"pr": "91"}))
        .is_accept());
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
fn shell_template_contract_quotes_arguments() {
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":[],"domains":[],"tools":[{"name":"cli_lookup","effects":["proc"],"params":[{"name":"query","ty":"str"},{"name":"limit","ty":"int"}],"concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":{"$template":"lookup --query ${shell:query} --limit ${shell:limit}"},"run_in_background":false,"timeout":120,"tty":false}}}]}"#,
    )
    .unwrap();
    let gate = LambdaGateState::with_host_caps(host);

    assert!(gate
        .admit_concrete_input_binding(
            "cli_lookup",
            &serde_json::json!({"query": "a' $(rm -rf /)", "limit": 10}),
            "Bash",
            &serde_json::json!({
                "command": "lookup --query 'a'\"'\"' $(rm -rf /)' --limit '10'",
                "run_in_background": false,
                "timeout": 120,
                "tty": false
            })
        )
        .is_accept());
}

#[test]
fn shell_join_template_contract_quotes_array_arguments() {
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":[],"domains":[],"tools":[{"name":"cli_lookup_many","effects":["proc"],"params":[{"name":"symbols","ty":"[str]"}],"concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":{"$template":"lookup ${shell_join:symbols}"},"run_in_background":false,"timeout":120,"tty":false}}}]}"#,
    )
    .unwrap();
    let gate = LambdaGateState::with_host_caps(host);

    assert!(gate
        .admit_concrete_input_binding(
            "cli_lookup_many",
            &serde_json::json!({"symbols": ["AAPL", "BRK B"]}),
            "Bash",
            &serde_json::json!({
                "command": "lookup 'AAPL' 'BRK B'",
                "run_in_background": false,
                "timeout": 120,
                "tty": false
            })
        )
        .is_accept());
}

#[test]
fn shell_json_concat_contract_quotes_structured_payload() {
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":[],"domains":[],"tools":[{"name":"notion_search","effects":["proc","net_r"],"params":[{"name":"query","ty":"str"}],"concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":{"$concat":["composio execute NOTION_SEARCH -d ",{"$shell_json":{"query":{"$arg":"query"}}}]},"run_in_background":false,"timeout":300000,"tty":false}}}]}"#,
    )
    .unwrap();
    let gate = LambdaGateState::with_host_caps(host);

    assert!(gate
        .admit_concrete_input_binding(
            "notion_search",
            &serde_json::json!({"query": "alpha' $(rm -rf /)"}),
            "Bash",
            &serde_json::json!({
                "command": "composio execute NOTION_SEARCH -d '{\"query\":\"alpha'\"'\"' $(rm -rf /)\"}'",
                "run_in_background": false,
                "timeout": 300000,
                "tty": false
            })
        )
        .is_accept());
    assert_eq!(
        gate.admit_concrete_input_binding(
            "notion_search",
            &serde_json::json!({"query": "alpha' $(rm -rf /)"}),
            "Bash",
            &serde_json::json!({
                "command": "composio execute NOTION_SEARCH -d '{\"query\":\"alpha' $(rm -rf /)\"}'",
                "run_in_background": false,
                "timeout": 300000,
                "tty": false
            })
        )
        .reason(),
        Some("concrete input for notion_search does not match the precompiled Bash contract")
    );
}

#[test]
fn int_arg_contract_coerces_numeric_string_arguments() {
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":[],"domains":[],"tools":[{"name":"process_submit","effects":["proc"],"params":[{"name":"session_id","ty":"SessionId"},{"name":"data","ty":"str"}],"concreteTools":["WriteStdin"],"concreteInputContracts":{"WriteStdin":{"process_id":{"$int_arg":"session_id"},"input":{"$template":"${data}\n"}}}}]}"#,
    )
    .unwrap();
    let gate = LambdaGateState::with_host_caps(host);

    assert!(gate
        .admit_concrete_input_binding(
            "process_submit",
            &serde_json::json!({"session_id": "1000", "data": "yes"}),
            "WriteStdin",
            &serde_json::json!({"process_id": 1000, "input": "yes\n"})
        )
        .is_accept());
    assert_eq!(
        gate.admit_concrete_input_binding(
            "process_submit",
            &serde_json::json!({"session_id": "1000", "data": "yes"}),
            "WriteStdin",
            &serde_json::json!({"process_id": "1000", "input": "yes\n"})
        )
        .reason(),
        Some(
            "concrete input for process_submit does not match the precompiled WriteStdin contract"
        )
    );
}

#[test]
fn url_template_contract_percent_encodes_arguments() {
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":[],"domains":[],"tools":[{"name":"public_lookup","effects":["net_r"],"params":[{"name":"query","ty":"str"}],"concreteTools":["WebFetch"],"concreteInputContracts":{"WebFetch":{"url":{"$template":"https://example.test/search?q=${url:query}"},"prompt":"Return the response."}}}]}"#,
    )
    .unwrap();
    let gate = LambdaGateState::with_host_caps(host);

    assert!(gate
        .admit_concrete_input_binding(
            "public_lookup",
            &serde_json::json!({"query": "EGFR inhibitor"}),
            "WebFetch",
            &serde_json::json!({
                "url": "https://example.test/search?q=EGFR%20inhibitor",
                "prompt": "Return the response."
            })
        )
        .is_accept());
}

#[test]
fn skill_path_contract_matches_loaded_skill_root() {
    let root = tempfile::tempdir().unwrap();
    let skill_source = root.path().join("skill.lskill");
    let catalogue = root.path().join("out/host.json");
    fs::create_dir_all(catalogue.parent().unwrap()).unwrap();
    fs::write(&skill_source, "host {}\nskill demo {}\n").unwrap();
    fs::write(
        &catalogue,
        r#"{"effects":[],"domains":[],"tools":[{"name":"load_schema","effects":[],"concreteTools":["Read"],"concreteInputContracts":{"Read":{"file_path":{"$skill_path":"references/schema.md"}}}}]}"#,
    )
    .unwrap();
    let mut skill = SkillSpec::default();
    skill.verification = Some(SkillVerificationSpec {
        system: "lambda-skill".to_string(),
        source_path: Some(skill_source.display().to_string()),
        generated_path: None,
        host_catalogue_path: Some(catalogue.display().to_string()),
        compiler_path: None,
        host_tool_bindings: Default::default(),
        require_approval: false,
        tools: None,
        actions: None,
    });

    let gate = gate_for_verified_skill(&skill).unwrap().unwrap();
    let expected = root.path().join("references/schema.md");
    assert!(gate
        .admit_concrete_input_binding(
            "load_schema",
            &serde_json::json!({}),
            "Read",
            &serde_json::json!({"file_path": expected.display().to_string()})
        )
        .is_accept());
    assert_eq!(
        gate.admit_concrete_input_binding(
            "load_schema",
            &serde_json::json!({}),
            "Read",
            &serde_json::json!({"file_path": "/tmp/schema.md"})
        )
        .reason(),
        Some("concrete input for load_schema does not match the precompiled Read contract")
    );
}

#[test]
fn skill_shell_path_template_contract_quotes_loaded_skill_root() {
    let root = tempfile::tempdir().unwrap();
    let skill_root = root.path().join("skill root");
    let skill_source = skill_root.join("skill.lskill");
    let catalogue = skill_root.join("out/host.json");
    fs::create_dir_all(catalogue.parent().unwrap()).unwrap();
    fs::write(&skill_source, "host {}\nskill demo {}\n").unwrap();
    fs::write(
        &catalogue,
        r#"{"effects":[],"domains":[],"tools":[{"name":"run_helper","effects":["proc"],"params":[{"name":"input","ty":"str"}],"concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":{"$template":"python3 ${skill_shell_path:scripts/run.py} --input ${shell:input}"},"run_in_background":false,"timeout":120,"tty":false}}}]}"#,
    )
    .unwrap();
    let mut skill = SkillSpec::default();
    skill.verification = Some(SkillVerificationSpec {
        system: "lambda-skill".to_string(),
        source_path: Some(skill_source.display().to_string()),
        generated_path: None,
        host_catalogue_path: Some(catalogue.display().to_string()),
        compiler_path: None,
        host_tool_bindings: Default::default(),
        require_approval: false,
        tools: None,
        actions: None,
    });

    let gate = gate_for_verified_skill(&skill).unwrap().unwrap();
    let expected_script = skill_root.join("scripts/run.py").display().to_string();
    assert!(gate
        .admit_concrete_input_binding(
            "run_helper",
            &serde_json::json!({"input": "a b"}),
            "Bash",
            &serde_json::json!({
                "command": format!("python3 '{}' --input 'a b'", expected_script),
                "run_in_background": false,
                "timeout": 120,
                "tty": false
            })
        )
        .is_accept());
}

#[test]
fn skill_path_template_contract_rejects_parent_escape() {
    let error = validate_host_catalogue_runtime(
        r#"{"effects":[],"domains":[],"tools":[{"name":"run_helper","effects":["proc"],"concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":{"$template":"python3 ${skill_shell_path:../run.py}"},"run_in_background":false,"timeout":120,"tty":false}}}]}"#,
    )
    .expect_err("escaping skill paths in templates must fail");

    assert!(format!("{error:#}").contains("$skill_path contract cannot escape"));
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
fn host_catalogue_runtime_validation_rejects_effectful_lambda_internal() {
    let error = validate_host_catalogue_runtime(
        r#"{
            "effects": [],
            "domains": [],
            "tools": [{
                "name": "send_mail",
                "effects": ["net_w"],
                "concreteTools": ["LambdaInternal"],
                "concreteInputContracts": {
                    "LambdaInternal": {
                        "step": "mail.send",
                        "args": {"to": {"$arg": "to"}}
                    }
                },
                "params": [{"name": "to", "ty": "str"}]
            }]
        }"#,
    )
    .expect_err("effectful LambdaInternal binding must fail");

    assert!(format!("{error:#}").contains("binds LambdaInternal despite runtime effects [net_w]"));
}

#[test]
fn host_catalogue_runtime_validation_rejects_proc_lambda_internal() {
    let error = validate_host_catalogue_runtime(
        r#"{
            "effects": [],
            "domains": [],
            "tools": [{
                "name": "classify",
                "effects": ["proc"],
                "concreteTools": ["LambdaInternal"],
                "concreteInputContracts": {
                    "LambdaInternal": {
                        "step": "demo.classify",
                        "args": {"text": {"$arg": "text"}}
                    }
                },
                "params": [{"name": "text", "ty": "str"}]
            }]
        }"#,
    )
    .expect_err("proc LambdaInternal binding must fail");

    assert!(format!("{error:#}").contains("binds LambdaInternal despite runtime effects [proc]"));
}

#[test]
fn host_catalogue_runtime_validation_accepts_pure_internal_lambda_step() {
    validate_host_catalogue_runtime(
        r#"{
            "effects": [],
            "domains": [],
            "tools": [{
                "name": "classify",
                "effects": [],
                "concreteTools": ["LambdaInternal"],
                "concreteInputContracts": {
                    "LambdaInternal": {
                        "step": "demo.classify",
                        "args": {"text": {"$arg": "text"}}
                    }
                },
                "params": [{"name": "text", "ty": "str"}]
            }]
        }"#,
    )
    .unwrap();
}

#[test]
fn host_catalogue_runtime_validation_rejects_impossible_concrete_effects() {
    let web_error = validate_host_catalogue_runtime(
        r#"{
            "effects": [],
            "domains": [],
            "tools": [{
                "name": "search_and_write",
                "effects": ["net_r", "fs_w"],
                "concreteTools": ["WebSearch"],
                "concreteInputContracts": {
                    "WebSearch": {
                        "query": "example"
                    }
                }
            }]
        }"#,
    )
    .expect_err("WebSearch cannot satisfy filesystem write effects");

    assert!(format!("{web_error:#}").contains("binds WebSearch despite unsupported effects [fs_w]"));

    let read_error = validate_host_catalogue_runtime(
        r#"{
            "effects": [],
            "domains": [],
            "tools": [{
                "name": "read_and_fetch",
                "effects": ["fs_r", "net_r"],
                "concreteTools": ["Read"],
                "concreteInputContracts": {
                    "Read": {
                        "file_path": "/tmp/input.txt"
                    }
                }
            }]
        }"#,
    )
    .expect_err("Read cannot satisfy network read effects");

    assert!(format!("{read_error:#}").contains("binds Read despite unsupported effects [net_r]"));
}

#[test]
fn host_catalogue_runtime_validation_accepts_supported_concrete_effects() {
    validate_host_catalogue_runtime(
        r#"{
            "effects": [],
            "domains": [],
            "tools": [{
                "name": "fetch",
                "effects": ["net_r", "proc"],
                "concreteTools": ["WebFetch"],
                "concreteInputContracts": {
                    "WebFetch": {
                        "url": "https://example.com",
                        "prompt": "Read the page"
                    }
                }
            }, {
                "name": "read",
                "effects": ["fs_r", "proc"],
                "concreteTools": ["Read"],
                "concreteInputContracts": {
                    "Read": {
                        "file_path": "/tmp/input.txt"
                    }
                }
            }, {
                "name": "search",
                "effects": ["net_r", "proc"],
                "concreteTools": ["WebSearch"],
                "concreteInputContracts": {
                    "WebSearch": {
                        "query": "example"
                    }
                }
            }]
        }"#,
    )
    .unwrap();
}

#[test]
fn proof_params_can_be_checked_without_concrete_binding() {
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":[],"domains":[],"tools":[
          {"name":"approve_pr","effects":[],"concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$template":"approve ${pr}"}}},"params":[{"name":"pr","ty":"str"}],"registers":[{"pred":"ci_passing","args":["pr"]}]},
          {"name":"merge_pr","effects":["net_w"],"concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":{"$template":"gh pr merge --merge --subject ${shell:subject}"},"run_in_background":false,"timeout":120000,"tty":false}},"params":[{"name":"pr","ty":"Pr{ci_passing(p)}"},{"name":"subject","ty":"str"}],"proofParams":["pr"]}
        ]}"#,
    )
    .unwrap();
    let mut gate = LambdaGateState::with_host_caps(host);

    assert!(gate
        .step_call_with_args("approve_pr", &serde_json::json!({"pr": "42"}))
        .is_accept());
    assert!(gate
        .admit_concrete_input_binding(
            "merge_pr",
            &serde_json::json!({"pr": "42", "subject": "release"}),
            "Bash",
            &serde_json::json!({
                "command": "gh pr merge --merge --subject 'release'",
                "run_in_background": false,
                "timeout": 120000,
                "tty": false
            }),
        )
        .is_accept());
}

#[test]
fn proof_params_must_be_refined_declared_parameters() {
    let undeclared = validate_host_catalogue_runtime(
        r#"{"effects":[],"domains":[],"tools":[{"name":"run","effects":["proc"],"concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":"run"}},"params":[],"proofParams":["approval"]}]}"#,
    )
    .expect_err("undeclared proof param must fail");

    assert!(format!("{undeclared:#}").contains("proof parameter approval is not declared"));

    let unrefined = validate_host_catalogue_runtime(
        r#"{"effects":[],"domains":[],"tools":[{"name":"run","effects":["proc"],"concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":"run"}},"params":[{"name":"approval","ty":"str"}],"proofParams":["approval"]}]}"#,
    )
    .expect_err("unrefined proof param must fail");

    assert!(format!("{unrefined:#}")
        .contains("proof parameter approval must carry a runtime refinement"));
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
fn host_catalogue_runtime_validation_rejects_unproduced_fact_refinement() {
    let error = validate_host_catalogue_runtime(
            r#"{"effects":[],"domains":[],"tools":[{"name":"apply_plan","effects":[],"concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":{"$template":"apply ${shell:plan}"}}},"params":[{"name":"plan","ty":"Plan{plan_approved(p)}"}]}]}"#,
        )
        .expect_err("fact refinement without a producer must fail");

    assert!(format!("{error:#}").contains("without a matching registered fact"));
}

#[test]
fn host_catalogue_runtime_validation_accepts_produced_fact_refinement() {
    validate_host_catalogue_runtime(
            r#"{"effects":[],"domains":[],"tools":[
              {"name":"approve_plan","effects":[],"concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$template":"approve ${plan}"}}},"params":[{"name":"plan","ty":"str"}],"registers":[{"pred":"plan_approved","args":["plan"]}]},
              {"name":"apply_plan","effects":[],"concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":{"$template":"apply ${shell:plan}"}}},"params":[{"name":"plan","ty":"Plan{plan_approved(p)}"}]}
            ]}"#,
        )
        .unwrap();
}

#[test]
fn host_catalogue_runtime_validation_accepts_result_bound_fact_producer() {
    validate_host_catalogue_runtime(
            r#"{"effects":[],"domains":[],"tools":[
              {"name":"resolve_pr","effects":[],"concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":"resolve pr"}},"registers":[{"pred":"pr_resolved","args":["pr"]}]},
              {"name":"read_pr","effects":[],"concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$template":"read ${pr}"}}},"params":[{"name":"pr","ty":"PRRef{pr_resolved(p)}"}]}
            ]}"#,
        )
        .unwrap();
}

#[test]
fn host_catalogue_runtime_validation_accepts_result_refinement_fact_producer() {
    validate_host_catalogue_runtime(
            r#"{"effects":[],"domains":[],"tools":[
              {"name":"resolve_pr","effects":[],"concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":"resolve pr"}},"result":"PRRef{pr_resolved(p)}"},
              {"name":"read_pr","effects":[],"concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$template":"read ${pr}"}}},"params":[{"name":"pr","ty":"PRRef{pr_resolved(p)}"}]}
            ]}"#,
        )
        .unwrap();
}

#[test]
fn native_mcp_tool_name_refinement_is_checked_semantically() {
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["proc","net_r","net_w"],"domains":[],"tools":[
          {"name":"discover_tools","effects":["proc","net_r"],"concreteTools":["WebSearch"],"concreteInputContracts":{"WebSearch":{"query":"discover mcp tools"}},"result":"ToolRegistry{tools_discovered(r)}"},
          {"name":"call_mcp_tool","effects":["proc","net_w"],"concreteTools":["McpToolCall"],"concreteInputContracts":{"McpToolCall":{"qualifiedToolName":{"$arg":"tool_name"},"argsJson":{"$arg":"args_json"},"registry":{"$arg":"registry"}}},"params":[{"name":"registry","ty":"ToolRegistry{tools_discovered(r)}"},{"name":"tool_name","ty":"str{tool_names_normalized(n)}"},{"name":"args_json","ty":"str"}]}
        ]}"#,
    )
    .unwrap();
    let mut gate = LambdaGateState::with_host_caps(host);
    let registry = serde_json::json!({"tools": ["mcp__agentmail__send_message"]});
    assert!(gate
        .step_call_with_args_and_result("discover_tools", &serde_json::json!({}), &registry)
        .is_accept());

    assert!(gate
        .admit_call_with_args(
            "call_mcp_tool",
            &serde_json::json!({
                "registry": registry,
                "tool_name": "mcp__agentmail__send_message",
                "args_json": "{}"
            })
        )
        .is_accept());
    assert!(gate
        .admit_call_with_args(
            "call_mcp_tool",
            &serde_json::json!({
                "registry": {"tools": []},
                "tool_name": "agentmail.send_message",
                "args_json": "{}"
            })
        )
        .reason()
        .is_some());
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
        require_approval: false,
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
        require_approval: false,
        tools: None,
        actions: None,
    });

    assert!(gate_for_verified_skill(&skill).unwrap().is_none());
}
