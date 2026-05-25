use super::*;

#[test]
fn task_flow_run_task_links_child_work_to_flow() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let created = crate::runtime::claude_tools::workflow::task_flow::execute_task_flow(
        &mut state,
        &cwd,
        json!({
            "action": "create_managed",
            "controllerId": "controller",
            "goal": "triage inbox",
            "currentStep": "classify",
            "stateJson": {"items": []}
        }),
    )
    .unwrap();
    let created: Value = serde_json::from_str(&created).unwrap();
    let flow_id = created["flowId"].as_str().unwrap();

    let child = crate::runtime::claude_tools::workflow::task_flow::execute_task_flow(
        &mut state,
        &cwd,
        json!({
            "action": "run_task",
            "flowId": flow_id,
            "runtime": "acp",
            "childSessionKey": "agent:main:subagent:classifier",
            "runId": "inbox-classify-1",
            "taskDesc": "Classify inbox messages"
        }),
    )
    .unwrap();
    let child: Value = serde_json::from_str(&child).unwrap();
    assert_eq!(child["created"], true);
    assert_eq!(child["revision"], 2);
    assert_eq!(child["child"]["run_id"], "inbox-classify-1");
    assert_eq!(child["child"]["runtime"], "acp");
    assert_eq!(
        child["child"]["child_session_key"],
        "agent:main:subagent:classifier"
    );

    let summary = crate::runtime::claude_tools::workflow::task_flow::execute_task_flow(
        &mut state,
        &cwd,
        json!({
            "action": "get_task_summary",
            "flowId": flow_id
        }),
    )
    .unwrap();
    let summary: Value = serde_json::from_str(&summary).unwrap();
    assert_eq!(
        summary["summary"]["children"][0]["task"],
        "Classify inbox messages"
    );
}
