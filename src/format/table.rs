use crate::aggregate::{Aggregate, GroupDim};

pub struct TableOpts {
  pub show_cost: bool,
  pub use_color: bool,
}

pub fn render_table(aggs: &[Aggregate], dims: &[GroupDim], opts: &TableOpts) -> String {
  let mut headers: Vec<Col> = dims.iter().map(|d| Col::text(d.label())).collect();
  headers.push(Col::num("input"));
  headers.push(Col::num("output"));
  headers.push(Col::num("reasoning"));
  headers.push(Col::num("cache_r"));
  headers.push(Col::num("cache_w"));
  headers.push(Col::num("total"));
  headers.push(Col::num("turns"));
  if opts.show_cost {
    headers.push(Col::num("cost($)"));
    headers.push(Col::num("cost_mult($)"));
  }

  let (mut tot_in, mut tot_out, mut tot_re, mut tot_cr, mut tot_cw, mut tot_tot, mut tot_t) =
    (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
  let (mut tot_base, mut tot_mult) = (0.0_f64, 0.0_f64);

  let mut rows: Vec<Vec<Col>> = Vec::new();
  for a in aggs {
    let mut row: Vec<Col> = a.keys.iter().map(|k| Col::text(k)).collect();
    row.push(Col::num(&fmt_int(a.input)));
    row.push(Col::num(&fmt_int(a.output)));
    row.push(Col::num(&fmt_int(a.reasoning)));
    row.push(Col::num(&fmt_int(a.cache_read)));
    row.push(Col::num(&fmt_int(a.cache_write)));
    row.push(Col::num(&fmt_int(a.total)));
    row.push(Col::num(&fmt_int(a.turns)));
    if opts.show_cost {
      row.push(Col::num(&fmt_cost(a.cost_base)));
      row.push(Col::num(&fmt_cost(a.cost_multiplied)));
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
  }

  let mut widths = vec![0usize; headers.len()];
  for (i, h) in headers.iter().enumerate() {
    widths[i] = widths[i].max(h.text.len());
  }
  for row in &rows {
    for (i, c) in row.iter().enumerate() {
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

  if aggs.len() > 1 {
    out.push_str(&sep);
    out.push('\n');

    let mut total_row: Vec<Col> = (0..dims.len())
      .map(|i| if i == 0 { Col::text("TOTAL") } else { Col::text("") })
      .collect();
    total_row.push(Col::num(&fmt_int(tot_in)));
    total_row.push(Col::num(&fmt_int(tot_out)));
    total_row.push(Col::num(&fmt_int(tot_re)));
    total_row.push(Col::num(&fmt_int(tot_cr)));
    total_row.push(Col::num(&fmt_int(tot_cw)));
    total_row.push(Col::num(&fmt_int(tot_tot)));
    total_row.push(Col::num(&fmt_int(tot_t)));
    if opts.show_cost {
      total_row.push(Col::num(&fmt_cost(tot_base)));
      total_row.push(Col::num(&fmt_cost(tot_mult)));
    }

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
    if i > 0 && (bytes.len() - i) % 3 == 0 {
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
