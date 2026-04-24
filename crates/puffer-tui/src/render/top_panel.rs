use super::prompt_border_style;
use super::summary::top_panel_compact_lines;
use image::{imageops::FilterType, DynamicImage, ImageReader, Rgba, RgbaImage};
use puffer_core::AppState;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_tools::ToolRegistry;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use ratatui::Frame;
use std::collections::VecDeque;
use std::env;
use std::sync::OnceLock;

pub(super) const TOP_PANEL_IMAGE_WIDTH: u32 = 16;
pub(super) const TOP_PANEL_IMAGE_HEIGHT: u32 = 13;
const PUFFER_IMAGE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/puffer.png");
const TOP_PANEL_GAP: u16 = 2;
const MIN_BOX_INNER_WIDTH_WITH_PUFFER: u16 = 44;

/// Initializes the cached top-panel puffer art before the first draw.
pub(crate) fn initialize_top_panel_image_state() {
    let _ = puffer_art_lines();
}

/// Renders the fixed top panel used on non-scrollable surfaces such as home.
pub(super) fn render_fixed_top_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    resources: &LoadedResources,
    auth_store: &AuthStore,
    tool_registry: &ToolRegistry,
    providers: &ProviderRegistry,
) {
    let art_lines = puffer_art_lines();
    let summary_lines =
        top_panel_compact_lines(state, resources, auth_store, tool_registry, providers);
    let plan = panel_render_plan(area, &art_lines, &summary_lines);

    if plan.show_puffer {
        Paragraph::new(Text::from(art_lines))
            .style(Style::reset())
            .render(plan.layout.art_area, frame.buffer_mut());
    }

    let block = panel_block(state);
    frame.render_widget(&block, plan.layout.box_area);
    Paragraph::new(Text::from(plan.summary_lines))
        .style(Style::reset())
        .render(inner_area(plan.layout.box_area), frame.buffer_mut());
}

/// Builds scrollable top-panel lines for transcript rendering.
pub(super) fn top_panel_lines(
    area_width: u16,
    state: &AppState,
    resources: &LoadedResources,
    auth_store: &AuthStore,
    tool_registry: &ToolRegistry,
    providers: &ProviderRegistry,
) -> Vec<Line<'static>> {
    let art_lines = puffer_art_lines();
    let summary_lines =
        top_panel_compact_lines(state, resources, auth_store, tool_registry, providers);
    let content_area = Rect {
        x: 0,
        y: 0,
        width: area_width.max(1),
        height: (art_lines.len().max(summary_lines.len().saturating_add(2)) as u16).max(1),
    };
    let plan = panel_render_plan(content_area, &art_lines, &summary_lines);
    let art_width = art_lines.iter().map(Line::width).max().unwrap_or(0);
    let gap = " ".repeat(TOP_PANEL_GAP as usize);
    let boxed_lines = boxed_summary_lines(&plan.summary_lines);
    let mut lines = Vec::new();

    lines.push(Line::default());
    if plan.show_puffer {
        let row_count = art_lines.len().max(boxed_lines.len());
        for index in 0..row_count {
            let mut spans = Vec::new();
            if let Some(art_line) = art_lines.get(index) {
                spans.extend(art_line.spans.clone());
            } else if art_width > 0 {
                spans.push(Span::raw(" ".repeat(art_width)));
            }
            if art_width > 0 {
                spans.push(Span::raw(gap.clone()));
            }
            if let Some(boxed_line) = boxed_lines.get(index) {
                spans.extend(boxed_line.spans.clone());
            }
            lines.push(Line::from(spans).patch_style(Style::reset()));
        }
    } else {
        lines.extend(boxed_lines);
    }
    lines.push(Line::default());
    lines
}

fn boxed_summary_lines(summary_lines: &[Line<'static>]) -> Vec<Line<'static>> {
    let title = " Puffer Code ";
    let inner_width = summary_lines
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
        .max(title.chars().count());
    let top_rule_width = inner_width.saturating_sub(title.chars().count());
    let mut lines = Vec::with_capacity(summary_lines.len().saturating_add(2));
    lines.push(Line::from(vec![
        Span::raw("╭"),
        Span::raw(title.to_string()),
        Span::raw("─".repeat(top_rule_width)),
        Span::raw("╮"),
    ]));
    for line in summary_lines {
        let text = line.to_string();
        let padding = inner_width.saturating_sub(line.width());
        lines.push(Line::from(vec![
            Span::raw("│"),
            Span::styled(text, Style::reset()),
            Span::raw(" ".repeat(padding)),
            Span::raw("│"),
        ]));
    }
    lines.push(Line::from(vec![
        Span::raw("╰"),
        Span::raw("─".repeat(inner_width)),
        Span::raw("╯"),
    ]));
    lines
}

fn panel_block(state: &AppState) -> Block<'static> {
    Block::default()
        .title(" Puffer Code ")
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(prompt_border_style(state))
}

fn panel_layout(
    area: Rect,
    show_puffer: bool,
    art_lines: &[Line<'static>],
    summary_lines: &[Line<'static>],
) -> TopPanelLayout {
    let art_width = art_lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let art_height = art_lines.len() as u16;
    let box_content_width = summary_lines
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
        .max(" Puffer Code ".chars().count()) as u16;
    let box_width = box_content_width.saturating_add(2);
    let box_height = summary_lines.len() as u16 + 2;
    let total_width = if show_puffer {
        art_width
            .saturating_add(TOP_PANEL_GAP)
            .saturating_add(box_width)
            .min(area.width)
    } else {
        box_width.min(area.width)
    };
    let start_x = area.x + area.width.saturating_sub(total_width) / 2;
    let art_x = start_x;
    let box_x = if show_puffer {
        (art_x + art_width + TOP_PANEL_GAP).min(area.x + area.width.saturating_sub(box_width))
    } else {
        area.x + area.width.saturating_sub(box_width) / 2
    };

    TopPanelLayout {
        art_area: Rect {
            x: art_x,
            y: area.y + area.height.saturating_sub(art_height) / 2,
            width: if show_puffer {
                art_width.min(area.width)
            } else {
                0
            },
            height: if show_puffer {
                art_height.min(area.height)
            } else {
                0
            },
        },
        box_area: Rect {
            x: box_x,
            y: area.y + area.height.saturating_sub(box_height) / 2,
            width: box_width.min(area.width.saturating_sub(box_x.saturating_sub(area.x))),
            height: box_height.min(area.height),
        },
    }
}

fn panel_render_plan(
    area: Rect,
    art_lines: &[Line<'static>],
    summary_lines: &[Line<'static>],
) -> TopPanelRenderPlan {
    let art_width = art_lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let show_puffer = should_show_puffer_with_art_width(area.width, art_width);
    let box_inner_width = if show_puffer {
        area.width
            .saturating_sub(art_width)
            .saturating_sub(TOP_PANEL_GAP)
            .saturating_sub(2)
    } else {
        area.width.saturating_sub(2)
    };
    let summary_lines = truncate_lines(summary_lines, box_inner_width as usize);
    let layout = panel_layout(area, show_puffer, art_lines, &summary_lines);
    TopPanelRenderPlan {
        layout,
        show_puffer,
        summary_lines,
    }
}

pub(super) fn should_show_puffer(total_width: u16) -> bool {
    should_show_puffer_with_art_width(total_width, (TOP_PANEL_IMAGE_WIDTH as u16) * 2)
}

fn should_show_puffer_with_art_width(total_width: u16, art_width: u16) -> bool {
    total_width
        >= art_width
            .saturating_add(TOP_PANEL_GAP)
            .saturating_add(MIN_BOX_INNER_WIDTH_WITH_PUFFER)
            .saturating_add(2)
}

fn truncate_lines(lines: &[Line<'static>], max_width: usize) -> Vec<Line<'static>> {
    lines
        .iter()
        .map(|line| truncate_line(line, max_width))
        .collect()
}

fn truncate_line(line: &Line<'static>, max_width: usize) -> Line<'static> {
    if line.width() <= max_width {
        return line.clone();
    }
    if max_width == 0 {
        return Line::default();
    }
    if max_width <= 3 {
        return Line::from(".".repeat(max_width)).patch_style(Style::reset());
    }

    let mut remaining = max_width - 3;
    let mut spans = Vec::new();
    for span in &line.spans {
        if remaining == 0 {
            break;
        }
        let mut taken = String::new();
        for ch in span.content.chars() {
            if remaining == 0 {
                break;
            }
            taken.push(ch);
            remaining -= 1;
        }
        if !taken.is_empty() {
            spans.push(Span::styled(taken, span.style));
        }
    }

    let ellipsis_style = spans
        .last()
        .map(|span| span.style)
        .unwrap_or(Style::reset());
    spans.push(Span::styled("...".to_string(), ellipsis_style));
    Line::from(spans).patch_style(Style::reset())
}

fn inner_area(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

fn puffer_art_lines() -> Vec<Line<'static>> {
    static LINES: OnceLock<Vec<Line<'static>>> = OnceLock::new();
    LINES.get_or_init(build_puffer_art_lines).clone()
}

fn build_puffer_art_lines() -> Vec<Line<'static>> {
    let Some(image) = puffer_image() else {
        return Vec::new();
    };
    let prepared = prepare_subject_image(&image);
    render_pixel_lines(&prepared, TOP_PANEL_IMAGE_WIDTH, TOP_PANEL_IMAGE_HEIGHT)
}

fn render_pixel_lines(image: &RgbaImage, width: u32, height: u32) -> Vec<Line<'static>> {
    let resized = DynamicImage::ImageRgba8(image.clone())
        .resize_exact(width, height, FilterType::Nearest)
        .to_rgba8();
    let mut lines = Vec::with_capacity(height as usize);
    for y in 0..height {
        let mut spans = Vec::with_capacity(width as usize);
        for x in 0..width {
            spans.push(pixel_span(*resized.get_pixel(x, y)));
        }
        lines.push(Line::from(spans).patch_style(Style::reset()));
    }
    lines
}

fn pixel_span(pixel: Rgba<u8>) -> Span<'static> {
    if pixel[3] < 24 {
        return Span::styled("  ", Style::reset());
    }
    Span::styled("██", color_style(pixel))
}

fn color_style(pixel: Rgba<u8>) -> Style {
    let color = resolved_color(pixel);
    Style::reset().fg(color).bg(color)
}

fn resolved_color(pixel: Rgba<u8>) -> Color {
    if supports_truecolor() {
        Color::Rgb(pixel[0], pixel[1], pixel[2])
    } else {
        indexed_palette_color(pixel)
    }
}

fn supports_truecolor() -> bool {
    static SUPPORTS_TRUECOLOR: OnceLock<bool> = OnceLock::new();
    *SUPPORTS_TRUECOLOR.get_or_init(|| {
        let color_term = env::var("COLORTERM")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let term = env::var("TERM").unwrap_or_default().to_ascii_lowercase();
        color_term.contains("truecolor")
            || color_term.contains("24bit")
            || term.contains("direct")
            || term.contains("truecolor")
    })
}

fn indexed_palette_color(pixel: Rgba<u8>) -> Color {
    // Preserve the eye as green; generic xterm nearest-color picks gray here.
    if pixel[0] < 40 && pixel[1] > 50 && pixel[2] < 90 {
        return Color::Indexed(28);
    }

    let (index, _) = PUFFER_INDEXED_PALETTE
        .iter()
        .copied()
        .min_by_key(|(_, candidate)| color_distance(pixel, *candidate))
        .unwrap_or((15, (255, 255, 255)));
    Color::Indexed(index)
}

fn color_distance(pixel: Rgba<u8>, candidate: (u8, u8, u8)) -> u32 {
    let dr = pixel[0] as i32 - candidate.0 as i32;
    let dg = pixel[1] as i32 - candidate.1 as i32;
    let db = pixel[2] as i32 - candidate.2 as i32;
    (dr * dr + dg * dg + db * db) as u32
}

const PUFFER_INDEXED_PALETTE: &[(u8, (u8, u8, u8))] = &[
    (15, (255, 255, 255)),
    (230, (255, 255, 215)),
    (229, (255, 255, 175)),
    (228, (255, 255, 135)),
    (221, (255, 215, 95)),
    (220, (255, 215, 0)),
    (214, (255, 175, 0)),
    (208, (255, 135, 0)),
    (172, (215, 135, 0)),
    (166, (215, 95, 0)),
    (123, (135, 255, 255)),
    (45, (0, 215, 255)),
    (14, (0, 255, 255)),
    (79, (95, 215, 175)),
    (28, (0, 135, 0)),
];

fn prepare_subject_image(image: &DynamicImage) -> RgbaImage {
    let mut rgba = image.to_rgba8();
    clear_edge_background(&mut rgba);
    crop_to_alpha_bounds(&rgba).unwrap_or(rgba)
}

struct TopPanelLayout {
    art_area: Rect,
    box_area: Rect,
}

struct TopPanelRenderPlan {
    layout: TopPanelLayout,
    show_puffer: bool,
    summary_lines: Vec<Line<'static>>,
}

fn clear_edge_background(image: &mut RgbaImage) {
    let (width, height) = image.dimensions();
    let mut seen = vec![false; (width * height) as usize];
    let mut queue = VecDeque::new();
    for x in 0..width {
        queue_edge_pixel(image, &mut seen, &mut queue, x, 0);
        queue_edge_pixel(image, &mut seen, &mut queue, x, height.saturating_sub(1));
    }
    for y in 0..height {
        queue_edge_pixel(image, &mut seen, &mut queue, 0, y);
        queue_edge_pixel(image, &mut seen, &mut queue, width.saturating_sub(1), y);
    }

    while let Some((x, y)) = queue.pop_front() {
        image.put_pixel(x, y, Rgba([0, 0, 0, 0]));
        for (next_x, next_y) in neighbors(x, y, width, height) {
            let index = pixel_index(next_x, next_y, width);
            if seen[index] || !is_background_candidate(*image.get_pixel(next_x, next_y)) {
                continue;
            }
            seen[index] = true;
            queue.push_back((next_x, next_y));
        }
    }
}

fn queue_edge_pixel(
    image: &RgbaImage,
    seen: &mut [bool],
    queue: &mut VecDeque<(u32, u32)>,
    x: u32,
    y: u32,
) {
    let index = pixel_index(x, y, image.width());
    if seen[index] || !is_background_candidate(*image.get_pixel(x, y)) {
        return;
    }
    seen[index] = true;
    queue.push_back((x, y));
}

fn neighbors(x: u32, y: u32, width: u32, height: u32) -> Vec<(u32, u32)> {
    let mut output = Vec::with_capacity(4);
    if x > 0 {
        output.push((x - 1, y));
    }
    if x + 1 < width {
        output.push((x + 1, y));
    }
    if y > 0 {
        output.push((x, y - 1));
    }
    if y + 1 < height {
        output.push((x, y + 1));
    }
    output
}

fn pixel_index(x: u32, y: u32, width: u32) -> usize {
    (y * width + x) as usize
}

fn is_background_candidate(pixel: Rgba<u8>) -> bool {
    pixel[3] > 200 && pixel[0] > 245 && pixel[1] > 245 && pixel[2] > 245
}

fn crop_to_alpha_bounds(image: &RgbaImage) -> Option<RgbaImage> {
    let (width, height) = image.dimensions();
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0;
    let mut max_y = 0;
    let mut found = false;
    for y in 0..height {
        for x in 0..width {
            if image.get_pixel(x, y)[3] == 0 {
                continue;
            }
            found = true;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    if !found {
        return None;
    }
    Some(
        DynamicImage::ImageRgba8(image.clone())
            .crop_imm(
                min_x,
                min_y,
                max_x.saturating_sub(min_x).saturating_add(1),
                max_y.saturating_sub(min_y).saturating_add(1),
            )
            .to_rgba8(),
    )
}

fn puffer_image() -> Option<DynamicImage> {
    static IMAGE: OnceLock<Option<DynamicImage>> = OnceLock::new();
    IMAGE
        .get_or_init(|| ImageReader::open(PUFFER_IMAGE_PATH).ok()?.decode().ok())
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageBuffer;

    #[test]
    fn edge_background_removal_keeps_internal_white_pixels() {
        let mut image = ImageBuffer::from_pixel(5, 5, Rgba([255, 255, 255, 255]));
        for x in 1..4 {
            for y in 1..4 {
                image.put_pixel(x, y, Rgba([255, 170, 0, 255]));
            }
        }
        image.put_pixel(2, 2, Rgba([255, 255, 246, 255]));

        clear_edge_background(&mut image);

        assert_eq!(image.get_pixel(0, 0)[3], 0);
        assert_eq!(*image.get_pixel(2, 2), Rgba([255, 255, 246, 255]));
    }

    #[test]
    fn indexed_palette_keeps_eye_green() {
        assert_eq!(
            indexed_palette_color(Rgba([14, 71, 54, 255])),
            Color::Indexed(28)
        );
    }

    #[test]
    fn indexed_palette_maps_body_yellow_to_bright_yellow() {
        assert_eq!(
            indexed_palette_color(Rgba([255, 196, 0, 255])),
            Color::Indexed(220)
        );
    }

    #[test]
    fn narrow_width_hides_puffer() {
        assert!(!should_show_puffer(72));
    }

    #[test]
    fn truncate_line_appends_ellipsis() {
        let line = Line::from("Session   Shipyard · 12345678-1234-5678-1234-567812345678");
        let truncated = truncate_line(&line, 20);
        assert_eq!(truncated.to_string(), "Session   Shipyar...");
    }
}
