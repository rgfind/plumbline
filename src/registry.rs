//! The claim checker. A registry lists contract *claims*: a dotted path into the
//! contract fixture, a comparison mode, and the expected value. `check_claims`
//! asserts every claim still holds, returning one human-readable failure per
//! drift. This is the machine-readable successor to a hand-kept coverage matrix.

use serde_json::Value;

/// Check every claim against the fixture. Returns a failure string per drifted
/// or unresolvable claim; an empty vec means every claim holds.
pub fn check_claims(fixture: &Value, claims: &[Value]) -> Vec<String> {
    let mut failures = Vec::new();
    for claim in claims {
        let id = claim["id"].as_str().unwrap_or("<unnamed>");
        let path = claim["fixture_path"].as_str().unwrap_or("");
        let mode = claim["mode"].as_str().unwrap_or("value");
        let expected = &claim["expected"];

        let resolved = match resolve(fixture, path) {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("[{id}] fixture path `{path}` did not resolve: {e}"));
                continue;
            }
        };

        let (actual_repr, ok) = match mode {
            "value" => (json_compact(resolved), resolved == expected),
            "keys" => match object_keys_sorted(resolved) {
                Ok(keys) => {
                    let want = string_vec(expected);
                    (json_list(&keys), Some(keys) == want)
                }
                Err(e) => {
                    failures.push(format!("[{id}] mode=keys: {e}"));
                    continue;
                }
            },
            "set" => match array_strings_sorted(resolved) {
                Ok(vals) => {
                    let want = string_vec(expected);
                    (json_list(&vals), Some(vals) == want)
                }
                Err(e) => {
                    failures.push(format!("[{id}] mode=set: {e}"));
                    continue;
                }
            },
            other => {
                failures.push(format!("[{id}] unknown mode `{other}`"));
                continue;
            }
        };

        if !ok {
            failures.push(format!(
                "[{id}] drift: expected {}, fixture has {}",
                json_compact(expected),
                actual_repr
            ));
        }
    }
    failures
}

/// Resolve a dotted path over the fixture. An empty path is the root. A numeric
/// segment indexes an array; any other segment is an object key.
pub fn resolve<'a>(root: &'a Value, path: &str) -> Result<&'a Value, String> {
    if path.is_empty() {
        return Ok(root);
    }
    let mut cur = root;
    for seg in path.split('.') {
        cur = match cur {
            Value::Array(items) => {
                let idx: usize = seg
                    .parse()
                    .map_err(|_| format!("segment `{seg}` is not an array index"))?;
                items
                    .get(idx)
                    .ok_or_else(|| format!("index {idx} out of range"))?
            }
            Value::Object(map) => map.get(seg).ok_or_else(|| format!("key `{seg}` absent"))?,
            _ => return Err(format!("cannot descend into scalar at `{seg}`")),
        };
    }
    Ok(cur)
}

fn object_keys_sorted(v: &Value) -> Result<Vec<String>, String> {
    let obj = v.as_object().ok_or("resolved value is not an object")?;
    let mut keys: Vec<String> = obj.keys().cloned().collect();
    keys.sort();
    Ok(keys)
}

fn array_strings_sorted(v: &Value) -> Result<Vec<String>, String> {
    let arr = v.as_array().ok_or("resolved value is not an array")?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        out.push(
            item.as_str()
                .ok_or("array element is not a string")?
                .to_string(),
        );
    }
    out.sort();
    Ok(out)
}

fn string_vec(expected: &Value) -> Option<Vec<String>> {
    let arr = expected.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        out.push(item.as_str()?.to_string());
    }
    out.sort();
    Some(out)
}

fn json_compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "<unserializable>".into())
}

fn json_list(v: &[String]) -> String {
    json_compact(&Value::from(v.to_vec()))
}

/// Drop volatile meta fields so two captures compare on contract content only.
pub fn normalized(doc: &Value, fields: &[String]) -> Value {
    let mut d = doc.clone();
    if let Some(meta) = d.get_mut("meta").and_then(Value::as_object_mut) {
        for f in fields {
            meta.remove(f);
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        json!({
            "exit_codes": {"0": {}, "1": {}, "5": {}},
            "warning_codes": ["A", "B", "C"],
            "verbs": {"find": {"stage": "enum[found,fd_name]"}}
        })
    }

    #[test]
    fn value_mode_passes_and_drifts() {
        let pass = vec![
            json!({"id":"s","fixture_path":"verbs.find.stage","mode":"value","expected":"enum[found,fd_name]"}),
        ];
        assert!(check_claims(&fixture(), &pass).is_empty());
        let drift = vec![
            json!({"id":"s","fixture_path":"verbs.find.stage","mode":"value","expected":"enum[found]"}),
        ];
        assert_eq!(check_claims(&fixture(), &drift).len(), 1);
    }

    #[test]
    fn keys_and_set_modes_are_order_independent() {
        let keys = vec![
            json!({"id":"e","fixture_path":"exit_codes","mode":"keys","expected":["5","1","0"]}),
        ];
        assert!(check_claims(&fixture(), &keys).is_empty());
        let set = vec![
            json!({"id":"w","fixture_path":"warning_codes","mode":"set","expected":["C","A","B"]}),
        ];
        assert!(check_claims(&fixture(), &set).is_empty());
    }

    #[test]
    fn unresolvable_path_is_a_failure() {
        let bad = vec![
            json!({"id":"x","fixture_path":"verbs.missing.stage","mode":"value","expected":1}),
        ];
        assert_eq!(check_claims(&fixture(), &bad).len(), 1);
    }

    #[test]
    fn normalized_drops_volatile_meta() {
        let doc = json!({"meta":{"request_id":"abc","verb":"x"},"data":[]});
        let n = normalized(&doc, &["request_id".to_string()]);
        assert!(n["meta"].get("request_id").is_none());
        assert_eq!(n["meta"]["verb"], json!("x"));
    }
}
