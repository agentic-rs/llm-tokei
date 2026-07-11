use crate::activity::{ActivityDay, ActivitySeries};
use crate::cli::{GraphChart, Unit};
use chrono::{Datelike, Duration, NaiveDate};

const PLOT_HEIGHT: usize = 6;

pub struct ActivityTerminalOpts {
  pub use_color: bool,
  pub width: Option<usize>,
}

pub fn render_activity_terminal(series: &ActivitySeries, chart: GraphChart, opts: &ActivityTerminalOpts) -> String {
  let chart = chart.resolve(series.len());
  match chart {
    GraphChart::Plot => render_plot(series, opts),
    GraphChart::Heatmap => render_heatmap(series, opts),
    GraphChart::Auto => unreachable!("auto chart is resolved before rendering"),
  }
}

fn render_plot(series: &ActivitySeries, opts: &ActivityTerminalOpts) -> String {
  let mut out = String::new();
  out.push_str(&title(series));
  out.push_str("\n\n");

  let max = series
    .days
    .iter()
    .map(|day| day.value)
    .filter(|value| value.is_finite())
    .fold(0.0, f64::max);
  let top_label = format_value(max, series.unit, false);
  let label_width = top_label.chars().count().max(1);
  let available = opts.width.unwrap_or(usize::MAX).saturating_sub(label_width + 2);
  let cell_width = usize::from(available >= series.len().saturating_mul(2)) + 1;
  let plot_width = series.len().saturating_mul(cell_width);

  for row in (1..=PLOT_HEIGHT).rev() {
    let tick = if row == PLOT_HEIGHT {
      top_label.as_str()
    } else if row == PLOT_HEIGHT / 2 {
      // A midpoint label provides scale without making the compact plot noisy.
      "mid"
    } else {
      ""
    };
    let tick = if tick == "mid" {
      format_value(max / 2.0, series.unit, false)
    } else {
      tick.to_string()
    };
    out.push_str(&format!("{tick:>label_width$} ┤"));
    for day in &series.days {
      let height = bar_height(day.value, max);
      if height >= row {
        out.push_str(&colorize(&"█".repeat(cell_width), day.level, opts.use_color));
      } else {
        out.push_str(&" ".repeat(cell_width));
      }
    }
    out.push('\n');
  }

  out.push_str(&format!("{:>label_width$} ┼{}\n", "0", "─".repeat(plot_width)));
  out.push_str(&" ".repeat(label_width + 2));
  out.push_str(&plot_date_labels(series, plot_width, cell_width));
  out.push('\n');
  out.push('\n');
  out.push_str(&summary(series));
  out.push('\n');
  out
}

fn render_heatmap(series: &ActivitySeries, opts: &ActivityTerminalOpts) -> String {
  let mut out = String::new();
  out.push_str(&title(series));
  out.push_str("\n\n");

  if series.is_empty() {
    out.push_str("(empty date range)\n");
    return out;
  }

  let grid_start = previous_sunday(series.start);
  let grid_end = next_saturday(series.end);
  let week_count = ((grid_end - grid_start).num_days() as usize / 7) + 1;
  out.push_str("    ");
  out.push_str(&month_labels(series, grid_start, week_count));
  out.push('\n');

  for row in 0..7 {
    out.push_str(weekday_label(row));
    out.push(' ');
    for week in 0..week_count {
      let date = grid_start + Duration::days((week * 7 + row) as i64);
      match series.day(date) {
        Some(day) => out.push_str(&heatmap_cell(day, opts.use_color)),
        None => out.push(' '),
      }
    }
    out.push('\n');
  }

  out.push('\n');
  out.push_str("Less ");
  for level in 0..=4 {
    let day = ActivityDay {
      date: series.start,
      value: f64::from(level),
      level,
      estimated: false,
    };
    out.push_str(&heatmap_cell(&day, opts.use_color));
  }
  out.push_str(" More\n\n");
  out.push_str(&summary(series));
  out.push('\n');
  out
}

fn bar_height(value: f64, max: f64) -> usize {
  if !value.is_finite() || value <= 0.0 || max <= 0.0 {
    return 0;
  }
  ((value / max * PLOT_HEIGHT as f64).ceil() as usize).clamp(1, PLOT_HEIGHT)
}

fn plot_date_labels(series: &ActivitySeries, plot_width: usize, cell_width: usize) -> String {
  if series.is_empty() || plot_width == 0 {
    return String::new();
  }
  let mut canvas = vec![' '; plot_width];
  let indices = [0, series.len() / 2, series.len() - 1];
  for index in indices {
    let label = format_date_short(series.days[index].date);
    let center = index * cell_width + cell_width / 2;
    place_centered(&mut canvas, center, &label);
  }
  canvas.into_iter().collect::<String>().trim_end().to_string()
}

fn month_labels(series: &ActivitySeries, grid_start: NaiveDate, week_count: usize) -> String {
  let mut canvas = vec![' '; week_count];
  let mut previous = None;
  for day in &series.days {
    let month = (day.date.year(), day.date.month());
    if previous == Some(month) {
      continue;
    }
    previous = Some(month);
    let week = ((day.date - grid_start).num_days() as usize) / 7;
    place_text(&mut canvas, week, &day.date.format("%b").to_string());
  }
  canvas.into_iter().collect::<String>().trim_end().to_string()
}

fn place_centered(canvas: &mut [char], center: usize, text: &str) {
  let width = text.chars().count();
  let start = center.saturating_sub(width / 2).min(canvas.len().saturating_sub(width));
  place_text(canvas, start, text);
}

fn place_text(canvas: &mut [char], start: usize, text: &str) {
  let chars = text.chars().collect::<Vec<_>>();
  if chars.is_empty() || start.saturating_add(chars.len()) > canvas.len() {
    return;
  }
  let before_clear = start == 0 || canvas[start - 1] == ' ';
  let target_clear = canvas[start..start + chars.len()].iter().all(|ch| *ch == ' ');
  let after = start + chars.len();
  let after_clear = after == canvas.len() || canvas[after] == ' ';
  if !before_clear || !target_clear || !after_clear {
    return;
  }
  canvas[start..start + chars.len()].copy_from_slice(&chars);
}

fn weekday_label(row: usize) -> &'static str {
  match row {
    1 => "Mon",
    3 => "Wed",
    5 => "Fri",
    _ => "   ",
  }
}

fn previous_sunday(date: NaiveDate) -> NaiveDate {
  date - Duration::days(date.weekday().num_days_from_sunday() as i64)
}

fn next_saturday(date: NaiveDate) -> NaiveDate {
  let remaining = 6 - date.weekday().num_days_from_sunday();
  date + Duration::days(remaining as i64)
}

fn heatmap_cell(day: &ActivityDay, use_color: bool) -> String {
  let glyph = match day.level {
    0 => "·",
    1 => "░",
    2 => "▒",
    3 => "▓",
    _ => "█",
  };
  colorize(glyph, day.level, use_color)
}

fn colorize(text: &str, level: u8, use_color: bool) -> String {
  if !use_color {
    return text.to_string();
  }
  let color = match level {
    0 => "72;79;88",
    1 => "14;68;41",
    2 => "0;109;50",
    3 => "38;166;65",
    _ => "57;211;83",
  };
  format!("\x1b[38;2;{color}m{text}\x1b[0m")
}

fn title(series: &ActivitySeries) -> String {
  format!(
    "{} activity · {}",
    unit_name(series.unit),
    format_date_range(series.start, series.end)
  )
}

fn summary(series: &ActivitySeries) -> String {
  let mut parts = vec![
    format!("Total {}", format_value(series.total, series.unit, series.estimated)),
    format!("Active {}/{} days", series.active_days, series.len()),
  ];
  if let Some(best) = series.best_day() {
    parts.push(format!(
      "Best {}: {}",
      format_date_short(best.date),
      format_value(best.value, series.unit, best.estimated)
    ));
  }
  parts.push(format!(
    "Longest streak {} {}",
    series.longest_streak,
    pluralize(series.longest_streak, "day", "days")
  ));
  parts.join(" · ")
}

fn unit_name(unit: Unit) -> &'static str {
  match unit {
    Unit::Tokens => "Token",
    Unit::Bytes => "Byte",
    Unit::Cost => "Cost",
  }
}

fn pluralize<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
  if count == 1 {
    singular
  } else {
    plural
  }
}

fn format_value(value: f64, unit: Unit, estimated: bool) -> String {
  let prefix = if estimated { "~" } else { "" };
  if unit == Unit::Cost {
    let value = if value >= 100.0 {
      format!("${value:.0}")
    } else if value >= 1.0 {
      format!("${value:.2}")
    } else if value > 0.0 {
      format!("${value:.4}")
    } else {
      "$0.00".to_string()
    };
    return format!("{prefix}{value}");
  }

  let (scaled, suffix) = if value >= 1_000_000_000_000.0 {
    (value / 1_000_000_000_000.0, "T")
  } else if value >= 1_000_000_000.0 {
    (value / 1_000_000_000.0, "B")
  } else if value >= 1_000_000.0 {
    (value / 1_000_000.0, "M")
  } else if value >= 1_000.0 {
    (value / 1_000.0, "K")
  } else {
    return format!("{prefix}{value:.0}");
  };
  format!("{prefix}{scaled:.1}{suffix}")
}

fn format_date_short(date: NaiveDate) -> String {
  date.format("%b %-d").to_string()
}

fn format_date_range(start: NaiveDate, end: NaiveDate) -> String {
  if start == end {
    return start.format("%b %-d, %Y").to_string();
  }
  if start.year() == end.year() && start.month() == end.month() {
    return format!("{}–{}", start.format("%b %-d"), end.format("%-d, %Y"));
  }
  if start.year() == end.year() {
    return format!("{}–{}", start.format("%b %-d"), end.format("%b %-d, %Y"));
  }
  format!("{}–{}", start.format("%b %-d, %Y"), end.format("%b %-d, %Y"))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
  }

  #[test]
  fn auto_uses_a_daily_plot_for_thirty_days() {
    let series = ActivitySeries::from_values(date(2026, 7, 1), (1..=30).map(f64::from).collect(), Unit::Tokens);
    let rendered = render_activity_terminal(
      &series,
      GraphChart::Auto,
      &ActivityTerminalOpts {
        use_color: false,
        width: Some(80),
      },
    );

    assert!(rendered.contains("Token activity · Jul 1–30, 2026"));
    assert!(rendered.contains("┼"));
    assert!(!rendered.contains("Less"));
    assert!(rendered.contains("Active 30/30 days"));
  }

  #[test]
  fn auto_uses_a_calendar_heatmap_for_longer_ranges() {
    let series = ActivitySeries::from_values(date(2026, 6, 1), vec![1.0; 31], Unit::Tokens);
    let rendered = render_activity_terminal(
      &series,
      GraphChart::Auto,
      &ActivityTerminalOpts {
        use_color: false,
        width: None,
      },
    );

    assert!(rendered.contains("Mon"));
    assert!(rendered.contains("Less ·░▒▓█ More"));
    assert!(!rendered.contains("\x1b["));
  }

  #[test]
  fn plot_respects_the_narrow_one_cell_layout() {
    let series = ActivitySeries::from_values(date(2026, 7, 1), vec![1.0; 10], Unit::Bytes);
    let rendered = render_activity_terminal(
      &series,
      GraphChart::Plot,
      &ActivityTerminalOpts {
        use_color: false,
        width: Some(15),
      },
    );
    let baseline = rendered.lines().find(|line| line.contains('┼')).unwrap();
    assert_eq!(baseline.matches('─').count(), 10);
  }

  #[test]
  fn color_mode_uses_truecolor_ansi() {
    let series = ActivitySeries::from_values(date(2026, 7, 1), vec![1.0; 31], Unit::Tokens);
    let rendered = render_activity_terminal(
      &series,
      GraphChart::Heatmap,
      &ActivityTerminalOpts {
        use_color: true,
        width: None,
      },
    );
    assert!(rendered.contains("\x1b[38;2;57;211;83m"));
  }

  #[test]
  fn formats_ranges_across_months_and_years() {
    assert_eq!(
      format_date_range(date(2026, 1, 1), date(2026, 7, 1)),
      "Jan 1–Jul 1, 2026"
    );
    assert_eq!(
      format_date_range(date(2025, 7, 1), date(2026, 7, 1)),
      "Jul 1, 2025–Jul 1, 2026"
    );
  }

  #[test]
  fn aligns_calendar_to_sunday_and_saturday() {
    assert_eq!(previous_sunday(date(2026, 7, 1)), date(2026, 6, 28));
    assert_eq!(next_saturday(date(2026, 7, 1)), date(2026, 7, 4));
    assert_eq!(date(2026, 7, 1).weekday(), chrono::Weekday::Wed);
  }
}
