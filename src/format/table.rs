use crate::aggregate::{Aggregate, GroupDim};
use crate::cli::AvgBy;

pub struct TableOpts {
  pub show_cost: bool,
  pub use_color: bool,
  pub split_input: bool,
  pub avg: Option<AvgBy>,
  pub bytes: bool,
  pub fit_width: Option<usize>,
}

pub fn render_table(aggs: &[Aggregate], dims: &[GroupDim], opts: &TableOpts) -> String {
  let model = build_table_model(aggs, dims, opts);
  let widths = measure_widths(&model);
  let fitted = fit_table(model, widths, opts.fit_width);
  render_fitted_table(&fitted, opts.use_color)
}

// Table model construction.

struct TableModel {
  headers: Vec<Col>,
  rows: Vec<Vec<Col>>,
  total_row: Vec<Col>,
  columns: Vec<ColumnLayout>,
  show_total: bool,
}

struct FittedTable {
  headers: Vec<Col>,
  rows: Vec<Vec<Col>>,
  total_row: Vec<Col>,
  widths: Vec<usize>,
  hidden: Vec<String>,
  show_total: bool,
}

fn build_table_model(aggs: &[Aggregate], dims: &[GroupDim], opts: &TableOpts) -> TableModel {
  let mut headers: Vec<Col> = dims.iter().map(|d| Col::text(d.label())).collect();
  let mut columns: Vec<ColumnLayout> = dims
    .iter()
    .map(|d| ColumnLayout::required_dim(d.label().to_string()))
    .collect();
  let avg_suffix = match opts.avg {
    Some(AvgBy::Turn) => "/t",
    Some(AvgBy::Round) => "/r",
    Some(AvgBy::Session) => "/s",
    None => "",
  };

  for spec in active_stat_specs(opts) {
    headers.push(Col::num(&spec.header(opts, avg_suffix)));
    columns.push(ColumnLayout::from_stat_spec(spec));
  }

  let mut totals = TableTotals::default();

  let mut rows: Vec<Vec<Col>> = Vec::new();
  for a in aggs {
    let mut row: Vec<Col> = a.keys.iter().map(|k| Col::text(k)).collect();
    for spec in active_stat_specs(opts) {
      row.push(spec.row_col(a, opts));
    }
    rows.push(row);
    totals.add(a);
  }

  let mut total_row: Vec<Col> = (0..dims.len())
    .map(|i| if i == 0 { Col::text("TOTAL") } else { Col::text("") })
    .collect();
  for spec in active_stat_specs(opts) {
    total_row.push(spec.total_col(&totals, aggs, opts));
  }

  TableModel {
    headers,
    rows,
    total_row,
    columns,
    show_total: aggs.len() > 1,
  }
}

fn measure_widths(model: &TableModel) -> Vec<usize> {
  let mut widths = vec![0usize; model.headers.len()];
  include_widths(&mut widths, &model.headers);
  for row in &model.rows {
    include_widths(&mut widths, row);
  }
  if model.show_total {
    include_widths(&mut widths, &model.total_row);
  }
  widths
}

fn include_widths(widths: &mut [usize], row: &[Col]) {
  for (i, c) in row.iter().enumerate() {
    widths[i] = widths[i].max(c.text.len());
  }
}

fn fit_table(model: TableModel, widths: Vec<usize>, target: Option<usize>) -> FittedTable {
  let fit = fit_columns(&model.columns, &widths, target);
  project_table(model, fit)
}

fn project_table(model: TableModel, fit: FitResult) -> FittedTable {
  FittedTable {
    headers: project_row(&model.headers, &fit.visible),
    rows: model
      .rows
      .iter()
      .map(|row| project_row(row, &fit.visible))
      .collect::<Vec<_>>(),
    total_row: project_row(&model.total_row, &fit.visible),
    widths: project_widths(&fit.widths, &fit.visible),
    hidden: fit.hidden,
    show_total: model.show_total,
  }
}

fn render_fitted_table(table: &FittedTable, use_color: bool) -> String {
  let mut out = String::new();

  let header_line = format_row(&table.headers, &table.widths);
  let sep = separator(&table.widths);
  let style = if use_color { Style::Header } else { Style::Plain };
  out.push_str(&colorize(&header_line, style));
  out.push('\n');
  out.push_str(&sep);
  out.push('\n');

  for row in &table.rows {
    out.push_str(&format_row(row, &table.widths));
    out.push('\n');
  }

  if table.show_total {
    out.push_str(&sep);
    out.push('\n');
    let total_style = if use_color { Style::Total } else { Style::Plain };
    out.push_str(&colorize(&format_row(&table.total_row, &table.widths), total_style));
    out.push('\n');
  }

  if !table.hidden.is_empty() {
    out.push_str("hidden columns: ");
    out.push_str(&table.hidden.join(", "));
    out.push('\n');
  }

  out
}

// Statistic column definitions and value extraction.

#[derive(Clone, Copy)]
enum StatColumnId {
  Input,
  Output,
  Reasoning,
  CacheRead,
  CacheWrite,
  Total,
  Turns,
  Rounds,
  Sessions,
  CostBase,
  CostMultiplied,
}

struct StatColumnSpec {
  id: StatColumnId,
  label: &'static str,
  priority: u8,
  required: bool,
  cost: bool,
}

const STAT_COLUMNS: &[StatColumnSpec] = &[
  stat(StatColumnId::Input, "input", 90, false, false),
  stat(StatColumnId::Output, "output", 90, false, false),
  stat(StatColumnId::Reasoning, "reasoning", 60, false, false),
  stat(StatColumnId::CacheRead, "cache_r", 60, false, false),
  stat(StatColumnId::CacheWrite, "cache_w", 60, false, false),
  stat(StatColumnId::Total, "total", u8::MAX, true, false),
  stat(StatColumnId::Turns, "turns", 80, false, false),
  stat(StatColumnId::Rounds, "rounds", 50, false, false),
  stat(StatColumnId::Sessions, "sessions", 50, false, false),
  stat(StatColumnId::CostBase, "cost($)", 70, false, true),
  stat(StatColumnId::CostMultiplied, "cost_mult($)", 70, false, true),
];

const fn stat(id: StatColumnId, label: &'static str, priority: u8, required: bool, cost: bool) -> StatColumnSpec {
  StatColumnSpec {
    id,
    label,
    priority,
    required,
    cost,
  }
}

impl StatColumnSpec {
  fn header(&self, opts: &TableOpts, avg_suffix: &str) -> String {
    match self.id {
      StatColumnId::Input => format!(
        "{}{}{}",
        if opts.split_input { "input_u" } else { "input" },
        if opts.bytes { "(B)" } else { "" },
        avg_suffix
      ),
      StatColumnId::Output => format!("output{}{}", if opts.bytes { "(B)" } else { "" }, avg_suffix),
      StatColumnId::Reasoning | StatColumnId::CacheRead | StatColumnId::CacheWrite | StatColumnId::Total => {
        format!("{}{avg_suffix}", self.label)
      }
      StatColumnId::Turns
      | StatColumnId::Rounds
      | StatColumnId::Sessions
      | StatColumnId::CostBase
      | StatColumnId::CostMultiplied => self.label.to_string(),
    }
  }

  fn row_col(&self, a: &Aggregate, opts: &TableOpts) -> Col {
    let den = avg_den(a, opts.avg);
    let text = match self.id {
      StatColumnId::Input => fmt_est_usage_avg(shown_input(a, opts), input_estimated(a, opts), den),
      StatColumnId::Output => fmt_est_usage_avg(
        if opts.bytes { a.output_bytes } else { a.output },
        if opts.bytes {
          a.output_bytes_estimated
        } else {
          a.output_estimated
        },
        den,
      ),
      StatColumnId::Reasoning => fmt_usage_avg(a.reasoning, den),
      StatColumnId::CacheRead => fmt_usage_avg(a.cache_read, den),
      StatColumnId::CacheWrite => fmt_usage_avg(a.cache_write, den),
      StatColumnId::Total => fmt_usage_avg(a.total, den),
      StatColumnId::Turns => fmt_int(a.turns),
      StatColumnId::Rounds => fmt_int(a.rounds),
      StatColumnId::Sessions => fmt_int(a.sessions),
      StatColumnId::CostBase => fmt_cost_avg(a.cost_base, den),
      StatColumnId::CostMultiplied => fmt_cost_avg(a.cost_multiplied, den),
    };
    Col::num(&text)
  }

  fn total_col(&self, totals: &TableTotals, aggs: &[Aggregate], opts: &TableOpts) -> Col {
    let den = totals.avg_den(opts.avg);
    let text = match self.id {
      StatColumnId::Input => fmt_est_usage_avg(totals.shown_input(opts), any_input_estimated(aggs, opts), den),
      StatColumnId::Output => fmt_est_usage_avg(totals.shown_output(opts), any_output_estimated(aggs, opts), den),
      StatColumnId::Reasoning => fmt_usage_avg(totals.reasoning, den),
      StatColumnId::CacheRead => fmt_usage_avg(totals.cache_read, den),
      StatColumnId::CacheWrite => fmt_usage_avg(totals.cache_write, den),
      StatColumnId::Total => fmt_usage_avg(totals.total, den),
      StatColumnId::Turns => fmt_int(totals.turns),
      StatColumnId::Rounds => fmt_int(totals.rounds),
      StatColumnId::Sessions => fmt_int(totals.sessions),
      StatColumnId::CostBase => fmt_cost_avg(totals.cost_base, den),
      StatColumnId::CostMultiplied => fmt_cost_avg(totals.cost_multiplied, den),
    };
    Col::num(&text)
  }
}

fn active_stat_specs(opts: &TableOpts) -> impl Iterator<Item = &'static StatColumnSpec> + '_ {
  STAT_COLUMNS.iter().filter(|spec| !spec.cost || opts.show_cost)
}

#[derive(Default)]
struct TableTotals {
  input: u64,
  output: u64,
  input_bytes: u64,
  output_bytes: u64,
  reasoning: u64,
  cache_read: u64,
  cache_write: u64,
  total: u64,
  turns: u64,
  rounds: u64,
  sessions: u64,
  cost_base: f64,
  cost_multiplied: f64,
}

impl TableTotals {
  fn add(&mut self, a: &Aggregate) {
    self.input += a.input;
    self.output += a.output;
    self.input_bytes += a.input_bytes;
    self.output_bytes += a.output_bytes;
    self.reasoning += a.reasoning;
    self.cache_read += a.cache_read;
    self.cache_write += a.cache_write;
    self.total += a.total;
    self.turns += a.turns;
    self.rounds += a.rounds;
    self.sessions += a.sessions;
    self.cost_base += a.cost_base;
    self.cost_multiplied += a.cost_multiplied;
  }

  fn shown_input(&self, opts: &TableOpts) -> u64 {
    if opts.bytes {
      self.input_bytes
    } else if opts.split_input {
      self
        .input
        .saturating_sub(self.cache_read)
        .saturating_sub(self.cache_write)
    } else {
      self.input
    }
  }

  fn shown_output(&self, opts: &TableOpts) -> u64 {
    if opts.bytes {
      self.output_bytes
    } else {
      self.output
    }
  }

  fn avg_den(&self, avg: Option<AvgBy>) -> u64 {
    match avg {
      Some(AvgBy::Turn) => self.turns,
      Some(AvgBy::Round) => self.rounds,
      Some(AvgBy::Session) => self.sessions,
      None => 1,
    }
  }
}

fn shown_input(a: &Aggregate, opts: &TableOpts) -> u64 {
  if opts.bytes {
    a.input_bytes
  } else if opts.split_input {
    a.input.saturating_sub(a.cache_read).saturating_sub(a.cache_write)
  } else {
    a.input
  }
}

fn input_estimated(a: &Aggregate, opts: &TableOpts) -> bool {
  if opts.bytes {
    a.input_bytes_estimated
  } else {
    a.input_estimated
  }
}

fn any_input_estimated(aggs: &[Aggregate], opts: &TableOpts) -> bool {
  if opts.bytes {
    aggs.iter().any(|a| a.input_bytes_estimated)
  } else {
    aggs.iter().any(|a| a.input_estimated)
  }
}

fn any_output_estimated(aggs: &[Aggregate], opts: &TableOpts) -> bool {
  if opts.bytes {
    aggs.iter().any(|a| a.output_bytes_estimated)
  } else {
    aggs.iter().any(|a| a.output_estimated)
  }
}

// Column layout and fitting.

struct ColumnLayout {
  label: String,
  priority: u8,
  required: bool,
  dim: bool,
}

impl ColumnLayout {
  fn required_dim(label: String) -> Self {
    Self {
      label,
      priority: u8::MAX,
      required: true,
      dim: true,
    }
  }

  fn from_stat_spec(spec: &StatColumnSpec) -> Self {
    Self {
      label: spec.label.to_string(),
      priority: spec.priority,
      required: spec.required,
      dim: false,
    }
  }
}

struct FitResult {
  visible: Vec<bool>,
  widths: Vec<usize>,
  hidden: Vec<String>,
}

fn fit_columns(columns: &[ColumnLayout], widths: &[usize], target: Option<usize>) -> FitResult {
  let Some(target) = target else {
    return FitResult {
      visible: vec![true; columns.len()],
      widths: widths.to_vec(),
      hidden: Vec::new(),
    };
  };

  let mut visible = vec![true; columns.len()];
  let mut adjusted = widths.to_vec();
  let mut hidden = Vec::new();
  let mut optional: Vec<usize> = columns
    .iter()
    .enumerate()
    .filter_map(|(idx, column)| (!column.required).then_some(idx))
    .collect();
  optional.sort_by_key(|idx| (columns[*idx].priority, *idx));

  for idx in optional {
    if table_width(&adjusted, &visible) <= target {
      break;
    }
    visible[idx] = false;
    hidden.push(columns[idx].label.clone());
  }

  fit_dimension_widths(columns, &mut adjusted, &visible, target);

  FitResult {
    visible,
    widths: adjusted,
    hidden,
  }
}

const MIN_DIM_COLUMN_WIDTH: usize = 1;

fn fit_dimension_widths(columns: &[ColumnLayout], widths: &mut [usize], visible: &[bool], target: usize) {
  while table_width(widths, visible) > target {
    let Some(idx) = columns
      .iter()
      .enumerate()
      .filter(|(idx, column)| visible[*idx] && column.dim && widths[*idx] > min_dim_width(column))
      .max_by_key(|(idx, _)| widths[*idx])
      .map(|(idx, _)| idx)
    else {
      break;
    };
    widths[idx] -= 1;
  }
}

fn min_dim_width(column: &ColumnLayout) -> usize {
  column.label.len().max(MIN_DIM_COLUMN_WIDTH)
}

fn table_width(widths: &[usize], visible: &[bool]) -> usize {
  let mut count = 0usize;
  let sum = widths
    .iter()
    .zip(visible)
    .filter_map(|(width, visible)| {
      count += usize::from(*visible);
      visible.then_some(*width)
    })
    .sum::<usize>();
  sum + 2 * count.saturating_sub(1)
}

fn project_row(cols: &[Col], visible: &[bool]) -> Vec<Col> {
  cols
    .iter()
    .zip(visible)
    .filter_map(|(col, visible)| visible.then(|| col.clone()))
    .collect()
}

fn project_widths(widths: &[usize], visible: &[bool]) -> Vec<usize> {
  widths
    .iter()
    .zip(visible)
    .filter_map(|(width, visible)| visible.then_some(*width))
    .collect()
}

// Row rendering.

#[derive(Clone)]
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
    let text = fit_cell(&c.text, w);
    if c.right {
      parts.push(format!("{text:>width$}", width = w));
    } else {
      parts.push(format!("{text:<width$}", width = w));
    }
  }
  parts.join("  ")
}

fn fit_cell(text: &str, width: usize) -> String {
  if text.chars().count() <= width {
    return text.to_string();
  }
  if width == 0 {
    return String::new();
  }
  if width == 1 {
    return "…".to_string();
  }
  let mut out = text.chars().take(width - 1).collect::<String>();
  out.push('…');
  out
}

fn separator(widths: &[usize]) -> String {
  let total: usize = widths.iter().sum::<usize>() + 2 * (widths.len().saturating_sub(1));
  "\u{2500}".repeat(total)
}

// Number formatting.

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

  fn opts(show_cost: bool, avg: Option<AvgBy>, bytes: bool, fit_width: Option<usize>) -> TableOpts {
    TableOpts {
      show_cost,
      use_color: false,
      split_input: false,
      avg,
      bytes,
      fit_width,
    }
  }

  fn render_test_table(aggs: &[Aggregate], dims: &[GroupDim], fit_width: Option<usize>) -> String {
    render_table(aggs, dims, &opts(true, None, false, fit_width))
  }

  #[test]
  fn table_compacts_usage_fields_but_not_counts_or_costs() {
    let table = render_table(
      &[aggregate(&["codex"])],
      &[GroupDim::Source],
      &opts(true, None, false, None),
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
      &opts(false, Some(AvgBy::Turn), false, None),
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
      &opts(false, None, true, None),
    );

    assert!(table.contains("input(B)"), "table output: {table}");
    assert!(table.contains("output(B)"), "table output: {table}");
    assert!(table.contains("~1.5K"), "table output: {table}");
    assert!(table.contains("2.5B"), "table output: {table}");
  }

  #[test]
  fn wide_table_width_keeps_all_columns_and_no_hidden_footer() {
    let table = render_test_table(&[aggregate(&["codex"])], &[GroupDim::Source], Some(160));
    let header = table.lines().next().unwrap_or_default();

    for expected in [
      "input",
      "output",
      "reasoning",
      "cache_r",
      "cache_w",
      "total",
      "turns",
      "rounds",
      "sessions",
      "cost($)",
      "cost_mult($)",
    ] {
      assert!(header.contains(expected), "header: {header}");
    }
    assert!(!table.contains("hidden columns:"), "table output: {table}");
  }

  #[test]
  fn narrow_table_width_hides_columns_by_priority_and_keeps_total() {
    let table = render_test_table(&[aggregate(&["codex"])], &[GroupDim::Source], Some(60));
    let header = table.lines().next().unwrap_or_default();

    assert!(header.contains("source"), "header: {header}");
    assert!(header.contains("total"), "header: {header}");
    assert!(!header.contains("rounds"), "header: {header}");
    assert!(!header.contains("sessions"), "header: {header}");
    assert!(!header.contains("reasoning"), "header: {header}");
    assert!(!header.contains("cache_r"), "header: {header}");
    assert!(!header.contains("cache_w"), "header: {header}");
    assert!(
      table.contains("hidden columns: rounds, sessions, reasoning, cache_r, cache_w"),
      "table output: {table}"
    );
  }

  #[test]
  fn very_narrow_width_truncates_dimension_values_and_keeps_total() {
    let table = render_test_table(
      &[aggregate(&["very-long-source-name", "extremely-long-model-name"])],
      &[GroupDim::Source, GroupDim::Model],
      Some(32),
    );
    let header = table.lines().next().unwrap_or_default();

    assert!(header.contains("source"), "header: {header}");
    assert!(header.contains("model"), "header: {header}");
    assert!(header.contains("total"), "header: {header}");
    assert!(table.contains('…'), "table output: {table}");
    assert!(table.contains("hidden columns:"), "table output: {table}");
  }
}
