use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
    #[serde(alias = "allowed-tools")]
    allowed_tools: Vec<String>,
    #[serde(alias = "user-invocable")]
    user_invocable: bool,
    #[serde(alias = "disable-model-invocation")]
    disable_model_invocation: bool,
    #[serde(default, alias = "requires-action", alias = "requiresAction")]
    requires_action: Option<bool>,
}

fn parse_skill(markdown: &str) -> (SkillFrontmatter, &str) {
    let rest = markdown
        .strip_prefix("---\n")
        .expect("skill starts with frontmatter");
    let (frontmatter, body) = rest
        .split_once("\n---\n")
        .expect("skill frontmatter terminates");
    let parsed = serde_yaml::from_str(frontmatter).expect("skill frontmatter parses");
    (parsed, body)
}

#[test]
fn image_generation_skill_guides_foreground_bash_helper_use() {
    let (frontmatter, body) = parse_skill(include_str!(
        "../../../resources/skills/image-generation/SKILL.md"
    ));

    assert_eq!(frontmatter.name, "image-generation");
    assert!(!frontmatter.description.contains("ImageGeneration"));
    assert_eq!(frontmatter.allowed_tools, vec!["Bash"]);
    assert!(frontmatter.user_invocable);
    assert!(!frontmatter.disable_model_invocation);
    assert_eq!(frontmatter.requires_action, Some(true));
    assert!(body.contains("foreground Bash"));
    assert!(body.contains("Progress-only or promise-only replies are not completion"));
    assert!(body.contains("explicit long Bash timeout"));
    assert!(body.contains("imagegen --prompt"));
    assert!(!body.contains("puffer internal-tool"));
    assert!(body.contains("--count"));
    assert!(body.contains("one logical request"));
    assert!(body.contains("prompt file paths"));
    assert!(body.contains("allowed-tools is guidance"));
    assert!(body.contains("Do not hand-author SVG"));
}

#[test]
fn video_generation_skill_guides_foreground_bash_helper_use() {
    let (frontmatter, body) = parse_skill(include_str!(
        "../../../resources/skills/video-generation/SKILL.md"
    ));

    assert_eq!(frontmatter.name, "video-generation");
    assert!(!frontmatter.description.contains("VideoGeneration"));
    assert_eq!(frontmatter.allowed_tools, vec!["Bash"]);
    assert!(frontmatter.user_invocable);
    assert!(!frontmatter.disable_model_invocation);
    assert_eq!(frontmatter.requires_action, Some(true));
    assert!(body.contains("foreground Bash"));
    assert!(body.contains("Progress-only or promise-only replies are not completion"));
    assert!(body.contains("explicit long Bash timeout"));
    assert!(body.contains("videogen --prompt"));
    assert!(!body.contains("puffer internal-tool"));
    assert!(body.contains("--parameters-json"));
    assert!(body.contains("--image-reference"));
    assert!(body.contains("https://"));
    assert!(body.contains("asset://"));
    assert!(body.contains("local paths"));
    assert!(body.contains("scalar"));
    assert!(body.contains("allowed-tools is guidance"));
    assert!(body.contains("persisted video artifact"));
    assert!(!body.contains("text-to-video only"));
}

#[test]
fn short_drama_skill_requires_action_after_activation() {
    let (frontmatter, body) = parse_skill(include_str!(
        "../../../resources/skills/short-drama-generation/SKILL.md"
    ));

    assert_eq!(frontmatter.name, "short-drama-generation");
    assert_eq!(frontmatter.requires_action, Some(true));
    assert!(frontmatter.allowed_tools.contains(&"Write".to_string()));
    assert!(body.contains("Progress-only or promise-only replies are not completion"));
}
