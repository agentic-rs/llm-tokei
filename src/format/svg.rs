pub fn render_svg_terminal(text: &str) -> String {
  const FONT_SIZE: usize = 14;
  const LINE_HEIGHT: usize = 20;
  const CHAR_WIDTH: usize = 8;
  const PADDING_X: usize = 18;
  const PADDING_Y: usize = 16;
  const MIN_WIDTH: usize = 360;

  let lines = non_empty_lines(text);
  let max_chars = lines.iter().map(|line| line.chars().count()).max().unwrap_or(0);
  let width = (PADDING_X * 2 + max_chars * CHAR_WIDTH).max(MIN_WIDTH);
  let height = PADDING_Y * 2 + lines.len().max(1) * LINE_HEIGHT;

  let mut out = String::new();
  out.push_str(&format!(
    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" role=\"img\" aria-labelledby=\"title desc\">\n"
  ));
  out.push_str("  <title id=\"title\">llm-tokei output</title>\n");
  out.push_str("  <desc id=\"desc\">Token usage statistics rendered as a terminal table.</desc>\n");
  out.push_str("  <rect width=\"100%\" height=\"100%\" rx=\"8\" fill=\"#111827\"/>\n");
  out.push_str(&format!(
    "  <text fill=\"#e5e7eb\" font-family=\"ui-monospace, SFMono-Regular, Menlo, Consolas, monospace\" font-size=\"{FONT_SIZE}\" xml:space=\"preserve\">\n"
  ));
  for (idx, line) in lines.iter().enumerate() {
    let y = PADDING_Y + FONT_SIZE + idx * LINE_HEIGHT;
    out.push_str(&format!(
      "    <tspan x=\"{PADDING_X}\" y=\"{y}\">{}</tspan>\n",
      escape_xml(line)
    ));
  }
  out.push_str("  </text>\n");
  out.push_str("</svg>\n");
  out
}

fn non_empty_lines(text: &str) -> Vec<&str> {
  let lines = text.lines().collect::<Vec<_>>();
  if lines.is_empty() {
    vec![""]
  } else {
    lines
  }
}

fn escape_xml(text: &str) -> String {
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
    let svg = render_svg_terminal("source  total\n<&>\"'  42\n");
    assert!(svg.contains("&lt;&amp;&gt;&quot;&apos;"));
    assert!(!svg.contains("<&>\"'"));
  }
}
