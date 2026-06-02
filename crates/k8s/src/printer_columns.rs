//! Generic CRD columns, the way `kubectl get` and k9s do it: every
//! `CustomResourceDefinition` declares `additionalPrinterColumns` (a `name` + a
//! `jsonPath` into the object). We harvest those once from the installed CRDs and
//! evaluate the jsonPaths against objects already in the informer cache — so any
//! CRD gets meaningful columns with no per-kind code, and without a single extra
//! API-server LIST (the watch we already hold feeds it).

use std::collections::HashMap;

use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::api::{Api, ListParams};
use kube::Client;
use serde_json::Value;

/// One declared printer column from a CRD.
#[derive(Clone, Debug)]
pub struct PrinterCol {
    pub name: String,
    pub json_path: String,
    /// OpenAPI column type: string | integer | number | boolean | date.
    pub col_type: String,
}

/// Printer columns indexed by (group, kind).
pub type ColumnMap = HashMap<(String, String), Vec<PrinterCol>>;

/// Borrow the columns for a (group, kind), or an empty slice if the kind isn't a
/// CRD (built-ins have no declared columns — they use the hand-written projectors).
pub fn cols_for<'a>(map: &'a ColumnMap, group: &str, kind: &str) -> &'a [PrinterCol] {
    map.get(&(group.to_string(), kind.to_string()))
        .map(|v| v.as_slice())
        .unwrap_or(&[])
}

/// Fetch every CRD and index its served printer columns by (group, kind). We keep
/// `priority > 0` ("wide") columns — k9s shows them by default, unlike `kubectl get`
/// — and only drop the redundant Age column (roder renders Age itself). A failure
/// (e.g. no RBAC to list CRDs) just yields an empty map → hand-written columns only.
pub async fn load(client: &Client) -> ColumnMap {
    let api: Api<CustomResourceDefinition> = Api::all(client.clone());
    let crds = match api.list(&ListParams::default()).await {
        Ok(l) => l.items,
        Err(e) => {
            tracing::debug!("CRD printer-column load skipped: {e}");
            return ColumnMap::new();
        }
    };

    let mut map = ColumnMap::new();
    for crd in crds {
        let group = crd.spec.group;
        let kind = crd.spec.names.kind;
        // Prefer the storage version's columns; fall back to the first served one.
        let version = crd
            .spec
            .versions
            .iter()
            .find(|v| v.storage)
            .or_else(|| crd.spec.versions.iter().find(|v| v.served));
        let Some(version) = version else { continue };
        let Some(defs) = &version.additional_printer_columns else { continue };

        let cols: Vec<PrinterCol> = defs
            .iter()
            .filter(|c| !c.name.eq_ignore_ascii_case("age"))
            .map(|c| PrinterCol {
                name: c.name.clone(),
                json_path: c.json_path.clone(),
                col_type: c.type_.clone(),
            })
            .collect();
        if !cols.is_empty() {
            map.insert((group, kind), cols);
        }
    }
    map
}

/// Evaluate a Kubernetes-dialect JSONPath against an object, rendering the matched
/// value(s) to a display string. Supports the forms printer columns actually use:
/// dot paths (`.spec.foo.bar`), array index (`.x[0]`), wildcards (`.x[*]`), and the
/// condition filter (`.status.conditions[?(@.type=="Ready")].status`). Multiple
/// matches join with commas; no match yields an empty string.
pub fn eval(path: &str, root: &Value) -> String {
    let mut cur: Vec<&Value> = vec![root];
    for seg in segments(path.trim_start_matches('.')) {
        let (field, sel) = parse_segment(&seg);
        let mut next: Vec<&Value> = Vec::new();
        for v in &cur {
            let target = if field.is_empty() { Some(*v) } else { v.get(&field) };
            let Some(target) = target else { continue };
            match &sel {
                Sel::None => next.push(target),
                Sel::Index(i) => {
                    if let Some(e) = target.as_array().and_then(|a| a.get(*i)) {
                        next.push(e);
                    }
                }
                Sel::Star => {
                    if let Some(a) = target.as_array() {
                        next.extend(a.iter());
                    }
                }
                Sel::Filter { key, val } => {
                    if let Some(a) = target.as_array() {
                        for e in a {
                            if e.get(key).and_then(|x| x.as_str()) == Some(val.as_str()) {
                                next.push(e);
                            }
                        }
                    }
                }
            }
        }
        cur = next;
    }
    cur.iter()
        .map(|v| render_scalar(v))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

/// A per-segment selector parsed from the `[...]` suffix, if any.
enum Sel {
    None,
    Index(usize),
    Star,
    Filter { key: String, val: String },
}

/// Split a path on `.`, but not on dots inside `[...]` (the condition filter
/// contains `@.type`, which must stay attached to its segment).
fn segments(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut buf = String::new();
    for ch in path.chars() {
        match ch {
            '[' => {
                depth += 1;
                buf.push(ch);
            }
            ']' => {
                depth = depth.saturating_sub(1);
                buf.push(ch);
            }
            '.' if depth == 0 => {
                if !buf.is_empty() {
                    out.push(std::mem::take(&mut buf));
                }
            }
            _ => buf.push(ch),
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

/// Split a segment like `conditions[?(@.type=="Ready")]` into its field name and
/// the bracket selector.
fn parse_segment(seg: &str) -> (String, Sel) {
    let Some(open) = seg.find('[') else {
        return (seg.to_string(), Sel::None);
    };
    let field = seg[..open].to_string();
    let inner = seg[open + 1..].trim_end_matches(']');
    let sel = if inner == "*" {
        Sel::Star
    } else if let Ok(i) = inner.parse::<usize>() {
        Sel::Index(i)
    } else if let Some(f) = parse_filter(inner) {
        f
    } else {
        Sel::None
    };
    (field, sel)
}

/// Parse a `?(@.key=="value")` filter (single or double quotes).
fn parse_filter(inner: &str) -> Option<Sel> {
    let inner = inner.strip_prefix("?(")?.strip_suffix(')')?;
    let key_start = inner.find("@.")? + 2;
    let eq = inner[key_start..].find("==")? + key_start;
    let key = inner[key_start..eq].trim().to_string();
    let rhs = inner[eq + 2..].trim();
    let quote = rhs.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let val = rhs[1..].strip_suffix(quote)?.to_string();
    Some(Sel::Filter { key, val })
}

fn render_scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::eval;
    use serde_json::json;

    #[test]
    fn dot_path() {
        let o = json!({"spec": {"secretStoreRef": {"name": "vault"}}});
        assert_eq!(eval(".spec.secretStoreRef.name", &o), "vault");
        assert_eq!(eval(".spec.missing.name", &o), "");
    }

    #[test]
    fn condition_filter() {
        let o = json!({"status": {"conditions": [
            {"type": "Synced", "status": "True"},
            {"type": "Ready", "status": "False", "reason": "SecretSyncedError"},
        ]}});
        // double and single quotes, and selecting different sub-fields
        assert_eq!(eval(r#".status.conditions[?(@.type=="Ready")].status"#, &o), "False");
        assert_eq!(eval(r#".status.conditions[?(@.type=='Ready')].reason"#, &o), "SecretSyncedError");
        assert_eq!(eval(r#".status.conditions[?(@.type=="Missing")].status"#, &o), "");
    }

    #[test]
    fn index_star_and_scalars() {
        let o = json!({
            "status": {"loadBalancer": {"ingress": [{"ip": "10.0.0.1"}, {"ip": "10.0.0.2"}]}},
            "spec": {"replicas": 3, "paused": true},
        });
        assert_eq!(eval(".status.loadBalancer.ingress[0].ip", &o), "10.0.0.1");
        assert_eq!(eval(".status.loadBalancer.ingress[*].ip", &o), "10.0.0.1,10.0.0.2");
        assert_eq!(eval(".spec.replicas", &o), "3");
        assert_eq!(eval(".spec.paused", &o), "true");
    }
}
