use super::activity_common::{
  format_date_short, format_value, month_labels as calendar_month_labels, summary, title, CalendarGrid,
};
use crate::activity::{ActivityDay, ActivitySeries};
use crate::cli::GraphChart;
#[cfg(test)]
use crate::cli::Unit;
#[cfg(test)]
use chrono::NaiveDate;

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

  let grid = CalendarGrid::new(series).expect("non-empty activity series has a calendar grid");
  out.push_str("    ");
  out.push_str(&terminal_month_labels(series, &grid));
  out.push('\n');

  for row in 0..7 {
    out.push_str(weekday_label(row));
    out.push(' ');
    for week in 0..grid.week_count {
      let date = grid.date(week, row);
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

fn terminal_month_labels(series: &ActivitySeries, grid: &CalendarGrid) -> String {
  let mut canvas = vec![' '; grid.week_count];
  for (week, label) in calendar_month_labels(series, grid) {
    place_text(&mut canvas, week, &label);
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
}
