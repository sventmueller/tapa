//! A checker for values against the OpenAPI schema of OpenCode 2.2.6 in `tests/fixtures`: the JSON Schema keywords
//! the schema uses (`$ref`, `anyOf`, `allOf`, `not`, `enum`, `type`, `properties`, `required`,
//! `additionalProperties`, `items`, `prefixItems`, `minItems`), and of `pattern` the prefixes that ids carry, such as
//! `^ses`. Operations are found by method and path, with `{param}` segments and a trailing `*`.

use serde_json::{Map, Value};

/// The schema, read once per test.
pub(crate) struct Spec {
    root: Value,
}

/// An operation of the schema with the values of its path parameters.
pub(crate) struct Operation<'a> {
    pub(crate) template: String,
    pub(crate) op: &'a Value,
    pub(crate) params: Vec<(String, String)>,
}

impl Spec {
    pub(crate) fn load() -> Spec {
        let path = format!("{}/tests/fixtures/openapi-2.2.6.json", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(path).unwrap();
        Spec {
            root: serde_json::from_str(&text).unwrap(),
        }
    }

    pub(crate) fn schema(&self, name: &str) -> &Value {
        self.root["components"]["schemas"]
            .get(name)
            .unwrap_or_else(|| panic!("no schema {name}"))
    }

    fn resolve<'a>(&'a self, schema: &'a Value) -> &'a Value {
        match schema.get("$ref").and_then(Value::as_str) {
            Some(reference) => {
                let name = reference.trim_start_matches("#/components/schemas/");
                self.resolve(self.schema(name))
            }
            None => schema,
        }
    }

    /// Whether `value` fits `schema`; the error names where it does not.
    pub(crate) fn check(&self, schema: &Value, value: &Value, at: &str) -> Result<(), String> {
        let schema = self.resolve(schema);
        if let Some(any) = schema
            .get("anyOf")
            .or_else(|| schema.get("oneOf"))
            .and_then(Value::as_array)
        {
            let errors: Vec<String> = any.iter().filter_map(|s| self.check(s, value, at).err()).collect();
            if errors.len() == any.len() {
                return Err(format!(
                    "{at}: fits none of {} choices: {}",
                    any.len(),
                    errors.join(" | ")
                ));
            }
        }
        for part in schema.get("allOf").and_then(Value::as_array).into_iter().flatten() {
            self.check(part, value, at)?;
        }
        if let Some(not) = schema.get("not")
            && not.as_object().is_some_and(|o| !o.is_empty())
            && self.check(not, value, at).is_ok()
        {
            return Err(format!("{at}: {value} is what the schema rules out"));
        }
        if let Some(choices) = schema.get("enum").and_then(Value::as_array)
            && !choices.contains(value)
        {
            return Err(format!("{at}: {value} is none of {choices:?}"));
        }
        let kind = schema.get("type").and_then(Value::as_str);
        match kind {
            Some("object") => self.object(schema, value, at),
            None if schema.get("properties").is_some() => self.object(schema, value, at),
            Some("array") => self.array(schema, value, at),
            Some("string") => {
                let text = value.as_str().ok_or_else(|| format!("{at}: {value} is no string"))?;
                prefix(schema, text, at)
            }
            Some("number") if !value.is_number() => Err(format!("{at}: {value} is no number")),
            Some("integer") if !(value.is_i64() || value.is_u64()) => Err(format!("{at}: {value} is no integer")),
            Some("boolean") if !value.is_boolean() => Err(format!("{at}: {value} is no boolean")),
            Some("null") if !value.is_null() => Err(format!("{at}: {value} is not null")),
            _ => Ok(()),
        }
    }

    fn object(&self, schema: &Value, value: &Value, at: &str) -> Result<(), String> {
        let object = value.as_object().ok_or_else(|| format!("{at}: {value} is no object"))?;
        let empty = Map::new();
        let properties = schema.get("properties").and_then(Value::as_object).unwrap_or(&empty);
        for name in schema.get("required").and_then(Value::as_array).into_iter().flatten() {
            let name = name.as_str().unwrap_or_default();
            if !object.contains_key(name) {
                return Err(format!("{at}: {name} is missing"));
            }
        }
        for (name, member) in object {
            let here = format!("{at}.{name}");
            match (properties.get(name), schema.get("additionalProperties")) {
                (Some(property), _) => self.check(property, member, &here)?,
                (None, _) if schema.get("patternProperties").is_some() => {}
                (None, Some(Value::Bool(false))) => return Err(format!("{here} is not in the schema")),
                (None, Some(extra)) if extra.is_object() => self.check(extra, member, &here)?,
                (None, _) => {}
            }
        }
        Ok(())
    }

    fn array(&self, schema: &Value, value: &Value, at: &str) -> Result<(), String> {
        let items = value.as_array().ok_or_else(|| format!("{at}: {value} is no array"))?;
        if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
            && (items.len() as u64) < min
        {
            return Err(format!("{at}: fewer than {min} items"));
        }
        let prefix_items = schema.get("prefixItems").and_then(Value::as_array);
        for (n, item) in items.iter().enumerate() {
            let here = format!("{at}[{n}]");
            match (prefix_items.and_then(|p| p.get(n)), schema.get("items")) {
                (Some(first), _) => self.check(first, item, &here)?,
                (None, Some(each)) => self.check(each, item, &here)?,
                (None, None) => {}
            }
        }
        Ok(())
    }

    /// The schema of an event of `kind`, from the members of `V2Event`.
    pub(crate) fn event_schema(&self, kind: &str) -> &Value {
        let members = self.schema("V2Event")["anyOf"].as_array().unwrap();
        members
            .iter()
            .map(|m| self.resolve(m))
            .find(|m| {
                m["properties"]["type"]["enum"]
                    .as_array()
                    .is_some_and(|e| e.iter().any(|t| t == kind))
            })
            .unwrap_or_else(|| panic!("no event {kind} in the schema"))
    }

    pub(crate) fn check_event(&self, event: &Value) -> Result<(), String> {
        let kind = event["type"].as_str().ok_or("an event without a type")?;
        self.check(self.event_schema(kind), event, kind)
    }

    /// The operation for `method` on `path`.
    pub(crate) fn operation(&self, method: &str, path: &str) -> Option<Operation<'_>> {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let paths = self.root["paths"].as_object()?;
        for (template, item) in paths {
            let Some(op) = item.get(method.to_ascii_lowercase()) else {
                continue;
            };
            let parts: Vec<&str> = template.split('/').filter(|s| !s.is_empty()).collect();
            let wild = parts.last() == Some(&"*");
            let fixed = if wild { &parts[..parts.len() - 1] } else { &parts[..] };
            if (wild && segments.len() < fixed.len()) || (!wild && segments.len() != fixed.len()) {
                continue;
            }
            let mut params = Vec::new();
            let fits = fixed.iter().zip(&segments).all(|(part, segment)| {
                if part.starts_with('{') {
                    params.push((part.trim_matches(['{', '}']).to_string(), segment.to_string()));
                    true
                } else {
                    part == segment
                }
            });
            if fits {
                return Some(Operation {
                    template: template.clone(),
                    op,
                    params,
                });
            }
        }
        None
    }

    /// Whether a request of `method` to `path` with `query` and `body` is one the schema describes.
    pub(crate) fn check_request(
        &self,
        method: &str,
        path: &str,
        query: &Map<String, Value>,
        body: &Value,
    ) -> Result<(), String> {
        let at = format!("{method} {path}");
        let operation = self
            .operation(method, path)
            .ok_or_else(|| format!("{at}: no such operation"))?;
        let declared = operation.op["parameters"].as_array().cloned().unwrap_or_default();
        for (name, value) in &operation.params {
            let param = declared
                .iter()
                .find(|p| p["name"] == name.as_str() && p["in"] == "path");
            let param = param.ok_or_else(|| format!("{at}: path parameter {name} is not declared"))?;
            let value = percent_decoded(value);
            self.check(&param["schema"], &Value::String(value), &format!("{at} {name}"))?;
        }
        for (name, value) in query {
            let base = name.split('[').next().unwrap_or(name);
            let param = declared.iter().find(|p| p["name"] == base && p["in"] == "query");
            let param = param.ok_or_else(|| format!("{at}: query parameter {name} is not declared"))?;
            if base == name.as_str() && param.get("style") != Some(&Value::from("deepObject")) {
                self.check(&param["schema"], value, &format!("{at} ?{name}"))?;
            }
        }
        match operation.op["requestBody"]["content"]["application/json"].get("schema") {
            Some(schema) if !body.is_null() => self.check(schema, body, &format!("{at} body")),
            Some(_) => match operation.op["requestBody"]["required"].as_bool() {
                Some(true) => Err(format!("{at}: the body is missing")),
                _ => Ok(()),
            },
            None if body.is_null() => Ok(()),
            None => Err(format!("{at}: takes no body, got {body}")),
        }
    }

    /// Whether `body` with `status` is an answer `method` on `path` may give.
    pub(crate) fn check_response(&self, method: &str, path: &str, status: u16, body: &Value) -> Result<(), String> {
        let at = format!("{method} {path} {status}");
        let operation = self
            .operation(method, path)
            .ok_or_else(|| format!("{at}: no such operation"))?;
        let response = operation.op["responses"]
            .get(status.to_string())
            .ok_or_else(|| format!("{at}: not a status of the operation"))?;
        match response["content"]["application/json"].get("schema") {
            Some(schema) => self.check(schema, body, &at),
            None if body.is_null() => Ok(()),
            None => Err(format!("{at}: has no body, got {body}")),
        }
    }
}

fn percent_decoded(text: &str) -> String {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = text.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The prefix of a pattern like `^ses` or `^msg_`, which ids carry; other patterns are not checked.
fn prefix(schema: &Value, text: &str, at: &str) -> Result<(), String> {
    let Some(pattern) = schema.get("pattern").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(start) = pattern.strip_prefix('^') else {
        return Ok(());
    };
    if !start.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Ok(());
    }
    if text.starts_with(start) {
        Ok(())
    } else {
        Err(format!("{at}: {text:?} does not start with {start:?}"))
    }
}
