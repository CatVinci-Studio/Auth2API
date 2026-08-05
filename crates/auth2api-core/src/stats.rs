//! Usage accounting.
//!
//! Every completed request appends one line to `usage.jsonl` in the config
//! dir. An append-only log rather than a running total, because the
//! interesting questions ("when do I actually use this?", "which model ate
//! the tokens?") are all slices of the raw events, and because a log survives
//! a crash mid-write with at most one lost line.
//!
//! On cost: a ChatGPT subscription is a flat monthly fee, so there is no
//! per-request charge to report. What `estimated_cost_usd` reports is the
//! equivalent list price - what these same tokens would have cost through an
//! API key - which is the number that tells you whether the subscription is
//! paying for itself. It is not money that was spent, and it is only
//! populated for models you have priced in `config.toml`.

use chrono::{DateTime, Datelike, Local, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Record {
    /// Unix seconds.
    pub ts: i64,
    pub endpoint: String,
    pub model: String,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Which local API key made the request. Only the id and name are ever
    /// written here - never the secret, because this log is far less guarded
    /// than `keys.json` and is the file a user pastes into a bug report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_name: Option<String>,
}

/// USD per 1M tokens, per model. Empty by default - see the module note.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ModelPricing {
    #[serde(default)]
    pub input: f64,
    /// Falls back to `input` when unset, which is the conservative direction
    /// (never understates the equivalent cost).
    #[serde(default)]
    pub cached_input: Option<f64>,
    #[serde(default)]
    pub output: f64,
}

pub type PricingTable = BTreeMap<String, ModelPricing>;

impl ModelPricing {
    fn cost(&self, record: &Record) -> f64 {
        let cached = record.cached_tokens.min(record.prompt_tokens);
        let fresh = record.prompt_tokens - cached;
        let cached_rate = self.cached_input.unwrap_or(self.input);
        (fresh as f64 * self.input
            + cached as f64 * cached_rate
            + record.completion_tokens as f64 * self.output)
            / 1_000_000.0
    }
}

pub fn log_path() -> Result<PathBuf, String> {
    Ok(crate::config::config_dir()?.join("usage.jsonl"))
}

/// Serializes appends. Two concurrent requests finishing at once would
/// otherwise be free to interleave their bytes and corrupt both lines.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

pub fn append(record: &Record) {
    let Ok(path) = log_path() else { return };
    let Ok(line) = serde_json::to_string(record) else {
        return;
    };
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Accounting must never take a request down with it - a full disk should
    // cost a log line, not the user's answer.
    if let Err(e) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| writeln!(f, "{line}"))
    {
        tracing::warn!("could not record usage: {e}");
    }
}

pub fn read_all() -> Result<Vec<Record>, String> {
    let path = log_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    // A partially written trailing line (killed mid-append) is skipped rather
    // than failing the whole report.
    Ok(text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

pub fn reset() -> Result<bool, String> {
    let path = log_path()?;
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&path).map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
    Ok(true)
}

// --- aggregation ----------------------------------------------------------

#[derive(Serialize, Default, Debug, Clone)]
pub struct Totals {
    pub requests: u64,
    pub failed: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cached_tokens: u64,
    pub reasoning_tokens: u64,
    /// Null when none of the models involved have a price configured.
    pub estimated_cost_usd: Option<f64>,
    /// Models seen here that have no entry in the pricing table, so the caller
    /// knows the cost above is partial rather than complete.
    pub unpriced_models: Vec<String>,
}

#[derive(Serialize, Default, Debug, Clone)]
pub struct Bucket {
    pub key: String,
    pub requests: u64,
    pub total_tokens: u64,
    pub estimated_cost_usd: Option<f64>,
    /// Set on per-key buckets only: the human name and the last time this key
    /// was seen. Derived here rather than stamped into `keys.json` on every
    /// request, which would mean a file write per API call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct Report {
    pub totals: Totals,
    pub by_model: Vec<Bucket>,
    /// One entry per local API key that has been used, busiest first.
    pub by_key: Vec<Bucket>,
    /// One entry per calendar hour that had traffic, oldest first, local time.
    pub by_hour: Vec<Bucket>,
    /// Requests per hour-of-day (0-23) across the whole log - the "when do I
    /// actually use this" view, which the timeline above cannot show.
    pub by_hour_of_day: Vec<Bucket>,
    pub by_day: Vec<Bucket>,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    /// Echoed back so a caller reading only this report can tell whether the
    /// window it describes is what it asked for.
    pub window_hours: Option<i64>,
}

fn local(ts: i64) -> DateTime<Local> {
    Local.timestamp_opt(ts, 0).single().unwrap_or_default()
}

#[derive(Default)]
struct Accumulator {
    requests: u64,
    tokens: u64,
    cost: f64,
    priced: bool,
    label: Option<String>,
    last_ts: i64,
}

impl Accumulator {
    fn add(&mut self, record: &Record, cost: Option<f64>) {
        self.requests += 1;
        self.tokens += record.total_tokens;
        self.last_ts = self.last_ts.max(record.ts);
        if let Some(cost) = cost {
            self.cost += cost;
            self.priced = true;
        }
    }
    /// Time buckets carry no identity, so they leave `label`/`last_used`
    /// unset rather than repeating a timestamp already implied by the key.
    fn into_bucket(self, key: String) -> Bucket {
        Bucket {
            key,
            requests: self.requests,
            total_tokens: self.tokens,
            estimated_cost_usd: self.priced.then_some(self.cost),
            label: None,
            last_used: None,
        }
    }

    fn into_identified_bucket(self, key: String, label: Option<String>) -> Bucket {
        let last_used = (self.last_ts > 0).then(|| local(self.last_ts).to_rfc3339());
        Bucket {
            label,
            last_used,
            ..self.into_bucket(key)
        }
    }
}

/// Builds the report over the last `window_hours` (None = the whole log).
pub fn report(records: &[Record], pricing: &PricingTable, window_hours: Option<i64>) -> Report {
    let cutoff = window_hours.map(|h| Local::now().timestamp() - h * 3600);
    let records: Vec<&Record> = records
        .iter()
        .filter(|r| cutoff.is_none_or(|c| r.ts >= c))
        .collect();

    let mut totals = Totals::default();
    let mut cost_total = 0.0;
    let mut any_priced = false;
    let mut unpriced: std::collections::BTreeSet<String> = Default::default();

    let mut by_model: BTreeMap<String, Accumulator> = BTreeMap::new();
    let mut by_key: BTreeMap<String, Accumulator> = BTreeMap::new();
    let mut by_hour: BTreeMap<String, Accumulator> = BTreeMap::new();
    let mut by_day: BTreeMap<String, Accumulator> = BTreeMap::new();
    let mut by_hour_of_day: BTreeMap<u32, Accumulator> = BTreeMap::new();

    for record in &records {
        let cost = match pricing.get(&record.model) {
            Some(p) => {
                any_priced = true;
                let c = p.cost(record);
                cost_total += c;
                Some(c)
            }
            None => {
                if !record.model.is_empty() {
                    unpriced.insert(record.model.clone());
                }
                None
            }
        };

        totals.requests += 1;
        if !record.ok {
            totals.failed += 1;
        }
        totals.prompt_tokens += record.prompt_tokens;
        totals.completion_tokens += record.completion_tokens;
        totals.total_tokens += record.total_tokens;
        totals.cached_tokens += record.cached_tokens;
        totals.reasoning_tokens += record.reasoning_tokens;

        let at = local(record.ts);
        by_model.entry(record.model.clone()).or_default().add(record, cost);

        // Requests made before any key existed (or on a loopback server with
        // none configured) still need somewhere to land, or the per-key view
        // silently omits traffic the totals include.
        let key_id = record.key_id.clone().unwrap_or_else(|| "(no key)".to_string());
        let entry = by_key.entry(key_id).or_default();
        // The name is taken from the most recent request that carried one, so
        // renaming a key updates its label without rewriting history.
        if record.key_name.is_some() {
            entry.label = record.key_name.clone();
        }
        entry.add(record, cost);

        by_hour
            .entry(format!(
                "{:04}-{:02}-{:02} {:02}:00",
                at.year(),
                at.month(),
                at.day(),
                at.hour()
            ))
            .or_default()
            .add(record, cost);
        by_day
            .entry(format!("{:04}-{:02}-{:02}", at.year(), at.month(), at.day()))
            .or_default()
            .add(record, cost);
        by_hour_of_day.entry(at.hour()).or_default().add(record, cost);
    }

    totals.estimated_cost_usd = any_priced.then_some(cost_total);
    totals.unpriced_models = unpriced.into_iter().collect();

    // Hours with no traffic are still meaningful in the hour-of-day view -
    // a gap reads as "never used at 4am", which is the point of the chart.
    let hour_of_day = (0..24)
        .map(|hour| {
            by_hour_of_day
                .remove(&hour)
                .unwrap_or_default()
                .into_bucket(format!("{hour:02}"))
        })
        .collect();

    let mut keys: Vec<Bucket> = by_key
        .into_iter()
        .map(|(id, acc)| {
            let label = acc.label.clone();
            acc.into_identified_bucket(id, label)
        })
        .collect();
    keys.sort_by(|a, b| b.requests.cmp(&a.requests).then_with(|| a.key.cmp(&b.key)));

    Report {
        totals,
        by_key: keys,
        by_model: by_model.into_iter().map(|(k, v)| v.into_bucket(k)).collect(),
        by_hour: by_hour.into_iter().map(|(k, v)| v.into_bucket(k)).collect(),
        by_hour_of_day: hour_of_day,
        by_day: by_day.into_iter().map(|(k, v)| v.into_bucket(k)).collect(),
        first_seen: records.first().map(|r| local(r.ts).to_rfc3339()),
        last_seen: records.last().map(|r| local(r.ts).to_rfc3339()),
        window_hours,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(ts: i64, model: &str, prompt: u64, completion: u64) -> Record {
        Record {
            ts,
            endpoint: "/v1/chat/completions".into(),
            model: model.into(),
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            cached_tokens: 0,
            reasoning_tokens: 0,
            stream: false,
            ok: true,
            duration_ms: 10,
            error: None,
            key_id: None,
            key_name: None,
        }
    }

    fn by_key(records: &[Record]) -> Vec<Bucket> {
        report(records, &PricingTable::new(), None).by_key
    }

    fn pricing(model: &str, input: f64, output: f64) -> PricingTable {
        let mut table = PricingTable::new();
        table.insert(
            model.into(),
            ModelPricing {
                input,
                cached_input: None,
                output,
            },
        );
        table
    }

    #[test]
    fn totals_sum_across_records() {
        let records = vec![record(1, "m", 100, 50), record(2, "m", 10, 5)];
        let report = report(&records, &PricingTable::new(), None);
        assert_eq!(report.totals.requests, 2);
        assert_eq!(report.totals.prompt_tokens, 110);
        assert_eq!(report.totals.total_tokens, 165);
    }

    /// The distinction the module note is about: with nothing priced, the
    /// cost must be absent, not a confident-looking zero.
    #[test]
    fn cost_is_absent_rather_than_zero_when_nothing_is_priced() {
        let report = report(&[record(1, "m", 100, 50)], &PricingTable::new(), None);
        assert_eq!(report.totals.estimated_cost_usd, None);
        assert_eq!(report.totals.unpriced_models, vec!["m".to_string()]);
    }

    #[test]
    fn cost_uses_per_million_token_rates() {
        let report = report(&[record(1, "m", 1_000_000, 500_000)], &pricing("m", 2.0, 8.0), None);
        // 1M input @ $2 + 0.5M output @ $8 = $6
        assert_eq!(report.totals.estimated_cost_usd, Some(6.0));
    }

    /// A partially priced log must still say so, or the cost silently
    /// undercounts every model the user forgot to price.
    #[test]
    fn a_partially_priced_log_reports_the_gap() {
        let records = vec![record(1, "priced", 1_000_000, 0), record(2, "other", 999, 999)];
        let report = report(&records, &pricing("priced", 1.0, 1.0), None);
        assert_eq!(report.totals.estimated_cost_usd, Some(1.0));
        assert_eq!(report.totals.unpriced_models, vec!["other".to_string()]);
    }

    #[test]
    fn cached_input_is_billed_at_its_own_rate_and_never_double_counted() {
        let mut table = PricingTable::new();
        table.insert(
            "m".into(),
            ModelPricing {
                input: 10.0,
                cached_input: Some(1.0),
                output: 0.0,
            },
        );
        let mut r = record(1, "m", 1_000_000, 0);
        r.cached_tokens = 900_000;
        // 100k fresh @ $10/M + 900k cached @ $1/M = $1 + $0.9
        let report = report(&[r], &table, None);
        assert_eq!(report.totals.estimated_cost_usd, Some(1.9));
    }

    #[test]
    fn hour_of_day_covers_all_twenty_four_slots() {
        let report = report(&[record(1, "m", 1, 1)], &PricingTable::new(), None);
        assert_eq!(report.by_hour_of_day.len(), 24);
        assert_eq!(report.by_hour_of_day.iter().map(|b| b.requests).sum::<u64>(), 1);
    }

    #[test]
    fn a_window_excludes_older_records() {
        let now = Local::now().timestamp();
        let records = vec![record(now - 10 * 3600, "m", 1, 1), record(now - 60, "m", 2, 2)];
        let report = report(&records, &PricingTable::new(), Some(1));
        assert_eq!(report.totals.requests, 1);
        assert_eq!(report.totals.prompt_tokens, 2);
    }

    #[test]
    fn usage_splits_by_key_busiest_first() {
        let mut quiet = record(1, "m", 1, 1);
        quiet.key_id = Some("k_quiet".into());
        quiet.key_name = Some("laptop".into());
        let mut busy_a = record(2, "m", 1, 1);
        busy_a.key_id = Some("k_busy".into());
        busy_a.key_name = Some("editor".into());
        let busy_b = busy_a.clone();

        let buckets = by_key(&[quiet, busy_a, busy_b]);
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[0].key, "k_busy");
        assert_eq!(buckets[0].requests, 2);
        assert_eq!(buckets[0].label.as_deref(), Some("editor"));
        assert_eq!(buckets[1].key, "k_quiet");
    }

    /// Traffic from before any key existed must still appear somewhere, or
    /// the per-key view quietly disagrees with the totals.
    #[test]
    fn keyless_requests_land_in_their_own_bucket() {
        let buckets = by_key(&[record(1, "m", 1, 1)]);
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].key, "(no key)");
    }

    /// Renaming a key should relabel its whole history rather than splitting
    /// it in two.
    #[test]
    fn a_renamed_key_keeps_one_bucket_under_its_newest_name() {
        let mut old = record(1, "m", 1, 1);
        old.key_id = Some("k_1".into());
        old.key_name = Some("old name".into());
        let mut new = record(2, "m", 1, 1);
        new.key_id = Some("k_1".into());
        new.key_name = Some("new name".into());

        let buckets = by_key(&[old, new]);
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].label.as_deref(), Some("new name"));
        assert!(buckets[0].last_used.is_some());
    }

    #[test]
    fn failed_requests_are_counted_separately() {
        let mut bad = record(1, "m", 0, 0);
        bad.ok = false;
        let report = report(&[record(1, "m", 1, 1), bad], &PricingTable::new(), None);
        assert_eq!(report.totals.requests, 2);
        assert_eq!(report.totals.failed, 1);
    }
}
