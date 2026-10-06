//! Tiny helpers for the generic profile: dotted paths into JSON values and
//! the request body template.

use serde_json::{Map, Value};

/// Value at `path` (`choices.0.delta.content`); `""` or `"$"` is the root.
pub fn get<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let path = path.trim();
    if path.is_empty() || path == "$" {
        return Some(value);
    }
    path.split('.')
        .try_fold(value, |current, segment| match current {
            Value::Array(items) => segment.parse::<usize>().ok().and_then(|i| items.get(i)),
            Value::Object(map) => map.get(segment),
            _ => None,
        })
}

/// String at `path` (numbers and booleans are rendered).
pub fn get_text(value: &Value, path: &str) -> Option<String> {
    match get(value, path)? {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        other => Some(other.to_string()),
    }
}

pub fn get_u64(value: &Value, path: &str) -> Option<u64> {
    match get(value, path)? {
        Value::Number(n) => n.as_u64().or_else(|| n.as_f64().map(|f| f.max(0.0) as u64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Values that can replace placeholders in a body template.
pub struct TemplateVars<'a> {
    pub model: &'a str,
    pub system: &'a str,
    pub prompt: &'a str,
    pub messages: Value,
    pub stream: bool,
    pub max_tokens: Option<u32>,
}

/// Fills a body template:
/// - a string that is exactly `{{messages}}`, `{{stream}}` or
///   `{{maxTokens}}` becomes that JSON value (`maxTokens` → `null` when
///   unset);
/// - `{{model}}`, `{{system}}` and `{{prompt}}` are replaced inside strings.
pub fn render(template: &Value, vars: &TemplateVars<'_>) -> Value {
    match template {
        Value::String(s) => match s.trim() {
            "{{messages}}" => vars.messages.clone(),
            "{{stream}}" => Value::Bool(vars.stream),
            "{{maxTokens}}" => vars.max_tokens.map_or(Value::Null, Value::from),
            _ => Value::String(interpolate(s, vars)),
        },
        Value::Array(items) => Value::Array(items.iter().map(|v| render(v, vars)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), render(v, vars)))
                .collect::<Map<_, _>>(),
        ),
        other => other.clone(),
    }
}

/// Replaces `{{model}}`, `{{system}}` and `{{prompt}}` inside `text`.
pub fn interpolate(text: &str, vars: &TemplateVars<'_>) -> String {
    text.replace("{{model}}", vars.model)
        .replace("{{system}}", vars.system)
        .replace("{{prompt}}", vars.prompt)
}

/// Recursively merges `extra` into `base` (objects merge, everything else
/// replaces).
pub fn merge(base: &mut Value, extra: &Value) {
    match (base, extra) {
        (Value::Object(base), Value::Object(extra)) => {
            for (key, value) in extra {
                match base.get_mut(key) {
                    Some(existing) if existing.is_object() && value.is_object() => {
                        merge(existing, value)
                    }
                    _ => {
                        base.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base, extra) if !extra.is_null() => *base = extra.clone(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths_walk_objects_and_arrays() {
        let v = json!({"choices": [{"delta": {"content": "oi"}}], "usage": {"in": 3, "s": "4"}});
        assert_eq!(
            get_text(&v, "choices.0.delta.content").as_deref(),
            Some("oi")
        );
        assert_eq!(get_u64(&v, "usage.in"), Some(3));
        assert_eq!(get_u64(&v, "usage.s"), Some(4));
        assert!(get(&v, "choices.1").is_none());
        assert!(get(&v, "choices.x").is_none());
        assert_eq!(get(&v, ""), Some(&v));
    }

    #[test]
    fn template_replaces_values_and_text() {
        let template = json!({
            "model": "{{model}}",
            "messages": "{{messages}}",
            "stream": "{{stream}}",
            "options": {"num_predict": "{{maxTokens}}"},
            "prompt": "S: {{system}}\n{{prompt}}"
        });
        let body = render(
            &template,
            &TemplateVars {
                model: "llama",
                system: "sys",
                prompt: "hi",
                messages: json!([{"role": "user", "content": "hi"}]),
                stream: true,
                max_tokens: None,
            },
        );
        assert_eq!(
            body,
            json!({
                "model": "llama",
                "messages": [{"role": "user", "content": "hi"}],
                "stream": true,
                "options": {"num_predict": null},
                "prompt": "S: sys\nhi"
            })
        );
    }

    #[test]
    fn merge_is_deep() {
        let mut base = json!({"a": 1, "o": {"x": 1, "y": 2}});
        merge(&mut base, &json!({"o": {"y": 3, "z": 4}, "b": true}));
        assert_eq!(
            base,
            json!({"a": 1, "o": {"x": 1, "y": 3, "z": 4}, "b": true})
        );
        merge(&mut base, &Value::Null);
        assert_eq!(base["a"], 1);
    }
}
