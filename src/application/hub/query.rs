//! Rebuildable bounded indexes. Only declared fields are retained, never full records.
use super::*;
use crate::model::hub::{IndexDefinition, Query};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
static ACTIVE_QUERIES: AtomicUsize = AtomicUsize::new(0);
struct QueryPermit;
impl QueryPermit {
    fn acquire() -> RefineResult<Self> {
        ACTIVE_QUERIES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < 4).then_some(count + 1)
            })
            .map(|_| Self)
            .map_err(|_| RefineError::Degraded("Knowledge Hub query capacity is busy".into()))
    }
}
impl Drop for QueryPermit {
    fn drop(&mut self) {
        ACTIVE_QUERIES.fetch_sub(1, Ordering::AcqRel);
    }
}
const MAX_INDEX_BYTES: usize = 128 * 1024 * 1024;
#[derive(Default)]
struct Index {
    rows: BTreeMap<String, Value>,
    search: BTreeMap<String, BTreeSet<String>>,
    fields: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    bytes: usize,
    generation: String,
    definition: IndexDefinition,
}
type IndexCache = BTreeMap<PathBuf, Arc<Mutex<Index>>>;
static CACHE: OnceLock<Mutex<IndexCache>> = OnceLock::new();
fn cache() -> &'static Mutex<IndexCache> {
    CACHE.get_or_init(Default::default)
}
pub fn invalidate(root: &Path) {
    if let Ok(mut c) = cache().lock() {
        c.retain(|p, _| !p.starts_with(root));
    }
}
fn field<'a>(data: &'a Value, path: &str) -> &'a Value {
    let mut value = data;
    for part in path.split('.') {
        value = &value[part];
    }
    value
}
fn tokens(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect()
}
pub(super) fn validate_data(definition: &IndexDefinition, data: &Value) -> RefineResult<()> {
    for (name, kind) in &definition.fields {
        let value = field(data, name);
        if !value.is_null()
            && !match kind.as_str() {
                "number" => value.is_number(),
                "boolean" => value.is_boolean(),
                "timestamp" => value
                    .as_str()
                    .is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok()),
                _ => value.is_string(),
            }
        {
            return Err(invalid(format!(
                "Field {name} does not match its declared {kind} index"
            )));
        }
    }
    Ok(())
}
fn timestamp(value: &Value) -> RefineResult<Value> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    let date = value
        .as_str()
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .ok_or_else(|| invalid("Timestamp filters require an RFC 3339 value"))?;
    Ok(json!(
        date.with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
    ))
}
// Order-preserving keys keep range reads proportional to their matching records.
// Numeric rounding can admit extra candidates; the typed predicate below remains
// authoritative, so key collisions never change query results.
fn ordered_key(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => {
            let number = number.as_f64()?;
            let bits = if number == 0.0 {
                0.0_f64.to_bits()
            } else {
                number.to_bits()
            };
            let ordered = if bits >> 63 == 1 {
                !bits
            } else {
                bits ^ (1_u64 << 63)
            };
            Some(format!("n{ordered:016x}"))
        }
        Value::String(value) => Some(format!("s{value}")),
        Value::Bool(value) => Some(format!("b{}", u8::from(*value))),
        _ => None,
    }
}
fn indexed_candidates(
    index: &Index,
    filter: &crate::model::hub::Filter,
) -> Option<BTreeSet<String>> {
    use std::ops::Bound::{Excluded, Included};
    let values = index.fields.get(&filter.field)?;
    let key = ordered_key(&filter.value)?;
    if filter.op == "eq" {
        return Some(values.get(&key).cloned().unwrap_or_default());
    }
    let prefix = &key[..1];
    let upper = match prefix {
        "n" => "o",
        "s" => "t",
        "b" => "c",
        _ => return None,
    };
    let bounds = match filter.op.as_str() {
        // Include equivalent numeric keys, then apply the exact predicate.
        "gt" | "gte" => (Included(key.as_str()), Excluded(upper)),
        "lt" | "lte" => (Included(prefix), Included(key.as_str())),
        _ => return None,
    };
    Some(
        values
            .range::<str, _>(bounds)
            .flat_map(|(_, ids)| ids.iter().cloned())
            .collect(),
    )
}
fn row_bytes(key: &str, row: &Value) -> usize {
    let field_bytes = row.as_object().map_or(0, |fields| {
        fields
            .iter()
            .filter(|(name, _)| !["id", "updated", "__tokens"].contains(&name.as_str()))
            .map(|(name, value)| 256 + key.len() + name.len() + value.to_string().len())
            .sum::<usize>()
    });
    field_bytes
        + row.to_string().len().saturating_mul(3)
        + 384
        + key.len()
        + row["__tokens"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|token| 96 + token.len() + key.len())
            .sum::<usize>()
}
fn insert(index: &mut Index, record: &Value) -> RefineResult<()> {
    let key = record["id"]
        .as_str()
        .ok_or_else(|| invalid("Record has no id"))?
        .to_string();
    if let Some(old) = index.rows.remove(&key) {
        for (name, values) in &mut index.fields {
            if let Some(value) = ordered_key(&old[name]) {
                if let Some(ids) = values.get_mut(&value) {
                    ids.remove(&key);
                    if ids.is_empty() {
                        values.remove(&value);
                    }
                }
            }
        }
        index.bytes = index.bytes.saturating_sub(row_bytes(&key, &old));
        if let Some(words) = old["__tokens"].as_array() {
            for token in words.iter().filter_map(Value::as_str) {
                if let Some(ids) = index.search.get_mut(token) {
                    ids.remove(&key);
                    if ids.is_empty() {
                        index.search.remove(token);
                    }
                }
            }
        }
    }
    if record["deleted"] == true {
        return Ok(());
    }
    let mut row = json!({"id":key,"updated":record["updated"]});
    for (name, kind) in &index.definition.fields {
        let v = field(&record["data"], name);
        if !v.is_null()
            && !match kind.as_str() {
                "number" => v.is_number(),
                "boolean" => v.is_boolean(),
                "timestamp" => v
                    .as_str()
                    .is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok()),
                _ => v.is_string(),
            }
        {
            return Err(invalid(format!(
                "Field {name} does not match its declared {kind} index"
            )));
        }
        row[name] = if kind == "timestamp" {
            timestamp(v)?
        } else {
            v.clone()
        };
    }
    let mut words = BTreeSet::new();
    for name in &index.definition.search {
        if let Some(text) = field(&record["data"], name).as_str() {
            for token in tokens(text) {
                words.insert(token.clone());
                index.search.entry(token).or_default().insert(key.clone());
            }
        }
    }
    row["__tokens"] = json!(words);
    index.bytes += row_bytes(&key, &row);
    if index.bytes > MAX_INDEX_BYTES {
        return Err(invalid(
            "Index exceeds the 128 MiB budget; reduce indexed fields or partition the collection",
        ));
    }
    for name in index.definition.fields.keys() {
        if let Some(value) = ordered_key(&row[name]) {
            index
                .fields
                .entry(name.clone())
                .or_default()
                .entry(value)
                .or_default()
                .insert(key.clone());
        }
    }
    index.rows.insert(key, row);
    Ok(())
}
pub(super) fn record_changed(hub: &Hub, site: &str, collection: &str, record: &Value) {
    let Ok(path) = hub.collection(site, collection) else {
        return;
    };
    let handle = cache().lock().ok().and_then(|c| c.get(&path).cloned());
    if let Some(handle) = handle {
        if let Ok(mut index) = handle.lock() {
            if insert(&mut index, record).is_err() {
                drop(index);
                invalidate(&hub.root);
                return;
            }
            index.generation = uuid::Uuid::new_v4().simple().to_string();
        }
    }
    if let Ok(mut cached) = cache().lock() {
        let bytes: usize = cached
            .values()
            .filter_map(|index| index.lock().ok().map(|index| index.bytes))
            .sum();
        if bytes > MAX_INDEX_BYTES {
            cached.clear();
        }
    }
}
fn compare(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a
            .as_f64()
            .partial_cmp(&b.as_f64())
            .unwrap_or(std::cmp::Ordering::Equal),
        (Value::String(a), Value::String(b)) => a.cmp(b),
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        _ => a.to_string().cmp(&b.to_string()),
    }
}
impl Hub {
    pub fn rebuild_index(&self, site: &str, collection: &str) -> RefineResult<Value> {
        let directory = self.collection(site, collection)?;
        // Serialize rebuild with collection schema changes. Record mutations invalidate
        // publication below using their record versions; rebuild is an explicit operation.
        with_record_lock(
            &self.root,
            &format!("hub-collection-{site}-{collection}"),
            || {
                let metadata = self.load_collection(site, collection)?;
                let definition: IndexDefinition =
                    serde_json::from_value(metadata["indexes"].clone())
                        .map_err(|e| invalid(e.to_string()))?;

                let mut index = Index {
                    definition,
                    generation: uuid::Uuid::new_v4().simple().to_string(),
                    ..Default::default()
                };
                let records = directory.join("records");
                let started = Instant::now();
                if records.exists() {
                    for shard in fs::read_dir(records).map_err(io)? {
                        let shard = shard.map_err(io)?;
                        if !shard.file_type().map_err(io)?.is_dir() {
                            continue;
                        }
                        for entry in fs::read_dir(shard.path()).map_err(io)? {
                            let entry = entry.map_err(io)?;
                            if !entry.file_type().map_err(io)?.is_file()
                                || entry.path().extension().and_then(|s| s.to_str()) != Some("json")
                            {
                                continue;
                            }
                            if started.elapsed() > Duration::from_secs(600) {
                                return Err(invalid("Index rebuild exceeded ten minutes"));
                            }
                            let record: Value = read_json(&entry.path())?;
                            insert(&mut index, &record)?;
                        }
                    }
                }
                let result = json!({"records":index.rows.len(),"indexed_bytes":index.bytes,"elapsed_ms":started.elapsed().as_millis(),"generation":index.generation});
                let mut c = cache()
                    .lock()
                    .map_err(|_| invalid("Index cache unavailable"))?;
                let retained: usize = c
                    .values()
                    .filter_map(|i| i.lock().ok().map(|i| i.bytes))
                    .sum();
                if retained + index.bytes > MAX_INDEX_BYTES || c.len() >= 32 {
                    c.clear();
                }
                c.insert(directory, Arc::new(Mutex::new(index)));
                Ok(result)
            },
        )
    }
    pub fn query(&self, site: &str, collection: &str, q: &Query) -> RefineResult<Value> {
        let _permit = QueryPermit::acquire()?;
        self.load_collection(site, collection)?;
        if q.version.is_some_and(|v| v != 1)
            || q.limit == 0
            || q.limit > 1000
            || q.filters.len() > 32
            || q.group_by.len() > 4
            || q.aggregates.len() > 16
            || q.select.len() > 128
            || q.search.as_ref().is_some_and(|text| text.len() > 4096)
        {
            return Err(invalid(
                "Unsupported query version or query bounds (limit 1–1000)",
            ));
        }
        let path = self.collection(site, collection)?;
        if !cache()
            .lock()
            .map_err(|_| invalid("Index cache unavailable"))?
            .contains_key(&path)
        {
            self.rebuild_index(site, collection)?;
        }
        let handle = cache()
            .lock()
            .map_err(|_| invalid("Index cache unavailable"))?
            .get(&path)
            .cloned()
            .ok_or_else(|| invalid("Index was invalidated; rebuild and query again"))?;
        let index = handle.lock().map_err(|_| invalid("Index unavailable"))?;
        let start = Instant::now();
        let grouped = !q.aggregates.is_empty() || !q.group_by.is_empty() || q.time_bucket.is_some();
        let indexed =
            |f: &str| f == "id" || f == "updated" || index.definition.fields.contains_key(f);
        for f in q
            .filters
            .iter()
            .map(|f| f.field.as_str())
            .chain(q.sort.iter().filter(|_| !grouped).map(String::as_str))
            .chain(q.group_by.iter().map(String::as_str))
            .chain(q.aggregates.values().filter_map(|a| a.field.as_deref()))
            .chain(q.time_bucket.iter().map(|t| t.field.as_str()))
        {
            if !indexed(f) {
                return Err(invalid(format!("Declare an index for {f}")));
            }
        }
        let mut normalized = q.clone();
        for filter in &mut normalized.filters {
            if index
                .definition
                .fields
                .get(&filter.field)
                .is_some_and(|kind| kind == "timestamp")
            {
                filter.value = timestamp(&filter.value)?;
            }
        }
        let q = &normalized;
        for filter in &q.filters {
            if let Some(kind) = index.definition.fields.get(&filter.field) {
                let valid = filter.value.is_null()
                    || match kind.as_str() {
                        "number" => filter.value.is_number(),
                        "boolean" => filter.value.is_boolean(),
                        _ => filter.value.is_string(),
                    };
                if !valid {
                    return Err(invalid("Filter value does not match its index type"));
                }
            }
            if !["eq", "ne", "gt", "gte", "lt", "lte"].contains(&filter.op.as_str()) {
                return Err(invalid("Filter operators: eq, ne, gt, gte, lt, lte"));
            }
        }
        if grouped
            && q.sort.as_ref().is_some_and(|sort| {
                !q.group_by.contains(sort)
                    && !q.aggregates.contains_key(sort)
                    && !(sort == "bucket" && q.time_bucket.is_some())
            })
        {
            return Err(invalid(
                "Grouped queries sort by a group field, aggregate name, or time bucket",
            ));
        }
        if q.aggregates
            .keys()
            .any(|name| q.group_by.contains(name) || (name == "bucket" && q.time_bucket.is_some()))
        {
            return Err(invalid(
                "Aggregate names must differ from group fields and the time bucket",
            ));
        }
        if q.time_bucket.as_ref().is_some_and(|bucket| {
            bucket.field != "updated"
                && !index
                    .definition
                    .fields
                    .get(&bucket.field)
                    .is_some_and(|kind| kind == "timestamp")
        }) {
            return Err(invalid("Time buckets require a timestamp index"));
        }
        for a in q.aggregates.values() {
            if !["count", "sum", "min", "max", "average"].contains(&a.op.as_str())
                || (a.op != "count"
                    && !a.field.as_ref().is_some_and(|field| {
                        index
                            .definition
                            .fields
                            .get(field)
                            .is_some_and(|kind| kind == "number")
                    }))
            {
                return Err(invalid(
                    "Aggregate requires count, sum, min, max or average and a numeric field",
                ));
            }
        }
        if q.time_bucket
            .as_ref()
            .is_some_and(|t| t.seconds == 0 || t.seconds > i64::MAX as u64)
        {
            return Err(invalid("Time bucket seconds must be positive"));
        }
        let query_key = digest(
            &serde_json::to_vec(&Query {
                cursor: None,
                ..q.clone()
            })
            .unwrap(),
        );
        let offset = if let Some(cursor) = &q.cursor {
            let p: Vec<_> = cursor.split(':').collect();
            if p.len() != 3 || p[0] != index.generation || p[1] != query_key {
                return Err(RefineError::Conflict(
                    "Query cursor is stale or belongs to another query".into(),
                ));
            }
            p[2].parse::<usize>()
                .map_err(|_| invalid("Invalid cursor"))?
        } else {
            0
        };
        let search_ids = if let Some(text) = &q.search {
            if index.definition.search.is_empty() {
                return Err(invalid("Declare search fields first"));
            }
            let mut sets = tokens(text)
                .into_iter()
                .map(|t| index.search.get(&t).cloned().unwrap_or_default());
            Some(
                sets.next()
                    .map(|first| sets.fold(first, |a, b| a.intersection(&b).cloned().collect()))
                    .unwrap_or_default(),
            )
        } else {
            None
        };
        let candidates = q
            .filters
            .iter()
            .filter_map(|filter| indexed_candidates(&index, filter))
            .chain(search_ids.iter().cloned())
            .min_by_key(BTreeSet::len);
        let rows: Box<dyn Iterator<Item = (&String, &Value)> + '_> = if let Some(ids) = &candidates
        {
            Box::new(ids.iter().filter_map(|id| index.rows.get_key_value(id)))
        } else {
            Box::new(index.rows.iter())
        };
        let mut matched = Vec::new();
        for (position, (id, row)) in rows.enumerate() {
            if position % 256 == 0 && start.elapsed() > Duration::from_secs(2) {
                return Err(invalid("Query exceeded two seconds; narrow its filters"));
            }
            if search_ids.as_ref().is_some_and(|s| !s.contains(id)) {
                continue;
            }
            if q.filters.iter().all(|f| {
                let cmp = compare(&row[&f.field], &f.value);
                match f.op.as_str() {
                    "eq" => row[&f.field] == f.value,
                    "ne" => row[&f.field] != f.value,
                    "gt" => !row[&f.field].is_null() && !f.value.is_null() && cmp.is_gt(),
                    "gte" => !row[&f.field].is_null() && !f.value.is_null() && !cmp.is_lt(),
                    "lt" => !row[&f.field].is_null() && !f.value.is_null() && cmp.is_lt(),
                    "lte" => !row[&f.field].is_null() && !f.value.is_null() && !cmp.is_gt(),
                    _ => false,
                }
            }) {
                matched.push(row);
            }
        }
        let mut rows: Vec<Value>;
        if grouped {
            let mut groups: BTreeMap<
                String,
                (Value, BTreeMap<String, (f64, f64, f64, usize)>, usize),
            > = BTreeMap::new();
            for row in matched {
                if start.elapsed() > Duration::from_secs(2) {
                    return Err(invalid("Aggregation exceeded two seconds"));
                }
                let mut key = json!({});
                for f in &q.group_by {
                    key[f] = row[f].clone();
                }
                if let Some(t) = &q.time_bucket {
                    let timestamp = row[&t.field]
                        .as_str()
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.timestamp());
                    key["bucket"] = timestamp
                        .map(|t0| json!(t0.div_euclid(t.seconds as i64) * t.seconds as i64))
                        .unwrap_or(Value::Null);
                }
                let group = groups
                    .entry(key.to_string())
                    .or_insert((key, BTreeMap::new(), 0));
                group.2 += 1;
                for (name, a) in &q.aggregates {
                    if let Some(v) = a.field.as_ref().and_then(|f| row[f].as_f64()) {
                        let acc = group.1.entry(name.clone()).or_insert((0., v, v, 0));
                        acc.0 += v;
                        acc.1 = acc.1.min(v);
                        acc.2 = acc.2.max(v);
                        acc.3 += 1;
                    }
                }
                if groups.len() > 10000 {
                    return Err(invalid("Aggregation exceeds 10,000 groups"));
                }
            }
            rows = groups
                .into_values()
                .map(|(mut v, acc, count)| {
                    for (name, a) in &q.aggregates {
                        v[name] = if a.op == "count" {
                            json!(count)
                        } else if let Some((sum, min, max, n)) = acc.get(name) {
                            json!(match a.op.as_str() {
                                "sum" => *sum,
                                "min" => *min,
                                "max" => *max,
                                _ => sum / (*n as f64),
                            })
                        } else {
                            Value::Null
                        };
                    }
                    v
                })
                .collect();
        } else {
            if let Some(sort) = &q.sort {
                matched.sort_by(|a, b| {
                    compare(&a[sort], &b[sort]).then_with(|| compare(&a["id"], &b["id"]))
                });
            }
            if q.descending {
                matched.reverse();
            }
            let total = matched.len();
            let mut data = Vec::new();
            let mut response_bytes = 0usize;
            for row in matched.into_iter().skip(offset).take(q.limit) {
                let record = self.get(site, collection, row["id"].as_str().unwrap())?;
                let output = if q.select.is_empty() {
                    record
                } else {
                    let mut projected = json!({});
                    for f in &q.select {
                        projected[f] = field(&record["item"]["data"], f).clone();
                    }
                    json!({"id":row["id"],"data":projected,"revision":record["revision"]})
                };
                response_bytes += output.to_string().len();
                if response_bytes > 4 * 1024 * 1024 - 4096 {
                    return Err(invalid(
                        "Query response exceeds 4 MiB; reduce limit or select fields",
                    ));
                }
                data.push(output);
            }
            let result = json!({"rows":data,"total":total,"next_cursor":(offset+q.limit<total).then(||format!("{}:{query_key}:{}",index.generation,offset+q.limit)),"generation":index.generation});
            if result.to_string().len() > 4 * 1024 * 1024 {
                return Err(invalid(
                    "Query response exceeds 4 MiB; reduce limit or select fields",
                ));
            }
            return Ok(result);
        }
        if let Some(sort) = &q.sort {
            rows.sort_by(|a, b| {
                compare(&a[sort], &b[sort]).then_with(|| a.to_string().cmp(&b.to_string()))
            });
        }
        if q.descending {
            rows.reverse();
        }
        let total = rows.len();
        let mut page = Vec::new();
        let mut response_bytes = 0usize;
        for row in rows.into_iter().skip(offset).take(q.limit) {
            response_bytes += row.to_string().len();
            if response_bytes > 4 * 1024 * 1024 - 4096 {
                return Err(invalid(
                    "Query response exceeds 4 MiB; reduce limit or select fields",
                ));
            }
            page.push(row);
        }
        Ok(
            json!({"rows":page,"total":total,"next_cursor":(offset+q.limit<total).then(||format!("{}:{query_key}:{}",index.generation,offset+q.limit)),"generation":index.generation}),
        )
    }
}
