use super::plot::{format_value, month_labels, summary, title, ActivityPlot, CalendarGrid};
use super::series::{ActivityDay, ActivitySeries, HourlyActivitySeries};
use crate::cli::{GraphChart, SvgTheme};
use crate::format::svg::escape_xml;
use crate::format::svg_theme::{write_svg_theme_styles, SvgColor};
use std::fmt::Write;

const FONT_FAMILY: &str = "-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif";
const MONO_FONT_FAMILY: &str = "ui-monospace, SFMono-Regular, Menlo, Consolas, 'Liberation Mono', monospace";
const HEADER_HEIGHT: usize = 48;
const CONTENT_TOP: usize = 72;
const MIN_WIDTH: usize = 360;
const MAX_WIDTH: usize = 1_400;

struct SvgFrame<'a> {
  chart_title: &'a str,
  chart_desc: &'a str,
  chart: &'a str,
  resolution: &'a str,
  command: &'a str,
}

pub(super) fn render_activity_svg(
  series: &ActivitySeries,
  chart: GraphChart,
  command: &str,
  theme: SvgTheme,
) -> String {
  match chart.resolve(series.len()) {
    GraphChart::Plot => render_plot(&ActivityPlot::from_daily(series), "day", command, theme),
    GraphChart::Heatmap => render_heatmap(series, command, theme),
    GraphChart::Auto => unreachable!("auto chart is resolved before rendering"),
  }
}

pub(super) fn render_hourly_activity_svg(series: &HourlyActivitySeries, command: &str, theme: SvgTheme) -> String {
  render_plot(&ActivityPlot::from_hourly(series), "hour", command, theme)
}

fn render_plot(plot: &ActivityPlot, resolution: &str, command: &str, theme: SvgTheme) -> String {
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

  let chart_desc = format!("{}. {}", plot.title, plot.summary);
  let mut out = svg_start(
    SvgFrame {
      chart_title: &plot.accessible_title,
      chart_desc: &chart_desc,
      chart: "plot",
      resolution,
      command,
    },
    width,
    height,
    theme,
  );
  text_element(
    &mut out,
    28.0,
    39.0,
    20,
    SvgColor::Text,
    theme,
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
      "  <line x1=\"{chart_left:.1}\" y1=\"{y:.1}\" x2=\"{chart_right:.1}\" y2=\"{y:.1}\" data-svg-stroke=\"{}\" stroke=\"{}\"/>",
      SvgColor::Grid.name(),
      SvgColor::Grid.color(theme)
    )
    .unwrap();
    text_element(
      &mut out,
      chart_left - 10.0,
      y + 4.0,
      12,
      SvgColor::Muted,
      theme,
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
      "  <rect class=\"activity-bar\" data-level=\"{}\" data-svg-fill=\"{}\" x=\"{x:.1}\" y=\"{y:.1}\" width=\"{bar_width:.1}\" height=\"{bar_height:.1}\" rx=\"2\" fill=\"{}\"/>",
        point.level,
        level_color(point.level).name(),
        level_color(point.level).color(theme)
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
        SvgColor::Muted,
        theme,
        anchor,
        &plot.points[index].axis_label,
        "class=\"activity-axis-label\"",
      );
    }
  }

  text_element(
    &mut out,
    28.0,
    329.0,
    13,
    SvgColor::Muted,
    theme,
    "start",
    &plot.summary,
    "",
  );
  out.push_str("  </g>\n</svg>\n");
  out
}

fn render_heatmap(series: &ActivitySeries, command: &str, theme: SvgTheme) -> String {
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
  let mut out = svg_start(
    SvgFrame {
      chart_title: &chart_title,
      chart_desc: &chart_desc,
      chart: "heatmap",
      resolution: "day",
      command,
    },
    width,
    height,
    theme,
  );
  text_element(
    &mut out,
    28.0,
    39.0,
    20,
    SvgColor::Text,
    theme,
    "start",
    &title(series),
    "font-weight=\"600\"",
  );

  if let Some(grid) = grid.as_ref() {
    for (week, label) in month_labels(series, grid) {
      let x = GRID_LEFT + week as f64 * PITCH;
      text_element(
        &mut out,
        x,
        GRID_TOP - 15.0,
        12,
        SvgColor::Muted,
        theme,
        "start",
        &label,
        "",
      );
    }

    for (row, label) in [(1, "Mon"), (3, "Wed"), (5, "Fri")] {
      let y = GRID_TOP + row as f64 * PITCH + CELL - 1.0;
      text_element(
        &mut out,
        GRID_LEFT - 10.0,
        y,
        12,
        SvgColor::Muted,
        theme,
        "end",
        label,
        "",
      );
    }

    for week in 0..grid.week_count {
      for weekday in 0..7 {
        let date = grid.date(week, weekday);
        let Some(day) = series.day(date) else {
          continue;
        };
        let x = GRID_LEFT + week as f64 * PITCH;
        let y = GRID_TOP + weekday as f64 * PITCH;
        let color = level_color(day.level);
        let stroke = if day.level == 0 { SvgColor::Border } else { color };
        writeln!(
          out,
          "  <rect class=\"activity-cell\" data-level=\"{}\" data-svg-fill=\"{}\" data-svg-stroke=\"{}\" x=\"{x:.1}\" y=\"{y:.1}\" width=\"{CELL:.1}\" height=\"{CELL:.1}\" rx=\"2\" fill=\"{}\" stroke=\"{}\"><title>{}</title></rect>",
          day.level,
          color.name(),
          stroke.name(),
          color.color(theme),
          stroke.color(theme),
          escape_xml(&day_tooltip(day, series))
        )
        .unwrap();
      }
    }
  }

  let legend_y = 211.0;
  text_element(
    &mut out,
    28.0,
    legend_y + 10.0,
    12,
    SvgColor::Muted,
    theme,
    "start",
    "Less",
    "",
  );
  for level in 0..=4 {
    let x = 61.0 + f64::from(level) * PITCH;
    writeln!(
      out,
      "  <rect data-level=\"{level}\" data-svg-fill=\"{}\" data-svg-stroke=\"{}\" x=\"{x:.1}\" y=\"{legend_y:.1}\" width=\"{CELL:.1}\" height=\"{CELL:.1}\" rx=\"2\" fill=\"{}\" stroke=\"{}\"/>",
      level_color(level).name(),
      if level == 0 { SvgColor::Border.name() } else { level_color(level).name() },
      level_color(level).color(theme),
      if level == 0 { SvgColor::Border.color(theme) } else { level_color(level).color(theme) }
    )
    .unwrap();
  }
  text_element(
    &mut out,
    136.0,
    legend_y + 10.0,
    12,
    SvgColor::Muted,
    theme,
    "start",
    "More",
    "",
  );
  text_element(
    &mut out,
    28.0,
    254.0,
    13,
    SvgColor::Muted,
    theme,
    "start",
    &summary(series),
    "",
  );
  out.push_str("  </g>\n</svg>\n");
  out
}

fn svg_start(frame: SvgFrame<'_>, width: usize, height: usize, theme: SvgTheme) -> String {
  let outer_height = height + CONTENT_TOP;
  let mut out = String::new();
  writeln!(
    out,
    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{outer_height}\" viewBox=\"0 0 {width} {outer_height}\" role=\"img\" aria-labelledby=\"title desc\" data-chart=\"{}\" data-resolution=\"{}\" data-svg-theme-default=\"{}\">",
    frame.chart,
    frame.resolution,
    theme.as_str()
  )
  .unwrap();
  writeln!(out, "  <title id=\"title\">{}</title>", escape_xml(frame.chart_title)).unwrap();
  writeln!(out, "  <desc id=\"desc\">{}</desc>", escape_xml(frame.chart_desc)).unwrap();
  write_svg_theme_styles(&mut out, theme);
  writeln!(
    out,
    "  <rect x=\"0.5\" y=\"0.5\" width=\"{}\" height=\"{}\" rx=\"16\" data-svg-fill=\"{}\" fill=\"{}\" data-svg-stroke=\"{}\" stroke=\"{}\"/>",
    width - 1,
    outer_height - 1,
    SvgColor::Background.name(),
    SvgColor::Background.color(theme),
    SvgColor::Border.name(),
    SvgColor::Border.color(theme)
  )
  .unwrap();
  writeln!(
    out,
    "  <rect x=\"0.5\" y=\"0.5\" width=\"{}\" height=\"{HEADER_HEIGHT}\" rx=\"16\" data-svg-fill=\"{}\" fill=\"{}\"/>",
    width - 1,
    SvgColor::Surface.name(),
    SvgColor::Surface.color(theme)
  )
  .unwrap();
  writeln!(
    out,
    "  <rect x=\"0.5\" y=\"32\" width=\"{}\" height=\"17\" data-svg-fill=\"{}\" fill=\"{}\"/>",
    width - 1,
    SvgColor::Surface.name(),
    SvgColor::Surface.color(theme)
  )
  .unwrap();
  circle_element(&mut out, 24.0, SvgColor::WindowRed, theme);
  circle_element(&mut out, 44.0, SvgColor::WindowYellow, theme);
  circle_element(&mut out, 64.0, SvgColor::WindowGreen, theme);
  chrome_text_element(&mut out, width as f64 / 2.0, 29.0, 13, theme, "middle", "llm-tokei");
  chrome_text_element(
    &mut out,
    22.0,
    75.0,
    14,
    theme,
    "start",
    &format!("$ {}", frame.command),
  );
  writeln!(out, "  <g transform=\"translate(0 {CONTENT_TOP})\">").unwrap();
  out
}

fn circle_element(out: &mut String, x: f64, color: SvgColor, theme: SvgTheme) {
  writeln!(
    out,
    "  <circle cx=\"{x:.1}\" cy=\"24\" r=\"6\" data-svg-fill=\"{}\" fill=\"{}\"/>",
    color.name(),
    color.color(theme)
  )
  .unwrap();
}

fn chrome_text_element(out: &mut String, x: f64, y: f64, size: usize, theme: SvgTheme, anchor: &str, text: &str) {
  writeln!(
    out,
    "  <text x=\"{x:.1}\" y=\"{y:.1}\" data-svg-fill=\"{}\" fill=\"{}\" font-family=\"{MONO_FONT_FAMILY}\" font-size=\"{size}\" text-anchor=\"{anchor}\">{}</text>",
    SvgColor::Muted.name(),
    SvgColor::Muted.color(theme),
    escape_xml(text)
  )
  .unwrap();
}

fn content_width(chart_width: usize, command: &str) -> usize {
  let command_width = command.chars().count().saturating_mul(8) + 44;
  chart_width.max(command_width).clamp(MIN_WIDTH, MAX_WIDTH)
}

#[allow(clippy::too_many_arguments)]
fn text_element(
  out: &mut String,
  x: f64,
  y: f64,
  size: usize,
  color: SvgColor,
  theme: SvgTheme,
  anchor: &str,
  text: &str,
  extra: &str,
) {
  writeln!(
    out,
    "  <text x=\"{x:.1}\" y=\"{y:.1}\" data-svg-fill=\"{}\" fill=\"{}\" font-family=\"{FONT_FAMILY}\" font-size=\"{size}\" text-anchor=\"{anchor}\" {extra}>{}</text>",
    color.name(),
    color.color(theme),
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

fn level_color(level: u8) -> SvgColor {
  match level {
    0 => SvgColor::Heat0,
    1 => SvgColor::Heat1,
    2 => SvgColor::Heat2,
    3 => SvgColor::Heat3,
    _ => SvgColor::Heat4,
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
    let svg = render_activity_svg(
      &series,
      GraphChart::Auto,
      "llm-tokei graph --month --format svg",
      SvgTheme::Dark,
    );

    assert!(svg.starts_with("<svg "));
    assert!(svg.contains("data-chart=\"plot\""));
    assert!(svg.contains("data-resolution=\"day\""));
    assert!(svg.contains("class=\"activity-bar\""));
    assert_eq!(svg.matches("class=\"activity-hit-target\"").count(), 30);
    assert!(svg.contains("data-svg-fill=\"window-red\" fill=\"#ff5f56\""));
    assert!(svg.contains("data-svg-fill=\"heat-4\""));
    assert!(svg.contains("fill=\"#39d353\""));
    assert!(svg.contains("$ llm-tokei graph --month --format svg"));
    assert!(svg.ends_with("</svg>\n"));
  }

  #[test]
  fn auto_renders_long_ranges_as_accessible_calendar_cells() {
    let series = ActivitySeries::from_values(date(2026, 6, 1), vec![1.0; 31], Unit::Tokens);
    let svg = render_activity_svg(
      &series,
      GraphChart::Auto,
      "llm-tokei graph --format svg",
      SvgTheme::Dark,
    );

    assert!(svg.contains("data-chart=\"heatmap\""));
    assert_eq!(svg.matches("class=\"activity-cell\"").count(), 31);
    assert!(svg.contains("aria-labelledby=\"title desc\""));
    assert!(svg.contains("<title>Jun 1, 2026: 1</title>"));
    assert!(svg.contains("data-svg-fill=\"heat-4\""));
    assert!(svg.contains("fill=\"#39d353\""));
  }

  #[test]
  fn plot_includes_zero_grid_and_summary() {
    let series = ActivitySeries::from_values(date(2026, 7, 1), vec![0.0; 7], Unit::Cost);
    let svg = render_activity_svg(
      &series,
      GraphChart::Plot,
      "llm-tokei graph --7d --format svg",
      SvgTheme::Dark,
    );
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
    let svg = render_hourly_activity_svg(&series, "llm-tokei graph --3h --format svg", SvgTheme::Dark);

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
      SvgTheme::Dark,
    );
    let two_hours = render_hourly_activity_svg(
      &HourlyActivitySeries::from_values(start, vec![10.0, 20.0], Unit::Tokens),
      "llm-tokei graph --2h --format svg",
      SvgTheme::Dark,
    );

    assert_eq!(one_hour.matches("class=\"activity-axis-label\"").count(), 1);
    assert_eq!(two_hours.matches("class=\"activity-axis-label\"").count(), 2);
  }

  #[test]
  fn activity_svg_uses_requested_theme_as_the_css_default() {
    let series = ActivitySeries::from_values(date(2026, 7, 1), vec![1.0], Unit::Tokens);
    let svg = render_activity_svg(&series, GraphChart::Plot, "llm-tokei graph", SvgTheme::Light);

    assert!(svg.contains("data-svg-theme-default=\"light\""));
    assert!(svg.contains("data-svg-fill=\"background\" fill=\"#ffffff\""));
    assert!(svg.contains("data-svg-fill=\"heat-4\""));
    assert!(svg.contains("fill=\"#216e39\""));
    assert!(svg.contains("@media (prefers-color-scheme: dark)"));
  }
}
