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
  let mut raw_lines: Vec<&str> = ansi.lines().collect();
  while raw_lines.last().is_some_and(|line| line.trim().is_empty()) {
    raw_lines.pop();
  }
  let lines: Vec<Vec<Span>> = raw_lines.into_iter().map(parse_ansi_line).collect();
  let font_size = 14usize;
  let char_width = 8usize;
  let line_height = 22usize;
  let padding_x = 22usize;
  let header_height = 48usize;
  let command_y = header_height + 25;
  let table_start_y = command_y + line_height + 6;
  let footer_padding = 10usize;
  let trailing_blank_lines = 1usize;
  let content_width = lines
    .iter()
    .map(|line| line.iter().map(|span| span.text.chars().count()).sum::<usize>())
    .max()
    .unwrap_or(80)
    .max(command.chars().count());
  let width = padding_x * 2 + content_width * char_width;
  let height = table_start_y + (lines.len().saturating_sub(1) + trailing_blank_lines) * line_height + footer_padding;
  let content_height = height - header_height - 2;

  let mut svg = String::new();
  svg.push_str(&format!(
    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" role=\"img\" aria-labelledby=\"title desc\">\n"
  ));
  svg.push_str("  <title id=\"title\">llm-tokei terminal output</title>\n");
  svg.push_str("  <desc id=\"desc\">A colored terminal table produced by llm-tokei.</desc>\n");
  svg.push_str(&format!(
    "  <rect width=\"{width}\" height=\"{height}\" rx=\"16\" fill=\"#0d1117\"/>\n"
  ));
  svg.push_str(&format!(
    "  <rect x=\"0\" y=\"0\" width=\"{width}\" height=\"48\" rx=\"16\" fill=\"#161b22\"/>\n"
  ));
  svg.push_str(&format!(
    "  <rect x=\"0\" y=\"32\" width=\"{width}\" height=\"16\" fill=\"#161b22\"/>\n"
  ));
  svg.push_str("  <circle cx=\"24\" cy=\"24\" r=\"6\" fill=\"#ff5f56\"/>\n");
  svg.push_str("  <circle cx=\"44\" cy=\"24\" r=\"6\" fill=\"#ffbd2e\"/>\n");
  svg.push_str("  <circle cx=\"64\" cy=\"24\" r=\"6\" fill=\"#27c93f\"/>\n");
  svg.push_str(&format!(
    "  <text x=\"{}\" y=\"29\" text-anchor=\"middle\" fill=\"#8b949e\" font-family=\"{}\" font-size=\"13\">llm-tokei</text>\n",
    width / 2,
    font_family()
  ));
  svg.push_str("  <defs>\n");
  svg.push_str(&format!(
    "    <clipPath id=\"terminal-content\"><rect x=\"{padding_x}\" y=\"{header_height}\" width=\"{}\" height=\"{content_height}\"/></clipPath>\n",
    width - padding_x * 2
  ));
  svg.push_str("  </defs>\n");
  svg.push_str("  <g clip-path=\"url(#terminal-content)\">\n");
  svg.push_str(&line_element(
    padding_x,
    command_y,
    font_size,
    &[Span {
      color: "#8b949e",
      text: format!("$ {command}"),
    }],
  ));

  for (idx, line) in lines.iter().enumerate() {
    let y = table_start_y + idx * line_height;
    svg.push_str(&line_element(padding_x, y, font_size, line));
  }
  let blank_y = table_start_y + lines.len() * line_height;
  svg.push_str(&line_element(padding_x, blank_y, font_size, &[]));

  svg.push_str("  </g>\n");
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

fn line_element(x: usize, y: usize, font_size: usize, spans: &[Span]) -> String {
  let mut out = format!(
    "    <text x=\"{x}\" y=\"{y}\" font-family=\"{}\" font-size=\"{font_size}\" xml:space=\"preserve\">",
    font_family()
  );
  for span in spans {
    out.push_str(&format!(
      "<tspan fill=\"{}\">{}</tspan>",
      span.color,
      escape_xml(&span.text)
    ));
  }
  out.push_str("</text>\n");
  out
}

fn font_family() -> &'static str {
  "ui-monospace, SFMono-Regular, Menlo, Consolas, 'Liberation Mono', monospace"
}

fn escape_xml(text: &str) -> String {
  text
    .replace('&', "&amp;")
    .replace('<', "&lt;")
    .replace('>', "&gt;")
    .replace('"', "&quot;")
}
