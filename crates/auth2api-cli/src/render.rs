//! Terminal rendering for the usage report and the key list.

use auth2api_core::keys::ApiKey;
use auth2api_core::stats::Report;

pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn bar(value: u64, max: u64, width: usize) -> String {
    if max == 0 {
        return String::new();
    }
    // Anything non-zero gets at least one block, so a quiet hour is visibly
    // different from an empty one.
    let filled = ((value as f64 / max as f64) * width as f64).round() as usize;
    let filled = if value > 0 { filled.max(1) } else { 0 };
    "█".repeat(filled)
}

fn cost_column(cost: Option<f64>) -> String {
    cost.map(|c| format!("   ${c:.4}")).unwrap_or_default()
}

pub fn key_table(keys: &[ApiKey], report: &Report) {
    println!("ID           NAME               KEY                   TOKENS       REQ  LAST USED");
    for key in keys {
        let usage = report.by_key.iter().find(|b| b.key == key.id);
        println!(
            "{:<12} {:<18} {:<16} {:>12}  {:>8}  {}{}",
            key.id,
            // A long name would otherwise push every following column out of
            // alignment and make the table unreadable.
            truncate(&key.name, 18),
            key.masked(),
            usage.map(|u| thousands(u.total_tokens)).unwrap_or_else(|| "0".into()),
            usage.map(|u| thousands(u.requests)).unwrap_or_else(|| "0".into()),
            usage
                .and_then(|u| u.last_used.as_deref())
                .map(short_time)
                .unwrap_or_else(|| "never".into()),
            if key.revoked { "   (revoked)" } else { "" },
        );
    }

    // Traffic from before any key existed is real traffic; leaving it out of
    // this table without a word would make the totals look wrong.
    if let Some(anon) = report.by_key.iter().find(|b| b.key == "(no key)") {
        println!(
            "\n{} token(s) over {} request(s) were served before any key existed, or with none required.",
            thousands(anon.total_tokens),
            thousands(anon.requests)
        );
    }
}

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    format!("{}…", s.chars().take(width - 1).collect::<String>())
}

/// Trims an RFC3339 timestamp to the minute, which is all this table has room
/// for and all anyone reads off it.
fn short_time(rfc3339: &str) -> String {
    rfc3339.get(0..16).unwrap_or(rfc3339).replace('T', " ")
}

pub fn report(report: &Report, hours: Option<i64>) {
    let scope = match hours {
        Some(h) => format!("last {h}h"),
        None => "all time".to_string(),
    };
    let t = &report.totals;
    println!("Usage - {scope}\n");
    println!("  requests        {}  ({} failed)", thousands(t.requests), t.failed);
    println!(
        "  input tokens    {}  ({} cached)",
        thousands(t.prompt_tokens),
        thousands(t.cached_tokens)
    );
    println!(
        "  output tokens   {}  ({} reasoning)",
        thousands(t.completion_tokens),
        thousands(t.reasoning_tokens)
    );
    println!("  total tokens    {}", thousands(t.total_tokens));
    match t.estimated_cost_usd {
        Some(cost) => {
            println!("  equivalent cost ${cost:.4}  (list price if this had been billed per token;");
            println!("                  a subscription charges a flat fee, so this is a comparison)");
            if !t.unpriced_models.is_empty() {
                println!("                  excludes unpriced: {}", t.unpriced_models.join(", "));
            }
        }
        None => println!("  equivalent cost -  (no prices configured; see `auth2api config init`)"),
    }

    println!("\n  by model");
    for bucket in &report.by_model {
        println!(
            "    {:<18} {:>12} tok   {:>7} req{}",
            truncate(&bucket.key, 18),
            thousands(bucket.total_tokens),
            thousands(bucket.requests),
            cost_column(bucket.estimated_cost_usd)
        );
    }

    if !report.by_key.is_empty() {
        println!("\n  by key");
        for bucket in &report.by_key {
            let label = bucket.label.clone().unwrap_or_else(|| bucket.key.clone());
            println!(
                "    {:<18} {:>12} tok   {:>7} req{}",
                truncate(&label, 18),
                thousands(bucket.total_tokens),
                thousands(bucket.requests),
                cost_column(bucket.estimated_cost_usd)
            );
        }
    }

    let max = report
        .by_hour_of_day
        .iter()
        .map(|b| b.total_tokens)
        .max()
        .unwrap_or(0);
    println!("\n  by hour of day (local time, tokens)");
    for bucket in &report.by_hour_of_day {
        println!(
            "    {}:00 {:>10} {}",
            bucket.key,
            thousands(bucket.total_tokens),
            bar(bucket.total_tokens, max, 36)
        );
    }

    let recent: Vec<_> = report.by_day.iter().rev().take(14).rev().collect();
    if recent.len() > 1 {
        let max = recent.iter().map(|b| b.total_tokens).max().unwrap_or(0);
        println!("\n  by day (most recent {}, tokens)", recent.len());
        for bucket in recent {
            println!(
                "    {} {:>10} {}",
                bucket.key,
                thousands(bucket.total_tokens),
                bar(bucket.total_tokens, max, 36)
            );
        }
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    /// A single request in an hour must still draw something, or a quiet hour
    /// is indistinguishable from an unused one.
    #[test]
    fn a_nonzero_value_always_draws_at_least_one_block() {
        assert_eq!(bar(0, 100, 40), "");
        assert_eq!(bar(1, 100_000, 40).chars().count(), 1);
        assert_eq!(bar(100, 100, 40).chars().count(), 40);
    }

    /// Truncation counts characters, not bytes - slicing a multi-byte name
    /// mid-character would panic.
    #[test]
    fn truncation_is_character_wise() {
        assert_eq!(truncate("short", 18), "short");
        assert_eq!(truncate("中文名字很长很长很长", 5), "中文名字…");
    }

    #[test]
    fn timestamps_shorten_to_the_minute() {
        assert_eq!(short_time("2026-08-05T14:32:11+08:00"), "2026-08-05 14:32");
    }
}
