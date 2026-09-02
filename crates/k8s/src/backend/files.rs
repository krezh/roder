use std::time::Duration;

use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, AttachParams};
use roder_core::{ContainerDirectory, ContainerFileContent, ContainerFileEntry, ContainerFileKind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{api_err, exec::detect_shell, Backend};
use crate::client::K8sError;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const DIRECTORY_OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const FILE_PREVIEW_LIMIT: usize = 1024 * 1024;
const FILE_TRANSFER_LIMIT: usize = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

const LIST_SCRIPT: &str = r#"
dir=$1
[ -d "$dir" ] || { printf 'not a directory: %s\n' "$dir" >&2; exit 44; }
for entry in "$dir"/* "$dir"/.[!.]* "$dir"/..?*; do
  [ -e "$entry" ] || [ -L "$entry" ] || continue
  if [ -L "$entry" ]; then kind=l; size=0
  elif [ -d "$entry" ]; then kind=d; size=0
  elif [ -f "$entry" ]; then kind=f; size=$(wc -c < "$entry" 2>/dev/null || printf 0)
  else kind=o; size=0
  fi
  printf '%s\000%s\000%s\000' "$kind" "$size" "${entry##*/}"
done
"#;

const READ_SCRIPT: &str = r#"
file=$1
[ -f "$file" ] && [ ! -L "$file" ] || { printf 'not a regular file: %s\n' "$file" >&2; exit 45; }
dd if="$file" bs=4096 count=257 2>/dev/null
"#;

const DOWNLOAD_SCRIPT: &str = r#"
file=$1
[ -f "$file" ] && [ ! -L "$file" ] || { printf 'not a regular file: %s\n' "$file" >&2; exit 45; }
cat "$file"
"#;

const WRITE_SCRIPT: &str = r#"
file=$1
[ -f "$file" ] && [ ! -L "$file" ] || { printf 'not a regular file: %s\n' "$file" >&2; exit 45; }
tmp="${file}.roder-write.$$"
trap 'rm -f "$tmp"' EXIT HUP INT TERM
(umask 077; set -C; cat > "$tmp") 2>/dev/null || { printf 'could not stage file contents\n' >&2; exit 46; }
cat "$tmp" > "$file"
"#;

const UPLOAD_SCRIPT: &str = r#"
file=$1
[ ! -d "$file" ] && [ ! -L "$file" ] || { printf 'not a writable file path: %s\n' "$file" >&2; exit 45; }
tmp="${file}.roder-upload.$$"
trap 'rm -f "$tmp"' EXIT HUP INT TERM
(umask 077; set -C; cat > "$tmp") || { printf 'could not stage upload\n' >&2; exit 46; }
if [ -f "$file" ]; then cat "$tmp" > "$file"
else (umask 077; set -C; cat "$tmp" > "$file") || { printf 'could not create upload target: %s\n' "$file" >&2; exit 47; }
fi
"#;

const CREATE_SCRIPT: &str = r#"
kind=$1
path=$2
[ "$path" != / ] || { printf 'cannot create the root directory\n' >&2; exit 46; }
case "$kind" in
  file) (set -C; : > "$path") 2>/dev/null || { printf 'path already exists: %s\n' "$path" >&2; exit 47; } ;;
  directory) mkdir "$path" ;;
  *) printf 'invalid entry type\n' >&2; exit 48 ;;
esac
"#;

const DELETE_SCRIPT: &str = r#"
path=$1
[ "$path" != / ] || { printf 'cannot delete the root directory\n' >&2; exit 49; }
if [ -L "$path" ]; then rm "$path"
elif [ -d "$path" ]; then rmdir "$path"
else rm "$path"
fi
"#;

impl Backend {
    pub async fn list_container_directory(
        &self,
        namespace: &str,
        pod: &str,
        container: &str,
        path: &str,
    ) -> Result<ContainerDirectory, K8sError> {
        let path = normalize_file_path(path)?;
        let output = self
            .run_container_command(
                namespace,
                pod,
                container,
                &["/bin/sh", "-c", LIST_SCRIPT, "roder-files", &path],
                None,
                DIRECTORY_OUTPUT_LIMIT,
            )
            .await?;
        let mut fields = output.split(|byte| *byte == 0);
        let mut entries = Vec::new();
        while let Some(kind) = fields.next() {
            if kind.is_empty() {
                break;
            }
            let size = fields
                .next()
                .ok_or_else(|| api_err("invalid directory response"))?;
            let name = fields
                .next()
                .ok_or_else(|| api_err("invalid directory response"))?;
            if entries.len() == MAX_ENTRIES {
                return Err(api_err(format!(
                    "directory contains more than {MAX_ENTRIES} entries"
                )));
            }
            let kind = match kind {
                b"d" => ContainerFileKind::Directory,
                b"f" => ContainerFileKind::File,
                b"l" => ContainerFileKind::Symlink,
                b"o" => ContainerFileKind::Other,
                _ => return Err(api_err("invalid file type in directory response")),
            };
            entries.push(ContainerFileEntry {
                name: String::from_utf8_lossy(name).into_owned(),
                kind,
                size: String::from_utf8_lossy(size).trim().parse().unwrap_or(0),
            });
        }
        entries.sort_by(|left, right| {
            let left_dir = left.kind == ContainerFileKind::Directory;
            let right_dir = right.kind == ContainerFileKind::Directory;
            right_dir
                .cmp(&left_dir)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        Ok(ContainerDirectory { path, entries })
    }

    pub async fn read_container_file(
        &self,
        namespace: &str,
        pod: &str,
        container: &str,
        path: &str,
    ) -> Result<ContainerFileContent, K8sError> {
        let path = normalize_file_path(path)?;
        let mut output = self
            .run_container_command(
                namespace,
                pod,
                container,
                &["/bin/sh", "-c", READ_SCRIPT, "roder-files", &path],
                None,
                FILE_PREVIEW_LIMIT + 4096,
            )
            .await?;
        let truncated = output.len() > FILE_PREVIEW_LIMIT;
        output.truncate(FILE_PREVIEW_LIMIT);
        let binary = output.contains(&0);
        Ok(ContainerFileContent {
            path,
            content: if binary {
                String::new()
            } else {
                String::from_utf8_lossy(&output).into_owned()
            },
            truncated,
            binary,
        })
    }

    pub async fn write_container_file(
        &self,
        namespace: &str,
        pod: &str,
        container: &str,
        path: &str,
        content: &[u8],
    ) -> Result<(), K8sError> {
        if content.len() > FILE_TRANSFER_LIMIT {
            return Err(api_err("file content exceeds the 16 MiB transfer limit"));
        }
        let path = normalize_mutation_path(path)?;
        self.run_container_command(
            namespace,
            pod,
            container,
            &["/bin/sh", "-c", WRITE_SCRIPT, "roder-files", &path],
            Some(content),
            64 * 1024,
        )
        .await?;
        Ok(())
    }

    pub async fn download_container_file(
        &self,
        namespace: &str,
        pod: &str,
        container: &str,
        path: &str,
    ) -> Result<Vec<u8>, K8sError> {
        let path = normalize_file_path(path)?;
        self.run_container_command(
            namespace,
            pod,
            container,
            &["/bin/sh", "-c", DOWNLOAD_SCRIPT, "roder-files", &path],
            None,
            FILE_TRANSFER_LIMIT,
        )
        .await
    }

    pub async fn upload_container_file(
        &self,
        namespace: &str,
        pod: &str,
        container: &str,
        path: &str,
        content: &[u8],
    ) -> Result<(), K8sError> {
        if content.len() > FILE_TRANSFER_LIMIT {
            return Err(api_err("file content exceeds the 16 MiB transfer limit"));
        }
        let path = normalize_mutation_path(path)?;
        self.run_container_command(
            namespace,
            pod,
            container,
            &["/bin/sh", "-c", UPLOAD_SCRIPT, "roder-files", &path],
            Some(content),
            64 * 1024,
        )
        .await?;
        Ok(())
    }

    pub async fn create_container_entry(
        &self,
        namespace: &str,
        pod: &str,
        container: &str,
        path: &str,
        directory: bool,
    ) -> Result<(), K8sError> {
        let path = normalize_mutation_path(path)?;
        let kind = if directory { "directory" } else { "file" };
        self.run_container_command(
            namespace,
            pod,
            container,
            &["/bin/sh", "-c", CREATE_SCRIPT, "roder-files", kind, &path],
            None,
            64 * 1024,
        )
        .await?;
        Ok(())
    }

    pub async fn delete_container_entry(
        &self,
        namespace: &str,
        pod: &str,
        container: &str,
        path: &str,
    ) -> Result<(), K8sError> {
        let path = normalize_mutation_path(path)?;
        self.run_container_command(
            namespace,
            pod,
            container,
            &["/bin/sh", "-c", DELETE_SCRIPT, "roder-files", &path],
            None,
            64 * 1024,
        )
        .await?;
        Ok(())
    }

    async fn run_container_command(
        &self,
        namespace: &str,
        pod: &str,
        container: &str,
        command: &[&str],
        input: Option<&[u8]>,
        output_limit: usize,
    ) -> Result<Vec<u8>, K8sError> {
        let api: Api<Pod> = Api::namespaced(self.client(), namespace);
        let shell = if command.first() == Some(&"/bin/sh") {
            Some(detect_shell(&api, pod, Some(container)).await)
        } else {
            None
        };
        let mut command = command.to_vec();
        if let Some(shell) = shell.as_deref() {
            command[0] = shell;
        }
        let params = AttachParams::default()
            .stdin(input.is_some())
            .stdout(true)
            .stderr(true)
            .tty(false)
            .container(container);
        let mut process = api.exec(pod, command, &params).await.map_err(api_err)?;
        let status = process
            .take_status()
            .ok_or_else(|| api_err("container exec did not provide an exit status"))?;
        let mut stdin = input
            .map(|_| {
                process
                    .stdin()
                    .ok_or_else(|| api_err("container exec did not provide stdin"))
            })
            .transpose()?;
        let input = input.map(<[u8]>::to_vec);
        let mut stdout = process
            .stdout()
            .ok_or_else(|| api_err("container exec did not provide stdout"))?
            .take((output_limit + 1) as u64);
        let mut stderr = process
            .stderr()
            .ok_or_else(|| api_err("container exec did not provide stderr"))?
            .take(64 * 1024);
        let result = tokio::time::timeout(COMMAND_TIMEOUT, async move {
            let mut output = Vec::new();
            let mut error = Vec::new();
            let write_input = async move {
                if let (Some(stdin), Some(input)) = (stdin.as_mut(), input.as_deref()) {
                    stdin.write_all(input).await.map_err(api_err)?;
                    stdin.shutdown().await.map_err(api_err)?;
                }
                Ok::<(), K8sError>(())
            };
            let (stdin_result, stdout_result, stderr_result, remote_status, join_result) = tokio::join!(
                write_input,
                stdout.read_to_end(&mut output),
                stderr.read_to_end(&mut error),
                status,
                process.join(),
            );
            stdin_result?;
            stdout_result.map_err(api_err)?;
            stderr_result.map_err(api_err)?;
            join_result.map_err(api_err)?;
            let remote_status = remote_status.ok_or_else(|| api_err("container command returned no status"))?;
            if remote_status.status.as_deref() != Some("Success") {
                let message = if error.is_empty() {
                    remote_status
                        .message
                        .or(remote_status.reason)
                        .unwrap_or_else(|| "container command failed".to_string())
                } else {
                    String::from_utf8_lossy(&error).trim().to_string()
                };
                return Err(K8sError::Container(message));
            }
            if !error.is_empty() {
                return Err(K8sError::Container(
                    String::from_utf8_lossy(&error).trim().to_string(),
                ));
            }
            if output.len() > output_limit {
                return Err(api_err("container command output limit exceeded"));
            }
            Ok(output)
        })
        .await
        .map_err(|_| api_err("container command timed out"))?;
        result
    }
}

fn normalize_mutation_path(path: &str) -> Result<String, K8sError> {
    let path = normalize_file_path(path)?;
    if path == "/" {
        return Err(api_err("the container root cannot be modified"));
    }
    Ok(path)
}

pub fn normalize_file_path(path: &str) -> Result<String, K8sError> {
    if !path.starts_with('/') || path.contains('\0') || path.len() > 4096 {
        return Err(api_err("path must be an absolute path up to 4096 bytes"));
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            value => parts.push(value),
        }
    }
    Ok(if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    })
}

#[cfg(test)]
mod tests {
    use super::{normalize_file_path, normalize_mutation_path};

    #[test]
    fn normalizes_absolute_container_paths() {
        assert_eq!(normalize_file_path("/").unwrap(), "/");
        assert_eq!(
            normalize_file_path("/var//log/./app").unwrap(),
            "/var/log/app"
        );
        assert_eq!(normalize_file_path("/var/log/../../etc").unwrap(), "/etc");
        assert_eq!(normalize_file_path("/../../etc").unwrap(), "/etc");
    }

    #[test]
    fn rejects_invalid_container_paths() {
        assert!(normalize_file_path("relative/path").is_err());
        assert!(normalize_file_path("/bad\0path").is_err());
        assert!(normalize_file_path(&format!("/{}", "a".repeat(4096))).is_err());
    }

    #[test]
    fn mutations_reject_the_container_root() {
        assert!(normalize_mutation_path("/").is_err());
        assert_eq!(normalize_mutation_path("/tmp/file").unwrap(), "/tmp/file");
    }
}
