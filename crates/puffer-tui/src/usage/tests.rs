use super::*;
use time::{Date, Time};

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

#[test]
fn extra_usage_reset_handles_month_end_dates() {
    let may_thirty_first = Date::from_calendar_date(2026, Month::May, 31)
        .unwrap()
        .with_time(Time::MIDNIGHT)
        .assume_utc();
    let reset = next_extra_usage_reset(may_thirty_first).unwrap();
    assert_eq!(reset.year(), 2026);
    assert_eq!(reset.month(), Month::June);
    assert_eq!(reset.day(), 1);
}
