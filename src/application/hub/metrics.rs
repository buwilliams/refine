//! Reproducible project metrics. Saved aggregates contain no Goal prompts or reports.
use super::*;
use chrono::{DateTime, Duration, Utc};
use std::collections::BTreeMap;
pub const ID: &str = "refine-metrics";
pub const SKILL_ID: &str = "update-metrics-hub";
pub fn site() -> Value {
    json!({"id":ID,"name":"Metrics Hub","description":"Goal delivery, iteration, and usage across this target app.","skill_id":SKILL_ID,"builtin":true,"read_only":true,"publication":{"version":env!("CARGO_PKG_VERSION")}})
}
pub fn asset(name: &str) -> RefineResult<&'static [u8]> {
    match name {
        "index.html" => Ok(include_bytes!("../../surfaces/metrics-hub/index.html")),
        "metrics.css" => Ok(include_bytes!("../../surfaces/metrics-hub/metrics.css")),
        "metrics.js" => Ok(include_bytes!("../../surfaces/metrics-hub/metrics.js")),
        _ => Err(RefineError::NotFound("Metrics Hub asset not found".into())),
    }
}
pub fn manifest() -> Value {
    json!(
        ["index.html", "metrics.css", "metrics.js"]
            .into_iter()
            .map(|name| (
                name,
                json!({"bytes":asset(name).unwrap().len(),"hash":digest(asset(name).unwrap())})
            ))
            .collect::<BTreeMap<_, _>>()
    )
}
impl Hub {
    pub fn metrics_snapshot(&self) -> RefineResult<Value> {
        let path = self.directory().join("sites").join(ID).join("metrics.json");
        if path.exists() {
            read_json(&path)
        } else {
            Ok(json!({"schema_version":1,"generated_at":null}))
        }
    }
    pub fn refresh_metrics(&self) -> RefineResult<Value> {
        with_record_lock(&self.root, &format!("hub-site-{ID}"), || {
            let started = Utc::now();
            let goals = crate::application::work_items::FileWorkItemService::new(&self.root)
                .metrics_goal_records()?;
            let mut value = summarize(&goals, started);
            value["collection_finished_at"] = json!(Utc::now().to_rfc3339());
            value["target_app"] = json!(
                crate::infrastructure::storage::project_layout::target_root_for_refine_dir(
                    &self.root
                )?
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Target app")
            );
            write_json(
                &self.directory().join("sites").join(ID).join("metrics.json"),
                &value,
            )?;
            Ok(value)
        })
    }
}
fn time(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|v| v.with_timezone(&Utc))
}
struct Observation {
    created: Option<DateTime<Utc>>,
    updated: Option<DateTime<Utc>>,
    delivered: Option<DateTime<Utc>>,
    rounds: Vec<Option<DateTime<Utc>>>,
    status: String,
    node: String,
    reporter: String,
}
fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 0 {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}
fn ratio(n: usize, d: usize) -> Option<f64> {
    (d > 0).then(|| n as f64 / d as f64)
}
fn within(value: Option<DateTime<Utc>>, start: DateTime<Utc>, end: DateTime<Utc>) -> bool {
    value.is_some_and(|v| v >= start && v < end)
}
fn period(goals: &[Observation], start: DateTime<Utc>, end: DateTime<Utc>) -> Value {
    let created: Vec<_> = goals
        .iter()
        .filter(|g| within(g.created, start, end))
        .collect();
    let delivered: Vec<_> = goals
        .iter()
        .filter(|g| within(g.delivered, start, end))
        .collect();
    let delivery_hours = delivered
        .iter()
        .filter_map(|g| {
            let (created, delivered) = (g.created?, g.delivered?);
            (delivered >= created).then(|| (delivered - created).num_seconds() as f64 / 3600.0)
        })
        .collect::<Vec<_>>();
    let mut distribution = [0usize; 4];
    for goal in &created {
        distribution[goal.rounds.len().min(3)] += 1;
    }
    let mut nodes = BTreeMap::<String, usize>::new();
    let mut reporters = BTreeMap::<String, usize>::new();
    for goal in goals.iter().filter(|g| within(g.updated, start, end)) {
        *nodes.entry(goal.node.clone()).or_default() += 1;
    }
    for goal in &created {
        *reporters.entry(goal.reporter.clone()).or_default() += 1;
    }
    json!({"goals_created":created.len(),"goals_delivered":delivered.len(),
        "rounds_per_goal":ratio(created.iter().map(|g|g.rounds.len()).sum(),created.len()),
        "median_rounds":median(created.iter().map(|g|g.rounds.len() as f64).collect()),
        "multiple_round_share":ratio(created.iter().filter(|g|g.rounds.len()>1).count(),created.len()),
        "single_round_delivery_share":ratio(delivered.iter().filter(|g|g.rounds.len()==1).count(),delivered.len()),
        "median_delivery_hours":median(delivery_hours.clone()),"delivery_time_samples":delivery_hours.len(),
        "rounds_created":goals.iter().flat_map(|g|&g.rounds).filter(|d|within(**d,start,end)).count(),
        "round_distribution":distribution,"nodes":nodes,"reporters":reporters})
}
pub(crate) fn summarize(records: &[Value], now: DateTime<Utc>) -> Value {
    let goals: Vec<_> = records
        .iter()
        .map(|g| {
            let rounds = g["rounds"].as_array().cloned().unwrap_or_default();
            let status = g["status"].as_str().unwrap_or("unknown").to_string();
            // Only the final Round's durable integration is completion evidence. updated is not a completion timestamp.
            let delivered = (status == "done")
                .then(|| {
                    rounds
                        .last()
                        .and_then(|r| time(&r["workflow_integration"]["integrated_at"]))
                })
                .flatten()
                .filter(|d| *d <= now);
            Observation {
                created: time(&g["created"]),
                updated: time(&g["updated"]),
                delivered,
                rounds: rounds.iter().map(|r| time(&r["created"])).collect(),
                status,
                node: g["node_id"].as_str().unwrap_or("Unassigned").into(),
                reporter: g["reporter"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Unassigned")
                    .into(),
            }
        })
        .collect();
    let mut periods = BTreeMap::new();
    for (key, days, bucket_hours) in [
        ("day", 1, 1),
        ("week", 7, 24),
        ("month", 30, 24),
        ("quarter", 90, 168),
        ("year", 365, 168),
    ] {
        let start = now - Duration::days(days);
        let mut result = period(&goals, start, now);
        result["start"] = json!(start.to_rfc3339());
        result["end"] = json!(now.to_rfc3339());
        result["previous"] = period(&goals, start - Duration::days(days), start);
        let mut buckets = Vec::new();
        let mut cursor = start;
        while cursor < now {
            let end = (cursor + Duration::hours(bucket_hours)).min(now);
            buckets.push(json!({"start":cursor.to_rfc3339(),"end":end.to_rfc3339(),"created":goals.iter().filter(|g|within(g.created,cursor,end)).count(),"delivered":goals.iter().filter(|g|within(g.delivered,cursor,end)).count()}));
            cursor = end;
        }
        result["trend"] = json!(buckets);
        result["bucket_hours"] = json!(bucket_hours);
        periods.insert(key, result);
    }
    let mut statuses = BTreeMap::<String, usize>::new();
    let mut ages = vec![];
    let mut quiet = 0;
    for g in &goals {
        *statuses.entry(g.status.clone()).or_default() += 1;
        if !["done", "cancelled"].contains(&g.status.as_str()) {
            if let Some(created) = g.created.filter(|d| *d <= now) {
                ages.push((now - created).num_seconds() as f64 / 86400.0);
            }
            if g.updated.is_some_and(|d| d < now - Duration::days(7)) {
                quiet += 1;
            }
        }
    }
    json!({"schema_version":1,"generated_at":now.to_rfc3339(),"periods":periods,
        "current":{"goals":goals.len(),"statuses":statuses,"unfinished":goals.iter().filter(|g| !["done","cancelled"].contains(&g.status.as_str())).count(),"median_age_days":median(ages),"quiet_seven_days":quiet},
        "coverage":{"done_without_integration_time":goals.iter().filter(|g|g.status=="done"&&g.delivered.is_none()).count(),"goals_without_created_time":goals.iter().filter(|g|g.created.is_none()).count(),"rounds_without_created_time":goals.iter().flat_map(|g|&g.rounds).filter(|v|v.is_none()).count()}})
}

#[cfg(test)]
mod tests;
