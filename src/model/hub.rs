//! Flat JSON collections and bounded, portable query contracts.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    #[serde(default)]
    pub version: Option<u32>,
    #[serde(default)]
    pub filters: Vec<Filter>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub descending: bool,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default = "page_size")]
    pub limit: usize,
    #[serde(default)]
    pub select: Vec<String>,
    #[serde(default)]
    pub group_by: Vec<String>,
    #[serde(default)]
    pub aggregates: BTreeMap<String, Aggregate>,
    #[serde(default)]
    pub time_bucket: Option<TimeBucket>,
}
fn page_size() -> usize {
    100
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub field: String,
    pub op: String,
    pub value: Value,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Aggregate {
    pub op: String,
    #[serde(default)]
    pub field: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimeBucket {
    pub field: String,
    pub seconds: u64,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IndexDefinition {
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    #[serde(default)]
    pub search: Vec<String>,
}

impl Default for Query {
    fn default() -> Self {
        serde_json::from_value(serde_json::json!({})).expect("default query")
    }
}
