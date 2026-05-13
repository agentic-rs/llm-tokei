use std::collections::BTreeMap;

pub fn norm(s: &str) -> String {
  s.trim().to_lowercase()
}

pub fn resolve_alias(aliases: &BTreeMap<String, String>, provider: &str, model: &str) -> Option<String> {
  let model = norm(model);
  aliases
    .get(&format!("{}/{}", norm(provider), &model))
    .or_else(|| aliases.get(&model))
    .cloned()
    .or_else(|| fuzzy_resolve(aliases, &model))
}

pub fn fuzzy_resolve(aliases: &BTreeMap<String, String>, model: &str) -> Option<String> {
  let candidate = model;
  for pass in 0..7 {
    let next = match pass {
      0 => strip_date_suffix(candidate),
      1 => strip_mode_suffix(candidate),
      2 => strip_variant_suffix(candidate),
      3 => strip_provider_prefix(candidate),
      4 => strip_slash_prefix(candidate),
      5 => normalize_version_sep(candidate, aliases),
      6 => {
        let s = strip_provider_prefix(candidate);
        normalize_version_sep(&s, aliases)
      }
      _ => return None,
    };
    if next == candidate {
      continue;
    }
    if let Some(canonical) = aliases.get(&next) {
      return Some(canonical.clone());
    }
    return fuzzy_resolve(aliases, &next);
  }
  None
}

pub fn strip_date_suffix(s: &str) -> String {
  let s = s.strip_suffix("@default").unwrap_or(s);
  if let Some(pos) = s.rfind('-') {
    let tail = &s[pos + 1..];
    if tail.len() == 8 && tail.chars().all(|c| c.is_ascii_digit()) {
      return s[..pos].to_string();
    }
  }
  if let Some(pos) = s.rfind('-') {
    let tail = &s[pos + 1..];
    if tail.len() == 6 && tail.chars().all(|c| c.is_ascii_digit()) {
      return s[..pos].to_string();
    }
  }
  if let Some(pos) = s.rfind('@') {
    let tail = &s[pos + 1..];
    if tail.len() == 8 && tail.chars().all(|c| c.is_ascii_digit()) {
      return s[..pos].to_string();
    }
  }
  if s.len() >= 11 {
    let candidate = &s[s.len() - 11..];
    if candidate.starts_with('-')
      && candidate.as_bytes()[5] == b'-'
      && candidate.as_bytes()[8] == b'-'
    {
      let tail = &candidate[1..];
      let parts: Vec<&str> = tail.split('-').collect();
      if parts.len() == 3
        && parts.iter().all(|p| p.len() == 4 || p.len() == 2)
        && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
      {
        return s[..s.len() - 11].to_string();
      }
    }
  }
  s.to_string()
}

pub fn strip_mode_suffix(s: &str) -> String {
  for suffix in [":thinking", "-thinking", "-think", "-fast"] {
    if let Some(stripped) = s.strip_suffix(suffix) {
      return stripped.to_string();
    }
  }
  s.to_string()
}

pub fn strip_variant_suffix(s: &str) -> String {
  for suffix in ["-latest", "-chat-latest", "-chat", "-preview"] {
    if let Some(stripped) = s.strip_suffix(suffix) {
      return stripped.to_string();
    }
  }
  s.to_string()
}

pub const PROVIDER_PREFIXES: &[&str] = &[
  "zai-org-",
  "anthropic-",
  "openai-",
  "copilot-",
  "google-",
  "zai-",
  "deepseek-",
  "alibaba-",
  "minimax-",
];

pub fn strip_provider_prefix(s: &str) -> String {
  for prefix in PROVIDER_PREFIXES {
    if let Some(stripped) = s.strip_prefix(prefix) {
      return stripped.to_string();
    }
  }
  s.to_string()
}

pub fn strip_slash_prefix(s: &str) -> String {
  if let Some((_prefix, rest)) = s.split_once('/') {
    if !rest.is_empty() {
      return rest.to_string();
    }
  }
  s.to_string()
}

pub fn normalize_version_sep(s: &str, aliases: &BTreeMap<String, String>) -> String {
  let bytes = s.as_bytes();
  let mut candidates = Vec::new();
  for i in 1..bytes.len() {
    if bytes[i] == b'-'
      && bytes[i - 1].is_ascii_digit()
      && i + 1 < bytes.len()
      && bytes[i + 1].is_ascii_digit()
    {
      let mut replaced = s.to_string();
      replaced.replace_range(i..i + 1, ".");
      candidates.push(replaced);
    }
  }
  for candidate in &candidates {
    if aliases.contains_key(candidate) {
      return candidate.clone();
    }
  }
  s.to_string()
}
