use leptos::prelude::*;
use serde_json::{Map, Value};

use crate::app::util::format::{camel_label, counted};

pub(crate) fn json_fields(value: Value) -> AnyView {
    json_fields_except(value, &[])
}

pub(crate) fn json_fields_except(value: Value, excluded: &[&str]) -> AnyView {
    let fields = match value {
        Value::Object(object) => object
            .into_iter()
            .filter(|(key, _)| !excluded.contains(&key.as_str()))
            .map(|(key, value)| field_view(key, value))
            .collect::<Vec<_>>(),
        value => vec![field_view("Value".to_string(), value)],
    };
    view! { <div class="json-fields">{fields}</div> }.into_any()
}

fn field_view(key: String, value: Value) -> AnyView {
    match value {
        Value::Object(object) if object.is_empty() => scalar_view(key, "(empty object)".into()),
        Value::Object(object) => {
            let label = camel_label(&key);
            view! {
                <section class="json-field-group">
                    <h5 title=key>{label}</h5>
                    {object_view(object)}
                </section>
            }
            .into_any()
        }
        Value::Array(values) if values.is_empty() => scalar_view(key, "(empty list)".into()),
        Value::Array(values) if values.iter().all(is_scalar) => scalar_array_view(key, values),
        Value::Array(values) => object_array_view(key, values),
        value => scalar_view(key, scalar_text(value)),
    }
}

fn object_view(object: Map<String, Value>) -> AnyView {
    view! {
        <div class="json-fields">
            {object.into_iter().map(|(key, value)| field_view(key, value)).collect_view()}
        </div>
    }
    .into_any()
}

fn object_array_view(key: String, values: Vec<Value>) -> AnyView {
    let label = camel_label(&key);
    let count = values.len();
    view! {
        <section class="json-field-collection">
            <div class="json-field-group-title">
                <h5 title=key>{label}</h5>
                <small>{counted(count, "item", "items")}</small>
            </div>
            <div class="json-field-items">
                {values.into_iter().enumerate().map(|(index, value)| {
                    let field_count = value_count(&value);
                    let (title, identity_key) = item_title(&value, index);
                    let body = match value {
                        Value::Object(object) if object.is_empty() => {
                            scalar_view("Value".into(), "(empty object)".into())
                        }
                        Value::Object(mut object) => {
                            if let Some(key) = identity_key {
                                object.remove(key);
                            }
                            object_view(object)
                        }
                        value => field_view(format!("Item {}", index + 1), value),
                    };
                    view! {
                        <details class="json-field-item" open=field_count <= 8>
                            <summary>
                                <strong>{title}</strong>
                                <small>{counted(field_count, "field", "fields")}</small>
                            </summary>
                            {body}
                        </details>
                    }
                }).collect_view()}
            </div>
        </section>
    }
    .into_any()
}

fn scalar_array_view(key: String, values: Vec<Value>) -> AnyView {
    let label = camel_label(&key);
    view! {
        <div class="json-field-row json-field-sequence-row">
            <span class="json-field-key" title=key>{label}</span>
            <span class="json-field-sequence">
                {values.into_iter().map(|value| view! { <code>{scalar_text(value)}</code> }).collect_view()}
            </span>
        </div>
    }
    .into_any()
}

fn scalar_view(key: String, value: String) -> AnyView {
    let label = camel_label(&key);
    view! {
        <div class="json-field-row">
            <span class="json-field-key" title=key>{label}</span>
            <span class="json-field-value">{value}</span>
        </div>
    }
    .into_any()
}

fn item_title(value: &Value, index: usize) -> (String, Option<&'static str>) {
    let Some(object) = value.as_object() else {
        return (format!("Item {}", index + 1), None);
    };
    for key in ["name", "key", "path", "mountPath", "containerPort", "type"] {
        if let Some(value) = object.get(key).and_then(short_scalar) {
            return (value, Some(key));
        }
    }
    if let [key] = object.keys().collect::<Vec<_>>().as_slice() {
        return (camel_label(key), None);
    }
    (format!("Item {}", index + 1), None)
}

fn short_scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn is_scalar(value: &Value) -> bool {
    !matches!(value, Value::Object(_) | Value::Array(_))
}

fn value_count(value: &Value) -> usize {
    match value {
        Value::Object(values) => values.values().map(value_count).sum::<usize>().max(1),
        Value::Array(values) => values.iter().map(value_count).sum::<usize>().max(1),
        _ => 1,
    }
}

fn scalar_text(value: Value) -> String {
    match value {
        Value::String(value) if value.is_empty() => "(empty string)".into(),
        Value::String(value) => value,
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Null => "null".into(),
        Value::Object(_) | Value::Array(_) => unreachable!("nested values are rendered separately"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn array_item_titles_use_stable_identity_fields() {
        assert_eq!(
            item_title(&json!({"name": "api", "image": "api:latest"}), 0),
            ("api".into(), Some("name"))
        );
        assert_eq!(
            item_title(&json!({"serviceAccountToken": {"path": "token"}}), 1),
            ("Service Account Token".into(), None)
        );
        assert_eq!(item_title(&json!({}), 2), ("Item 3".into(), None));
    }

    #[test]
    fn nested_field_count_includes_every_leaf() {
        assert_eq!(
            value_count(&json!({
                "args": ["--port", "8080"],
                "probe": {"path": "/health", "port": 8080},
                "securityContext": {}
            })),
            5
        );
    }
}
