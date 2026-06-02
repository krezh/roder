//! Small JSON-value accessors shared by the per-kind projectors.

use serde_json::Value;

pub(crate) fn str_at(v: &Value, path: &[&str]) -> Option<String> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_str().map(|s| s.to_string())
}

pub(crate) fn int_at(v: &Value, path: &[&str]) -> Option<i64> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_i64()
}

/// Read an IntOrString field (e.g. PDB minAvailable, which may be `2` or `"50%"`).
pub(crate) fn intstr_at(v: &Value, path: &[&str]) -> Option<String> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    match cur {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Number of keys across the given top-level object fields (e.g. data + binaryData).
pub(crate) fn data_count(data: &Value, fields: &[&str]) -> usize {
    fields
        .iter()
        .filter_map(|f| data.get(*f).and_then(|d| d.as_object()).map(|m| m.len()))
        .sum()
}

/// Serialize a k8s `Time` (or anything serde-stringy) to its RFC3339 string,
/// without depending on k8s-openapi's inner time representation.
pub(crate) fn ts_string<T: serde::Serialize>(t: &T) -> Option<String> {
    serde_json::to_value(t)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
}
