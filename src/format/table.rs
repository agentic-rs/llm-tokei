use crate::aggregate::{Aggregate, GroupDim};
use comfy_table::{presets::UTF8_FULL, Cell, CellAlignment, Color, ContentArrangement, Table};

pub struct TableOpts {
    pub show_cost: bool,
    pub use_color: bool,
}

pub fn render_table(aggs: &[Aggregate], dims: &[GroupDim], opts: &TableOpts) -> String {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);

    let mut header: Vec<Cell> = dims
        .iter()
        .map(|d| header_cell(d.label(), opts.use_color))
        .collect();
    for h in [
        "input",
        "output",
        "reasoning",
        "cache_r",
        "cache_w",
        "total",
        "turns",
    ] {
        header.push(header_cell(h, opts.use_color));
    }
    if opts.show_cost {
        header.push(header_cell("cost($)", opts.use_color));
        header.push(header_cell("cost×mult($)", opts.use_color));
    }
    table.set_header(header);

    let (mut tot_in, mut tot_out, mut tot_re, mut tot_cr, mut tot_cw, mut tot_tot, mut tot_t) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut tot_base, mut tot_mult) = (0.0_f64, 0.0_f64);

    for a in aggs {
        let mut row: Vec<Cell> = a.keys.iter().map(|k| Cell::new(k)).collect();
        row.push(num_cell(a.input));
        row.push(num_cell(a.output));
        row.push(num_cell(a.reasoning));
        row.push(num_cell(a.cache_read));
        row.push(num_cell(a.cache_write));
        row.push(num_cell(a.total));
        row.push(num_cell(a.turns));
        if opts.show_cost {
            row.push(cost_cell(a.cost_base));
            row.push(cost_cell(a.cost_multiplied));
            tot_base += a.cost_base;
            tot_mult += a.cost_multiplied;
        }
        table.add_row(row);
        tot_in += a.input;
        tot_out += a.output;
        tot_re += a.reasoning;
        tot_cr += a.cache_read;
        tot_cw += a.cache_write;
        tot_tot += a.total;
        tot_t += a.turns;
    }

    if aggs.len() > 1 {
        let mut row: Vec<Cell> = (0..dims.len())
            .map(|i| {
                if i == 0 {
                    bold_cell("TOTAL", opts.use_color)
                } else {
                    Cell::new("")
                }
            })
            .collect();
        row.push(num_cell_bold(tot_in, opts.use_color));
        row.push(num_cell_bold(tot_out, opts.use_color));
        row.push(num_cell_bold(tot_re, opts.use_color));
        row.push(num_cell_bold(tot_cr, opts.use_color));
        row.push(num_cell_bold(tot_cw, opts.use_color));
        row.push(num_cell_bold(tot_tot, opts.use_color));
        row.push(num_cell_bold(tot_t, opts.use_color));
        if opts.show_cost {
            row.push(cost_cell_bold(tot_base, opts.use_color));
            row.push(cost_cell_bold(tot_mult, opts.use_color));
        }
        table.add_row(row);
    }

    table.to_string()
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

fn num_cell(n: u64) -> Cell {
    Cell::new(fmt_int(n)).set_alignment(CellAlignment::Right)
}
fn num_cell_bold(n: u64, color: bool) -> Cell {
    let c = Cell::new(fmt_int(n)).set_alignment(CellAlignment::Right);
    if color {
        c.fg(Color::Yellow)
    } else {
        c
    }
}
fn cost_cell(v: f64) -> Cell {
    let s = if v == 0.0 {
        "-".to_string()
    } else {
        format!("{:.4}", v)
    };
    Cell::new(s).set_alignment(CellAlignment::Right)
}
fn cost_cell_bold(v: f64, color: bool) -> Cell {
    let s = if v == 0.0 {
        "-".to_string()
    } else {
        format!("{:.4}", v)
    };
    let c = Cell::new(s).set_alignment(CellAlignment::Right);
    if color {
        c.fg(Color::Yellow)
    } else {
        c
    }
}
fn header_cell(s: &str, color: bool) -> Cell {
    let c = Cell::new(s);
    if color {
        c.fg(Color::Cyan)
    } else {
        c
    }
}
fn bold_cell(s: &str, color: bool) -> Cell {
    let c = Cell::new(s);
    if color {
        c.fg(Color::Yellow)
    } else {
        c
    }
}
