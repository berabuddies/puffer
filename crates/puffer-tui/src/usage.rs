use crate::OverlayState;
use puffer_core::AppState;
use puffer_provider_openai::{fetch_usage_summary, OpenAIUsageError, OpenAIUsageSummary};
use puffer_provider_registry::{
    AuthStore, ProviderDescriptor, ProviderRegistry, StoredCredential,
};
use puffer_transport_anthropic::{
    fetch_oauth_usage, AnthropicExtraUsage, AnthropicRateLimit, AnthropicUtilization,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::thread;
use time::format_description::well_known::Rfc3339;
use time::{Month, OffsetDateTime, UtcOffset};

const WIDE_BAR_WIDTH: usize = 50;
const MIN_OVERLAY_WIDTH: u16 = 34;
const MAX_OVERLAY_WIDTH: u16 = 84;

/// Stores the mutable usage panel state shared between the TUI and the loader thread.
#[derive(Clone)]
pub(crate) struct UsageOverlay {
    shared: Arc<Mutex<UsageOverlayState>>,
}

#[derive(Debug, Clone)]
struct UsageOverlayState {
    view: UsageOverlayView,
    scroll: u16,
    generation: u64,
    request: Option<UsageLoadRequest>,
}

#[derive(Debug, Clone)]
enum UsageOverlayView {
    Loading,
    Ready(UsageReadyState),
    Error(String),
}

#[derive(Debug, Clone)]
enum UsageReadyState {
    Static(StaticUsageState),
    Anthropic {
        plan_type: Option<String>,
        utilization: AnthropicUtilization,
    },
    OpenAi {
        title: String,
        details: Vec<String>,
        summary: OpenAIUsageSummary,
    },
}

#[derive(Debug, Clone)]
struct StaticUsageState {
    title: String,
    details: Vec<String>,
    message: String,
}

#[derive(Debug, Clone)]
enum UsageLoadRequest {
    Anthropic {
        base_url: String,
        plan_type: Option<String>,
        access_token: String,
    },
    OpenAi {
        base_url: String,
        title: String,
        details: Vec<String>,
        bearer_token: String,
        unsupported_message: String,
    },
}

#[derive(Debug, Clone)]
struct UsageOverlaySnapshot {
    view: UsageOverlayView,
    scroll: u16,
    can_retry: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsageProviderFamily {
    Anthropic,
    OpenAi,
    Unsupported,
}

impl UsageOverlay {
    /// Builds a provider-specific usage overlay for the active session provider.
    pub(crate) fn open(
        state: &AppState,
        providers: &ProviderRegistry,
        auth_store: &AuthStore,
    ) -> OverlayState {
        match build_usage_overlay(state, providers, auth_store) {
            UsageOverlayInit::Ready(view) => OverlayState::Usage(UsageOverlay::with_view(view)),
            UsageOverlayInit::Load(request) => {
                let overlay = UsageOverlay::with_request(request.clone());
                overlay.start_load(request);
                OverlayState::Usage(overlay)
            }
        }
    }

    /// Scrolls the usage panel upward by one row.
    pub(crate) fn scroll_up(&self) {
        if let Ok(mut state) = self.shared.lock() {
            state.scroll = state.scroll.saturating_sub(1);
        }
    }

    /// Scrolls the usage panel downward by one row.
    pub(crate) fn scroll_down(&self) {
        if let Ok(mut state) = self.shared.lock() {
            state.scroll = state.scroll.saturating_add(1);
        }
    }

    /// Scrolls the usage panel upward by one page.
    pub(crate) fn page_up(&self) {
        if let Ok(mut state) = self.shared.lock() {
            state.scroll = state.scroll.saturating_sub(10);
        }
    }

    /// Scrolls the usage panel downward by one page.
    pub(crate) fn page_down(&self) {
        if let Ok(mut state) = self.shared.lock() {
            state.scroll = state.scroll.saturating_add(10);
        }
    }

    /// Retries the live provider usage fetch when the overlay supports it.
    pub(crate) fn retry(&self) {
        let request = self
            .shared
            .lock()
            .ok()
            .and_then(|state| state.request.clone());
        if let Some(request) = request {
            self.start_load(request);
        }
    }

    fn with_view(view: UsageReadyState) -> Self {
        Self {
            shared: Arc::new(Mutex::new(UsageOverlayState {
                view: UsageOverlayView::Ready(view),
                scroll: 0,
                generation: 0,
                request: None,
            })),
        }
    }

    fn with_request(request: UsageLoadRequest) -> Self {
        Self {
            shared: Arc::new(Mutex::new(UsageOverlayState {
                view: UsageOverlayView::Loading,
                scroll: 0,
                generation: 0,
                request: Some(request),
            })),
        }
    }

    fn start_load(&self, request: UsageLoadRequest) {
        let generation = if let Ok(mut state) = self.shared.lock() {
            state.generation = state.generation.saturating_add(1);
            state.view = UsageOverlayView::Loading;
            state.scroll = 0;
            state.request = Some(request.clone());
            state.generation
        } else {
            return;
        };
        let shared = Arc::clone(&self.shared);
        thread::spawn(move || {
            let view = fetch_usage_view(request);
            let Ok(mut state) = shared.lock() else {
                return;
            };
            if state.generation == generation {
                state.view = view;
                state.scroll = 0;
            }
        });
    }

    fn snapshot(&self) -> UsageOverlaySnapshot {
        self.shared
            .lock()
            .map(|state| UsageOverlaySnapshot {
                view: state.view.clone(),
                scroll: state.scroll,
                can_retry: state.request.is_some(),
            })
            .unwrap_or(UsageOverlaySnapshot {
                view: UsageOverlayView::Error("Usage panel is unavailable.".to_string()),
                scroll: 0,
                can_retry: false,
            })
    }

    #[cfg(test)]
    pub(crate) fn loading_for_test() -> Self {
        UsageOverlay::with_request(UsageLoadRequest::Anthropic {
            base_url: "https://api.anthropic.com".to_string(),
            access_token: "test-token".to_string(),
            plan_type: Some("max".to_string()),
        })
    }

    #[cfg(test)]
    pub(crate) fn unavailable_for_test() -> Self {
        UsageOverlay::with_view(static_usage(
            "Usage",
            Vec::new(),
            "Select a provider to view usage.",
        ))
    }

    #[cfg(test)]
    pub(crate) fn error_for_test(message: &str) -> Self {
        let overlay = UsageOverlay::with_request(UsageLoadRequest::Anthropic {
            base_url: "https://api.anthropic.com".to_string(),
            access_token: "test-token".to_string(),
            plan_type: Some("max".to_string()),
        });
        if let Ok(mut state) = overlay.shared.lock() {
            state.view = UsageOverlayView::Error(message.to_string());
        }
        overlay
    }

    #[cfg(test)]
    pub(crate) fn ready_anthropic_for_test(
        plan_type: Option<&str>,
        utilization: AnthropicUtilization,
    ) -> Self {
        UsageOverlay::with_view(UsageReadyState::Anthropic {
            plan_type: plan_type.map(str::to_string),
            utilization,
        })
    }

    #[cfg(test)]
    pub(crate) fn ready_openai_for_test(
        details: Vec<String>,
        summary: OpenAIUsageSummary,
    ) -> Self {
        UsageOverlay::with_view(UsageReadyState::OpenAi {
            title: "OpenAI/Codex account usage".to_string(),
            details,
            summary,
        })
    }
}

impl PartialEq for UsageOverlay {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
}

impl Eq for UsageOverlay {}

impl fmt::Debug for UsageOverlay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("UsageOverlay").finish_non_exhaustive()
    }
}

enum UsageOverlayInit {
    Ready(UsageReadyState),
    Load(UsageLoadRequest),
}

/// Renders the provider-specific usage overlay with live account state when available.
pub(crate) fn render_usage_overlay(
    frame: &mut Frame<'_>,
    viewport: Rect,
    overlay: &UsageOverlay,
) {
    let snapshot = overlay.snapshot();
    let width = viewport
        .width
        .saturating_sub(8)
        .clamp(MIN_OVERLAY_WIDTH, MAX_OVERLAY_WIDTH);
    let content_width = usize::from(width.saturating_sub(4));
    let mut lines = body_lines(&snapshot.view, content_width);
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    lines.push(footer_line(&snapshot));
    let height = (lines.len() as u16 + 2)
        .min(viewport.height.saturating_sub(2))
        .max(6);
    let area = Rect {
        x: viewport.x + viewport.width.saturating_sub(width) / 2,
        y: viewport.y + viewport.height.saturating_sub(height) / 3,
        width,
        height,
    };
    let visible_rows = usize::from(area.height.saturating_sub(2));
    let max_scroll = lines.len().saturating_sub(visible_rows) as u16;
    let scroll = snapshot.scroll.min(max_scroll);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0))
            .block(
                Block::default()
                    .title("Usage")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            ),
        area,
    );
}

fn build_usage_overlay(
    state: &AppState,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
) -> UsageOverlayInit {
    let Some(provider_id) = state.current_provider.as_deref() else {
        return UsageOverlayInit::Ready(static_usage(
            "Usage",
            Vec::new(),
            "Select a provider to view usage.",
        ));
    };
    let Some(provider) = providers.provider(provider_id) else {
        return UsageOverlayInit::Ready(static_usage(
            "Usage",
            vec![format!("Provider: {provider_id}")],
            "Usage display is not supported for the selected provider.",
        ));
    };
    let credential = auth_store.get(provider.id.as_str());
    match provider_family(provider) {
        UsageProviderFamily::Anthropic => build_anthropic_overlay(provider, credential),
        UsageProviderFamily::OpenAi => build_openai_overlay(provider, credential),
        UsageProviderFamily::Unsupported => UsageOverlayInit::Ready(static_usage(
            format!("{} usage", provider.display_name),
            provider_details(provider, credential, UsageProviderFamily::Unsupported),
            "Usage display is not supported for this provider.",
        )),
    }
}

fn build_anthropic_overlay(
    provider: &ProviderDescriptor,
    credential: Option<&StoredCredential>,
) -> UsageOverlayInit {
    match credential {
        Some(StoredCredential::OAuth(credential)) => UsageOverlayInit::Load(UsageLoadRequest::Anthropic {
            base_url: provider.base_url.clone(),
            access_token: credential.access_token.clone(),
            plan_type: credential.plan_type.clone(),
        }),
        Some(StoredCredential::ApiKey { .. }) => UsageOverlayInit::Ready(static_usage(
            "Claude subscription usage",
            provider_details(provider, credential, UsageProviderFamily::Anthropic),
            "Sign in with Anthropic OAuth to view Claude subscription usage.",
        )),
        None => UsageOverlayInit::Ready(static_usage(
            "Claude subscription usage",
            provider_details(provider, credential, UsageProviderFamily::Anthropic),
            "Add Anthropic OAuth credentials to view Claude subscription usage.",
        )),
    }
}

fn build_openai_overlay(
    provider: &ProviderDescriptor,
    credential: Option<&StoredCredential>,
) -> UsageOverlayInit {
    let title = "OpenAI/Codex account usage".to_string();
    let details = provider_details(provider, credential, UsageProviderFamily::OpenAi);
    let Some(bearer_token) = openai_bearer_token(credential) else {
        return UsageOverlayInit::Ready(static_usage(
            title,
            details,
            "Add OpenAI credentials to view account usage.",
        ));
    };
    let unsupported_message = match credential {
        Some(StoredCredential::OAuth(_)) => {
            "Usage display is not supported for Codex OAuth credentials. OpenAI's documented organization usage endpoints require an admin API key.".to_string()
        }
        Some(StoredCredential::ApiKey { .. }) => {
            "Usage display is only supported for OpenAI admin API keys on the documented organization usage endpoints.".to_string()
        }
        None => "Usage display is not supported for the current credential.".to_string(),
    };
    UsageOverlayInit::Load(UsageLoadRequest::OpenAi {
        base_url: provider.base_url.clone(),
        title,
        details,
        bearer_token,
        unsupported_message,
    })
}

fn fetch_usage_view(request: UsageLoadRequest) -> UsageOverlayView {
    match request {
        UsageLoadRequest::Anthropic {
            base_url,
            access_token,
            plan_type,
        } => match fetch_oauth_usage(&base_url, &access_token) {
            Ok(utilization) => UsageOverlayView::Ready(UsageReadyState::Anthropic {
                plan_type,
                utilization,
            }),
            Err(error) => UsageOverlayView::Error(format!("Failed to load usage data: {error}")),
        },
        UsageLoadRequest::OpenAi {
            base_url,
            title,
            details,
            bearer_token,
            unsupported_message,
        } => match fetch_usage_summary(&base_url, &bearer_token) {
            Ok(summary) => UsageOverlayView::Ready(UsageReadyState::OpenAi {
                title,
                details,
                summary,
            }),
            Err(OpenAIUsageError::UnsupportedCredential | OpenAIUsageError::UnsupportedProvider) => {
                UsageOverlayView::Ready(static_usage(title, details, unsupported_message))
            }
            Err(error) => UsageOverlayView::Error(format!("Failed to load usage data: {error}")),
        },
    }
}

fn openai_bearer_token(credential: Option<&StoredCredential>) -> Option<String> {
    match credential {
        Some(StoredCredential::OAuth(credential)) => Some(credential.access_token.clone()),
        Some(StoredCredential::ApiKey { key }) => Some(key.clone()),
        None => None,
    }
}

fn provider_family(provider: &ProviderDescriptor) -> UsageProviderFamily {
    let provider_id = provider.id.as_str();
    let api = provider.default_api.as_str();
    if provider_id == "anthropic" || api == "anthropic-messages" {
        UsageProviderFamily::Anthropic
    } else if (provider_id == "openai"
        || api.starts_with("openai")
        || api.contains("responses")
        || api.contains("completions"))
        && provider.base_url.contains("openai.com")
    {
        UsageProviderFamily::OpenAi
    } else {
        UsageProviderFamily::Unsupported
    }
}

fn static_usage(
    title: impl Into<String>,
    details: Vec<String>,
    message: impl Into<String>,
) -> UsageReadyState {
    UsageReadyState::Static(StaticUsageState {
        title: title.into(),
        details,
        message: message.into(),
    })
}

fn provider_details(
    provider: &ProviderDescriptor,
    credential: Option<&StoredCredential>,
    family: UsageProviderFamily,
) -> Vec<String> {
    let mut lines = vec![
        format!("Provider: {}", provider.display_name),
        format!("Authentication: {}", auth_label(credential)),
    ];
    if let Some(StoredCredential::OAuth(credential)) = credential {
        if let Some(email) = credential.email.as_deref() {
            lines.push(format!("Logged in as: {email}"));
        }
        match family {
            UsageProviderFamily::Anthropic => {
                if let Some(organization_id) = credential.organization_id.as_deref() {
                    lines.push(format!("Organization: {organization_id}"));
                }
                if let Some(plan_type) = credential.plan_type.as_deref() {
                    lines.push(format!("Plan: {}", format_metadata_value(plan_type)));
                }
                if let Some(rate_limit_tier) = credential.rate_limit_tier.as_deref() {
                    lines.push(format!(
                        "Rate limit tier: {}",
                        format_metadata_value(rate_limit_tier)
                    ));
                }
                if let Some(account_id) = credential.account_id.as_deref() {
                    lines.push(format!("Account ID: {account_id}"));
                }
            }
            UsageProviderFamily::OpenAi => {
                if let Some(account_id) = credential.account_id.as_deref() {
                    lines.push(format!("Account ID: {account_id}"));
                }
                if let Some(plan_type) = credential.plan_type.as_deref() {
                    lines.push(format!("Plan: {}", format_metadata_value(plan_type)));
                }
            }
            UsageProviderFamily::Unsupported => {
                if let Some(account_id) = credential.account_id.as_deref() {
                    lines.push(format!("Account ID: {account_id}"));
                }
            }
        }
    }
    lines
}

fn auth_label(credential: Option<&StoredCredential>) -> &'static str {
    match credential {
        Some(StoredCredential::OAuth(_)) => "OAuth",
        Some(StoredCredential::ApiKey { .. }) => "API key",
        None => "missing",
    }
}

fn format_metadata_value(value: &str) -> String {
    value
        .split(['_', '-', ' '])
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut chars = segment.chars();
            let Some(first) = chars.next() else {
                return String::new();
            };
            let mut word = String::new();
            word.extend(first.to_uppercase());
            word.push_str(chars.as_str());
            word
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn body_lines(view: &UsageOverlayView, max_width: usize) -> Vec<Line<'static>> {
    match view {
        UsageOverlayView::Loading => vec![Line::from(Span::styled(
            "Loading usage data...",
            Style::default().add_modifier(Modifier::DIM),
        ))],
        UsageOverlayView::Error(error) => vec![Line::from(vec![
            Span::styled("Error: ", Style::default().fg(Color::Red)),
            Span::styled(error.clone(), Style::default().fg(Color::Red)),
        ])],
        UsageOverlayView::Ready(UsageReadyState::Static(state)) => static_lines(state),
        UsageOverlayView::Ready(UsageReadyState::Anthropic {
            plan_type,
            utilization,
        }) => anthropic_lines(plan_type.as_deref(), utilization, max_width),
        UsageOverlayView::Ready(UsageReadyState::OpenAi {
            title,
            details,
            summary,
        }) => openai_lines(title, details, summary),
    }
}

fn static_lines(state: &StaticUsageState) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        state.title.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    if !state.details.is_empty() {
        lines.push(Line::default());
        lines.extend(state.details.iter().cloned().map(Line::from));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        state.message.clone(),
        Style::default().add_modifier(Modifier::DIM),
    )));
    lines
}

fn anthropic_lines(
    plan_type: Option<&str>,
    utilization: &AnthropicUtilization,
    max_width: usize,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        "Claude subscription usage",
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    let mut body = Vec::new();
    for (title, limit) in limit_buckets(plan_type, utilization) {
        if let Some(limit_lines) = limit_bar_lines(title, limit, max_width, true, None) {
            if !body.is_empty() {
                body.push(Line::default());
            }
            body.extend(limit_lines);
        }
    }
    if body.is_empty() {
        body.push(Line::from(Span::styled(
            "Live Claude subscription usage is unavailable.",
            Style::default().add_modifier(Modifier::DIM),
        )));
    }
    if let Some(extra_lines) = extra_usage_lines(plan_type, utilization.extra_usage.as_ref(), max_width) {
        if !body.is_empty() {
            body.push(Line::default());
        }
        body.extend(extra_lines);
    }
    lines.push(Line::default());
    lines.extend(body);
    lines
}

fn openai_lines(
    title: &str,
    details: &[String],
    summary: &OpenAIUsageSummary,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        title.to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    if !details.is_empty() {
        lines.push(Line::default());
        lines.extend(details.iter().cloned().map(Line::from));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Recent organization usage (last 7 days)",
        Style::default().add_modifier(Modifier::DIM),
    )));
    lines.push(Line::from(format!(
        "Requests: {}",
        format_count(summary.num_model_requests)
    )));
    lines.push(Line::from(format!(
        "Input tokens: {}",
        format_count(summary.input_tokens)
    )));
    lines.push(Line::from(format!(
        "Cached input tokens: {}",
        format_count(summary.input_cached_tokens)
    )));
    lines.push(Line::from(format!(
        "Output tokens: {}",
        format_count(summary.output_tokens)
    )));
    lines.push(Line::from(format!(
        "Cost: {}",
        summary
            .total_cost_usd
            .map(format_cost)
            .unwrap_or_else(|| "unavailable".to_string())
    )));
    lines
}

fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::new();
    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped.chars().rev().collect()
}

fn limit_buckets<'a>(
    plan_type: Option<&'a str>,
    utilization: &'a AnthropicUtilization,
) -> Vec<(&'static str, &'a AnthropicRateLimit)> {
    let mut limits = Vec::new();
    if let Some(limit) = utilization.five_hour.as_ref() {
        limits.push(("Current session", limit));
    }
    if let Some(limit) = utilization.seven_day.as_ref() {
        limits.push(("Current week (all models)", limit));
    }
    if shows_sonnet_limit(plan_type) {
        if let Some(limit) = utilization.seven_day_sonnet.as_ref() {
            limits.push(("Current week (Sonnet only)", limit));
        }
    }
    limits
}

fn limit_bar_lines(
    title: &'static str,
    limit: &AnthropicRateLimit,
    max_width: usize,
    show_time_in_reset: bool,
    extra_subtext: Option<String>,
) -> Option<Vec<Line<'static>>> {
    let utilization = limit.utilization?;
    let used_text = format!("{}% used", utilization.floor() as i64);
    let mut subtext = limit
        .resets_at
        .as_deref()
        .and_then(|value| format_reset_text(value, true, show_time_in_reset))
        .map(|value| format!("Resets {value}"));
    if let Some(extra_subtext) = extra_subtext {
        subtext = Some(match subtext {
            Some(existing) => format!("{extra_subtext} · {existing}"),
            None => extra_subtext,
        });
    }
    let bar_width = if max_width >= 62 {
        WIDE_BAR_WIDTH
    } else {
        max_width.saturating_sub(2).clamp(10, WIDE_BAR_WIDTH)
    };
    let bar = progress_bar(utilization / 100.0, bar_width);
    if max_width >= 62 {
        let mut lines = vec![
            Line::from(Span::styled(
                title,
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(bar_with_text(bar, &used_text)),
        ];
        if let Some(subtext) = subtext {
            lines.push(Line::from(Span::styled(
                subtext,
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        Some(lines)
    } else {
        let mut title_line = vec![Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        )];
        if let Some(subtext) = subtext {
            title_line.push(Span::raw(" "));
            title_line.push(Span::styled(
                format!("· {subtext}"),
                Style::default().add_modifier(Modifier::DIM),
            ));
        }
        Some(vec![
            Line::from(title_line),
            Line::from(bar),
            Line::from(used_text),
        ])
    }
}

fn extra_usage_lines(
    plan_type: Option<&str>,
    extra_usage: Option<&AnthropicExtraUsage>,
    max_width: usize,
) -> Option<Vec<Line<'static>>> {
    if !supports_extra_usage(plan_type) {
        return None;
    }
    let extra_usage = extra_usage?;
    if !extra_usage.is_enabled {
        return None;
    }
    if extra_usage.monthly_limit.is_none() {
        return Some(vec![
            Line::from(Span::styled(
                "Extra usage",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "Unlimited",
                Style::default().add_modifier(Modifier::DIM),
            )),
        ]);
    }
    let utilization = extra_usage.utilization?;
    let monthly_limit = extra_usage.monthly_limit?;
    let used_credits = extra_usage.used_credits?;
    let current = OffsetDateTime::now_utc().to_offset(local_offset());
    let next_month = if current.month() == Month::December {
        current
            .replace_year(current.year() + 1)
            .ok()?
            .replace_month(Month::January)
            .ok()?
    } else {
        current.replace_month(current.month().next()).ok()?
    };
    let reset = next_month
        .replace_day(1)
        .ok()?
        .replace_hour(0)
        .ok()?
        .replace_minute(0)
        .ok()?
        .replace_second(0)
        .ok()?
        .replace_millisecond(0)
        .ok()?;
    let reset_text = reset.format(&Rfc3339).ok();
    let extra_subtext = format!(
        "{} / {} spent",
        format_cost(used_credits / 100.0),
        format_cost(monthly_limit / 100.0)
    );
    let limit = AnthropicRateLimit {
        utilization: Some(utilization),
        resets_at: reset_text,
    };
    limit_bar_lines("Extra usage", &limit, max_width, false, Some(extra_subtext))
}

fn bar_with_text(bar: Vec<Span<'static>>, used_text: &str) -> Vec<Span<'static>> {
    let mut spans = bar;
    spans.push(Span::raw(" "));
    spans.push(Span::raw(used_text.to_string()));
    spans
}

fn progress_bar(ratio: f64, width: usize) -> Vec<Span<'static>> {
    let ratio = ratio.clamp(0.0, 1.0);
    let filled = ((ratio * width as f64).round() as usize).min(width);
    let empty = width.saturating_sub(filled);
    let mut spans = vec![Span::raw("[")];
    if filled > 0 {
        spans.push(Span::styled("#".repeat(filled), Style::default().fg(Color::Cyan)));
    }
    if empty > 0 {
        spans.push(Span::styled(
            "-".repeat(empty),
            Style::default().add_modifier(Modifier::DIM),
        ));
    }
    spans.push(Span::raw("]"));
    spans
}

fn footer_line(snapshot: &UsageOverlaySnapshot) -> Line<'static> {
    let footer = if matches!(snapshot.view, UsageOverlayView::Error(_)) && snapshot.can_retry {
        "r retry · Esc cancel"
    } else {
        "Esc cancel"
    };
    Line::from(Span::styled(
        footer,
        Style::default().add_modifier(Modifier::DIM),
    ))
}

fn shows_sonnet_limit(plan_type: Option<&str>) -> bool {
    matches!(plan_type, Some("max" | "team")) || plan_type.is_none()
}

fn supports_extra_usage(plan_type: Option<&str>) -> bool {
    matches!(plan_type, Some("pro" | "max"))
}

fn format_cost(value: f64) -> String {
    format!("${value:.2}")
}

fn format_reset_text(resets_at: &str, show_timezone: bool, show_time: bool) -> Option<String> {
    let parsed = OffsetDateTime::parse(resets_at, &Rfc3339).ok()?;
    let offset = local_offset();
    let reset = parsed.to_offset(offset);
    let now = OffsetDateTime::now_utc().to_offset(offset);
    let hours_until_reset = (reset.unix_timestamp() - now.unix_timestamp()) as f64 / 3600.0;
    let text = if hours_until_reset > 24.0 {
        format_date_and_time(reset, now, show_time)
    } else {
        format_time(reset)
    };
    if show_timezone {
        Some(format!("{text} ({})", timezone_label(offset)))
    } else {
        Some(text)
    }
}

fn format_date_and_time(reset: OffsetDateTime, now: OffsetDateTime, show_time: bool) -> String {
    let mut parts = vec![format!("{} {}", month_name(reset.month()), reset.day())];
    if reset.year() != now.year() {
        parts.push(reset.year().to_string());
    }
    if show_time {
        parts.push(format_time(reset));
    }
    parts.join(", ")
}

fn format_time(reset: OffsetDateTime) -> String {
    let hour = reset.hour();
    let minute = reset.minute();
    let (display_hour, suffix) = match hour {
        0 => (12, "am"),
        1..=11 => (hour, "am"),
        12 => (12, "pm"),
        _ => (hour - 12, "pm"),
    };
    if minute == 0 {
        format!("{display_hour}{suffix}")
    } else {
        format!("{display_hour}:{minute:02}{suffix}")
    }
}

fn local_offset() -> UtcOffset {
    UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC)
}

fn timezone_label(offset: UtcOffset) -> String {
    if offset == UtcOffset::UTC {
        "UTC".to_string()
    } else {
        let seconds = offset.whole_seconds();
        let sign = if seconds >= 0 { '+' } else { '-' };
        let absolute = seconds.abs();
        let hours = absolute / 3600;
        let minutes = (absolute % 3600) / 60;
        format!("{sign}{hours:02}:{minutes:02}")
    }
}

fn month_name(month: Month) -> &'static str {
    match month {
        Month::January => "Jan",
        Month::February => "Feb",
        Month::March => "Mar",
        Month::April => "Apr",
        Month::May => "May",
        Month::June => "Jun",
        Month::July => "Jul",
        Month::August => "Aug",
        Month::September => "Sep",
        Month::October => "Oct",
        Month::November => "Nov",
        Month::December => "Dec",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_methods_update_snapshot_offset() {
        let overlay = UsageOverlay::unavailable_for_test();
        overlay.scroll_down();
        overlay.page_down();
        overlay.scroll_up();
        let snapshot = overlay.snapshot();
        assert_eq!(snapshot.scroll, 10);
    }

    #[test]
    fn unavailable_overlay_shows_expected_note() {
        let overlay = UsageOverlay::unavailable_for_test();
        let snapshot = overlay.snapshot();
        let lines = body_lines(&snapshot.view, 60)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        assert!(lines.contains(&"Usage".to_string()));
        assert!(lines.contains(&"Select a provider to view usage.".to_string()));
    }

    #[test]
    fn format_count_adds_grouping() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(1234), "1,234");
        assert_eq!(format_count(9876543), "9,876,543");
    }
}
