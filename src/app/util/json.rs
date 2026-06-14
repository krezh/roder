//! JSON extraction helpers for the describe-style Info view: scalars, labels,
//! conditions, owner refs, RBAC rules, and ConfigMap/Secret data.

use serde_json::Value;

pub(crate) struct Cond {
    pub(crate) type_: String,
    pub(crate) status: String,
    pub(crate) reason: String,
    pub(crate) message: String,
}

pub(crate) fn conditions(v: &Value) -> Vec<Cond> {
    v.get("status")
        .and_then(|s| s.get("conditions"))
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .map(|c| Cond {
                    type_: c
                        .get("type")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    status: c
                        .get("status")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    reason: c
                        .get("reason")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    message: c
                        .get("message")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Scalar fields under `status` (excluding conditions) as key/value pairs.
pub(crate) fn status_scalars(v: &Value) -> Vec<(String, String)> {
    section_scalars(v, "status")
}

/// Top-level scalar fields of a section (`spec`/`status`) as label/value pairs —
/// the generic, works-for-any-resource part of the Info view. Nested objects and
/// arrays are skipped (they'd need bespoke rendering); `conditions` is handled
/// separately. Empty strings/objects are dropped to avoid noise.
pub(crate) fn section_scalars(v: &Value, section: &str) -> Vec<(String, String)> {
    v.get(section)
        .and_then(|s| s.as_object())
        .map(|m| {
            m.iter()
                .filter(|(k, _)| k.as_str() != "conditions")
                .filter_map(|(k, val)| {
                    let s = match val {
                        Value::String(s) if !s.is_empty() => s.clone(),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        _ => return None,
                    };
                    Some((k.clone(), s))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `(Kind, name)` owner references — the resource's controllers ("Controlled By").
pub(crate) fn owner_refs(v: &Value) -> Vec<(String, String)> {
    v.get("metadata")
        .and_then(|m| m.get("ownerReferences"))
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    let k = r.get("kind").and_then(|v| v.as_str())?;
                    let n = r.get("name").and_then(|v| v.as_str())?;
                    Some((k.to_string(), n.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn json_str(v: &Value, path: &[&str]) -> Option<String> {
    let mut cur = v;
    for k in path {
        cur = cur.get(k)?;
    }
    match cur {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

pub(crate) fn json_map(v: &Value, path: &[&str]) -> Vec<(String, String)> {
    let mut cur = v;
    for k in path {
        match cur.get(k) {
            Some(c) => cur = c,
            None => return vec![],
        }
    }
    cur.as_object()
        .map(|m| {
            m.iter()
                .map(|(k, val)| {
                    let s = match val {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    (k.clone(), s)
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) struct PolicyRule {
    pub(crate) groups: String,
    pub(crate) resources: String,
    pub(crate) verbs: String,
    pub(crate) names: String,
}

/// Flatten a Role/ClusterRole's `rules[]` into displayable rows (core group shown
/// as "core"; non-resource URLs surfaced in the names column).
pub(crate) fn rbac_rules(o: &Value) -> Vec<PolicyRule> {
    let Some(arr) = o.get("rules").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .map(|rule| {
            let join = |key: &str| {
                rule.get(key)
                    .and_then(|a| a.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default()
            };
            let groups = rule
                .get("apiGroups")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .map(|g| if g.is_empty() { "core" } else { g })
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let resources = join("resources");
            let urls = join("nonResourceURLs");
            let names = if urls.is_empty() {
                join("resourceNames")
            } else {
                urls
            };
            let resources = if resources.is_empty() && !names.is_empty() {
                "(non-resource)".to_string()
            } else {
                resources
            };
            PolicyRule {
                groups,
                resources,
                verbs: join("verbs"),
                names,
            }
        })
        .collect()
}

/// Entries for a ConfigMap/Secret `data` (+ ConfigMap `binaryData`). For secrets
/// the base64 values are decoded; the bool marks whether the value is sensitive.
pub(crate) fn data_entries(o: &Value, is_secret: bool) -> Vec<(String, String, bool)> {
    let mut out: Vec<(String, String, bool)> = Vec::new();
    if let Some(map) = o.get("data").and_then(|d| d.as_object()) {
        for (k, val) in map {
            let raw = val.as_str().unwrap_or("");
            let value = if is_secret {
                decode_secret(raw)
            } else {
                raw.to_string()
            };
            out.push((k.clone(), value, is_secret));
        }
    }
    if !is_secret {
        if let Some(map) = o.get("binaryData").and_then(|d| d.as_object()) {
            for (k, _) in map {
                out.push((k.clone(), "(binary data)".to_string(), false));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Decode a base64 secret value to UTF-8, or describe it if it's binary.
fn decode_secret(raw: &str) -> String {
    use base64::Engine as _;
    match base64::engine::general_purpose::STANDARD.decode(raw) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => format!("({} bytes binary)", e.into_bytes().len()),
        },
        Err(_) => raw.to_string(),
    }
}

/// Extract container images from a Pod spec (direct or via template).
pub(crate) fn container_images(o: &Value) -> Vec<(String, String)> {
    let spec = o
        .get("spec")
        .and_then(|s| s.get("template"))
        .and_then(|t| t.get("spec"))
        .or_else(|| o.get("spec"));
    let Some(spec) = spec else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, label) in [("containers", ""), ("initContainers", "(init) ")] {
        if let Some(arr) = spec.get(key).and_then(|c| c.as_array()) {
            for c in arr {
                let name = c
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("?")
                    .to_string();
                let image = c
                    .get("image")
                    .and_then(|i| i.as_str())
                    .unwrap_or("?")
                    .to_string();
                out.push((format!("{label}{name}"), image));
            }
        }
    }
    out
}

/// Build a `k=v,k2=v2` label selector from a workload's `spec.selector.matchLabels`.
pub(crate) fn selector_from(o: &Value) -> String {
    o.get("spec")
        .and_then(|s| s.get("selector"))
        .and_then(|sel| sel.get("matchLabels"))
        .and_then(|m| m.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| format!("{k}={v}")))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default()
}
