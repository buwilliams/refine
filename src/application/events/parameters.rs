//! Typed Skill input resolution.
use crate::error::{RefineError, RefineResult};
use crate::model::automation::*;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub fn resolve_parameters(
    parameters: &[Parameter],
    explicit: &BTreeMap<String, Value>,
    mapped: &BTreeMap<String, Value>,
) -> RefineResult<BTreeMap<String, Value>> {
    let names: BTreeSet<_> = parameters.iter().map(|p| p.name.as_str()).collect();
    if let Some(name) = explicit.keys().find(|k| !names.contains(k.as_str())) {
        return Err(RefineError::InvalidInput(format!(
            "unknown parameter: {name}"
        )));
    }
    let mut values = BTreeMap::new();
    let mut missing = Vec::new();
    for p in parameters {
        match explicit
            .get(&p.name)
            .or_else(|| mapped.get(&p.name))
            .or(p.default.as_ref())
        {
            Some(value)
                if p.accepts(value)
                    && !(p.required && value.as_str().is_some_and(|v| v.trim().is_empty())) =>
            {
                values.insert(p.name.clone(), value.clone());
            }
            Some(_) => {
                return Err(RefineError::InvalidInput(format!(
                    "invalid parameter: {}",
                    p.name
                )));
            }
            None if p.required => missing.push(p.name.clone()),
            None => {}
        }
    }
    if !missing.is_empty() {
        return Err(RefineError::InvalidInput(format!(
            "missing required parameters: {}",
            missing.join(", ")
        )));
    }
    Ok(values)
}

pub(super) fn field<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(value, |v, key| {
        if v.is_array() {
            v.get(key.parse::<usize>().ok()?)
        } else {
            v.get(key)
        }
    })
}
