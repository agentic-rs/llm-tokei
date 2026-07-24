use crate::cli::SvgTheme;
use std::fmt::Write;

#[derive(Clone, Copy)]
pub(crate) enum SvgColor {
  Background,
  Surface,
  Border,
  Text,
  TerminalText,
  Muted,
  Grid,
  Cyan,
  Yellow,
  Dim,
  Heat0,
  Heat1,
  Heat2,
  Heat3,
  Heat4,
  WindowRed,
  WindowYellow,
  WindowGreen,
}

impl SvgColor {
  const ALL: [Self; 18] = [
    Self::Background,
    Self::Surface,
    Self::Border,
    Self::Text,
    Self::TerminalText,
    Self::Muted,
    Self::Grid,
    Self::Cyan,
    Self::Yellow,
    Self::Dim,
    Self::Heat0,
    Self::Heat1,
    Self::Heat2,
    Self::Heat3,
    Self::Heat4,
    Self::WindowRed,
    Self::WindowYellow,
    Self::WindowGreen,
  ];

  pub(crate) const fn name(self) -> &'static str {
    match self {
      Self::Background => "background",
      Self::Surface => "surface",
      Self::Border => "border",
      Self::Text => "text",
      Self::TerminalText => "terminal-text",
      Self::Muted => "muted",
      Self::Grid => "grid",
      Self::Cyan => "cyan",
      Self::Yellow => "yellow",
      Self::Dim => "dim",
      Self::Heat0 => "heat-0",
      Self::Heat1 => "heat-1",
      Self::Heat2 => "heat-2",
      Self::Heat3 => "heat-3",
      Self::Heat4 => "heat-4",
      Self::WindowRed => "window-red",
      Self::WindowYellow => "window-yellow",
      Self::WindowGreen => "window-green",
    }
  }

  pub(crate) const fn color(self, theme: SvgTheme) -> &'static str {
    match (self, theme) {
      (Self::Background, SvgTheme::Dark) => "#0d1117",
      (Self::Background, SvgTheme::Light) => "#ffffff",
      (Self::Surface, SvgTheme::Dark) => "#161b22",
      (Self::Surface, SvgTheme::Light) => "#f6f8fa",
      (Self::Border, SvgTheme::Dark) => "#30363d",
      (Self::Border, SvgTheme::Light) => "#d0d7de",
      (Self::Text, SvgTheme::Dark) => "#f0f6fc",
      (Self::Text, SvgTheme::Light) => "#24292f",
      (Self::TerminalText, SvgTheme::Dark) => "#c9d1d9",
      (Self::TerminalText, SvgTheme::Light) => "#24292f",
      (Self::Muted, SvgTheme::Dark) => "#8b949e",
      (Self::Muted, SvgTheme::Light) => "#57606a",
      (Self::Grid, SvgTheme::Dark) => "#21262d",
      (Self::Grid, SvgTheme::Light) => "#d8dee4",
      (Self::Cyan, SvgTheme::Dark) => "#39c5cf",
      (Self::Cyan, SvgTheme::Light) => "#0969da",
      (Self::Yellow, SvgTheme::Dark) => "#d29922",
      (Self::Yellow, SvgTheme::Light) => "#9a6700",
      (Self::Dim, SvgTheme::Dark) => "#6e7681",
      (Self::Dim, SvgTheme::Light) => "#57606a",
      (Self::Heat0, SvgTheme::Dark) => "#161b22",
      (Self::Heat0, SvgTheme::Light) => "#ebedf0",
      (Self::Heat1, SvgTheme::Dark) => "#0e4429",
      (Self::Heat1, SvgTheme::Light) => "#9be9a8",
      (Self::Heat2, SvgTheme::Dark) => "#006d32",
      (Self::Heat2, SvgTheme::Light) => "#40c463",
      (Self::Heat3, SvgTheme::Dark) => "#26a641",
      (Self::Heat3, SvgTheme::Light) => "#30a14e",
      (Self::Heat4, SvgTheme::Dark) => "#39d353",
      (Self::Heat4, SvgTheme::Light) => "#216e39",
      (Self::WindowRed, SvgTheme::Dark) => "#ff5f56",
      (Self::WindowRed, SvgTheme::Light) => "#cf222e",
      (Self::WindowYellow, SvgTheme::Dark) => "#ffbd2e",
      (Self::WindowYellow, SvgTheme::Light) => "#bf8700",
      (Self::WindowGreen, SvgTheme::Dark) => "#27c93f",
      (Self::WindowGreen, SvgTheme::Light) => "#1a7f37",
    }
  }
}

pub(crate) fn write_svg_theme_styles(out: &mut String, default_theme: SvgTheme) {
  out.push_str("  <style>\n    :root { color-scheme: light dark;");
  write_palette_variables(out, default_theme);
  out.push_str(" }\n");

  for color in SvgColor::ALL {
    writeln!(
      out,
      "    [data-svg-fill=\"{}\"] {{ fill: var(--svg-{}); }}",
      color.name(),
      color.name()
    )
    .unwrap();
    writeln!(
      out,
      "    [data-svg-stroke=\"{}\"] {{ stroke: var(--svg-{}); }}",
      color.name(),
      color.name()
    )
    .unwrap();
  }

  writeln!(
    out,
    "    @media (prefers-color-scheme: {}) {{",
    default_theme.opposite().as_str()
  )
  .unwrap();
  out.push_str("      :root {");
  write_palette_variables(out, default_theme.opposite());
  out.push_str(" }\n    }\n  </style>\n");
}

fn write_palette_variables(out: &mut String, theme: SvgTheme) {
  for color in SvgColor::ALL {
    write!(out, " --svg-{}: {};", color.name(), color.color(theme)).unwrap();
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn styles_keep_the_requested_theme_as_a_fallback() {
    let mut dark = String::new();
    write_svg_theme_styles(&mut dark, SvgTheme::Dark);
    assert!(dark.contains("--svg-background: #0d1117;"));
    assert!(dark.contains("@media (prefers-color-scheme: light)"));
    assert!(dark.contains("--svg-background: #ffffff;"));

    let mut light = String::new();
    write_svg_theme_styles(&mut light, SvgTheme::Light);
    assert!(light.contains("--svg-background: #ffffff;"));
    assert!(light.contains("@media (prefers-color-scheme: dark)"));
    assert!(light.contains("--svg-background: #0d1117;"));
  }
}
