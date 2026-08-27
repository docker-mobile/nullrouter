//! JSON Schema normalization for the Gemini API.
//!
//! Ports `cleanJSONSchemaForAntigravity` and its helpers from
//! `open-sse/translator/formats/gemini.js`. Gemini rejects large parts of JSON
//! Schema, so tool parameter schemas are rewritten into the subset it accepts.

use serde_json::{Map, Value, json};

/// Keywords Gemini rejects. Complex keywords (`anyOf`/`oneOf`/`allOf`) appear
/// here too and are stripped only after the flattening phases have consumed them.
/// (Upstream lists `deprecated` twice; it is deduplicated here.)
const UNSUPPORTED_SCHEMA_CONSTRAINTS: &[&str] = &[
    // Basic constraints.
    "minLength",
    "maxLength",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "minItems",
    "maxItems",
    "format",
    // Claude rejects these in VALIDATED mode.
    "default",
    "examples",
    // JSON Schema meta keywords.
    "$schema",
    "$defs",
    "definitions",
    "const",
    "$ref",
    "$comment",
    // Annotation keywords.
    "deprecated",
    "readOnly",
    "writeOnly",
    // Object validation keywords.
    "additionalProperties",
    "propertyNames",
    "patternProperties",
    "enumDescriptions",
    // Complex schema keywords.
    "anyOf",
    "oneOf",
    "allOf",
    "not",
    // Dependency keywords.
    "dependencies",
    "dependentSchemas",
    "dependentRequired",
    // Other unsupported keywords.
    "title",
    "optional",
    "if",
    "then",
    "else",
    "contentMediaType",
    "contentEncoding",
    // UI/styling properties leaking in from some client tool definitions.
    "cornerRadius",
    "fillColor",
    "fontFamily",
    "fontSize",
    "fontWeight",
    "gap",
    "padding",
    "strokeColor",
    "strokeThickness",
    "textColor",
];

/// Gemini's default safety settings (all categories off).
pub fn default_safety_settings() -> Value {
    json!([
        { "category": "HARM_CATEGORY_HATE_SPEECH", "threshold": "OFF" },
        { "category": "HARM_CATEGORY_DANGEROUS_CONTENT", "threshold": "OFF" },
        { "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT", "threshold": "OFF" },
        { "category": "HARM_CATEGORY_HARASSMENT", "threshold": "OFF" },
        { "category": "HARM_CATEGORY_CIVIC_INTEGRITY", "threshold": "OFF" },
    ])
}

/// Rewrite a tool parameter schema into the subset Gemini accepts.
pub fn clean_schema(schema: &Value) -> Value {
    if !schema.is_object() && !schema.is_array() {
        return schema.clone();
    }
    let mut cleaned = schema.clone();

    // Phase 1: convert and prepare.
    convert_const_to_enum(&mut cleaned);
    convert_enum_values_to_strings(&mut cleaned);
    // Phase 2: flatten complex structures.
    merge_all_of(&mut cleaned);
    flatten_any_of_one_of(&mut cleaned);
    flatten_type_arrays(&mut cleaned);
    // Phase 2.5: Gemini requires an explicit type when properties exist.
    ensure_object_type(&mut cleaned);
    // Phase 3: strip unsupported keywords at every level.
    remove_unsupported_keywords(&mut cleaned);
    // Phase 4: drop `required` entries with no matching property.
    cleanup_required(&mut cleaned);
    // Phase 5: empty object schemas need a placeholder property.
    add_placeholders(&mut cleaned);

    cleaned
}

/// Apply `visit` to every nested object/array value, depth-first after the
/// current level, matching the JS helpers' `Object.values` recursion.
fn recurse_children(node: &mut Value, visit: fn(&mut Value)) {
    match node {
        Value::Object(map) => {
            for value in map.values_mut() {
                if value.is_object() || value.is_array() {
                    visit(value);
                }
            }
        }
        Value::Array(items) => {
            for value in items {
                if value.is_object() || value.is_array() {
                    visit(value);
                }
            }
        }
        _ => {}
    }
}

fn convert_const_to_enum(node: &mut Value) {
    if let Some(map) = node.as_object_mut()
        && let Some(constant) = map.get("const").cloned()
        && !map.contains_key("enum")
    {
        map.insert("enum".to_owned(), json!([constant]));
        map.remove("const");
    }
    recurse_children(node, convert_const_to_enum);
}

fn convert_enum_values_to_strings(node: &mut Value) {
    if let Some(map) = node.as_object_mut()
        && let Some(Value::Array(values)) = map.get("enum").cloned()
    {
        let stringified: Vec<Value> = values
            .iter()
            .map(|value| {
                Value::String(
                    value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_owned),
                )
            })
            .collect();
        map.insert("enum".to_owned(), Value::Array(stringified));
        // Gemini returns 400 for an enum without an explicit string type.
        map.entry("type").or_insert_with(|| json!("string"));
    }
    recurse_children(node, convert_enum_values_to_strings);
}

fn merge_all_of(node: &mut Value) {
    if let Some(map) = node.as_object_mut()
        && let Some(Value::Array(items)) = map.get("allOf").cloned()
    {
        let mut merged_properties = Map::new();
        let mut merged_required: Vec<Value> = Vec::new();

        for item in &items {
            if let Some(properties) = item.get("properties").and_then(Value::as_object) {
                for (key, value) in properties {
                    merged_properties.insert(key.clone(), value.clone());
                }
            }
            if let Some(required) = item.get("required").and_then(Value::as_array) {
                for field in required {
                    if !merged_required.contains(field) {
                        merged_required.push(field.clone());
                    }
                }
            }
        }

        map.remove("allOf");
        if !merged_properties.is_empty() {
            let mut properties = map
                .get("properties")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            for (key, value) in merged_properties {
                properties.insert(key, value);
            }
            map.insert("properties".to_owned(), Value::Object(properties));
        }
        if !merged_required.is_empty() {
            let mut required = map
                .get("required")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            required.extend(merged_required);
            map.insert("required".to_owned(), Value::Array(required));
        }
    }
    recurse_children(node, merge_all_of);
}

/// Score a candidate schema: objects beat arrays beat plain types
/// (upstream `selectBest`).
fn select_best(items: &[Value]) -> usize {
    let mut best_index = 0;
    let mut best_score = -1_i32;
    for (index, item) in items.iter().enumerate() {
        let kind = item.get("type").and_then(Value::as_str);
        let score = if kind == Some("object") || item.get("properties").is_some() {
            3
        } else if kind == Some("array") || item.get("items").is_some() {
            2
        } else {
            i32::from(kind.is_some_and(|kind| kind != "null"))
        };
        if score > best_score {
            best_score = score;
            best_index = index;
        }
    }
    best_index
}

fn flatten_any_of_one_of(node: &mut Value) {
    for keyword in ["anyOf", "oneOf"] {
        let Some(map) = node.as_object_mut() else {
            break;
        };
        let Some(Value::Array(items)) = map.get(keyword).cloned() else {
            continue;
        };
        if items.is_empty() {
            continue;
        }
        let non_null: Vec<Value> = items
            .into_iter()
            .filter(|item| {
                item.is_object() && item.get("type").and_then(Value::as_str) != Some("null")
            })
            .collect();
        if non_null.is_empty() {
            continue;
        }
        let selected = non_null
            .get(select_best(&non_null))
            .cloned()
            .unwrap_or(Value::Null);
        map.remove(keyword);
        if let Some(fields) = selected.as_object() {
            for (key, value) in fields {
                map.insert(key.clone(), value.clone());
            }
        }
    }
    recurse_children(node, flatten_any_of_one_of);
}

fn flatten_type_arrays(node: &mut Value) {
    if let Some(map) = node.as_object_mut()
        && let Some(Value::Array(types)) = map.get("type").cloned()
    {
        let first_non_null = types
            .iter()
            .find(|kind| kind.as_str() != Some("null"))
            .cloned()
            .unwrap_or_else(|| json!("string"));
        map.insert("type".to_owned(), first_non_null);
    }
    recurse_children(node, flatten_type_arrays);
}

fn ensure_object_type(node: &mut Value) {
    if let Some(map) = node.as_object_mut()
        && map.contains_key("properties")
        && !map.contains_key("type")
    {
        map.insert("type".to_owned(), json!("object"));
    }
    recurse_children(node, ensure_object_type);
}

fn remove_unsupported_keywords(node: &mut Value) {
    if let Some(map) = node.as_object_mut() {
        map.retain(|key, _| {
            !UNSUPPORTED_SCHEMA_CONSTRAINTS.contains(&key.as_str()) && !key.starts_with("x-")
        });
    }
    recurse_children(node, remove_unsupported_keywords);
}

fn cleanup_required(node: &mut Value) {
    if let Some(map) = node.as_object_mut()
        && let Some(Value::Array(required)) = map.get("required").cloned()
        && let Some(properties) = map.get("properties").and_then(Value::as_object).cloned()
    {
        let valid: Vec<Value> = required
            .into_iter()
            .filter(|field| {
                field
                    .as_str()
                    .is_some_and(|field| properties.contains_key(field))
            })
            .collect();
        if valid.is_empty() {
            map.remove("required");
        } else {
            map.insert("required".to_owned(), Value::Array(valid));
        }
    }
    recurse_children(node, cleanup_required);
}

fn add_placeholders(node: &mut Value) {
    if let Some(map) = node.as_object_mut()
        && map.get("type").and_then(Value::as_str) == Some("object")
    {
        let is_empty = map
            .get("properties")
            .and_then(Value::as_object)
            .is_none_or(Map::is_empty);
        if is_empty {
            map.insert(
                "properties".to_owned(),
                json!({
                    "reason": {
                        "type": "string",
                        "description": "Brief explanation of why you are calling this tool",
                    },
                }),
            );
            map.insert("required".to_owned(), json!(["reason"]));
        }
    }
    recurse_children(node, add_placeholders);
}

/// Sanitize a function name for Gemini: must start with `[a-zA-Z_]`, contain
/// only `[a-zA-Z0-9_.:-]`, and be at most 64 characters.
pub fn sanitize_function_name(name: &str) -> String {
    if name.is_empty() {
        return "_unknown".to_owned();
    }
    let mut sanitized: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if !sanitized
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
    {
        sanitized.insert(0, '_');
    }
    // Truncate on a char boundary; sanitization leaves only ASCII.
    sanitized.truncate(64);
    sanitized
}
