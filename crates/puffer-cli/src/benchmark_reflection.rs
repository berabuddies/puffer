use puffer_core::{ReflectionConfig, ReflectionLanguage};

/// Builds the benchmark reflection policy, allowing environment overrides for the LLM judge.
pub(crate) fn benchmark_reflection_config(model_selector: &str) -> ReflectionConfig {
    let mut config = ReflectionConfig::default();
    config.language = ReflectionLanguage::Chinese;
    if let Some(llm_judge) = config.llm_judge.as_mut() {
        llm_judge.model_selector = Some(
            std::env::var("PUFFER_BENCHMARK_LLM_JUDGE_MODEL")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| model_selector.to_string()),
        );
        llm_judge.effort_level = Some(
            std::env::var("PUFFER_BENCHMARK_LLM_JUDGE_EFFORT")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "low".to_string()),
        );
    }
    config
}
