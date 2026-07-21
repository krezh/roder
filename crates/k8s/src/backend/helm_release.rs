//! Reads a HelmRelease's *deployed* revision straight from Helm's own storage
//! (the `sh.helm.release.v1.<name>.v<revision>` Secret) — there is no typed
//! inventory field on HelmRelease, unlike Kustomization. Assumes a modern
//! helm-controller whose `status.history[]` entries carry explicit `name`/
//! `namespace` for the release (this project's "always latest" convention).

use k8s_openapi::api::core::v1::Secret;
use kube::api::{Api, DynamicObject};
use serde_json::Value;

use super::Backend;

const MAX_HELM_RELEASE_BYTES: u64 = 32 * 1024 * 1024;

/// One object referenced by a Helm release's stored manifest, before catalog
/// resolution (see `Backend::resolve_child`).
pub(super) struct ManifestObjectRef {
    pub group: String,
    pub version: String,
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
}

impl Backend {
    /// The children of a HelmRelease: every object in its deployed revision's
    /// stored manifest. `hr` is the already-fetched HelmRelease object's JSON.
    pub(super) async fn helm_release_children(
        &self,
        hr: &Value,
        semaphore: &tokio::sync::Semaphore,
    ) -> Result<Vec<ManifestObjectRef>, String> {
        let history = hr
            .get("status")
            .and_then(|s| s.get("history"))
            .and_then(|h| h.as_array())
            .filter(|h| !h.is_empty())
            .ok_or_else(|| "HelmRelease has no deployed revision yet".to_string())?;
        let deployed = history
            .iter()
            .find(|h| h.get("status").and_then(|s| s.as_str()) == Some("deployed"))
            .ok_or_else(|| "HelmRelease has no deployed revision yet".to_string())?;
        let rel_name = deployed
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or("Helm release history entry missing name")?;
        let rel_ns = deployed
            .get("namespace")
            .and_then(|v| v.as_str())
            .ok_or("Helm release history entry missing namespace")?;
        let revision = deployed
            .get("version")
            .and_then(|v| v.as_i64())
            .ok_or("Helm release history entry missing version")?;

        let secret_name = format!("sh.helm.release.v1.{rel_name}.v{revision}");
        let api: Api<Secret> = Api::namespaced(self.client(), rel_ns);
        let secret = super::tree::with_api_permit(semaphore, api.get(&secret_name))
            .await
            .map_err(|e| format!("could not read Helm storage secret {secret_name}: {e}"))?;
        let raw = secret
            .data
            .as_ref()
            .and_then(|d| d.get("release"))
            .map(|b| b.0.clone())
            .ok_or_else(|| format!("Helm storage secret {secret_name} has no 'release' key"))?;

        let manifest = decode_helm_manifest(&raw)?;
        Ok(parse_helm_manifest(&manifest, rel_ns))
    }
}

/// Helm's storage driver encodes a release as `base64(gzip(json))` — and since
/// kube-rs already base64-decodes the k8s Secret's own `data` field into raw
/// bytes, `raw` here is just that inner base64 string's bytes. Decode: base64
/// (Helm's layer) → gunzip → JSON → take `.manifest`.
fn decode_helm_manifest(raw: &[u8]) -> Result<String, String> {
    use base64::Engine as _;
    let gz = base64::engine::general_purpose::STANDARD
        .decode(raw)
        .map_err(|e| format!("invalid Helm release secret encoding: {e}"))?;
    let json_bytes = decompress_helm_release(&gz, MAX_HELM_RELEASE_BYTES)?;
    let release: Value = serde_json::from_slice(&json_bytes)
        .map_err(|e| format!("invalid Helm release JSON: {e}"))?;
    release
        .get("manifest")
        .and_then(|m| m.as_str())
        .map(str::to_string)
        .ok_or_else(|| "Helm release has no manifest".to_string())
}

fn decompress_helm_release(gz: &[u8], limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read as _;

    let mut json_bytes = Vec::new();
    flate2::read::GzDecoder::new(gz)
        .take(limit + 1)
        .read_to_end(&mut json_bytes)
        .map_err(|e| format!("failed to decompress Helm release secret: {e}"))?;
    if json_bytes.len() as u64 > limit {
        return Err(format!(
            "decompressed Helm release exceeds {limit} byte limit"
        ));
    }
    Ok(json_bytes)
}

/// Split Helm's multi-document `.manifest` YAML on `---` document separator
/// lines (Helm also emits `# Source: ...` comment lines before each — those
/// are harmless, `serde_yaml` ignores comments) and parse each into a
/// `DynamicObject` the same way `apply_yaml` parses a single doc. Docs that
/// fail to parse, or lack a kind/name, are skipped (best-effort) rather than
/// failing the whole release.
fn parse_helm_manifest(manifest: &str, release_namespace: &str) -> Vec<ManifestObjectRef> {
    split_yaml_docs(manifest)
        .into_iter()
        .filter_map(|doc| serde_yaml::from_str::<DynamicObject>(&doc).ok())
        .filter_map(|obj| {
            let types = obj.types?;
            let name = obj.metadata.name?;
            let (group, version) = match types.api_version.split_once('/') {
                Some((g, v)) => (g.to_string(), v.to_string()),
                None => (String::new(), types.api_version),
            };
            Some(ManifestObjectRef {
                group,
                version,
                kind: types.kind,
                name,
                namespace: obj
                    .metadata
                    .namespace
                    .or_else(|| Some(release_namespace.to_string())),
            })
        })
        .collect()
}

fn split_yaml_docs(manifest: &str) -> Vec<String> {
    let mut docs = Vec::new();
    let mut current = String::new();
    for line in manifest.lines() {
        if line.trim_end() == "---" {
            if !current.trim().is_empty() {
                docs.push(std::mem::take(&mut current));
            }
            current.clear();
            continue;
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        docs.push(current);
    }
    docs
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
---
# Source: podinfo/templates/deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: podinfo
  namespace: apps
---
# Source: podinfo/templates/sa.yaml
apiVersion: v1
kind: ServiceAccount
metadata:
  name: podinfo
";

    #[test]
    fn splits_and_parses_multiple_docs() {
        let refs = parse_helm_manifest(SAMPLE, "apps");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].kind, "Deployment");
        assert_eq!(refs[0].group, "apps");
        assert_eq!(refs[0].version, "v1");
        assert_eq!(refs[0].namespace.as_deref(), Some("apps"));
    }

    #[test]
    fn core_kind_has_empty_group() {
        let refs = parse_helm_manifest(SAMPLE, "apps");
        assert_eq!(refs[1].kind, "ServiceAccount");
        assert_eq!(refs[1].group, "");
        assert_eq!(refs[1].version, "v1");
    }

    #[test]
    fn doc_missing_namespace_defaults_to_release_namespace() {
        let refs = parse_helm_manifest(SAMPLE, "apps");
        assert_eq!(refs[1].namespace.as_deref(), Some("apps"));
    }

    #[test]
    fn leading_and_trailing_separators_dont_produce_empty_docs() {
        let m = "---\napiVersion: v1\nkind: ServiceAccount\nmetadata:\n  name: a\n---\n";
        assert_eq!(parse_helm_manifest(m, "ns").len(), 1);
    }

    #[test]
    fn garbage_doc_is_skipped_not_fatal() {
        let m = "---\n: this is not : valid : yaml{{{\n---\napiVersion: v1\nkind: ServiceAccount\nmetadata:\n  name: ok\n";
        let refs = parse_helm_manifest(m, "ns");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "ok");
    }

    #[test]
    fn doc_missing_name_is_skipped() {
        let m = "apiVersion: v1\nkind: ConfigMap\nmetadata: {}\n";
        assert!(parse_helm_manifest(m, "ns").is_empty());
    }

    #[test]
    fn empty_manifest_yields_no_docs() {
        assert!(parse_helm_manifest("", "ns").is_empty());
        assert!(parse_helm_manifest("   \n\n", "ns").is_empty());
    }

    #[test]
    fn decode_helm_manifest_round_trips_gzip_base64_json() {
        use base64::Engine as _;
        use std::io::Write;
        let payload = serde_json::json!({ "manifest": "apiVersion: v1\nkind: Pod\n" }).to_string();
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(payload.as_bytes()).unwrap();
        let gz = enc.finish().unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(gz);
        let decoded = decode_helm_manifest(b64.as_bytes()).unwrap();
        assert!(decoded.contains("kind: Pod"));
    }

    #[test]
    fn decode_helm_manifest_rejects_garbage() {
        assert!(decode_helm_manifest(b"not base64 at all!!").is_err());
    }

    #[test]
    fn helm_release_decompression_is_bounded() {
        use std::io::Write;

        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&vec![b'x'; 1025]).unwrap();
        let gz = enc.finish().unwrap();
        let error = decompress_helm_release(&gz, 1024).unwrap_err();
        assert!(error.contains("exceeds 1024 byte limit"));
    }
}
