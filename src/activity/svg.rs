use super::plot::{format_value, month_labels, summary, title, ActivityPlot, CalendarGrid};
use super::series::{ActivityDay, ActivitySeries, HourlyActivitySeries};
use crate::cli::GraphChart;
use crate::format::svg::escape_xml;
use std::fmt::Write;

const FONT_FAMILY: &str = "-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif";
const MONO_FONT_FAMILY: &str = "ui-monospace, SFMono-Regular, Menlo, Consolas, 'Liberation Mono', monospace";
const BACKGROUND: &str = "#0d1117";
const BORDER: &str = "#30363d";
const TEXT: &str = "#f0f6fc";
const MUTED: &str = "#8b949e";
const GRID: &str = "#21262d";
const HEADER_HEIGHT: usize = 48;
const CONTENT_TOP: usize = 96;
const MIN_WIDTH: usize = 360;
const MAX_WIDTH: usize = 1_400;

pub(super) fn render_activity_svg(series: &ActivitySeries, chart: GraphChart, command: &str) -> String {
  match chart.resolve(series.len()) {
    GraphChart::Plot => render_plot(&ActivityPlot::from_daily(series), "day", command),
    GraphChart::Heatmap => render_heatmap(series, command),
    GraphChart::Auto => unreachable!("auto chart is resolved before rendering"),
  }
}

pub(super) fn render_hourly_activity_svg(series: &HourlyActivitySeries, command: &str) -> String {
  render_plot(&ActivityPlot::from_hourly(series), "hour", command)
}

fn render_plot(plot: &ActivityPlot, resolution: &str, command: &str) -> String {
  let data_width = plot.len().saturating_mul(18) + 110;
  let title_width = plot.title.chars().count().saturating_mul(11) + 56;
  let summary_width = plot.summary.chars().count().saturating_mul(7) + 56;
  let width = content_width(data_width.max(title_width).max(summary_width), command);
  let height = 360;
  let chart_left = 78.0;
  let chart_right = width as f64 - 30.0;
  let chart_top = 76.0;
  let chart_bottom = 260.0;
  let chart_width = chart_right - chart_left;
  let chart_height = chart_bottom - chart_top;
  let max = plot
    .points
    .iter()
    .map(|point| point.value)
    .filter(|value| value.is_finite())
    .fold(0.0, f64::max);

  let mut out = svg_start(
    &plot.accessible_title,
    &format!("{}. {}", plot.title, plot.summary),
    "plot",
    resolution,
    width,
    height,
    command,
  );
  text_element(
    &mut out,
    28.0,
    39.0,
    20,
    TEXT,
    "start",
    &plot.title,
    "font-weight=\"600\"",
  );

  for tick in 0..=3 {
    let fraction = tick as f64 / 3.0;
    let y = chart_bottom - fraction * chart_height;
    let value = max * fraction;
    writeln!(
      out,
      "  <line x1=\"{chart_left:.1}\" y1=\"{y:.1}\" x2=\"{chart_right:.1}\" y2=\"{y:.1}\" stroke=\"{GRID}\"/>"
    )
    .unwrap();
    text_element(
      &mut out,
      chart_left - 10.0,
      y + 4.0,
      12,
      MUTED,
      "end",
      &format_value(value, plot.unit, false),
      "",
    );
  }

  let slot = if plot.is_empty() {
    chart_width
  } else {
    chart_width / plot.len() as f64
  };
  let bar_width = (slot * 0.72).max(1.0);
  for (index, point) in plot.points.iter().enumerate() {
    let slot_x = chart_left + index as f64 * slot;
    if point.value > 0.0 && point.value.is_finite() && max > 0.0 {
      let bar_height = (point.value / max * chart_height).max(1.0);
      let x = slot_x + (slot - bar_width) / 2.0;
      let y = chart_bottom - bar_height;
      writeln!(
        out,
        "  <rect class=\"activity-bar\" x=\"{x:.1}\" y=\"{y:.1}\" width=\"{bar_width:.1}\" height=\"{bar_height:.1}\" rx=\"2\" fill=\"{}\"/>",
        level_color(point.level)
      )
      .unwrap();
    }
    writeln!(
      out,
      "  <rect class=\"activity-hit-target\" x=\"{slot_x:.1}\" y=\"{chart_top:.1}\" width=\"{slot:.1}\" height=\"{chart_height:.1}\" fill=\"#000000\" fill-opacity=\"0\"><title>{}</title></rect>",
      escape_xml(&point.tooltip)
    )
    .unwrap();
  }

  if !plot.is_empty() {
    let labels = match plot.len() {
      1 => vec![(0, "middle", chart_left + chart_width / 2.0)],
      2 => vec![(0, "start", chart_left), (1, "end", chart_right)],
      _ => vec![
        (0, "start", chart_left),
        (plot.len() / 2, "middle", chart_left + chart_width / 2.0),
        (plot.len() - 1, "end", chart_right),
      ],
    };
    for (index, anchor, x) in labels {
      text_element(
        &mut out,
        x,
        chart_bottom + 25.0,
        12,
        MUTED,
        anchor,
        &plot.points[index].axis_label,
        "class=\"activity-axis-label\"",
      );
    }
  }

  text_element(&mut out, 28.0, 329.0, 13, MUTED, "start", &plot.summary, "");
  out.push_str("  </g>\n</svg>\n");
  out
}

fn render_heatmap(series: &ActivitySeries, command: &str) -> String {
  const CELL: f64 = 11.0;
  const GAP: f64 = 3.0;
  const PITCH: f64 = CELL + GAP;
  const GRID_LEFT: f64 = 64.0;
  const GRID_TOP: f64 = 92.0;

  let grid = CalendarGrid::new(series);
  let week_count = grid.as_ref().map(|grid| grid.week_count).unwrap_or_default();
  let data_width = (GRID_LEFT + week_count as f64 * PITCH + 30.0).ceil() as usize;
  let height = 280;
  let chart_title = format!("{} activity graph", super::plot::unit_name(series.unit));
  let chart_desc = format!("{}. {}", title(series), summary(series));
  let text_width = chart_desc.chars().count().saturating_mul(7) + 56;
  let width = content_width(data_width.max(text_width), command);
  let mut out = svg_start(&chart_title, &chart_desc, "heatmap", "day", width, height, command);
  text_element(
    &mut out,
    28.0,
    39.0,
    20,
    TEXT,
    "start",
    &title(series),
    "font-weight=\"600\"",
  );

  if let Some(grid) = grid.as_ref() {
    for (week, label) in month_labels(series, grid) {
      let x = GRID_LEFT + week as f64 * PITCH;
      text_element(&mut out, x, GRID_TOP - 15.0, 12, MUTED, "start", &label, "");
    }

    for (row, label) in [(1, "Mon"), (3, "Wed"), (5, "Fri")] {
      let y = GRID_TOP + row as f64 * PITCH + CELL - 1.0;
      text_element(&mut out, GRID_LEFT - 10.0, y, 12, MUTED, "end", label, "");
    }

    for week in 0..grid.week_count {
      for weekday in 0..7 {
        let date = grid.date(week, weekday);
        let Some(day) = series.day(date) else {
          continue;
        };
        let x = GRID_LEFT + week as f64 * PITCH;
        let y = GRID_TOP + weekday as f64 * PITCH;
        let stroke = if day.level == 0 { BORDER } else { level_color(day.level) };
        writeln!(
          out,
          "  <rect class=\"activity-cell\" x=\"{x:.1}\" y=\"{y:.1}\" width=\"{CELL:.1}\" height=\"{CELL:.1}\" rx=\"2\" fill=\"{}\" stroke=\"{stroke}\"><title>{}</title></rect>",
          level_color(day.level),
          escape_xml(&day_tooltip(day, series))
        )
        .unwrap();
      }
    }
  }

  let legend_y = 211.0;
  text_element(&mut out, 28.0, legend_y + 10.0, 12, MUTED, "start", "Less", "");
  for level in 0..=4 {
    let x = 61.0 + f64::from(level) * PITCH;
    writeln!(
      out,
      "  <rect x=\"{x:.1}\" y=\"{legend_y:.1}\" width=\"{CELL:.1}\" height=\"{CELL:.1}\" rx=\"2\" fill=\"{}\" stroke=\"{}\"/>",
      level_color(level),
      if level == 0 { BORDER } else { level_color(level) }
    )
    .unwrap();
  }
  text_element(&mut out, 136.0, legend_y + 10.0, 12, MUTED, "start", "More", "");
  text_element(&mut out, 28.0, 254.0, 13, MUTED, "start", &summary(series), "");
  out.push_str("  </g>\n</svg>\n");
  out
}

fn svg_start(
  chart_title: &str,
  chart_desc: &str,
  chart: &str,
  resolution: &str,
  width: usize,
  height: usize,
  command: &str,
) -> String {
  let outer_height = height + CONTENT_TOP;
  let mut out = String::new();
  writeln!(
    out,
    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{outer_height}\" viewBox=\"0 0 {width} {outer_height}\" role=\"img\" aria-labelledby=\"title desc\" data-chart=\"{chart}\" data-resolution=\"{resolution}\">"
  )
  .unwrap();
  writeln!(out, "  <title id=\"title\">{}</title>", escape_xml(chart_title)).unwrap();
  writeln!(out, "  <desc id=\"desc\">{}</desc>", escape_xml(chart_desc)).unwrap();
  writeln!(
    out,
    "  <rect x=\"0.5\" y=\"0.5\" width=\"{}\" height=\"{}\" rx=\"16\" fill=\"{BACKGROUND}\" stroke=\"{BORDER}\"/>",
    width - 1,
    outer_height - 1
  )
  .unwrap();
  writeln!(
    out,
    "  <rect x=\"0.5\" y=\"0.5\" width=\"{}\" height=\"{HEADER_HEIGHT}\" rx=\"16\" fill=\"#161b22\"/>",
    width - 1
  )
  .unwrap();
  writeln!(
    out,
    "  <rect x=\"0.5\" y=\"32\" width=\"{}\" height=\"17\" fill=\"#161b22\"/>",
    width - 1
  )
  .unwrap();
  out.push_str("  <circle cx=\"24\" cy=\"24\" r=\"6\" fill=\"#ff5f56\"/>\n");
  out.push_str("  <circle cx=\"44\" cy=\"24\" r=\"6\" fill=\"#ffbd2e\"/>\n");
  out.push_str("  <circle cx=\"64\" cy=\"24\" r=\"6\" fill=\"#27c93f\"/>\n");
  chrome_text_element(&mut out, width as f64 / 2.0, 29.0, 13, "middle", "llm-tokei");
  chrome_text_element(&mut out, 22.0, 75.0, 14, "start", &format!("$ {command}"));
  writeln!(out, "  <g transform=\"translate(0 {CONTENT_TOP})\">").unwrap();
  out
}

fn chrome_text_element(out: &mut String, x: f64, y: f64, size: usize, anchor: &str, text: &str) {
  writeln!(
    out,
    "  <text x=\"{x:.1}\" y=\"{y:.1}\" fill=\"{MUTED}\" font-family=\"{MONO_FONT_FAMILY}\" font-size=\"{size}\" text-anchor=\"{anchor}\">{}</text>",
    escape_xml(text)
  )
  .unwrap();
}

fn content_width(chart_width: usize, command: &str) -> usize {
  let command_width = command.chars().count().saturating_mul(8) + 44;
  chart_width.max(command_width).clamp(MIN_WIDTH, MAX_WIDTH)
}

#[allow(clippy::too_many_arguments)]
fn text_element(out: &mut String, x: f64, y: f64, size: usize, color: &str, anchor: &str, text: &str, extra: &str) {
  writeln!(
    out,
    "  <text x=\"{x:.1}\" y=\"{y:.1}\" fill=\"{color}\" font-family=\"{FONT_FAMILY}\" font-size=\"{size}\" text-anchor=\"{anchor}\" {extra}>{}</text>",
    escape_xml(text)
  )
  .unwrap();
}

fn day_tooltip(day: &ActivityDay, series: &ActivitySeries) -> String {
  format!(
    "{}: {}",
    day.date.format("%b %-d, %Y"),
    format_value(day.value, series.unit, day.estimated)
  )
}

fn level_color(level: u8) -> &'static str {
  match level {
    0 => "#161b22",
    1 => "#0e4429",
    2 => "#006d32",
    3 => "#26a641",
    _ => "#39d353",
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::cli::Unit;
  use chrono::NaiveDate;

  fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
  }

  #[test]
  fn auto_renders_short_ranges_as_native_svg_bars() {
    let series = ActivitySeries::from_values(date(2026, 7, 1), (1..=30).map(f64::from).collect(), Unit::Tokens);
    let svg = render_activity_svg(&series, GraphChart::Auto, "llm-tokei graph --month --format svg");

    assert!(svg.starts_with("<svg "));
    assert!(svg.contains("data-chart=\"plot\""));
    assert!(svg.contains("data-resolution=\"day\""));
    assert!(svg.contains("class=\"activity-bar\""));
    assert_eq!(svg.matches("class=\"activity-hit-target\"").count(), 30);
    assert!(svg.contains("fill=\"#ff5f56\""));
    assert!(svg.contains("$ llm-tokei graph --month --format svg"));
    assert!(svg.ends_with("</svg>\n"));
  }

  #[test]
  fn auto_renders_long_ranges_as_accessible_calendar_cells() {
    let series = ActivitySeries::from_values(date(2026, 6, 1), vec![1.0; 31], Unit::Tokens);
    let svg = render_activity_svg(&series, GraphChart::Auto, "llm-tokei graph --format svg");

    assert!(svg.contains("data-chart=\"heatmap\""));
    assert_eq!(svg.matches("class=\"activity-cell\"").count(), 31);
    assert!(svg.contains("aria-labelledby=\"title desc\""));
    assert!(svg.contains("<title>Jun 1, 2026: 1</title>"));
    assert!(svg.contains("fill=\"#39d353\""));
  }

  #[test]
  fn plot_includes_zero_grid_and_summary() {
    let series = ActivitySeries::from_values(date(2026, 7, 1), vec![0.0; 7], Unit::Cost);
    let svg = render_activity_svg(&series, GraphChart::Plot, "llm-tokei graph --7d --format svg");
    assert!(svg.contains("$0.00"));
    assert!(svg.contains("Active 0/7 days"));
    assert!(!svg.contains("class=\"activity-bar\""));
  }

  #[test]
  fn hourly_svg_uses_hour_resolution_and_tooltips() {
    use chrono::{DateTime, Utc};

    let start = DateTime::parse_from_rfc3339("2026-07-11T01:00:00Z")
      .unwrap()
      .with_timezone(&Utc);
    let series = HourlyActivitySeries::from_values(start, vec![0.0, 10.0, 20.0], Unit::Tokens);
    let svg = render_hourly_activity_svg(&series, "llm-tokei graph --3h --format svg");

    assert!(svg.contains("data-chart=\"plot\""));
    assert!(svg.contains("data-resolution=\"hour\""));
    assert!(svg.contains("Hourly token activity"));
    assert_eq!(svg.matches("class=\"activity-hit-target\"").count(), 3);
    assert_eq!(svg.matches("class=\"activity-axis-label\"").count(), 3);
    let width = svg
      .split_once("width=\"")
      .and_then(|(_, rest)| rest.split_once('"'))
      .and_then(|(width, _)| width.parse::<usize>().ok())
      .unwrap();
    assert!(width < 680);
  }

  #[test]
  fn short_hourly_svg_does_not_repeat_axis_labels() {
    use chrono::{DateTime, Utc};

    let start = DateTime::parse_from_rfc3339("2026-07-11T01:00:00Z")
      .unwrap()
      .with_timezone(&Utc);
    let one_hour = render_hourly_activity_svg(
      &HourlyActivitySeries::from_values(start, vec![10.0], Unit::Tokens),
      "llm-tokei graph --1h --format svg",
    );
    let two_hours = render_hourly_activity_svg(
      &HourlyActivitySeries::from_values(start, vec![10.0, 20.0], Unit::Tokens),
      "llm-tokei graph --2h --format svg",
    );

    assert_eq!(one_hour.matches("class=\"activity-axis-label\"").count(), 1);
    assert_eq!(two_hours.matches("class=\"activity-axis-label\"").count(), 2);
  }
}
