pub fn render_svg_terminal(command: &str, ansi: &str) -> String {
  let mut raw_lines: Vec<&str> = ansi.lines().collect();
  while raw_lines.last().is_some_and(|line| line.trim().is_empty()) {
    raw_lines.pop();
  }
  let lines = raw_lines.into_iter().map(parse_ansi_line).collect::<Vec<_>>();
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

  let mut out = String::new();
  out.push_str(&format!(
    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" role=\"img\" aria-labelledby=\"title desc\">\n"
  ));
  out.push_str("  <title id=\"title\">llm-tokei terminal output</title>\n");
  out.push_str("  <desc id=\"desc\">A colored terminal table produced by llm-tokei.</desc>\n");
  out.push_str(&format!(
    "  <rect width=\"{width}\" height=\"{height}\" rx=\"16\" fill=\"#0d1117\"/>\n"
  ));
  out.push_str(&format!(
    "  <rect x=\"0\" y=\"0\" width=\"{width}\" height=\"48\" rx=\"16\" fill=\"#161b22\"/>\n"
  ));
  out.push_str(&format!(
    "  <rect x=\"0\" y=\"32\" width=\"{width}\" height=\"16\" fill=\"#161b22\"/>\n"
  ));
  out.push_str("  <circle cx=\"24\" cy=\"24\" r=\"6\" fill=\"#ff5f56\"/>\n");
  out.push_str("  <circle cx=\"44\" cy=\"24\" r=\"6\" fill=\"#ffbd2e\"/>\n");
  out.push_str("  <circle cx=\"64\" cy=\"24\" r=\"6\" fill=\"#27c93f\"/>\n");
  out.push_str(&format!(
    "  <text x=\"{}\" y=\"29\" text-anchor=\"middle\" fill=\"#8b949e\" font-family=\"{}\" font-size=\"13\">llm-tokei</text>\n",
    width / 2,
    font_family()
  ));
  out.push_str("  <defs>\n");
  out.push_str(&format!(
    "    <clipPath id=\"terminal-content\"><rect x=\"{padding_x}\" y=\"{header_height}\" width=\"{}\" height=\"{content_height}\"/></clipPath>\n",
    width - padding_x * 2
  ));
  out.push_str("  </defs>\n");
  out.push_str("  <g clip-path=\"url(#terminal-content)\">\n");
  out.push_str(&line_element(
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
    out.push_str(&line_element(padding_x, y, font_size, line));
  }
  let blank_y = table_start_y + lines.len() * line_height;
  out.push_str(&line_element(padding_x, blank_y, font_size, &[]));
  out.push_str("  </g>\n");
  out.push_str("</svg>\n");
  out
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

pub(crate) fn escape_xml(text: &str) -> String {
  let mut out = String::with_capacity(text.len());
  for ch in text.chars() {
    match ch {
      '&' => out.push_str("&amp;"),
      '<' => out.push_str("&lt;"),
      '>' => out.push_str("&gt;"),
      '"' => out.push_str("&quot;"),
      '\'' => out.push_str("&apos;"),
      _ => out.push(ch),
    }
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn svg_escapes_text_content() {
    let svg = render_svg_terminal("llm-tokei --format svg", "source  total\n<&>\"'  42\n");
    assert!(svg.contains("&lt;&amp;&gt;&quot;&apos;"));
    assert!(!svg.contains("<&>\"'"));
  }

  #[test]
  fn svg_renders_showcase_terminal_chrome_and_ansi_colors() {
    let svg = render_svg_terminal(
      "llm-tokei --format svg",
      "\x1b[36msource\x1b[0m\n\x1b[33mTOTAL\x1b[0m\n",
    );
    assert!(svg.contains("rx=\"16\" fill=\"#0d1117\""));
    assert!(svg.contains("fill=\"#ff5f56\""));
    assert!(svg.contains("$ llm-tokei --format svg"));
    assert!(svg.contains("fill=\"#39c5cf\""));
    assert!(svg.contains("fill=\"#d29922\""));
  }
}
