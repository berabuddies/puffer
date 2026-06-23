#[test]
fn ask_user_question_schema_allows_canvas_offer_metadata_kind() {
    let tool: serde_json::Value = serde_yaml::from_str(include_str!(
        "../../../resources/tools/ask_user_question.yaml"
    ))
    .expect("AskUserQuestion tool YAML");
    let metadata = &tool["input_schema"]["properties"]["metadata"];

    assert_eq!(metadata["additionalProperties"], false);
    assert_eq!(metadata["properties"]["kind"]["type"], "string");
    assert!(metadata["properties"]["kind"]["description"]
        .as_str()
        .is_some_and(|value| value.contains("canvas-offer")));
}
