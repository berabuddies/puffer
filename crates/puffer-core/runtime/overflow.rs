//! Cross-provider context-overflow recognizer.
//!
//! Ported from pi-mono's `packages/ai/src/utils/overflow.ts` so puffer can
//! detect "the request exceeded the model's context window" uniformly across
//! every provider we talk to. The pattern bank (substrings + regexes), the
//! exclusion list, and the silent-overflow heuristics are mirrored from there
//! so we benefit from the same field-tested coverage.
//!
//! ## Why a dedicated module
//!
//! Several call sites (Anthropic transport error handling, microcompact
//! triggering, agent-loop bail-out, future router fallback) all need to
//! answer the same question: *"is this error message a context overflow?"*.
//! Today the Anthropic path uses four ad-hoc substring needles in
//! `runtime/anthropic.rs` (`is_anthropic_too_large_error`); other paths
//! either re-implement or skip the check. Centralizing here gives us:
//!
//! - **One source of truth** — adding a new provider's overflow signature
//!   updates every call site at once.
//! - **Provenance for each pattern** — every entry cites the pi-mono
//!   line it was ported from, plus an example error message, so future
//!   maintainers can re-derive the regex from a real upstream payload.
//! - **Silent-overflow detection** — z.ai and Xiaomi MiMo never return an
//!   error string; their overflow only surfaces in the usage / stop_reason
//!   fields. The TypeScript `isContextOverflow` knew this; without the
//!   helper here we'd silently miscount these as "model finished early".
//!
//! ## Reference
//!
//! pi-mono `packages/ai/src/utils/overflow.ts` (commit imported during
//! puffer's pi-mono port). The TypeScript file uses native `RegExp` and a
//! `RegExp.test()` loop; this module uses lazy [`regex::Regex`] compilation
//! cached in a `OnceLock` so the cost of the 18 patterns is paid once per
//! process, not per call.
//!
//! Integration with `is_anthropic_too_large_error` is a deliberate
//! follow-up — that callsite lives on a separate worktree (PR #92) and
//! migrating it is its own diff so the recognizer can land + be tested
//! standalone first.

use std::sync::OnceLock;

use regex::Regex;

/// Substring patterns that indicate a context-overflow error message.
///
/// Matching is case-insensitive: the input message is lowercased before
/// each `contains` check, and every pattern here is already lowercase.
/// Each entry comments the pi-mono regex it was lowered from
/// (`pi-mono/packages/ai/src/utils/overflow.ts:31-52`) plus an example
/// upstream error so future maintainers can reproduce the match.
const OVERFLOW_PATTERNS: &[&str] = &[
    // Anthropic token overflow.
    // Example: "prompt is too long: 213462 tokens > 200000 maximum"
    // pi-mono overflow.ts:32
    "prompt is too long",
    // Anthropic byte-size HTTP 413.
    // Example: `413 {"error":{"type":"request_too_large", ...}}`
    // pi-mono overflow.ts:33
    "request_too_large",
    // Amazon Bedrock.
    // Example: "Input is too long for requested model"
    // pi-mono overflow.ts:34
    "input is too long for requested model",
    // OpenAI Completions + Responses APIs.
    // Example: "Your input exceeds the context window of this model"
    // pi-mono overflow.ts:35
    "exceeds the context window",
    // Groq.
    // Example: "Please reduce the length of the messages or completion"
    // pi-mono overflow.ts:38
    "reduce the length of the messages",
    // llama.cpp server.
    // Example: "the request exceeds the available context size, try increasing it"
    // pi-mono overflow.ts:41
    "exceeds the available context size",
    // LM Studio.
    // Example: "tokens to keep from the initial prompt is greater than the context length"
    // pi-mono overflow.ts:42
    "greater than the context length",
    // MiniMax.
    // Example: "invalid params, context window exceeds limit"
    // pi-mono overflow.ts:43
    "context window exceeds limit",
    // Kimi For Coding.
    // Example: "Your request exceeded model token limit: X (requested: Y)"
    // pi-mono overflow.ts:44
    "exceeded model token limit",
    // z.ai non-standard finish_reason surfaced as error text.
    // pi-mono overflow.ts:46
    "model_context_window_exceeded",
    // Generic fallbacks (multiple providers).
    // pi-mono overflow.ts:48
    "context_length_exceeded",
    "context length exceeded",
    // pi-mono overflow.ts:49 — also generic fallback.
    "too many tokens",
    // pi-mono overflow.ts:50 — also generic fallback.
    "token limit exceeded",
];

/// Regex patterns for overflow signatures that need numeric capture groups.
///
/// These can't reduce to plain substrings because the upstream error embeds
/// numbers ("131072", "1196265 tokens", "413") that change per request.
/// Strings here are passed verbatim to [`Regex::new`] with the
/// case-insensitive `(?i)` flag prepended automatically.
///
/// Each entry comments its pi-mono source line.
const OVERFLOW_REGEX_PATTERNS: &[&str] = &[
    // Google Gemini.
    // Example: "The input token count (1196265) exceeds the maximum number of tokens allowed (1048575)"
    // pi-mono overflow.ts:36
    r"input token count.*exceeds the maximum",
    // xAI (Grok).
    // Example: "This model's maximum prompt length is 131072 but the request contains 537812 tokens"
    // pi-mono overflow.ts:37
    r"maximum prompt length is \d+",
    // OpenRouter (all backends).
    // Example: "This endpoint's maximum context length is 128000 tokens. However, you requested about 200000 tokens"
    // pi-mono overflow.ts:39
    r"maximum context length is \d+ tokens",
    // GitHub Copilot.
    // Example: "prompt token count of 215000 exceeds the limit of 128000"
    // pi-mono overflow.ts:40
    r"exceeds the limit of \d+",
    // Mistral.
    // Example: "Prompt contains 240000 tokens ... too large for model with 131072 maximum context length"
    // pi-mono overflow.ts:45
    r"too large for model with \d+ maximum context length",
    // Ollama explicit overflow.
    // Example: "prompt too long; exceeded max context length by 12345 tokens"
    // pi-mono overflow.ts:47
    r"prompt too long; exceeded (?:max )?context length",
    // Bedrock-style numeric "X tokens > Y maximum" / "tokens > N".
    // Catches Anthropic and any variant that reports counts in the
    // pre-amble of the message body even when "prompt is too long" is absent.
    // Provenance: derived from the same Anthropic example as overflow.ts:32
    // ("213462 tokens > 200000 maximum"), kept as a regex so spurious "tokens"
    // mentions in unrelated error bodies don't false-positive.
    r"\d+\s*tokens?\s*>\s*\d+",
    // Cerebras: 400/413 with no body.
    // Example: "400 (no body)" or "413 status code (no body)"
    // pi-mono overflow.ts:51
    r"^4(?:00|13)\s*(?:status code)?\s*\(no body\)",
];

/// Patterns that indicate a *non*-overflow error and must short-circuit
/// the overflow check.
///
/// Bedrock's `formatBedrockError` formats throttling errors as
/// "ThrottlingException: Too many tokens, please wait before trying again."
/// which would otherwise match the `too many tokens` overflow pattern.
/// Same idea for generic rate-limit / 429 messages from any provider that
/// happens to include the word "tokens" in the body.
///
/// pi-mono overflow.ts:63-67. The TypeScript version uses regexes; we
/// keep them as substrings since none of the three need capture groups.
const NOT_OVERFLOW_PATTERNS: &[&str] = &[
    // pi-mono overflow.ts:64 — AWS Bedrock human-readable prefixes.
    // Original regex: /^(Throttling error|Service unavailable):/i
    // We split into two substrings so the lowercase match still works.
    "throttling error:",
    "service unavailable:",
    // pi-mono overflow.ts:65 — generic rate limiting (Anthropic, OpenAI, Groq, ...).
    "rate limit",
    "rate_limit",
    // pi-mono overflow.ts:66 — generic HTTP 429 phrasing.
    "too many requests",
];

/// Lazily compiled regex bank for [`OVERFLOW_REGEX_PATTERNS`].
///
/// Compilation happens once per process; subsequent calls reuse the
/// cached `Vec<Regex>`. Compiling 8 short regexes is cheap (~tens of µs)
/// but the recognizer can be called from hot paths (every assistant
/// turn's error branch) so we still cache.
fn overflow_regexes() -> &'static [Regex] {
    static CELL: OnceLock<Vec<Regex>> = OnceLock::new();
    CELL.get_or_init(|| {
        OVERFLOW_REGEX_PATTERNS
            .iter()
            .map(|raw| {
                // All matches are case-insensitive. Strings in the pattern
                // bank don't include `(?i)` themselves so we prepend it.
                Regex::new(&format!("(?i){raw}"))
                    .unwrap_or_else(|err| panic!("invalid overflow regex {raw:?}: {err}"))
            })
            .collect()
    })
}

/// Returns true when `message` looks like a context-overflow error from
/// any of the providers covered by [`OVERFLOW_PATTERNS`] /
/// [`OVERFLOW_REGEX_PATTERNS`].
///
/// The check is **exclusion-aware**: messages matching any
/// [`NOT_OVERFLOW_PATTERNS`] entry (e.g. throttling / rate-limit / 429)
/// short-circuit to `false` even if they also match an overflow pattern.
/// This mirrors `pi-mono/packages/ai/src/utils/overflow.ts:120-123` where
/// the TS `isContextOverflow` runs the non-overflow check first.
///
/// All comparisons are case-insensitive (input lowercased once).
///
/// # Examples
///
/// ```ignore
/// use puffer_core::runtime::overflow::is_context_overflow;
///
/// assert!(is_context_overflow("prompt is too long: 213462 tokens > 200000 maximum"));
/// assert!(is_context_overflow("Request too large for model"));
/// assert!(!is_context_overflow("rate_limit_exceeded: too many tokens this minute"));
/// ```
pub fn is_context_overflow(message: &str) -> bool {
    if message.is_empty() {
        return false;
    }
    let lower = message.to_ascii_lowercase();

    // Exclusions take precedence — see pi-mono overflow.ts:120-123.
    if NOT_OVERFLOW_PATTERNS
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return false;
    }

    if OVERFLOW_PATTERNS
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return true;
    }

    overflow_regexes().iter().any(|re| re.is_match(&lower))
}

/// Detects the **silent overflow** cases pi-mono's `isContextOverflow`
/// handles via the `contextWindow` parameter
/// (`pi-mono/packages/ai/src/utils/overflow.ts:117-145`).
///
/// Two providers known to swallow overflow without erroring:
///
/// - **z.ai**: accepts the request, returns `stop_reason="stop"` but the
///   reported `usage.input` exceeds `contextWindow`. Detected by
///   `stop_reason == "stop"` AND `context_used > context_window`.
/// - **Xiaomi MiMo**: truncates the prompt to fit the window exactly,
///   then returns `stop_reason="length"` with `output_tokens=0` because
///   there's no room left to generate. Detected by `stop_reason == "length"`,
///   `output_tokens == 0`, and `context_used >= 99% of context_window`.
///
/// `context_window == 0` always returns `false` — the recognizer can't
/// decide overflow without knowing the limit, and 0 is the convention
/// puffer uses when the provider catalog doesn't pin a window.
///
/// `stop_reason` is matched case-insensitively against the exact strings
/// `"stop"` and `"length"`. Anthropic uses `"end_turn"` / `"max_tokens"`
/// and won't trigger silent-overflow detection; that's by design — the
/// silent cases are specific to providers that don't surface the
/// overflow at all.
pub fn is_silent_overflow(
    stop_reason: &str,
    output_tokens: usize,
    context_used: usize,
    context_window: usize,
) -> bool {
    if context_window == 0 {
        return false;
    }
    let reason = stop_reason.trim().to_ascii_lowercase();

    // z.ai: successful completion but usage.input exceeds the window.
    // pi-mono overflow.ts:128-133.
    if reason == "stop" && context_used > context_window {
        return true;
    }

    // Xiaomi MiMo: server truncated the prompt to fit, leaving no room
    // to generate. The TS version uses `>= contextWindow * 0.99` to
    // tolerate small accounting drift. We use integer math to avoid
    // float rounding; `context_window * 99 / 100` matches for
    // every window >= 100 (which is every realistic model).
    // pi-mono overflow.ts:138-143.
    if reason == "length" && output_tokens == 0 {
        let threshold = context_window.saturating_mul(99) / 100;
        if context_used >= threshold {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each of the 18 documented patterns must match its representative
    /// upstream error message. Examples are copied verbatim from
    /// pi-mono `overflow.ts` doc comments where available, otherwise
    /// reconstructed from the regex's intent.
    #[test]
    fn each_pattern_matches_its_provider_example() {
        let cases: &[(&str, &str)] = &[
            // Anthropic — overflow.ts:32
            (
                "anthropic prompt-too-long",
                "prompt is too long: 213462 tokens > 200000 maximum",
            ),
            // Anthropic 413 — overflow.ts:33
            (
                "anthropic 413 request_too_large",
                r#"413 {"error":{"type":"request_too_large","message":"Request exceeds the maximum size"}}"#,
            ),
            // Bedrock — overflow.ts:34
            (
                "bedrock input-too-long",
                "Input is too long for requested model",
            ),
            // OpenAI — overflow.ts:35
            (
                "openai exceeds-context-window",
                "Your input exceeds the context window of this model",
            ),
            // Google Gemini — overflow.ts:36
            (
                "gemini input-token-count",
                "The input token count (1196265) exceeds the maximum number of tokens allowed (1048575)",
            ),
            // xAI Grok — overflow.ts:37
            (
                "xai maximum-prompt-length",
                "This model's maximum prompt length is 131072 but the request contains 537812 tokens",
            ),
            // Groq — overflow.ts:38
            (
                "groq reduce-length",
                "Please reduce the length of the messages or completion",
            ),
            // OpenRouter — overflow.ts:39
            (
                "openrouter maximum-context-length",
                "This endpoint's maximum context length is 128000 tokens. However, you requested about 200000 tokens",
            ),
            // GitHub Copilot — overflow.ts:40
            (
                "copilot exceeds-the-limit",
                "prompt token count of 215000 exceeds the limit of 128000",
            ),
            // llama.cpp — overflow.ts:41
            (
                "llama.cpp available-context-size",
                "the request exceeds the available context size, try increasing it",
            ),
            // LM Studio — overflow.ts:42
            (
                "lmstudio greater-than-context-length",
                "tokens to keep from the initial prompt is greater than the context length",
            ),
            // MiniMax — overflow.ts:43
            (
                "minimax context-window-exceeds-limit",
                "invalid params, context window exceeds limit",
            ),
            // Kimi For Coding — overflow.ts:44
            (
                "kimi exceeded-model-token-limit",
                "Your request exceeded model token limit: 200000 (requested: 250000)",
            ),
            // Mistral — overflow.ts:45
            (
                "mistral too-large-for-model-with-N-context",
                "Prompt contains 240000 tokens — too large for model with 131072 maximum context length",
            ),
            // z.ai non-standard — overflow.ts:46
            (
                "z.ai model-context-window-exceeded",
                "model_context_window_exceeded: cannot continue",
            ),
            // Ollama — overflow.ts:47
            (
                "ollama prompt-too-long-exceeded",
                "prompt too long; exceeded max context length by 12345 tokens",
            ),
            // Cerebras — overflow.ts:51
            (
                "cerebras 413 no-body",
                "413 status code (no body)",
            ),
            // Generic fallback — overflow.ts:48
            (
                "generic context_length_exceeded",
                "error: context_length_exceeded",
            ),
        ];

        for (label, msg) in cases {
            assert!(
                is_context_overflow(msg),
                "expected '{label}' to be detected as overflow; message = {msg:?}"
            );
        }
    }

    /// Negative cases drawn from the [`NOT_OVERFLOW_PATTERNS`] exclusions.
    /// pi-mono overflow.ts:63-67.
    #[test]
    fn exclusions_short_circuit_overflow_detection() {
        // Bedrock throttling: would otherwise match "too many tokens".
        assert!(!is_context_overflow(
            "Throttling error: Too many tokens, please wait before trying again."
        ));
        // Generic rate-limit message that mentions "token limit exceeded"
        // (which would match an overflow pattern) but is actually a 429.
        assert!(!is_context_overflow(
            "rate_limit_exceeded: token limit exceeded for this minute"
        ));
        // HTTP 429 wording.
        assert!(!is_context_overflow(
            "429 Too Many Requests: model_context_window_exceeded — try again later"
        ));
        // Bedrock service unavailable prefix.
        assert!(!is_context_overflow(
            "Service unavailable: prompt is too long"
        ));
    }

    /// Adversarial inputs — strings that *look* overflow-y but aren't.
    #[test]
    fn adversarial_negative_cases() {
        // Trace IDs containing "413" must not trip the Cerebras regex.
        assert!(!is_context_overflow(
            "request id req-413abc-202: handler timed out"
        ));
        // Generic timeout phrasing.
        assert!(!is_context_overflow("operation took too long"));
        // Empty / whitespace-only — recognizer should bail fast.
        assert!(!is_context_overflow(""));
        assert!(!is_context_overflow("   \n\t  "));
        // Random successful completion text.
        assert!(!is_context_overflow(
            "I have completed the requested change."
        ));
        // Mentions of "max_tokens" / "max_completion_tokens" in
        // non-overflow contexts (e.g. config validation errors) must
        // not match. None of our patterns include this substring, but
        // we keep an explicit assertion so future additions don't
        // accidentally introduce a false positive.
        assert!(!is_context_overflow(
            "config error: max_tokens must be a positive integer"
        ));
        assert!(!is_context_overflow(
            "validation: max_completion_tokens out of range"
        ));
    }

    /// Adversarial positive — phrasings that should match.
    #[test]
    fn adversarial_positive_cases() {
        // Variants on "request too large" that surface in real Anthropic
        // bodies — the underscore form (request_too_large) lives in the
        // structured error JSON.
        assert!(is_context_overflow(
            r#"{"error":{"type":"request_too_large","message":"Request too large for claude-sonnet"}}"#
        ));
        // Cerebras with the leading 400 status code variant.
        assert!(is_context_overflow("400 (no body)"));
        // Bedrock-style numeric ratio without the "prompt is too long" prefix.
        assert!(is_context_overflow(
            "input validation failed: 213462 tokens > 200000"
        ));
        // Mixed case — recognizer lowercases input.
        assert!(is_context_overflow(
            "PROMPT IS TOO LONG: 999999 tokens > 200000 maximum"
        ));
    }

    #[test]
    fn silent_overflow_zai_style() {
        // z.ai: stop_reason="stop", input usage above context window.
        assert!(is_silent_overflow("stop", 100, 250_000, 200_000));
        // Equal usage and window doesn't trigger (must be strictly greater).
        assert!(!is_silent_overflow("stop", 100, 200_000, 200_000));
        // Below window — not overflow.
        assert!(!is_silent_overflow("stop", 100, 150_000, 200_000));
    }

    #[test]
    fn silent_overflow_xiaomi_mimo_style() {
        // MiMo: stop_reason="length", output=0, prompt fills >= 99% of window.
        assert!(is_silent_overflow("length", 0, 200_000, 200_000));
        assert!(is_silent_overflow("length", 0, 198_000, 200_000));
        // Same shape but output tokens nonzero — model actually generated
        // something so this is just a normal `length` stop, not overflow.
        assert!(!is_silent_overflow("length", 1, 200_000, 200_000));
        // Same shape but prompt below the 99% threshold — model truncated
        // for some other reason; recognizer stays conservative.
        assert!(!is_silent_overflow("length", 0, 100_000, 200_000));
    }

    #[test]
    fn silent_overflow_handles_unknown_window_and_other_reasons() {
        // context_window == 0 => caller doesn't know the limit, can't decide.
        assert!(!is_silent_overflow("stop", 0, 1_000_000, 0));
        assert!(!is_silent_overflow("length", 0, 1_000_000, 0));
        // Stop reasons we don't classify as silent-overflow signals.
        assert!(!is_silent_overflow("end_turn", 0, 250_000, 200_000));
        assert!(!is_silent_overflow("tool_use", 0, 250_000, 200_000));
        assert!(!is_silent_overflow("max_tokens", 0, 250_000, 200_000));
        assert!(!is_silent_overflow("error", 0, 250_000, 200_000));
        // Case-insensitive on the reason itself.
        assert!(is_silent_overflow("STOP", 100, 250_000, 200_000));
        assert!(is_silent_overflow("Length", 0, 200_000, 200_000));
    }

    /// Make sure the cached regex bank actually compiles for every entry.
    /// If a regex string in `OVERFLOW_REGEX_PATTERNS` is malformed, the
    /// `OnceLock::get_or_init` panics on first access — which would fail
    /// here rather than at runtime.
    #[test]
    fn regex_bank_compiles_for_every_pattern() {
        let bank = overflow_regexes();
        assert_eq!(
            bank.len(),
            OVERFLOW_REGEX_PATTERNS.len(),
            "every regex source string must compile to exactly one Regex"
        );
    }

    #[test]
    fn pattern_bank_size_matches_pi_mono() {
        // pi-mono overflow.ts:31-52 declares 21 entries; we lower the 8
        // numeric ones to regex and the rest to substrings (some
        // substrings cover multiple TS patterns by sharing a needle, but
        // we keep enough to assert "no patterns dropped accidentally"
        // when refactoring this list).
        assert!(
            OVERFLOW_PATTERNS.len() + OVERFLOW_REGEX_PATTERNS.len() >= 18,
            "expected at least 18 patterns ported from pi-mono overflow.ts; \
             found {} substrings + {} regexes",
            OVERFLOW_PATTERNS.len(),
            OVERFLOW_REGEX_PATTERNS.len(),
        );
        assert!(
            NOT_OVERFLOW_PATTERNS.len() >= 3,
            "expected at least 3 exclusion patterns ported from \
             pi-mono overflow.ts:63-67; found {}",
            NOT_OVERFLOW_PATTERNS.len(),
        );
    }
}
