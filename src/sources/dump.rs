#[derive(Debug, Clone, serde::Serialize)]
pub struct DumpRecord {
  pub role: &'static str,
  pub text: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub encrypted_text: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub display: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub call_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DumpedSession {
  pub session_id: String,
  pub records: Vec<DumpRecord>,
}
