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
        a.input.saturating_sub(a.cache_read)
      }
    } else {
      if opts.bytes { a.input_bytes } else { a.input }
    };
    row.push(Col::num(&fmt_est_avg(
      shown_input,
      if opts.bytes {
        a.input_bytes_estimated
      } else {
        a.input_estimated
      },
      avg_den(a, opts.avg),
    )));
    row.push(Col::num(&fmt_est_avg(
      if opts.bytes { a.output_bytes } else { a.output },
      if opts.bytes {
        a.output_bytes_estimated
      } else {
        a.output_estimated
      },
      avg_den(a, opts.avg),
    )));
    row.push(Col::num(&fmt_avg(a.reasoning, avg_den(a, opts.avg))));
    row.push(Col::num(&fmt_avg(a.cache_read, avg_den(a, opts.avg))));
    row.push(Col::num(&fmt_avg(a.cache_write, avg_den(a, opts.avg))));
    row.push(Col::num(&fmt_avg(a.total, avg_den(a, opts.avg))));
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
      tot_in.saturating_sub(tot_cr)
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
  total_row.push(Col::num(&fmt_est_avg(
    shown_total_input,
    if opts.bytes {
      aggs.iter().any(|a| a.input_bytes_estimated)
    } else {
      aggs.iter().any(|a| a.input_estimated)
    },
    total_den,
  )));
  total_row.push(Col::num(&fmt_est_avg(
    tot_out_display,
    if opts.bytes {
      aggs.iter().any(|a| a.output_bytes_estimated)
    } else {
      aggs.iter().any(|a| a.output_estimated)
    },
    total_den,
  )));
  total_row.push(Col::num(&fmt_avg(tot_re, total_den)));
  total_row.push(Col::num(&fmt_avg(tot_cr, total_den)));
  total_row.push(Col::num(&fmt_avg(tot_cw, total_den)));
  total_row.push(Col::num(&fmt_avg(tot_tot, total_den)));
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

fn fmt_avg(n: u64, den: u64) -> String {
  if den <= 1 {
    return fmt_int(n);
  }
  fmt_float(n as f64 / den as f64)
}

fn fmt_est_avg(n: u64, estimated: bool, den: u64) -> String {
  let body = fmt_avg(n, den);
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
