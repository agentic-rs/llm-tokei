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

  let output = Command::new(&bin)
    .args(&llm_args)
    .env_remove("NO_COLOR")
    .output()
    .with_context(|| format!("running {}", bin.display()))?;
  if !output.status.success() {
    bail!("llm-tokei failed: {}", String::from_utf8_lossy(&output.stderr));
  }

  let ansi = String::from_utf8(output.stdout).context("llm-tokei output was not utf-8")?;
  let command = format!("llm-tokei {}", llm_args.join(" "));
  let svg = render_svg(&command, &ansi);

  if let Some(parent) = out.parent() {
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
  }
  std::fs::write(&out, svg).with_context(|| format!("writing {}", out.display()))?;
  Ok(())
}

fn print_help() {
  println!("Usage: cargo run --example gen-showcase -- --args \"<llm-tokei args>\" --out <path>");
}

fn split_args(input: &str) -> Vec<String> {
  input.split_whitespace().map(str::to_string).collect()
}

fn render_svg(command: &str, ansi: &str) -> String {
  let lines: Vec<Vec<Span>> = ansi.lines().map(parse_ansi_line).collect();
  let char_width = 9usize;
  let line_height = 26usize;
  let padding_x = 24usize;
  let content_width = lines
    .iter()
    .map(|line| line.iter().map(|span| span.text.chars().count()).sum::<usize>())
    .max()
    .unwrap_or(80)
    .max(command.chars().count());
  let width = padding_x * 2 + content_width * char_width;
  let height = 88 + lines.len() * line_height + 28;

  let mut svg = String::new();
  svg.push_str(&format!(
    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" role=\"img\" aria-labelledby=\"title desc\">\n"
  ));
  svg.push_str("  <title id=\"title\">llm-tokei terminal output</title>\n");
  svg.push_str("  <desc id=\"desc\">A colored terminal table produced by llm-tokei.</desc>\n");
  svg.push_str(&format!(
    "  <rect width=\"{width}\" height=\"{height}\" rx=\"14\" fill=\"#0d1117\"/>\n"
  ));
  svg.push_str("  <circle cx=\"28\" cy=\"26\" r=\"6\" fill=\"#ff5f56\"/>\n");
  svg.push_str("  <circle cx=\"48\" cy=\"26\" r=\"6\" fill=\"#ffbd2e\"/>\n");
  svg.push_str("  <circle cx=\"68\" cy=\"26\" r=\"6\" fill=\"#27c93f\"/>\n");
  svg.push_str(&text_element(padding_x, 62, "#8b949e", &format!("$ {command}")));

  for (idx, line) in lines.iter().enumerate() {
    let y = 102 + idx * line_height;
    let mut x = padding_x;
    for span in line {
      svg.push_str(&text_element(x, y, span.color, &span.text));
      x += span.text.chars().count() * char_width;
    }
  }

  svg.push_str("</svg>\n");
  svg
}

#[derive(Clone)]
struct Span {
  color: &'static str,
  text: String,
}

fn parse_ansi_line(line: &str) -> Vec<Span> {
  let mut spans = Vec::new();
  let mut color = "#c9d1d9";
  let mut buf = String::new();
  let mut chars = line.chars().peekable();

  while let Some(ch) = chars.next() {
    if ch == '\x1b' && chars.peek() == Some(&'[') {
      chars.next();
      let mut code = String::new();
      for c in chars.by_ref() {
        if c == 'm' {
          break;
        }
        code.push(c);
      }
      push_span(&mut spans, color, &mut buf);
      color = match code.as_str() {
        "0" => "#c9d1d9",
        "36" => "#39c5cf",
        "33" => "#d29922",
        "90" => "#6e7681",
        _ => color,
      };
    } else {
      buf.push(ch);
    }
  }
  push_span(&mut spans, color, &mut buf);
  spans
}

fn push_span(spans: &mut Vec<Span>, color: &'static str, buf: &mut String) {
  if !buf.is_empty() {
    spans.push(Span {
      color,
      text: std::mem::take(buf),
    });
  }
}

fn text_element(x: usize, y: usize, fill: &str, text: &str) -> String {
  format!(
    "  <text x=\"{x}\" y=\"{y}\" fill=\"{fill}\" font-family=\"ui-monospace, SFMono-Regular, Menlo, Consolas, monospace\" font-size=\"15\" xml:space=\"preserve\">{}</text>\n",
    escape_xml(text)
  )
}

fn escape_xml(text: &str) -> String {
  text
    .replace('&', "&amp;")
    .replace('<', "&lt;")
    .replace('>', "&gt;")
    .replace('"', "&quot;")
}
