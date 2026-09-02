use leptos::prelude::*;
use roder_core::{ContainerDirectory, ContainerFileContent, ContainerFileEntry, ContainerFileKind};

use crate::app::controllers::detail::format_bytes;
use crate::app::state::DetailTarget;
use crate::data;

#[cfg(target_arch = "wasm32")]
const FILE_TRANSFER_LIMIT: u64 = 16 * 1024 * 1024;

#[derive(Clone)]
struct FileItemMenu {
    x: i32,
    y: i32,
    path: String,
    name: String,
    kind: ContainerFileKind,
}

#[cfg(target_arch = "wasm32")]
async fn read_upload(event: leptos::ev::Event) -> Result<Option<(String, String)>, String> {
    use base64::Engine;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let input = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
        .ok_or_else(|| "Could not read the selected file".to_string())?;
    let Some(file) = input.files().and_then(|files| files.item(0)) else {
        return Ok(None);
    };
    if file.size() > FILE_TRANSFER_LIMIT as f64 {
        input.set_value("");
        return Err("File exceeds the 16 MiB upload limit".to_string());
    }
    let name = file.name();
    let buffer = JsFuture::from(file.array_buffer())
        .await
        .map_err(|_| "Could not read the selected file".to_string())?;
    let content = js_sys::Uint8Array::new(&buffer).to_vec();
    input.set_value("");
    Ok(Some((
        name,
        base64::engine::general_purpose::STANDARD.encode(content),
    )))
}

#[cfg(not(target_arch = "wasm32"))]
async fn read_upload(_event: leptos::ev::Event) -> Result<Option<(String, String)>, String> {
    Ok(None)
}

pub(crate) fn container_names(object: &serde_json::Value) -> Vec<String> {
    ["containers", "ephemeralContainers"]
        .into_iter()
        .filter_map(|key| object.pointer(&format!("/spec/{key}"))?.as_array())
        .flatten()
        .filter_map(|container| container.get("name")?.as_str().map(str::to_owned))
        .collect()
}

fn child_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn parent_path(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
        .unwrap_or("/")
        .to_string()
}

fn concise_file_error(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("/bin/sh") && lower.contains("no such file or directory") {
        return "File browser unavailable: this container does not include /bin/sh.".to_string();
    }
    if lower.contains("read-only file system") {
        return "This location is read-only.".to_string();
    }
    if lower.contains("permission denied") {
        return "Permission denied.".to_string();
    }
    if error.lines().count() == 1
        && error.len() <= 160
        && !lower.contains("internal error occurred")
    {
        return error.to_string();
    }
    "File operation failed. See the browser console for details.".to_string()
}

fn report_file_error(error: String) -> String {
    leptos::logging::error!("container file operation failed: {error}");
    concise_file_error(&error)
}

#[component]
pub(crate) fn TargetFileBrowser(target: DetailTarget) -> impl IntoView {
    let namespace = target.namespace.clone().unwrap_or_default();
    let pod = target.name.clone();
    let resource_target = target;
    let detail = LocalResource::new(move || {
        let target = resource_target.clone();
        async move {
            data::fetch_json::<roder_core::ObjectDetail>(&data::detail_url(
                &target.key,
                target.namespace.as_deref(),
                &target.name,
            ))
            .await
        }
    });
    view! {
        <Suspense fallback=|| view! { <div class="file-message">"Loading pod..."</div> }>
            {move || detail.get().map(|result| match result {
                Ok(detail) => view! { <FileBrowser namespace=namespace.clone() pod=pod.clone() object=detail.object /> }.into_any(),
                Err(error) => view! { <div class="file-message error">{error}</div> }.into_any(),
            })}
        </Suspense>
    }
}

#[component]
pub(crate) fn FileBrowser(
    namespace: String,
    pod: String,
    object: serde_json::Value,
) -> impl IntoView {
    let containers = container_names(&object);
    let selected_container = RwSignal::new(containers.first().cloned().unwrap_or_default());
    let path = RwSignal::new("/".to_string());
    let path_input = RwSignal::new("/".to_string());
    let selected_file = RwSignal::new(None::<String>);
    let editing = RwSignal::new(false);
    let draft = RwSignal::new(String::new());
    let creating = RwSignal::new(None::<bool>);
    let new_name = RwSignal::new(String::new());
    let delete_target = RwSignal::new(None::<String>);
    let item_menu = RwSignal::new(None::<FileItemMenu>);
    let uploading = RwSignal::new(false);
    let mutation_status = RwSignal::new(None::<Result<String, String>>);
    let target = StoredValue::new((namespace.clone(), pod.clone()));

    let directory_namespace = namespace.clone();
    let directory_pod = pod.clone();
    let directory = LocalResource::new(move || {
        let namespace = directory_namespace.clone();
        let pod = directory_pod.clone();
        let container = selected_container.get();
        let path = path.get();
        async move {
            data::fetch_json::<ContainerDirectory>(&data::container_file_url(
                "files", &namespace, &pod, &container, &path,
            ))
            .await
            .map_err(report_file_error)
        }
    });
    let preview_namespace = namespace;
    let preview_pod = pod;
    let preview = LocalResource::new(move || {
        let namespace = preview_namespace.clone();
        let pod = preview_pod.clone();
        let container = selected_container.get();
        let file = selected_file.get();
        async move {
            let Some(file) = file else {
                return Ok(None);
            };
            data::fetch_json::<ContainerFileContent>(&data::container_file_url(
                "file", &namespace, &pod, &container, &file,
            ))
            .await
            .map(Some)
            .map_err(report_file_error)
        }
    });
    Effect::new(move |_| {
        if editing.get_untracked() {
            return;
        }
        if let Some(Ok(Some(file))) = preview.get() {
            if !file.binary {
                draft.set(file.content);
            }
        }
    });

    let navigate = move |next: String| {
        path_input.set(next.clone());
        path.set(next);
        selected_file.set(None);
        editing.set(false);
        delete_target.set(None);
        item_menu.set(None);
    };

    view! {
        <section class="file-browser">
            <div class="file-toolbar">
                {(containers.len() > 1).then(|| view! {
                    <label class="file-container">
                        <span>"Container"</span>
                        <select prop:value=move || selected_container.get() on:change=move |event| {
                            selected_container.set(event_target_value(&event));
                            navigate("/".to_string());
                        }>
                            {containers.into_iter().map(|container| {
                                let option_value = container.clone();
                                view! { <option value=option_value>{container}</option> }
                            }).collect_view()}
                        </select>
                    </label>
                })}
                <form class="file-path" on:submit=move |event| {
                    event.prevent_default();
                    let next = path_input.get();
                    navigate(if next.starts_with('/') { next } else { format!("/{next}") });
                }>
                    <button type="button" class="file-up" disabled=move || path.get() == "/" on:click=move |_| navigate(parent_path(&path.get_untracked()))>"Up"</button>
                    <input aria-label="Container path" spellcheck="false" prop:value=move || path_input.get() on:input=move |event| path_input.set(event_target_value(&event)) />
                    <button type="submit">"Go"</button>
                </form>
                <div class="file-actions">
                    <label class="file-upload" class:busy=move || uploading.get()>
                        {move || if uploading.get() { "Uploading..." } else { "Upload" }}
                        <input type="file" disabled=move || uploading.get() on:change=move |event| {
                            uploading.set(true);
                            mutation_status.set(None);
                            leptos::task::spawn_local(async move {
                                match read_upload(event).await {
                                    Ok(Some((name, content))) if !name.is_empty() && !name.contains('/') => {
                                        let (namespace, pod) = target.get_value();
                                        let body = serde_json::json!({
                                            "namespace": namespace,
                                            "pod": pod,
                                            "container": selected_container.get_untracked(),
                                            "path": child_path(&path.get_untracked(), &name),
                                            "content": content,
                                        });
                                        match data::post_json::<serde_json::Value>("/api/file/upload", &body).await {
                                            Ok(_) => {
                                                mutation_status.set(Some(Ok(format!("Uploaded {name}"))));
                                                directory.refetch();
                                            }
                                            Err(error) => mutation_status.set(Some(Err(report_file_error(error)))),
                                        }
                                    }
                                    Ok(Some(_)) => mutation_status.set(Some(Err("Invalid file name".to_string()))),
                                    Ok(None) => {}
                                    Err(error) => mutation_status.set(Some(Err(error))),
                                }
                                uploading.set(false);
                            });
                        } />
                    </label>
                    <button type="button" on:click=move |_| { creating.set(Some(false)); new_name.set(String::new()); }>"New file"</button>
                    <button type="button" on:click=move |_| { creating.set(Some(true)); new_name.set(String::new()); }>"New directory"</button>
                    <button type="button" class="danger" disabled=move || path.get() == "/" on:click=move |_| delete_target.set(Some(path.get_untracked()))>"Delete directory"</button>
                </div>
            </div>
            {move || creating.get().map(|is_directory| view! {
                <form class="file-create" on:submit=move |event| {
                    event.prevent_default();
                    let name = new_name.get().trim().to_string();
                    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
                        mutation_status.set(Some(Err("Name must be one path segment".to_string())));
                        return;
                    }
                    let (namespace, pod) = target.get_value();
                    let body = serde_json::json!({
                        "namespace": namespace,
                        "pod": pod,
                        "container": selected_container.get_untracked(),
                        "path": child_path(&path.get_untracked(), &name),
                        "directory": is_directory,
                    });
                    mutation_status.set(None);
                    leptos::task::spawn_local(async move {
                        match data::post_json::<serde_json::Value>("/api/files", &body).await {
                            Ok(_) => {
                                creating.set(None);
                                new_name.set(String::new());
                                mutation_status.set(Some(Ok(if is_directory { "Directory created" } else { "File created" }.to_string())));
                                directory.refetch();
                            }
                            Err(error) => mutation_status.set(Some(Err(report_file_error(error)))),
                        }
                    });
                }>
                    <input aria-label=if is_directory { "New directory name" } else { "New file name" } placeholder=if is_directory { "Directory name" } else { "File name" } prop:value=move || new_name.get() on:input=move |event| new_name.set(event_target_value(&event)) />
                    <button type="submit">"Create"</button>
                    <button type="button" on:click=move |_| creating.set(None)>"Cancel"</button>
                </form>
            })}
            {move || delete_target.get().map(|delete_path| {
                let shown_path = delete_path.clone();
                view! {
                    <div class="file-delete-confirm">
                        <span>"Delete "<code>{shown_path}</code>"? Directories must be empty."</span>
                        <button type="button" on:click=move |_| delete_target.set(None)>"Cancel"</button>
                        <button type="button" class="danger" on:click=move |_| {
                            let (namespace, pod) = target.get_value();
                            let body = serde_json::json!({
                                "namespace": namespace,
                                "pod": pod,
                                "container": selected_container.get_untracked(),
                                "path": delete_path,
                            });
                            let deleting_current_directory = body.get("path").and_then(serde_json::Value::as_str) == Some(path.get_untracked().as_str());
                            mutation_status.set(None);
                            leptos::task::spawn_local(async move {
                                match data::post_json::<serde_json::Value>("/api/file/delete", &body).await {
                                    Ok(_) => {
                                        delete_target.set(None);
                                        selected_file.set(None);
                                        editing.set(false);
                                        mutation_status.set(Some(Ok("Deleted".to_string())));
                                        if deleting_current_directory {
                                            navigate(parent_path(&path.get_untracked()));
                                        } else {
                                            directory.refetch();
                                        }
                                    }
                                    Err(error) => mutation_status.set(Some(Err(report_file_error(error)))),
                                }
                            });
                        }>"Delete"</button>
                    </div>
                }
            })}
            {move || mutation_status.get().map(|status| match status {
                Ok(message) => view! { <div class="file-mutation-status ok">{message}</div> }.into_any(),
                Err(error) => view! { <div class="file-mutation-status error">{error}</div> }.into_any(),
            })}
            <Suspense fallback=|| view! { <div class="file-message">"Reading directory..."</div> }>
                {move || directory.get().map(|result| match result {
                    Err(error) => view! { <div class="file-message error">{error}</div> }.into_any(),
                    Ok(directory) if directory.entries.is_empty() => view! { <div class="file-message">"Empty directory"</div> }.into_any(),
                    Ok(directory) => view! {
                        <div class="file-list" role="list">
                            {directory.entries.into_iter().map(|entry| {
                                let entry_path = child_path(&directory.path, &entry.name);
                                file_entry(entry, entry_path, navigate, selected_file, editing, delete_target, item_menu)
                            }).collect_view()}
                        </div>
                    }.into_any(),
                })}
            </Suspense>
            {move || item_menu.get().map(|menu| {
                let open_path = menu.path.clone();
                let delete_path = menu.path.clone();
                let kind = menu.kind;
                let download_name = menu.name.clone();
                let download_url = data::container_file_url(
                    "file/download",
                    &target.get_value().0,
                    &target.get_value().1,
                    &selected_container.get_untracked(),
                    &menu.path,
                );
                let style = format!(
                    "left:min({}px,calc(100vw - 10.5rem));top:min({}px,calc(100vh - 9rem))",
                    menu.x, menu.y
                );
                view! {
                    <div class="file-menu-scrim" on:click=move |_| item_menu.set(None) on:contextmenu=move |event| {
                        event.prevent_default();
                        item_menu.set(None);
                    }></div>
                    <div class="file-item-menu" style=style>
                        {matches!(kind, ContainerFileKind::Directory | ContainerFileKind::File).then(|| view! {
                            <button type="button" on:click=move |_| {
                                item_menu.set(None);
                                if kind == ContainerFileKind::Directory {
                                    navigate(open_path.clone());
                                } else {
                                    selected_file.set(Some(open_path.clone()));
                                    editing.set(false);
                                    delete_target.set(None);
                                }
                            }>{if kind == ContainerFileKind::Directory { "Open" } else { "Preview" }}</button>
                        })}
                        {(kind == ContainerFileKind::File).then(|| view! {
                            <a href=download_url download=download_name on:click=move |_| item_menu.set(None)>"Download"</a>
                        })}
                        <button type="button" class="danger" on:click=move |_| {
                            item_menu.set(None);
                            delete_target.set(Some(delete_path.clone()));
                        }>"Delete"</button>
                    </div>
                }
            })}
            <Suspense fallback=|| view! { <div class="file-preview file-message">"Reading file..."</div> }>
                {move || preview.get().map(|result| match result {
                    Err(error) => view! { <div class="file-preview file-message error">{error}</div> }.into_any(),
                    Ok(None) => ().into_any(),
                    Ok(Some(file)) if file.binary => view! { <div class="file-preview file-message">"Binary file preview is not available."</div> }.into_any(),
                    Ok(Some(file)) => {
                        let file_path = StoredValue::new(file.path);
                        let file_content = StoredValue::new(file.content);
                        let truncated = file.truncated;
                        view! {
                        <div class="file-preview">
                            <div class="file-preview-head">
                                <code>{file_path.get_value()}</code>
                                <div>
                                    {truncated.then(|| view! { <span>"Preview truncated at 1 MiB"</span> })}
                                    <Show when=move || !editing.get() fallback=move || view! {
                                        <button type="button" on:click=move |_| {
                                            editing.set(false);
                                            if let Some(Ok(Some(file))) = preview.get_untracked() {
                                                draft.set(file.content);
                                            }
                                        }>"Cancel"</button>
                                        <button type="button" on:click=move |_| {
                                            let Some(file) = selected_file.get_untracked() else { return };
                                            let (namespace, pod) = target.get_value();
                                            let body = serde_json::json!({
                                                "namespace": namespace,
                                                "pod": pod,
                                                "container": selected_container.get_untracked(),
                                                "path": file,
                                                "content": draft.get_untracked(),
                                            });
                                            mutation_status.set(None);
                                            leptos::task::spawn_local(async move {
                                                match data::post_json::<serde_json::Value>("/api/file", &body).await {
                                                    Ok(_) => {
                                                        editing.set(false);
                                                        mutation_status.set(Some(Ok("File saved".to_string())));
                                                        preview.refetch();
                                                        directory.refetch();
                                                    }
                                                    Err(error) => mutation_status.set(Some(Err(report_file_error(error)))),
                                                }
                                            });
                                        }>"Save"</button>
                                    }>
                                        <button type="button" disabled=truncated on:click=move |_| { draft.set(file_content.get_value()); editing.set(true); }>"Edit"</button>
                                        <button type="button" class="danger" on:click=move |_| delete_target.set(Some(file_path.get_value()))>"Delete"</button>
                                    </Show>
                                </div>
                            </div>
                            {move || if editing.get() {
                                view! { <textarea class="file-editor" spellcheck="false" prop:value=move || draft.get() on:input=move |event| draft.set(event_target_value(&event))></textarea> }.into_any()
                            } else {
                                view! { <pre>{file_content.get_value()}</pre> }.into_any()
                            }}
                        </div>
                        }.into_any()
                    },
                })}
            </Suspense>
        </section>
    }
}

fn file_entry(
    entry: ContainerFileEntry,
    entry_path: String,
    navigate: impl Fn(String) + Copy + Send + Sync + 'static,
    selected_file: RwSignal<Option<String>>,
    editing: RwSignal<bool>,
    delete_target: RwSignal<Option<String>>,
    item_menu: RwSignal<Option<FileItemMenu>>,
) -> impl IntoView {
    let kind = entry.kind;
    let label = match kind {
        ContainerFileKind::Directory => "DIR",
        ContainerFileKind::File => "FILE",
        ContainerFileKind::Symlink => "LINK",
        ContainerFileKind::Other => "OTHER",
    };
    let clickable = matches!(kind, ContainerFileKind::Directory | ContainerFileKind::File);
    let context_path = entry_path.clone();
    let context_name = entry.name.clone();
    view! {
        <button class="file-entry" class:directory=kind == ContainerFileKind::Directory class:inactive=!clickable on:contextmenu=move |event| {
            event.prevent_default();
            item_menu.set(Some(FileItemMenu {
                x: event.client_x(),
                y: event.client_y(),
                path: context_path.clone(),
                name: context_name.clone(),
                kind,
            }));
        } on:click=move |_| {
            item_menu.set(None);
            if kind == ContainerFileKind::Directory {
                navigate(entry_path.clone());
            } else if kind == ContainerFileKind::File {
                selected_file.set(Some(entry_path.clone()));
                editing.set(false);
                delete_target.set(None);
            }
        }>
            <span class="file-kind">{label}</span>
            <span class="file-name">{entry.name}</span>
            <span class="file-size">{(kind == ContainerFileKind::File).then(|| format_bytes(entry.size))}</span>
        </button>
    }
}

#[cfg(test)]
mod tests {
    use super::{child_path, concise_file_error, container_names, parent_path};

    #[test]
    fn extracts_regular_and_ephemeral_containers() {
        let object = serde_json::json!({"spec": {
            "containers": [{"name": "app"}, {"name": "sidecar"}],
            "initContainers": [{"name": "setup"}],
            "ephemeralContainers": [{"name": "debug"}]
        }});
        assert_eq!(container_names(&object), ["app", "sidecar", "debug"]);
    }

    #[test]
    fn joins_and_navigates_container_paths() {
        assert_eq!(child_path("/", "etc"), "/etc");
        assert_eq!(child_path("/var", "log"), "/var/log");
        assert_eq!(parent_path("/var/log"), "/var");
        assert_eq!(parent_path("/etc"), "/");
    }

    #[test]
    fn verbose_container_errors_are_reduced_for_the_ui() {
        let missing_shell = "Internal error occurred: error executing command in container: exec: \"/bin/sh\": stat /bin/sh: no such file or directory";
        assert_eq!(
            concise_file_error(missing_shell),
            "File browser unavailable: this container does not include /bin/sh."
        );
        assert_eq!(
            concise_file_error("can't create /tmp/file: Read-only file system"),
            "This location is read-only."
        );
        assert_eq!(
            concise_file_error("not a directory: /missing"),
            "not a directory: /missing"
        );
        assert_eq!(
            concise_file_error(&"internal failure ".repeat(20)),
            "File operation failed. See the browser console for details."
        );
    }
}
