use super::*;
use crate::runtime::lambda_gate::{
    validate_host_catalogue_runtime, LambdaGateState, LambdaHostEnv,
};
use puffer_tools::ToolExecutionResult;

fn arxiv_host_json() -> String {
    serde_json::to_string(&json!({
        "effects": ["net_r", "proc"],
        "domains": ["ArxivXml", "Paper", "BibTex"],
        "tools": [
            {
                "name": "arxiv_search",
                "params": [
                    {"name": "query", "ty": "str"},
                    {"name": "max_results", "ty": "int{(n > 0 && n <= 30000)}"},
                    {"name": "sort_by", "ty": "str"},
                    {"name": "sort_order", "ty": "str"}
                ],
                "result": "ArxivXml",
                "effects": ["net_r"],
                "concreteTools": ["Bash"],
                "concreteInputContracts": {
                    "Bash": {
                        "command": {
                            "$template": "python3 - <<'PY'\nquery = ${json:query}\nmax_results = ${json:max_results}\nsort_by = ${json:sort_by}\nsort_order = ${json:sort_order}\nprint(query, max_results, sort_by, sort_order)\nPY"
                        },
                        "run_in_background": false,
                        "timeout": 60000
                    }
                }
            },
            {
                "name": "arxiv_fetch_by_id",
                "params": [{"name": "id_list", "ty": "str{valid_arxiv_id(id)}"}],
                "result": "ArxivXml",
                "effects": ["net_r"],
                "concreteTools": ["Bash"],
                "concreteInputContracts": {
                    "Bash": {
                        "command": {"$template": "curl -s \"https://export.arxiv.org/api/query?id_list=${id_list}\""},
                        "run_in_background": false,
                        "timeout": 60000
                    }
                }
            },
            {
                "name": "parse_arxiv_xml",
                "params": [{"name": "xml", "ty": "ArxivXml"}],
                "result": "[Paper]{parsed_ok(p)}",
                "effects": ["proc"],
                "concreteTools": ["Bash"],
                "concreteInputContracts": {
                    "Bash": {
                        "command": {"$template": "python3 - <<'PY'\nxml = ${json:xml}\nprint(xml)\nPY"},
                        "run_in_background": false,
                        "timeout": 60000
                    }
                }
            },
            {
                "name": "generate_bibtex",
                "params": [{"name": "paper", "ty": "Paper{parsed_ok(p)}"}],
                "result": "BibTex",
                "effects": ["proc"],
                "concreteTools": ["Bash"],
                "concreteInputContracts": {
                    "Bash": {
                        "command": {"$template": "python3 - <<'PY'\npaper = ${json:paper}\nprint(paper)\nPY"},
                        "run_in_background": false,
                        "timeout": 60000
                    }
                }
            },
            {
                "name": "web_extract_abstract",
                "params": [{"name": "arxiv_id", "ty": "str{valid_arxiv_id(id)}"}],
                "result": "str",
                "effects": ["net_r"],
                "concreteTools": ["Bash"],
                "concreteInputContracts": {
                    "Bash": {
                        "command": {"$template": "curl -L -s \"https://arxiv.org/abs/${arxiv_id}\""},
                        "run_in_background": false,
                        "timeout": 60000
                    }
                }
            },
            {
                "name": "web_extract_pdf",
                "params": [{"name": "arxiv_id", "ty": "str{valid_arxiv_id(id)}"}],
                "result": "str",
                "effects": ["net_r"],
                "concreteTools": ["Bash"],
                "concreteInputContracts": {
                    "Bash": {
                        "command": {"$template": "curl -L -s \"https://arxiv.org/pdf/${arxiv_id}\""},
                        "run_in_background": false,
                        "timeout": 60000
                    }
                }
            },
            {
                "name": "semantic_scholar_paper",
                "params": [
                    {"name": "arxiv_id", "ty": "str{valid_arxiv_id(id)}"},
                    {"name": "fields", "ty": "str"}
                ],
                "result": "SemanticPaper",
                "effects": ["net_r"],
                "concreteTools": ["Bash"],
                "concreteInputContracts": {
                    "Bash": {
                        "command": {"$template": "semantic-scholar paper ${arxiv_id} ${fields}"},
                        "run_in_background": false,
                        "timeout": 60000
                    }
                }
            },
            {
                "name": "semantic_scholar_citations",
                "params": [
                    {"name": "arxiv_id", "ty": "str{valid_arxiv_id(id)}"},
                    {"name": "limit", "ty": "int"}
                ],
                "result": "[SemanticPaper]",
                "effects": ["net_r"],
                "concreteTools": ["Bash"],
                "concreteInputContracts": {
                    "Bash": {
                        "command": {"$template": "semantic-scholar citations ${arxiv_id} ${limit}"},
                        "run_in_background": false,
                        "timeout": 60000
                    }
                }
            },
            {
                "name": "semantic_scholar_references",
                "params": [
                    {"name": "arxiv_id", "ty": "str{valid_arxiv_id(id)}"},
                    {"name": "limit", "ty": "int"}
                ],
                "result": "[SemanticPaper]",
                "effects": ["net_r"],
                "concreteTools": ["Bash"],
                "concreteInputContracts": {
                    "Bash": {
                        "command": {"$template": "semantic-scholar references ${arxiv_id} ${limit}"},
                        "run_in_background": false,
                        "timeout": 60000
                    }
                }
            },
            {
                "name": "semantic_scholar_recommendations",
                "params": [
                    {"name": "positive_ids", "ty": "[str]"},
                    {"name": "negative_ids", "ty": "[str]"}
                ],
                "result": "[SemanticPaper]",
                "effects": ["net_r"],
                "concreteTools": ["Bash"],
                "concreteInputContracts": {
                    "Bash": {
                        "command": {"$template": "semantic-scholar recommendations ${json:positive_ids} ${json:negative_ids}"},
                        "run_in_background": false,
                        "timeout": 60000
                    }
                }
            },
            {
                "name": "semantic_scholar_author",
                "params": [{"name": "query", "ty": "str"}],
                "result": "AuthorProfile",
                "effects": ["net_r"],
                "concreteTools": ["Bash"],
                "concreteInputContracts": {
                    "Bash": {
                        "command": {"$template": "semantic-scholar author ${query}"},
                        "run_in_background": false,
                        "timeout": 60000
                    }
                }
            }
        ]
    }))
    .unwrap()
}

fn arxiv_search_command(query: &str, max_results: i64, sort_by: &str, sort_order: &str) -> String {
    format!(
        "python3 - <<'PY'\nquery = {}\nmax_results = {}\nsort_by = {}\nsort_order = {}\nprint(query, max_results, sort_by, sort_order)\nPY",
        serde_json::to_string(query).unwrap(),
        max_results,
        serde_json::to_string(sort_by).unwrap(),
        serde_json::to_string(sort_order).unwrap()
    )
}

fn bash_input(command: String) -> Value {
    json!({
        "command": command,
        "run_in_background": false,
        "timeout": 60000,
    })
}

fn bash_pending_input(command: String) -> Value {
    json!({
        "command": command,
        "run_in_background": false,
        "timeout": 60000,
        "tty": false,
    })
}

fn resources() -> LoadedResources {
    LoadedResources {
        tools: vec![
            loaded_tool(
                "LambdaHostCall",
                "Admit Lambda host call",
                "runtime:lambda_host_call",
            ),
            loaded_tool("Bash", "Run shell", "bash"),
        ],
        ..LoadedResources::default()
    }
}

fn run_lambda_host_call(args: Value, input: Option<Value>) -> ToolExecutionResult {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let raw = arxiv_host_json();
    let host = LambdaHostEnv::from_json_str(&raw).unwrap();
    state.lambda_gate = Some(LambdaGateState::with_host_caps(host));
    let resources = resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let request_config = test_openai_request_config();

    let mut tool_input = json!({
        "host_tool": "arxiv_search",
        "args": args,
        "tool": "Bash",
    });
    if let Some(input) = input {
        tool_input["input"] = input;
    }

    execute_tool_call(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &registry,
        "gpt-5",
        &cwd,
        ToolExecutionBackend::OpenAi {
            request_config: &request_config,
            structured_output: None,
        },
        None,
        "LambdaHostCall",
        tool_input,
    )
    .unwrap()
}

#[test]
fn arxiv_host_catalogue_is_runtime_ready_with_precompiled_contracts() {
    validate_host_catalogue_runtime(&arxiv_host_json()).unwrap();
}

#[test]
fn arxiv_search_admits_exactly_bound_bash_input() {
    let args = json!({
        "query": "au:\"Hanzhi Liu\"",
        "max_results": 10,
        "sort_by": "submittedDate",
        "sort_order": "descending",
    });
    let input = bash_input(arxiv_search_command(
        "au:\"Hanzhi Liu\"",
        10,
        "submittedDate",
        "descending",
    ));

    let result = run_lambda_host_call(args, Some(input));

    assert!(result.success, "{}", result.output.stdout);
    assert!(result
        .output
        .stdout
        .contains("Next call must be Bash with this exact input"));
}

#[test]
fn arxiv_search_materializes_bash_input_when_omitted() {
    let args = json!({
        "query": "au:\"Hanzhi Liu\"",
        "max_results": 10,
        "sort_by": "submittedDate",
        "sort_order": "descending",
    });
    let expected = bash_pending_input(arxiv_search_command(
        "au:\"Hanzhi Liu\"",
        10,
        "submittedDate",
        "descending",
    ));

    let result = run_lambda_host_call(args, None);

    assert!(result.success, "{}", result.output.stdout);
    assert_eq!(
        result.output.metadata["lambda_skill"]["concrete_input"],
        expected
    );
}

#[test]
fn arxiv_search_accepts_noop_bash_ui_fields_in_host_call_input() {
    let args = json!({
        "query": "au:\"Hanzhi Liu\"",
        "max_results": 10,
        "sort_by": "submittedDate",
        "sort_order": "descending",
    });
    let command = arxiv_search_command("au:\"Hanzhi Liu\"", 10, "submittedDate", "descending");
    let mut input = bash_input(command.clone());
    input["description"] = json!("Query arXiv for Hanzhi Liu papers");
    input["tty"] = json!(false);

    let result = run_lambda_host_call(args, Some(input));

    assert!(result.success, "{}", result.output.stdout);
    assert_eq!(
        result.output.metadata["lambda_skill"]["concrete_input"],
        bash_pending_input(command)
    );
}

#[test]
fn arxiv_search_rejects_invalid_refinement_arguments() {
    let args = json!({
        "query": "au:\"Hanzhi Liu\"",
        "max_results": 0,
        "sort_by": "submittedDate",
        "sort_order": "descending",
    });
    let input = bash_input(arxiv_search_command(
        "au:\"Hanzhi Liu\"",
        0,
        "submittedDate",
        "descending",
    ));

    let result = run_lambda_host_call(args, Some(input));

    assert!(!result.success);
    assert!(result
        .output
        .stdout
        .contains("formal arg max_results for arxiv_search does not match"));
}

#[test]
fn arxiv_search_rejects_bash_input_that_breaks_the_binding() {
    let args = json!({
        "query": "au:\"Hanzhi Liu\"",
        "max_results": 10,
        "sort_by": "submittedDate",
        "sort_order": "descending",
    });
    let input = bash_input(arxiv_search_command(
        "au:\"Other Author\"",
        10,
        "submittedDate",
        "descending",
    ));

    let result = run_lambda_host_call(args, Some(input));

    assert!(!result.success);
    assert!(result
        .output
        .stdout
        .contains("concrete input for arxiv_search does not match"));
}

#[test]
fn arxiv_id_refinement_guards_fetch_and_bibtex_inputs() {
    let raw = arxiv_host_json();
    let host = LambdaHostEnv::from_json_str(&raw).unwrap();
    let gate = LambdaGateState::with_host_caps(host);
    assert!(gate
        .admit_call_with_args(
            "arxiv_fetch_by_id",
            &json!({"id_list": "2402.03300, hep-th/0601001v2"})
        )
        .is_accept());
    assert!(!gate
        .admit_call_with_args(
            "arxiv_fetch_by_id",
            &json!({"id_list": "https://arxiv.org/abs/2402.03300"})
        )
        .is_accept());
    assert!(gate
        .admit_call_with_args(
            "generate_bibtex",
            &json!({"paper": {"title": "Attention Is All You Need", "arxiv_id": "1706.03762v7"}}),
        )
        .is_accept());
    assert!(!gate
        .admit_call_with_args(
            "generate_bibtex",
            &json!({"paper": {"title": "Missing id"}})
        )
        .is_accept());
}

#[test]
fn arxiv_related_tool_refinements_cover_the_full_catalogue() {
    let raw = arxiv_host_json();
    let host = LambdaHostEnv::from_json_str(&raw).unwrap();
    let gate = LambdaGateState::with_host_caps(host);

    assert!(gate
        .admit_call_with_args("web_extract_abstract", &json!({"arxiv_id": "2402.03300"}))
        .is_accept());
    assert!(gate
        .admit_call_with_args("web_extract_pdf", &json!({"arxiv_id": "hep-th/0601001v2"}))
        .is_accept());
    assert!(!gate
        .admit_call_with_args(
            "web_extract_pdf",
            &json!({"arxiv_id": "https://arxiv.org/pdf/2402.03300"})
        )
        .is_accept());

    assert!(gate
        .admit_call_with_args(
            "semantic_scholar_paper",
            &json!({"arxiv_id": "1706.03762v7", "fields": "title,authors"})
        )
        .is_accept());
    assert!(!gate
        .admit_call_with_args(
            "semantic_scholar_paper",
            &json!({"arxiv_id": "arXiv:1706.03762", "fields": "title"})
        )
        .is_accept());
    assert!(gate
        .admit_call_with_args(
            "semantic_scholar_citations",
            &json!({"arxiv_id": "2402.03300", "limit": 10})
        )
        .is_accept());
    assert!(gate
        .admit_call_with_args(
            "semantic_scholar_references",
            &json!({"arxiv_id": "2402.03300", "limit": 10})
        )
        .is_accept());
    assert!(gate
        .admit_call_with_args(
            "semantic_scholar_recommendations",
            &json!({"positive_ids": ["arXiv:2402.03300"], "negative_ids": []})
        )
        .is_accept());
    assert!(!gate
        .admit_call_with_args(
            "semantic_scholar_recommendations",
            &json!({"positive_ids": "arXiv:2402.03300", "negative_ids": []})
        )
        .is_accept());
    assert!(gate
        .admit_call_with_args("semantic_scholar_author", &json!({"query": "Hanzhi Liu"}))
        .is_accept());
}
