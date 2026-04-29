mod aggregate;
mod cli;
mod format;
mod model;
mod pricing;
mod sources;
mod time;

use anyhow::{Context, Result};
use clap::Parser;

use crate::aggregate::{aggregate, sort_aggs, Filters, GroupDim, SortKey};
use crate::cli::{Args, Format};
use crate::format::{json::render_json, table::render_table};
use crate::model::UsageRecord;
use crate::pricing::PricingTable;
use crate::sources::{codex::CodexSource, opencode::OpenCodeSource, UsageSource};

fn main() -> Result<()> {
    let args = Args::parse();
    let use_color = !args.no_color && std::env::var_os("NO_COLOR").is_none();

    // Resolve sources.
    let want = args
        .source
        .as_ref()
        .map(|v| v.iter().map(|s| s.to_lowercase()).collect::<Vec<_>>())
        .unwrap_or_else(|| vec!["codex".into(), "opencode".into()]);

    let mut all: Vec<UsageRecord> = Vec::new();

    if want.iter().any(|s| s == "codex") {
        let path = args.codex_dir.clone().or_else(CodexSource::default_path);
        if let Some(p) = path {
            let src = CodexSource::new(p);
            match src.collect() {
                Ok(mut v) => {
                    if args.verbose {
                        eprintln!("codex: {} session record(s)", v.len());
                    }
                    all.append(&mut v);
                }
                Err(e) if args.verbose => eprintln!("codex: error: {e:#}"),
                Err(_) => {}
            }
        }
    }

    if want.iter().any(|s| s == "opencode") {
        let path = args
            .opencode_db
            .clone()
            .or_else(OpenCodeSource::default_path);
        if let Some(p) = path {
            let src = OpenCodeSource::new(p);
            match src.collect() {
                Ok(mut v) => {
                    if args.verbose {
                        eprintln!("opencode: {} message record(s)", v.len());
                    }
                    all.append(&mut v);
                }
                Err(e) if args.verbose => eprintln!("opencode: error: {e:#}"),
                Err(_) => {}
            }
        }
    }

    // Filters.
    let since = args
        .since
        .as_deref()
        .map(time::parse_when)
        .transpose()
        .context("parsing --since")?;
    let until = args
        .until
        .as_deref()
        .map(time::parse_when)
        .transpose()
        .context("parsing --until")?;
    let filters = Filters {
        since,
        until,
        model_glob: args
            .model
            .as_deref()
            .map(glob::Pattern::new)
            .transpose()
            .context("parsing --model glob")?,
        provider_glob: args
            .provider
            .as_deref()
            .map(glob::Pattern::new)
            .transpose()
            .context("parsing --provider glob")?,
        cwd_glob: args
            .cwd
            .as_deref()
            .map(glob::Pattern::new)
            .transpose()
            .context("parsing --cwd glob")?,
    };

    // Pricing.
    let mut pricing = PricingTable::load_bundled();
    if let Some(p) = &args.pricing {
        pricing.merge_file(p)?;
    }

    // Group dims.
    let dims: Vec<GroupDim> = args
        .group_by
        .iter()
        .filter_map(|s| GroupDim::parse(s))
        .collect();
    let dims = if dims.is_empty() {
        vec![GroupDim::Source, GroupDim::Model]
    } else {
        dims
    };

    let mut aggs = aggregate(&all, &dims, args.date_bucket.as_str(), &filters, &pricing);

    let sort_key = SortKey::parse(&args.sort).unwrap_or(SortKey::Total);
    sort_aggs(&mut aggs, sort_key, !args.asc);

    if let Some(n) = args.limit {
        aggs.truncate(n);
    }

    let show_cost = !args.no_cost;

    match args.format {
        Format::Table => {
            if aggs.is_empty() {
                println!("(no records found)");
            } else {
                println!(
                    "{}",
                    render_table(
                        &aggs,
                        &dims,
                        &crate::format::table::TableOpts {
                            show_cost,
                            use_color,
                        },
                    )
                );
            }
        }
        Format::Json => {
            println!("{}", render_json(&aggs, &dims));
        }
    }

    Ok(())
}
