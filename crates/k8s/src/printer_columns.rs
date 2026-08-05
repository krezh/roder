//! Generic CRD columns
//! `CustomResourceDefinition` declares `additionalPrinterColumns` (a `name` + a
//! `jsonPath` into the object). We harvest those once from the installed CRDs and
//! evaluate the jsonPaths against objects already in the informer cache — so any
//! CRD gets meaningful columns

use std::borrow::Cow;
use std::collections::HashMap;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{Api, ListParams};
use kube::core::ClusterResourceScope;
use kube::{Client, Resource};
use serde::Deserialize;
use serde_json::Value;

/// One declared printer column from a CRD.
#[derive(Clone, Debug)]
pub struct PrinterCol {
    pub name: String,
    pub json_path: String,
    /// OpenAPI column type: string | integer | number | boolean | date.
    /// Currently unused at runtime — RFC3339 values are detected on the
    /// client via `data::looks_like_rfc3339` (mirrors the built-in Age
    /// column's render path). Retained so a future typed-metadata push to
    /// the client can flag date columns without re-reving `CrdLite`.
    #[allow(dead_code)]
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

/// A `CustomResourceDefinition` deserialized *without* its
/// `spec.versions[].schema` (the OpenAPI validation schema). That schema is by
/// far the largest part of a CRD — Rook/Ceph and Prometheus-operator ones run to
/// megabytes — and deserializing it into the recursive `JSONSchemaProps` tree
/// spikes memory on startup (the in-memory form is many times the JSON size). We
/// only need the printer columns, so we drop the schema on the floor simply by
/// never declaring the field; serde skips it instead of allocating it.
#[derive(Clone, Debug, Default, Deserialize)]
struct CrdLite {
    #[serde(default)]
    metadata: ObjectMeta,
    #[serde(default)]
    spec: CrdLiteSpec,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct CrdLiteSpec {
    #[serde(default)]
    group: String,
    #[serde(default)]
    names: CrdLiteNames,
    #[serde(default)]
    versions: Vec<CrdLiteVersion>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct CrdLiteNames {
    #[serde(default)]
    kind: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrdLiteVersion {
    #[serde(default)]
    served: bool,
    #[serde(default)]
    storage: bool,
    #[serde(default)]
    additional_printer_columns: Vec<CrdLiteCol>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrdLiteCol {
    #[serde(default)]
    name: String,
    #[serde(default)]
    json_path: String,
    #[serde(default, rename = "type")]
    type_: String,
}

// Hand-rolled `Resource` so `Api<CrdLite>` lists CRDs via the right URL while
// deserializing into the lightweight type above.
impl Resource for CrdLite {
    type DynamicType = ();
    type Scope = ClusterResourceScope;
    fn kind(_: &()) -> Cow<'_, str> {
        "CustomResourceDefinition".into()
    }
    fn group(_: &()) -> Cow<'_, str> {
        "apiextensions.k8s.io".into()
    }
    fn version(_: &()) -> Cow<'_, str> {
        "v1".into()
    }
    fn plural(_: &()) -> Cow<'_, str> {
        "customresourcedefinitions".into()
    }
    fn meta(&self) -> &ObjectMeta {
        &self.metadata
    }
    fn meta_mut(&mut self) -> &mut ObjectMeta {
        &mut self.metadata
    }
}

/// Fetch every CRD and index its served printer columns by (group, kind). We keep
/// `priority > 0` ("wide") columns — k9s shows them by default, unlike `kubectl get`
/// — and only drop the redundant Age column (roder renders Age itself). A failure
/// (e.g. no RBAC to list CRDs) just yields an empty map → hand-written columns only.
pub async fn load(client: &Client) -> ColumnMap {
    let api: Api<CrdLite> = Api::all(client.clone());
    let mut map = ColumnMap::new();
    // Page the list: a cluster can have hundreds of CRDs and the full response
    // body runs to tens of MB. CrdLite already drops the (huge) schemas from the
    // *parsed* structs; paging also bounds the transient *response* body to one
    // page rather than holding all of them at once.
    let mut params = ListParams::default().limit(50);
    loop {
        let list = match api.list(&params).await {
            Ok(l) => l,
            Err(e) => {
                tracing::debug!("CRD printer-column load skipped: {e}");
                break;
            }
        };
        for crd in list.items {
            let group = crd.spec.group;
            let kind = crd.spec.names.kind;
            // Prefer the storage version's columns; fall back to first served.
            let version = crd
                .spec
                .versions
                .iter()
                .find(|v| v.storage)
                .or_else(|| crd.spec.versions.iter().find(|v| v.served));
            let Some(version) = version else { continue };

            let cols: Vec<PrinterCol> = version
                .additional_printer_columns
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
        match list.metadata.continue_ {
            Some(token) if !token.is_empty() => params = params.continue_token(&token),
            _ => break,
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
            let target = if field.is_empty() {
                Some(*v)
            } else {
                v.get(&field)
            };
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
                            // Compare via render_scalar so integers/booleans match
                            // their string representation (e.g. `port==8080` matches
                            // the JSON number 8080, not just the string "8080").
                            if e.get(key).map(render_scalar).as_deref() == Some(val.as_str()) {
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
        // `\n`-joined, not `,` — the table cell/tooltip layer treats `\n` as the
        // list-value marker: the cell collapses it to a compact ", " form while
        // the tooltip renders it as a proper line-per-item list on hover.
        .join("\n")
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
        // An array at the end of a path: join scalar elements (consistent with
        // [*] expansion), or return the element count if they're objects.
        Value::Array(a) => {
            let scalars: Vec<String> = a
                .iter()
                .filter_map(|e| match e {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    Value::Bool(b) => Some(b.to_string()),
                    _ => None,
                })
                .collect();
            if scalars.len() == a.len() {
                // See the `\n`-join note on `eval`'s [*] expansion above.
                scalars.join("\n")
            } else {
                a.len().to_string()
            }
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{eval, CrdLite};
    use crate::test_alloc::min_delta;
    use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
    use serde_json::json;

    /// One CRD object: tiny `additionalPrinterColumns`, but a large
    /// `schema.openAPIV3Schema` — mirroring real operators (Rook, Prometheus,
    /// Crunchy postgres) whose schemas dwarf everything else.
    fn crd_value(idx: usize, schema_props: usize) -> serde_json::Value {
        let mut props = serde_json::Map::new();
        for i in 0..schema_props {
            props.insert(
                format!("field{i}"),
                json!({ "type": "string", "description": "some documentation ".repeat(8) }),
            );
        }
        json!({
            "apiVersion": "apiextensions.k8s.io/v1",
            "kind": "CustomResourceDefinition",
            "metadata": { "name": format!("widget{idx}s.example.com") },
            "spec": {
                "group": format!("g{idx}.example.com"),
                "scope": "Namespaced",
                "names": { "kind": format!("Widget{idx}"), "plural": format!("widget{idx}s") },
                "versions": [{
                    "name": "v1",
                    "served": true,
                    "storage": true,
                    "additionalPrinterColumns": [
                        { "name": "Phase", "type": "string", "jsonPath": ".status.phase" },
                        { "name": "Age", "type": "date", "jsonPath": ".metadata.creationTimestamp" }
                    ],
                    "schema": { "openAPIV3Schema": {
                        "type": "object",
                        "properties": { "spec": { "type": "object", "properties": props } }
                    }}
                }]
            }
        })
    }

    /// A `CustomResourceDefinitionList` of `n` big-schema CRDs — the shape the
    /// apiserver returns to `load()`, and what actually spiked memory: every
    /// schema deserialised into a `JSONSchemaProps` tree at once.
    fn crd_list_json(n: usize, schema_props: usize) -> String {
        let items: Vec<_> = (0..n).map(|i| crd_value(i, schema_props)).collect();
        json!({
            "apiVersion": "apiextensions.k8s.io/v1",
            "kind": "CustomResourceDefinitionList",
            "items": items,
        })
        .to_string()
    }

    /// Minimal list wrapper so a CRD-list JSON deserialises into `Vec<T>` for any
    /// `T` (the full CRD or `CrdLite`) — mirroring what `Api::list` does.
    #[derive(serde::Deserialize)]
    struct TestList<T> {
        items: Vec<T>,
    }

    #[test]
    fn crdlite_parses_columns_and_skips_the_schema() {
        let json = crd_value(0, 800).to_string();
        let lite: CrdLite = serde_json::from_str(&json).unwrap();
        assert_eq!(lite.spec.group, "g0.example.com");
        assert_eq!(lite.spec.names.kind, "Widget0");
        let names: Vec<&str> = lite.spec.versions[0]
            .additional_printer_columns
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, ["Phase", "Age"]);
    }

    #[test]
    fn crdlite_list_is_far_lighter_than_full_at_scale() {
        // Hundreds of CRDs arrive in one list response; deserialising them all
        // into the recursive `JSONSchemaProps` tree is the >300MB startup spike
        // (the in-memory tree is many× the JSON). Dropping the schema (CrdLite)
        // lets the whole list parse for a fraction. A wide margin keeps it robust
        // against parallel-test allocation noise.
        let json = crd_list_json(60, 400);
        // Sanity: the lite list deserialises every item (schema dropped, columns kept).
        let parsed: TestList<CrdLite> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.items.len(), 60);

        let full = min_delta(|| {
            serde_json::from_str::<TestList<CustomResourceDefinition>>(&json).unwrap()
        });
        let lite = min_delta(|| serde_json::from_str::<TestList<CrdLite>>(&json).unwrap());
        assert!(
            lite.saturating_mul(10) < full,
            "CrdLite list should allocate <1/10 of the full list: lite={lite} full={full}"
        );
    }

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
        assert_eq!(
            eval(r#".status.conditions[?(@.type=="Ready")].status"#, &o),
            "False"
        );
        assert_eq!(
            eval(r#".status.conditions[?(@.type=='Ready')].reason"#, &o),
            "SecretSyncedError"
        );
        assert_eq!(
            eval(r#".status.conditions[?(@.type=="Missing")].status"#, &o),
            ""
        );
    }

    #[test]
    fn array_fallback() {
        let o = json!({
            "status": {
                "projects": [
                    {"name": "foo", "status": "completed"},
                    {"name": "bar", "status": "completed"},
                    {"name": "baz", "status": "failed"},
                ]
            }
        });
        // Path ending at an array of objects → count
        assert_eq!(eval(".status.projects", &o), "3");
        // Path ending at an array of scalars → joined
        assert_eq!(
            eval(".status.projects[*].status", &o),
            "completed\ncompleted\nfailed"
        );
        // Mixed array with scalars only → joined
        let o2 = json!({"items": ["a", "b", "c"]});
        assert_eq!(eval(".items", &o2), "a\nb\nc");
    }

    #[test]
    fn index_star_and_scalars() {
        let o = json!({
            "status": {"loadBalancer": {"ingress": [{"ip": "10.0.0.1"}, {"ip": "10.0.0.2"}]}},
            "spec": {"replicas": 3, "paused": true},
        });
        assert_eq!(eval(".status.loadBalancer.ingress[0].ip", &o), "10.0.0.1");
        assert_eq!(
            eval(".status.loadBalancer.ingress[*].ip", &o),
            "10.0.0.1\n10.0.0.2"
        );
        assert_eq!(eval(".spec.replicas", &o), "3");
        assert_eq!(eval(".spec.paused", &o), "true");
    }
}
