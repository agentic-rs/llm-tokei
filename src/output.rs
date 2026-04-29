use crate::model::Summary;

pub fn json(summary: &Summary) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(summary)?)
}

pub fn table(summary: &Summary) -> String {
    let mut rows = vec![vec![
        "Provider".to_string(),
        "Sessions".to_string(),
        "Messages".to_string(),
        "Input".to_string(),
        "Output".to_string(),
        "Cache Create".to_string(),
        "Cache Read".to_string(),
        "Total".to_string(),
    ]];

    rows.push(vec![
        summary.provider.clone(),
        summary.totals.sessions.to_string(),
        summary.totals.messages.to_string(),
        summary.totals.usage.input_tokens.to_string(),
        summary.totals.usage.output_tokens.to_string(),
        summary.totals.usage.cache_creation_tokens.to_string(),
        summary.totals.usage.cache_read_tokens.to_string(),
        summary.totals.total_tokens.to_string(),
    ]);

    format_rows(rows)
}

fn format_rows(rows: Vec<Vec<String>>) -> String {
    let widths = (0..rows[0].len())
        .map(|column| rows.iter().map(|row| row[column].len()).max().unwrap_or(0))
        .collect::<Vec<_>>();

    rows.into_iter()
        .map(|row| {
            row.into_iter()
                .enumerate()
                .map(|(index, value)| format!("{value:<width$}", width = widths[index]))
                .collect::<Vec<_>>()
                .join("  ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use crate::{model::Summary, output::table};

    #[test]
    fn table_includes_totals() {
        let summary = Summary::from_records("opencode", vec![]);
        let output = table(&summary);

        assert!(output.contains("Provider"));
        assert!(output.contains("opencode"));
    }
}
