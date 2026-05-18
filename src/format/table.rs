use crate::aggregate::{Aggregate, GroupDim};
use crate::cli::{AvgBy, Unit};
use std::collections::BTreeMap;

pub struct TableOpts {
  pub show_cost: bool,
  pub use_color: bool,
  pub split_input: bool,
  pub avg: Option<AvgBy>,
  pub unit: Unit,
  pub human: bool,
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
    Some(AvgBy::Call) => "/c",
    Some(AvgBy::Round) => "/r",
    Some(AvgBy::Session) => "/s",
    None => "",
  };

  for spec in active_stat_specs(opts) {
    headers.push(Col::num(&spec.header(opts, avg_suffix)));
    columns.push(ColumnLayout::from_stat_spec(spec));
  }

  let cost_per_columns = if opts.show_cost {
    top_cost_per_columns(aggs)
  } else {
    Vec::new()
  };
  for key in &cost_per_columns {
    headers.push(Col::num(&short_cost_per_header(key)));
    columns.push(ColumnLayout::optional_stat(short_cost_per_header(key), 65));
  }

  let mut totals = TableTotals::default();

  let max_units = max_human_units(aggs, opts);

  let mut rows: Vec<Vec<Col>> = Vec::new();
  for a in aggs {
    let mut row: Vec<Col> = a.keys.iter().map(|k| Col::text(k)).collect();
    for spec in active_stat_specs(opts) {
      row.push(spec.row_col(a, opts, &max_units));
    }
    for key in &cost_per_columns {
      row.push(Col::num(&fmt_cost(*a.cost_per.get(key).unwrap_or(&0.0))));
    }
    rows.push(row);
    totals.add(a);
  }

  let mut total_row: Vec<Col> = (0..dims.len())
    .map(|i| if i == 0 { Col::text("TOTAL") } else { Col::text("") })
    .collect();
  for spec in active_stat_specs(opts) {
    total_row.push(spec.total_col(&totals, aggs, opts, &max_units));
  }
  for key in &cost_per_columns {
    let total = aggs.iter().map(|a| a.cost_per.get(key).copied().unwrap_or(0.0)).sum();
    total_row.push(Col::num(&fmt_cost(total)));
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

  let header_line = format_row(&table.headers, &table.widths, use_color);
  let sep = separator(&table.widths);
  let style = if use_color { Style::Header } else { Style::Plain };
  out.push_str(&colorize(&header_line, style));
  out.push('\n');
  out.push_str(&sep);
  out.push('\n');

  for row in &table.rows {
    out.push_str(&format_row(row, &table.widths, use_color));
    out.push('\n');
  }

  if table.show_total {
    out.push_str(&sep);
    out.push('\n');
    let total_style = if use_color { Style::Total } else { Style::Plain };
    out.push_str(&colorize(
      &format_row(&table.total_row, &table.widths, use_color),
      total_style,
    ));
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
  Calls,
  Rounds,
  Sessions,
  CostBase,
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
  stat(StatColumnId::Calls, "calls", 80, false, false),
  stat(StatColumnId::Rounds, "rounds", 50, false, false),
  stat(StatColumnId::Sessions, "sessions", 50, false, false),
  stat(StatColumnId::CostBase, "cost($)", 70, false, true),
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
        usage_unit_suffix(opts, self.id),
        avg_suffix
      ),
      StatColumnId::Output => format!("output{}{}", usage_unit_suffix(opts, self.id), avg_suffix),
      StatColumnId::Reasoning | StatColumnId::CacheRead | StatColumnId::CacheWrite | StatColumnId::Total => {
        format!("{}{}{avg_suffix}", self.label, usage_unit_suffix(opts, self.id))
      }
      StatColumnId::Calls | StatColumnId::Rounds | StatColumnId::Sessions | StatColumnId::CostBase => {
        self.label.to_string()
      }
    }
  }

  fn row_col(&self, a: &Aggregate, opts: &TableOpts, max_units: &[usize]) -> Col {
    let den = avg_den(a, opts.avg);
    let text = match self.id {
      StatColumnId::Input => fmt_usage_value_avg(shown_input(a, opts), input_estimated(a, opts), den, opts),
      StatColumnId::Output => fmt_usage_value_avg(shown_output(a, opts), output_estimated(a, opts), den, opts),
      StatColumnId::Reasoning => fmt_usage_value_avg(shown_reasoning(a, opts), false, den, opts),
      StatColumnId::CacheRead => fmt_usage_value_avg(shown_cache_read(a, opts), false, den, opts),
      StatColumnId::CacheWrite => fmt_usage_value_avg(shown_cache_write(a, opts), false, den, opts),
      StatColumnId::Total => fmt_usage_value_avg(shown_total(a, opts), false, den, opts),
      StatColumnId::Calls => fmt_int(a.calls),
      StatColumnId::Rounds => fmt_int(a.rounds),
      StatColumnId::Sessions => fmt_int(a.sessions),
      StatColumnId::CostBase => fmt_cost_avg(a.cost, den),
    };
    let muted = opts.human
      && self
        .human_unit(a, opts, den)
        .is_some_and(|unit| unit < max_units[self.id.idx()]);
    Col::num(&text).with_muted(muted)
  }

  fn total_col(&self, totals: &TableTotals, aggs: &[Aggregate], opts: &TableOpts, max_units: &[usize]) -> Col {
    let den = totals.avg_den(opts.avg);
    let text = match self.id {
      StatColumnId::Input => fmt_usage_value_avg(totals.shown_input(opts), any_input_estimated(aggs, opts), den, opts),
      StatColumnId::Output => {
        fmt_usage_value_avg(totals.shown_output(opts), any_output_estimated(aggs, opts), den, opts)
      }
      StatColumnId::Reasoning => fmt_usage_value_avg(totals.shown_reasoning(opts), false, den, opts),
      StatColumnId::CacheRead => fmt_usage_value_avg(totals.shown_cache_read(opts), false, den, opts),
      StatColumnId::CacheWrite => fmt_usage_value_avg(totals.shown_cache_write(opts), false, den, opts),
      StatColumnId::Total => fmt_usage_value_avg(totals.shown_total(opts), false, den, opts),
      StatColumnId::Calls => fmt_int(totals.calls),
      StatColumnId::Rounds => fmt_int(totals.rounds),
      StatColumnId::Sessions => fmt_int(totals.sessions),
      StatColumnId::CostBase => fmt_cost_avg(totals.cost, den),
    };
    let muted = opts.human
      && self
        .total_human_unit(totals, opts, den)
        .is_some_and(|unit| unit < max_units[self.id.idx()]);
    Col::num(&text).with_muted(muted)
  }

  fn human_unit(&self, a: &Aggregate, opts: &TableOpts, den: u64) -> Option<usize> {
    if opts.unit == Unit::Cost {
      return None;
    }
    let value = match self.id {
      StatColumnId::Input => shown_input(a, opts),
      StatColumnId::Output => shown_output(a, opts),
      StatColumnId::Reasoning => shown_reasoning(a, opts),
      StatColumnId::CacheRead => shown_cache_read(a, opts),
      StatColumnId::CacheWrite => shown_cache_write(a, opts),
      StatColumnId::Total => shown_total(a, opts),
      _ => return None,
    };
    Some(human_unit(value / den.max(1) as f64))
  }

  fn total_human_unit(&self, totals: &TableTotals, opts: &TableOpts, den: u64) -> Option<usize> {
    if opts.unit == Unit::Cost {
      return None;
    }
    let value = match self.id {
      StatColumnId::Input => totals.shown_input(opts),
      StatColumnId::Output => totals.shown_output(opts),
      StatColumnId::Reasoning => totals.shown_reasoning(opts),
      StatColumnId::CacheRead => totals.shown_cache_read(opts),
      StatColumnId::CacheWrite => totals.shown_cache_write(opts),
      StatColumnId::Total => totals.shown_total(opts),
      _ => return None,
    };
    Some(human_unit(value / den.max(1) as f64))
  }
}

impl StatColumnId {
  fn idx(self) -> usize {
    match self {
      StatColumnId::Input => 0,
      StatColumnId::Output => 1,
      StatColumnId::Reasoning => 2,
      StatColumnId::CacheRead => 3,
      StatColumnId::CacheWrite => 4,
      StatColumnId::Total => 5,
      StatColumnId::Calls => 6,
      StatColumnId::Rounds => 7,
      StatColumnId::Sessions => 8,
      StatColumnId::CostBase => 9,
    }
  }
}

fn max_human_units(aggs: &[Aggregate], opts: &TableOpts) -> Vec<usize> {
  let mut max_units = vec![0; STAT_COLUMNS.len()];
  if !opts.human {
    return max_units;
  }
  let mut totals = TableTotals::default();
  for a in aggs {
    totals.add(a);
    for spec in active_stat_specs(opts) {
      let den = avg_den(a, opts.avg);
      if let Some(unit) = spec.human_unit(a, opts, den) {
        max_units[spec.id.idx()] = max_units[spec.id.idx()].max(unit);
      }
    }
  }
  let total_den = totals.avg_den(opts.avg);
  for spec in active_stat_specs(opts) {
    if let Some(unit) = spec.total_human_unit(&totals, opts, total_den) {
      max_units[spec.id.idx()] = max_units[spec.id.idx()].max(unit);
    }
  }
  max_units
}

fn active_stat_specs(opts: &TableOpts) -> impl Iterator<Item = &'static StatColumnSpec> + '_ {
  STAT_COLUMNS
    .iter()
    .filter(|spec| !spec.cost || (opts.show_cost && opts.unit != Unit::Cost))
}

#[derive(Default)]
struct TableTotals {
  input: u64,
  output: u64,
  prompt_cost: f64,
  completion_cost: f64,
  reasoning_cost: f64,
  cache_read_cost: f64,
  cache_write_cost: f64,
  input_bytes: u64,
  output_bytes: u64,
  reasoning: u64,
  cache_read: u64,
  cache_write: u64,
  total: u64,
  calls: u64,
  rounds: u64,
  sessions: u64,
  cost: f64,
}

impl TableTotals {
  fn add(&mut self, a: &Aggregate) {
    self.input += a.input;
    self.output += a.output;
    self.prompt_cost += a.prompt_cost;
    self.completion_cost += a.completion_cost;
    self.reasoning_cost += a.reasoning_cost;
    self.cache_read_cost += a.cache_read_cost;
    self.cache_write_cost += a.cache_write_cost;
    self.input_bytes += a.input_bytes;
    self.output_bytes += a.output_bytes;
    self.reasoning += a.reasoning;
    self.cache_read += a.cache_read;
    self.cache_write += a.cache_write;
    self.total += a.total;
    self.calls += a.calls;
    self.rounds += a.rounds;
    self.sessions += a.sessions;
    self.cost += a.cost;
  }

  fn shown_input(&self, opts: &TableOpts) -> f64 {
    match opts.unit {
      Unit::Tokens => {
        if opts.split_input {
          self
            .input
            .saturating_sub(self.cache_read)
            .saturating_sub(self.cache_write) as f64
        } else {
          self.input as f64
        }
      }
      Unit::Bytes => self.input_bytes as f64,
      Unit::Cost => {
        if opts.split_input {
          self.prompt_cost
        } else {
          self.prompt_cost + self.cache_read_cost + self.cache_write_cost
        }
      }
    }
  }

  fn shown_output(&self, opts: &TableOpts) -> f64 {
    match opts.unit {
      Unit::Tokens => self.output as f64,
      Unit::Bytes => self.output_bytes as f64,
      Unit::Cost => self.completion_cost + self.reasoning_cost,
    }
  }

  fn shown_reasoning(&self, opts: &TableOpts) -> f64 {
    match opts.unit {
      Unit::Cost => self.reasoning_cost,
      _ => self.reasoning as f64,
    }
  }

  fn shown_cache_read(&self, opts: &TableOpts) -> f64 {
    match opts.unit {
      Unit::Cost => self.cache_read_cost,
      _ => self.cache_read as f64,
    }
  }

  fn shown_cache_write(&self, opts: &TableOpts) -> f64 {
    match opts.unit {
      Unit::Cost => self.cache_write_cost,
      _ => self.cache_write as f64,
    }
  }

  fn shown_total(&self, opts: &TableOpts) -> f64 {
    match opts.unit {
      Unit::Cost => self.cost,
      _ => self.total as f64,
    }
  }

  fn avg_den(&self, avg: Option<AvgBy>) -> u64 {
    match avg {
      Some(AvgBy::Call) => self.calls,
      Some(AvgBy::Round) => self.rounds,
      Some(AvgBy::Session) => self.sessions,
      None => 1,
    }
  }
}

fn shown_input(a: &Aggregate, opts: &TableOpts) -> f64 {
  match opts.unit {
    Unit::Tokens => {
      if opts.split_input {
        a.input.saturating_sub(a.cache_read).saturating_sub(a.cache_write) as f64
      } else {
        a.input as f64
      }
    }
    Unit::Bytes => a.input_bytes as f64,
    Unit::Cost => {
      if opts.split_input {
        a.prompt_cost
      } else {
        a.prompt_cost + a.cache_read_cost + a.cache_write_cost
      }
    }
  }
}

fn input_estimated(a: &Aggregate, opts: &TableOpts) -> bool {
  if opts.unit == Unit::Bytes {
    a.input_bytes_estimated
  } else {
    a.input_estimated
  }
}

fn shown_output(a: &Aggregate, opts: &TableOpts) -> f64 {
  match opts.unit {
    Unit::Tokens => a.output as f64,
    Unit::Bytes => a.output_bytes as f64,
    Unit::Cost => a.completion_cost + a.reasoning_cost,
  }
}

fn output_estimated(a: &Aggregate, opts: &TableOpts) -> bool {
  if opts.unit == Unit::Bytes {
    a.output_bytes_estimated
  } else {
    a.output_estimated
  }
}

fn shown_reasoning(a: &Aggregate, opts: &TableOpts) -> f64 {
  match opts.unit {
    Unit::Cost => a.reasoning_cost,
    _ => a.reasoning as f64,
  }
}

fn shown_cache_read(a: &Aggregate, opts: &TableOpts) -> f64 {
  match opts.unit {
    Unit::Cost => a.cache_read_cost,
    _ => a.cache_read as f64,
  }
}

fn shown_cache_write(a: &Aggregate, opts: &TableOpts) -> f64 {
  match opts.unit {
    Unit::Cost => a.cache_write_cost,
    _ => a.cache_write as f64,
  }
}

fn shown_total(a: &Aggregate, opts: &TableOpts) -> f64 {
  match opts.unit {
    Unit::Cost => a.cost,
    _ => a.total as f64,
  }
}

fn any_input_estimated(aggs: &[Aggregate], opts: &TableOpts) -> bool {
  if opts.unit == Unit::Bytes {
    aggs.iter().any(|a| a.input_bytes_estimated)
  } else {
    aggs.iter().any(|a| a.input_estimated)
  }
}

fn any_output_estimated(aggs: &[Aggregate], opts: &TableOpts) -> bool {
  if opts.unit == Unit::Bytes {
    aggs.iter().any(|a| a.output_bytes_estimated)
  } else {
    aggs.iter().any(|a| a.output_estimated)
  }
}

fn fmt_usage_value_avg(value: f64, estimated: bool, den: u64, opts: &TableOpts) -> String {
  match opts.unit {
    Unit::Cost => fmt_cost_avg(value, den),
    _ => fmt_est_usage_avg(value as u64, estimated, den, opts.human),
  }
}

fn usage_unit_suffix(opts: &TableOpts, id: StatColumnId) -> &'static str {
  if !matches!(
    id,
    StatColumnId::Input
      | StatColumnId::Output
      | StatColumnId::Reasoning
      | StatColumnId::CacheRead
      | StatColumnId::CacheWrite
      | StatColumnId::Total
  ) {
    return "";
  }
  match opts.unit {
    Unit::Tokens => "",
    Unit::Bytes => "(B)",
    Unit::Cost => "($)",
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

  fn optional_stat(label: String, priority: u8) -> Self {
    Self {
      label,
      priority,
      required: false,
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
    .filter(|&(_, visible)| *visible)
    .map(|(col, _)| col.clone())
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
  muted: bool,
}

impl Col {
  fn text(s: &str) -> Self {
    Self {
      text: s.to_string(),
      right: false,
      muted: false,
    }
  }
  fn num(s: &str) -> Self {
    Self {
      text: s.to_string(),
      right: true,
      muted: false,
    }
  }
  fn with_muted(mut self, muted: bool) -> Self {
    self.muted = muted;
    self
  }
}

enum Style {
  Plain,
  Header,
  Total,
  Muted,
}

fn colorize(line: &str, style: Style) -> String {
  match style {
    Style::Plain => line.to_string(),
    Style::Header => format!("\x1b[36m{}\x1b[0m", line),
    Style::Total => format!("\x1b[33m{}\x1b[0m", line),
    Style::Muted => format!("\x1b[90m{}\x1b[0m", line),
  }
}

fn format_row(cols: &[Col], widths: &[usize], use_color: bool) -> String {
  let mut parts: Vec<String> = Vec::with_capacity(cols.len());
  for (i, c) in cols.iter().enumerate() {
    let w = widths[i];
    let text = fit_cell(&c.text, w);
    if c.right {
      parts.push(format!("{text:>width$}", width = w));
    } else {
      parts.push(format!("{text:<width$}", width = w));
    }
    let idx = parts.len() - 1;
    if use_color && c.muted {
      parts[idx] = colorize(&parts[idx], Style::Muted);
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

fn top_cost_per_columns(aggs: &[Aggregate]) -> Vec<String> {
  let mut totals: BTreeMap<String, f64> = BTreeMap::new();
  for agg in aggs {
    for (key, cost) in &agg.cost_per {
      *totals.entry(key.clone()).or_default() += *cost;
    }
  }
  let mut items = totals.into_iter().collect::<Vec<_>>();
  items.sort_by(|a, b| {
    b.1
      .partial_cmp(&a.1)
      .unwrap_or(std::cmp::Ordering::Equal)
      .then_with(|| a.0.cmp(&b.0))
  });
  items.into_iter().take(3).map(|(key, _)| key).collect()
}

fn short_cost_per_header(key: &str) -> String {
  key.chars().take(10).collect()
}

fn avg_den(a: &Aggregate, avg: Option<AvgBy>) -> u64 {
  match avg {
    Some(AvgBy::Call) => a.calls,
    Some(AvgBy::Round) => a.rounds,
    Some(AvgBy::Session) => a.sessions,
    None => 1,
  }
}

fn fmt_usage_avg(n: u64, den: u64, human: bool) -> String {
  if den <= 1 {
    return fmt_usage(n as f64, human);
  }
  fmt_usage(n as f64 / den as f64, human)
}

fn fmt_est_usage_avg(n: u64, estimated: bool, den: u64, human: bool) -> String {
  let body = fmt_usage_avg(n, den, human);
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

fn fmt_usage(v: f64, human: bool) -> String {
  const UNITS: [&str; 5] = ["", "K", "M", "B", "T"];
  if !human {
    return fmt_float(v);
  }
  if v < 999.95 {
    return fmt_float(v);
  }

  let mut scaled = v;
  let mut unit_idx = 0usize;
  while scaled >= 999.95 && unit_idx < UNITS.len() - 1 {
    scaled /= 1000.0;
    unit_idx += 1;
  }

  format!("{scaled:.1}{}", UNITS[unit_idx])
}

fn human_unit(v: f64) -> usize {
  let mut scaled = v;
  let mut unit_idx = 0usize;
  while scaled >= 999.95 && unit_idx < 4 {
    scaled /= 1000.0;
    unit_idx += 1;
  }
  unit_idx
}

#[cfg(test)]
mod tests {
  use super::*;

  fn aggregate(keys: &[&str]) -> Aggregate {
    Aggregate {
      keys: keys.iter().map(|s| s.to_string()).collect(),
      input: 1_234,
      output: 2_500_000,
      prompt_cost: 1.2345,
      completion_cost: 9.8765,
      reasoning_cost: 0.1111,
      cache_read_cost: 0.2222,
      cache_write_cost: 0.9013,
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
      calls: 1_234,
      rounds: 2_345,
      sessions: 3_456,
      cost_embedded: 0.0,
      cost: 12.3456,
      cost_per: BTreeMap::new(),
      first_ts: None,
      last_ts: None,
    }
  }

  fn opts(show_cost: bool, avg: Option<AvgBy>, unit: Unit, human: bool, fit_width: Option<usize>) -> TableOpts {
    TableOpts {
      show_cost,
      use_color: false,
      split_input: false,
      avg,
      unit,
      human,
      fit_width,
    }
  }

  fn render_test_table(aggs: &[Aggregate], dims: &[GroupDim], fit_width: Option<usize>) -> String {
    render_table(aggs, dims, &opts(true, None, Unit::Tokens, false, fit_width))
  }

  #[test]
  fn table_uses_plain_numbers_by_default() {
    let table = render_table(
      &[aggregate(&["codex"])],
      &[GroupDim::Source],
      &opts(true, None, Unit::Tokens, false, None),
    );

    assert!(table.contains("~1,234"), "table output: {table}");
    assert!(table.contains("2,500,000"), "table output: {table}");
    assert!(table.contains("1,000,000"), "table output: {table}");
    assert!(table.contains("3,501,233"), "table output: {table}");
    assert!(table.contains("1,234"), "table output: {table}");
    assert!(table.contains("2,345"), "table output: {table}");
    assert!(table.contains("3,456"), "table output: {table}");
    assert!(table.contains("12.3456"), "table output: {table}");
  }

  #[test]
  fn human_table_compacts_usage_fields_but_not_counts_or_costs() {
    let table = render_table(
      &[aggregate(&["codex"])],
      &[GroupDim::Source],
      &opts(true, None, Unit::Tokens, true, None),
    );

    assert!(table.contains("~1.2K"), "table output: {table}");
    assert!(table.contains("2.5M"), "table output: {table}");
    assert!(table.contains("999"), "table output: {table}");
    assert!(table.contains("1.5K"), "table output: {table}");
    assert!(table.contains("1.0M"), "table output: {table}");
    assert!(table.contains("3.5M"), "table output: {table}");
    assert!(table.contains("1,234"), "table output: {table}");
    assert!(table.contains("2,345"), "table output: {table}");
    assert!(table.contains("3,456"), "table output: {table}");
    assert!(table.contains("12.3456"), "table output: {table}");
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
    agg.calls = 10;

    let table = render_table(
      &[agg],
      &[GroupDim::Source],
      &opts(false, Some(AvgBy::Call), Unit::Tokens, true, None),
    );

    assert!(table.contains("~1.2K"), "table output: {table}");
    assert!(table.contains("999.90"), "table output: {table}");
    assert!(table.contains("99.90"), "table output: {table}");
    assert!(table.contains("1.2K"), "table output: {table}");
    assert!(table.contains("123.5K"), "table output: {table}");
    assert!(table.contains("250.0K"), "table output: {table}");
  }

  #[test]
  fn bytes_headers_stay_plain_while_byte_values_compact() {
    let table = render_table(
      &[aggregate(&["codex"])],
      &[GroupDim::Source],
      &opts(false, None, Unit::Bytes, true, None),
    );

    assert!(table.contains("input(B)"), "table output: {table}");
    assert!(table.contains("output(B)"), "table output: {table}");
    assert!(table.contains("~1.5K"), "table output: {table}");
    assert!(table.contains("2.5B"), "table output: {table}");
  }

  #[test]
  fn human_table_mutes_values_below_largest_column_unit() {
    let small = aggregate(&["small"]);
    let mut large = aggregate(&["large"]);
    large.output = 5_000_000;
    let mut table_opts = opts(false, None, Unit::Tokens, true, None);
    table_opts.use_color = true;

    let table = render_table(&[small, large], &[GroupDim::Source], &table_opts);

    assert!(table.contains("\x1b[90m"), "table output: {table:?}");
    assert!(table.contains("5.0M"), "table output: {table:?}");
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
      "calls",
      "rounds",
      "sessions",
      "cost($)",
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
