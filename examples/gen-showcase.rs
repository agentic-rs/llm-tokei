use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Command;

fn main() -> Result<()> {
  let mut out: Option<PathBuf> = None;
  let mut llm_args = Vec::new();
  let mut args = std::env::args().skip(1);

  while let Some(arg) = args.next() {
    match arg.as_str() {
      "--out" => out = Some(PathBuf::from(args.next().context("--out requires a path")?)),
      "--args" => llm_args.extend(split_args(&args.next().context("--args requires a value")?)),
      "--help" => {
        print_help();
        return Ok(());
      }
      other => bail!("unknown argument {other}"),
    }
  }

  let out = out.context("missing --out")?;
  if llm_args.is_empty() {
    bail!("missing --args");
  }

  let bin = std::env::current_exe()
    .context("locating current executable")?
    .parent()
    .and_then(|p| p.parent())
    .map(|p| p.join("llm-tokei"))
    .context("locating llm-tokei binary")?;
  let svg_args = svg_args(&llm_args);

  let output = Command::new(&bin)
    .args(&svg_args)
    .env_remove("NO_COLOR")
    .output()
    .with_context(|| format!("running {}", bin.display()))?;
  if !output.status.success() {
    bail!("llm-tokei failed: {}", String::from_utf8_lossy(&output.stderr));
  }

  if let Some(parent) = out.parent() {
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
  }
  std::fs::write(&out, output.stdout).with_context(|| format!("writing {}", out.display()))?;
  Ok(())
}

fn print_help() {
  println!("Usage: cargo run --example gen-showcase -- --args \"<llm-tokei args>\" --out <path>");
}

fn split_args(input: &str) -> Vec<String> {
  input.split_whitespace().map(str::to_string).collect()
}

fn svg_args(args: &[String]) -> Vec<String> {
  let mut svg_args = Vec::with_capacity(args.len() + 2);
  let mut args = args.iter().peekable();
  while let Some(arg) = args.next() {
    if arg == "--format" {
      args.next();
    } else if !arg.starts_with("--format=") {
      svg_args.push(arg.clone());
    }
  }
  svg_args.push("--format".to_string());
  svg_args.push("svg".to_string());
  svg_args
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn svg_args_replaces_an_existing_format() {
    let args = ["graph", "--format", "table", "--svg-theme", "light"]
      .map(str::to_string)
      .to_vec();

    assert_eq!(
      svg_args(&args),
      ["graph", "--svg-theme", "light", "--format", "svg"]
        .map(str::to_string)
        .to_vec()
    );
  }

  #[test]
  fn svg_args_replaces_equals_format() {
    let args = ["--format=json", "--month"].map(str::to_string).to_vec();

    assert_eq!(
      svg_args(&args),
      ["--month", "--format", "svg"].map(str::to_string).to_vec()
    );
  }
}
