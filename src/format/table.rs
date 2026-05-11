use crate::aggregate::{Aggregate, GroupDim};
use crate::cli::AvgBy;

pub struct TableOpts {
  pub show_cost: bool,
  pub use_color: bool,
  pub split_input: bool,
  pub avg: Option<AvgBy>,
  pub bytes: bool,
}

pub fn render_table(aggs: &[Aggregate], dims: &[GroupDim], opts: &TableOpts) -> String {
  let mut headers: Vec<Col> = dims.iter().map(|d| Col::text(d.label())).collect();
  let avg_suffix = match opts.avg {
    Some(AvgBy::Turn) => "/t",
    Some(AvgBy::Round) => "/r",
    Some(AvgBy::Session) => "/s",
    None => "",
  };
  headers.push(Col::num(&format!(
    "{}{}{}",
    if opts.split_input { "input_u" } else { "input" },
    if opts.bytes { "(B)" } else { "" },
    avg_suffix
  )));
  headers.push(Col::num(&format!(
    "output{}{}",
    if opts.bytes { "(B)" } else { "" },
    avg_suffix
  )));
  headers.push(Col::num(&format!("reasoning{avg_suffix}")));
  headers.push(Col::num(&format!("cache_r{avg_suffix}")));
  headers.push(Col::num(&format!("cache_w{avg_suffix}")));
  headers.push(Col::num(&format!("total{avg_suffix}")));
  headers.push(Col::num("turns"));
  headers.push(Col::num("rounds"));
  headers.push(Col::num("sessions"));
  if opts.show_cost {
    headers.push(Col::num("cost($)"));
    headers.push(Col::num("cost_mult($)"));
  }

  let (mut tot_in, mut tot_out, mut tot_re, mut tot_cr, mut tot_cw, mut tot_tot, mut tot_t, mut tot_r, mut tot_s) =
    (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
  let (mut tot_base, mut tot_mult) = (0.0_f64, 0.0_f64);

  let mut rows: Vec<Vec<Col>> = Vec::new();
  for a in aggs {
    let mut row: Vec<Col> = a.keys.iter().map(|k| Col::text(k)).collect();
    let shown_input = if opts.split_input {
      if opts.bytes {
        a.input_bytes
      } else {
        a.input.saturating_sub(a.cache_read).saturating_sub(a.cache_write)
      }
    } else {
      if opts.bytes {
        a.input_bytes
      } else {
        a.input
      }
    };
    row.push(Col::num(&fmt_est_usage_avg(
      shown_input,
      if opts.bytes {
        a.input_bytes_estimated
      } else {
        a.input_estimated
      },
      avg_den(a, opts.avg),
    )));
    row.push(Col::num(&fmt_est_usage_avg(
      if opts.bytes { a.output_bytes } else { a.output },
      if opts.bytes {
        a.output_bytes_estimated
      } else {
        a.output_estimated
      },
      avg_den(a, opts.avg),
    )));
    row.push(Col::num(&fmt_usage_avg(a.reasoning, avg_den(a, opts.avg))));
    row.push(Col::num(&fmt_usage_avg(a.cache_read, avg_den(a, opts.avg))));
    row.push(Col::num(&fmt_usage_avg(a.cache_write, avg_den(a, opts.avg))));
    row.push(Col::num(&fmt_usage_avg(a.total, avg_den(a, opts.avg))));
    row.push(Col::num(&fmt_int(a.turns)));
    row.push(Col::num(&fmt_int(a.rounds)));
    row.push(Col::num(&fmt_int(a.sessions)));
    if opts.show_cost {
      row.push(Col::num(&fmt_cost_avg(a.cost_base, avg_den(a, opts.avg))));
      row.push(Col::num(&fmt_cost_avg(a.cost_multiplied, avg_den(a, opts.avg))));
      tot_base += a.cost_base;
      tot_mult += a.cost_multiplied;
    }
    rows.push(row);
    tot_in += a.input;
    tot_out += a.output;
    tot_re += a.reasoning;
    tot_cr += a.cache_read;
    tot_cw += a.cache_write;
    tot_tot += a.total;
    tot_t += a.turns;
    tot_r += a.rounds;
    tot_s += a.sessions;
  }

  let mut total_row: Vec<Col> = (0..dims.len())
    .map(|i| if i == 0 { Col::text("TOTAL") } else { Col::text("") })
    .collect();
  let shown_total_input = if opts.split_input {
    if opts.bytes {
      aggs.iter().map(|a| a.input_bytes).sum()
    } else {
      tot_in.saturating_sub(tot_cr).saturating_sub(tot_cw)
    }
  } else {
    if opts.bytes {
      aggs.iter().map(|a| a.input_bytes).sum()
    } else {
      tot_in
    }
  };
  let tot_out_display = if opts.bytes {
    aggs.iter().map(|a| a.output_bytes).sum()
  } else {
    tot_out
  };
  let total_den = match opts.avg {
    Some(AvgBy::Turn) => tot_t,
    Some(AvgBy::Round) => tot_r,
    Some(AvgBy::Session) => tot_s,
    None => 1,
  };
  total_row.push(Col::num(&fmt_est_usage_avg(
    shown_total_input,
    if opts.bytes {
      aggs.iter().any(|a| a.input_bytes_estimated)
    } else {
      aggs.iter().any(|a| a.input_estimated)
    },
    total_den,
  )));
  total_row.push(Col::num(&fmt_est_usage_avg(
    tot_out_display,
    if opts.bytes {
      aggs.iter().any(|a| a.output_bytes_estimated)
    } else {
      aggs.iter().any(|a| a.output_estimated)
    },
    total_den,
  )));
  total_row.push(Col::num(&fmt_usage_avg(tot_re, total_den)));
  total_row.push(Col::num(&fmt_usage_avg(tot_cr, total_den)));
  total_row.push(Col::num(&fmt_usage_avg(tot_cw, total_den)));
  total_row.push(Col::num(&fmt_usage_avg(tot_tot, total_den)));
  total_row.push(Col::num(&fmt_int(tot_t)));
  total_row.push(Col::num(&fmt_int(tot_r)));
  total_row.push(Col::num(&fmt_int(tot_s)));
  if opts.show_cost {
    total_row.push(Col::num(&fmt_cost_avg(tot_base, total_den)));
    total_row.push(Col::num(&fmt_cost_avg(tot_mult, total_den)));
  }

  let show_total = aggs.len() > 1;

  let mut widths = vec![0usize; headers.len()];
  for (i, h) in headers.iter().enumerate() {
    widths[i] = widths[i].max(h.text.len());
  }
  for row in &rows {
    for (i, c) in row.iter().enumerate() {
      widths[i] = widths[i].max(c.text.len());
    }
  }
  if show_total {
    for (i, c) in total_row.iter().enumerate() {
      widths[i] = widths[i].max(c.text.len());
    }
  }

  let mut out = String::new();

  let header_line = format_row(&headers, &widths);
  let sep = separator(&widths);
  let style = if opts.use_color { Style::Header } else { Style::Plain };
  out.push_str(&colorize(&header_line, style));
  out.push('\n');
  out.push_str(&sep);
  out.push('\n');

  for row in &rows {
    out.push_str(&format_row(row, &widths));
    out.push('\n');
  }

  if show_total {
    out.push_str(&sep);
    out.push('\n');
    let total_style = if opts.use_color { Style::Total } else { Style::Plain };
    out.push_str(&colorize(&format_row(&total_row, &widths), total_style));
    out.push('\n');
  }

  out
}

struct Col {
  text: String,
  right: bool,
}

impl Col {
  fn text(s: &str) -> Self {
    Self {
      text: s.to_string(),
      right: false,
    }
  }
  fn num(s: &str) -> Self {
    Self {
      text: s.to_string(),
      right: true,
    }
  }
}

enum Style {
  Plain,
  Header,
  Total,
}

fn colorize(line: &str, style: Style) -> String {
  match style {
    Style::Plain => line.to_string(),
    Style::Header => format!("\x1b[36m{}\x1b[0m", line),
    Style::Total => format!("\x1b[33m{}\x1b[0m", line),
  }
}

fn format_row(cols: &[Col], widths: &[usize]) -> String {
  let mut parts: Vec<String> = Vec::with_capacity(cols.len());
  for (i, c) in cols.iter().enumerate() {
    let w = widths[i];
    if c.right {
      parts.push(format!("{:>width$}", c.text, width = w));
    } else {
      parts.push(format!("{:<width$}", c.text, width = w));
    }
  }
  parts.join("  ")
}

fn separator(widths: &[usize]) -> String {
  let total: usize = widths.iter().sum::<usize>() + 2 * (widths.len().saturating_sub(1));
  "\u{2500}".repeat(total)
}

fn fmt_int(n: u64) -> String {
  let s = n.to_string();
  let bytes = s.as_bytes();
  let mut out = String::with_capacity(s.len() + s.len() / 3);
  for (i, b) in bytes.iter().enumerate() {
    if i > 0 && (bytes.len() - i).is_multiple_of(3) {
      out.push(',');
    }
    out.push(*b as char);
  }
  out
}

fn fmt_cost(v: f64) -> String {
  if v == 0.0 {
    "-".to_string()
  } else {
    format!("{:.4}", v)
  }
}

fn avg_den(a: &Aggregate, avg: Option<AvgBy>) -> u64 {
  match avg {
    Some(AvgBy::Turn) => a.turns,
    Some(AvgBy::Round) => a.rounds,
    Some(AvgBy::Session) => a.sessions,
    None => 1,
  }
}

fn fmt_usage_avg(n: u64, den: u64) -> String {
  if den <= 1 {
    return fmt_usage(n as f64);
  }
  fmt_usage(n as f64 / den as f64)
}

fn fmt_est_usage_avg(n: u64, estimated: bool, den: u64) -> String {
  let body = fmt_usage_avg(n, den);
  if estimated {
    format!("~{body}")
  } else {
    body
  }
}

fn fmt_cost_avg(v: f64, den: u64) -> String {
  if den <= 1 {
    return fmt_cost(v);
  }
  fmt_cost(v / den as f64)
}

fn fmt_float(v: f64) -> String {
  if (v.fract()).abs() < 1e-9 {
    fmt_int(v as u64)
  } else {
    format!("{v:.2}")
  }
}

fn fmt_usage(v: f64) -> String {
  const UNITS: [&str; 5] = ["", "K", "M", "B", "T"];
  if v < 999.95 {
    return fmt_float(v);
  }

  let mut scaled = v;
  let mut unit_idx = 0usize;
  while scaled >= 999.95 && unit_idx < UNITS.len() - 1 {
    scaled /= 1000.0;
    unit_idx += 1;
  }

  let body = format!("{scaled:.1}");
  let body = body.strip_suffix(".0").unwrap_or(&body);
  format!("{body}{}", UNITS[unit_idx])
}

#[cfg(test)]
mod tests {
  use super::*;

  fn aggregate(keys: &[&str]) -> Aggregate {
    Aggregate {
      keys: keys.iter().map(|s| s.to_string()).collect(),
      input: 1_234,
      output: 2_500_000,
      input_bytes: 1_500,
      output_bytes: 2_500_000_000,
      input_estimated: true,
      output_estimated: false,
      input_bytes_estimated: true,
      output_bytes_estimated: false,
      reasoning: 999,
      cache_read: 1_500,
      cache_write: 1_000_000,
      total: 3_501_233,
      turns: 1_234,
      rounds: 2_345,
      sessions: 3_456,
      cost_embedded: 0.0,
      cost_base: 12.3456,
      cost_multiplied: 23.4567,
      first_ts: None,
      last_ts: None,
    }
  }

  #[test]
  fn table_compacts_usage_fields_but_not_counts_or_costs() {
    let table = render_table(
      &[aggregate(&["codex"])],
      &[GroupDim::Source],
      &TableOpts {
        show_cost: true,
        use_color: false,
        split_input: false,
        avg: None,
        bytes: false,
      },
    );

    assert!(table.contains("~1.2K"), "table output: {table}");
    assert!(table.contains("2.5M"), "table output: {table}");
    assert!(table.contains("999"), "table output: {table}");
    assert!(table.contains("1.5K"), "table output: {table}");
    assert!(table.contains("1M"), "table output: {table}");
    assert!(table.contains("3.5M"), "table output: {table}");
    assert!(table.contains("1,234"), "table output: {table}");
    assert!(table.contains("2,345"), "table output: {table}");
    assert!(table.contains("3,456"), "table output: {table}");
    assert!(table.contains("12.3456"), "table output: {table}");
    assert!(table.contains("23.4567"), "table output: {table}");
  }

  #[test]
  fn table_compacts_usage_after_averaging() {
    let mut agg = aggregate(&["codex"]);
    agg.input = 12_345;
    agg.output = 9_999;
    agg.reasoning = 999;
    agg.cache_read = 12_000;
    agg.cache_write = 1_234_567;
    agg.total = 2_500_000;
    agg.turns = 10;

    let table = render_table(
      &[agg],
      &[GroupDim::Source],
      &TableOpts {
        show_cost: false,
        use_color: false,
        split_input: false,
        avg: Some(AvgBy::Turn),
        bytes: false,
      },
    );

    assert!(table.contains("~1.2K"), "table output: {table}");
    assert!(table.contains("999.90"), "table output: {table}");
    assert!(table.contains("99.90"), "table output: {table}");
    assert!(table.contains("1.2K"), "table output: {table}");
    assert!(table.contains("123.5K"), "table output: {table}");
    assert!(table.contains("250K"), "table output: {table}");
  }

  #[test]
  fn bytes_headers_stay_plain_while_byte_values_compact() {
    let table = render_table(
      &[aggregate(&["codex"])],
      &[GroupDim::Source],
      &TableOpts {
        show_cost: false,
        use_color: false,
        split_input: false,
        avg: None,
        bytes: true,
      },
    );

    assert!(table.contains("input(B)"), "table output: {table}");
    assert!(table.contains("output(B)"), "table output: {table}");
    assert!(table.contains("~1.5K"), "table output: {table}");
    assert!(table.contains("2.5B"), "table output: {table}");
  }
}
