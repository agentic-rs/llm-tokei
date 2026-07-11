use super::activity_common::{format_date_short, format_value, month_labels, summary, title, CalendarGrid};
use super::svg::escape_xml;
use crate::activity::{ActivityDay, ActivitySeries};
use crate::cli::GraphChart;
use std::fmt::Write;

const FONT_FAMILY: &str = "-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif";
const BACKGROUND: &str = "#0d1117";
const BORDER: &str = "#30363d";
const TEXT: &str = "#f0f6fc";
const MUTED: &str = "#8b949e";
const GRID: &str = "#21262d";

pub fn render_activity_svg(series: &ActivitySeries, chart: GraphChart) -> String {
  match chart.resolve(series.len()) {
    GraphChart::Plot => render_plot(series),
    GraphChart::Heatmap => render_heatmap(series),
    GraphChart::Auto => unreachable!("auto chart is resolved before rendering"),
  }
}

fn render_plot(series: &ActivitySeries) -> String {
  let width = (series.len().saturating_mul(18) + 110).clamp(680, 1_400);
  let height = 360;
  let chart_left = 78.0;
  let chart_right = width as f64 - 30.0;
  let chart_top = 76.0;
  let chart_bottom = 260.0;
  let chart_width = chart_right - chart_left;
  let chart_height = chart_bottom - chart_top;
  let max = series
    .days
    .iter()
    .map(|day| day.value)
    .filter(|value| value.is_finite())
    .fold(0.0, f64::max);

  let mut out = svg_start(series, "plot", width, height);
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
      &format_value(value, series.unit, false),
      "",
    );
  }

  let slot = if series.is_empty() {
    chart_width
  } else {
    chart_width / series.len() as f64
  };
  let bar_width = (slot * 0.72).max(1.0);
  for (index, day) in series.days.iter().enumerate() {
    let slot_x = chart_left + index as f64 * slot;
    if day.value > 0.0 && day.value.is_finite() && max > 0.0 {
      let bar_height = (day.value / max * chart_height).max(1.0);
      let x = slot_x + (slot - bar_width) / 2.0;
      let y = chart_bottom - bar_height;
      writeln!(
        out,
        "  <rect class=\"activity-bar\" x=\"{x:.1}\" y=\"{y:.1}\" width=\"{bar_width:.1}\" height=\"{bar_height:.1}\" rx=\"2\" fill=\"{}\"/>",
        level_color(day.level)
      )
      .unwrap();
    }
    writeln!(
      out,
      "  <rect class=\"activity-hit-target\" x=\"{slot_x:.1}\" y=\"{chart_top:.1}\" width=\"{slot:.1}\" height=\"{chart_height:.1}\" fill=\"#000000\" fill-opacity=\"0\"><title>{}</title></rect>",
      escape_xml(&day_tooltip(day, series))
    )
    .unwrap();
  }

  if !series.is_empty() {
    let indices = [0, series.len() / 2, series.len() - 1];
    let anchors = ["start", "middle", "end"];
    let positions = [chart_left, chart_left + chart_width / 2.0, chart_right];
    for ((index, anchor), x) in indices.into_iter().zip(anchors).zip(positions) {
      text_element(
        &mut out,
        x,
        chart_bottom + 25.0,
        12,
        MUTED,
        anchor,
        &format_date_short(series.days[index].date),
        "",
      );
    }
  }

  text_element(&mut out, 28.0, 329.0, 13, MUTED, "start", &summary(series), "");
  out.push_str("</svg>\n");
  out
}

fn render_heatmap(series: &ActivitySeries) -> String {
  const CELL: f64 = 11.0;
  const GAP: f64 = 3.0;
  const PITCH: f64 = CELL + GAP;
  const GRID_LEFT: f64 = 64.0;
  const GRID_TOP: f64 = 92.0;

  let grid = CalendarGrid::new(series);
  let week_count = grid.as_ref().map(|grid| grid.week_count).unwrap_or_default();
  let width = ((GRID_LEFT + week_count as f64 * PITCH + 30.0).ceil() as usize).max(680);
  let height = 280;
  let mut out = svg_start(series, "heatmap", width, height);
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
  out.push_str("</svg>\n");
  out
}

fn svg_start(series: &ActivitySeries, chart: &str, width: usize, height: usize) -> String {
  let chart_title = format!("{} activity graph", super::activity_common::unit_name(series.unit));
  let chart_desc = format!("{}. {}", title(series), summary(series));
  let mut out = String::new();
  writeln!(
    out,
    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" role=\"img\" aria-labelledby=\"title desc\" data-chart=\"{chart}\">"
  )
  .unwrap();
  writeln!(out, "  <title id=\"title\">{}</title>", escape_xml(&chart_title)).unwrap();
  writeln!(out, "  <desc id=\"desc\">{}</desc>", escape_xml(&chart_desc)).unwrap();
  writeln!(
    out,
    "  <rect x=\"0.5\" y=\"0.5\" width=\"{}\" height=\"{}\" rx=\"12\" fill=\"{BACKGROUND}\" stroke=\"{BORDER}\"/>",
    width - 1,
    height - 1
  )
  .unwrap();
  out
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
    let svg = render_activity_svg(&series, GraphChart::Auto);

    assert!(svg.starts_with("<svg "));
    assert!(svg.contains("data-chart=\"plot\""));
    assert!(svg.contains("class=\"activity-bar\""));
    assert_eq!(svg.matches("class=\"activity-hit-target\"").count(), 30);
    assert!(!svg.contains("terminal-content"));
    assert!(svg.ends_with("</svg>\n"));
  }

  #[test]
  fn auto_renders_long_ranges_as_accessible_calendar_cells() {
    let series = ActivitySeries::from_values(date(2026, 6, 1), vec![1.0; 31], Unit::Tokens);
    let svg = render_activity_svg(&series, GraphChart::Auto);

    assert!(svg.contains("data-chart=\"heatmap\""));
    assert_eq!(svg.matches("class=\"activity-cell\"").count(), 31);
    assert!(svg.contains("aria-labelledby=\"title desc\""));
    assert!(svg.contains("<title>Jun 1, 2026: 1</title>"));
    assert!(svg.contains("fill=\"#39d353\""));
  }

  #[test]
  fn plot_includes_zero_grid_and_summary() {
    let series = ActivitySeries::from_values(date(2026, 7, 1), vec![0.0; 7], Unit::Cost);
    let svg = render_activity_svg(&series, GraphChart::Plot);
    assert!(svg.contains("$0.00"));
    assert!(svg.contains("Active 0/7 days"));
    assert!(!svg.contains("class=\"activity-bar\""));
  }
}
