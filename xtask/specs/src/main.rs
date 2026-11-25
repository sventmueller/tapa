//! `cargo run --manifest-path xtask/specs/Cargo.toml -- <withfig/autocomplete checkout>` turns the static part of the
//! Fig completion specs into `crates/term-suggest/specs/fig.tsv` (E8): one spec per line, its name, a tab and its
//! JSON. Only literals are taken over: objects, arrays, strings, numbers and what top-level constants and relative
//! imports name; functions, generators' scripts and anything computed drop out (R10). Nothing of the checkout runs.

mod eval;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Map, Value};

/// The keys of a spec, subcommand, option, argument or suggestion that `term-suggest` reads.
const KEPT: &[&str] = &[
    "name",
    "description",
    "subcommands",
    "options",
    "args",
    "suggestions",
    "template",
    "isOptional",
    "isVariadic",
    "isRepeatable",
    "isPersistent",
    "isCommand",
    "loadSpec",
    "hidden",
    "requiresSeparator",
    "requiresEquals",
    "dynamic",
];

/// Folders of sub-specs left out: the cloud CLIs' own (each service a spec, 67 of 78 MB) and examples; their
/// top-level spec stays.
const SKIPPED_TREES: &[&str] = &["aws", "gcloud", "az", "example"];

/// The most characters of a description kept: the field shows one line.
const LONGEST_DESCRIPTION: usize = 100;

fn main() -> ExitCode {
    let Some(checkout) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: fig-specs <withfig/autocomplete checkout> [output file]");
        return ExitCode::FAILURE;
    };
    let output = std::env::args().nth(2).map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/term-suggest/specs/fig.tsv"),
        PathBuf::from,
    );
    match convert(&checkout.join("src"), &output) {
        Ok((count, bytes)) => {
            println!("{count} specs, {bytes} bytes in {}", output.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("fig-specs: {error}");
            ExitCode::FAILURE
        }
    }
}

fn convert(src: &Path, output: &Path) -> Result<(usize, usize), String> {
    let mut files = Vec::new();
    collect(src, &mut files).map_err(|e| format!("{}: {e}", src.display()))?;
    files.sort();
    let allocator = oxc_allocator::Allocator::default();
    let mut modules = eval::Modules::new(&allocator);
    let mut specs = BTreeMap::new();
    for file in &files {
        let Some(name) = spec_name(src, file) else { continue };
        let Some(value) = modules.default_export(file) else {
            continue;
        };
        let Some(spec) = clean(&value, "spec") else { continue };
        if spec.get("name").is_some() {
            specs.insert(name, spec);
        }
    }
    let mut text = String::new();
    for (name, spec) in &specs {
        text.push_str(name);
        text.push('\t');
        text.push_str(&serde_json::to_string(spec).map_err(|e| e.to_string())?);
        text.push('\n');
    }
    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(output, &text).map_err(|e| format!("{}: {e}", output.display()))?;
    Ok((specs.len(), text.len()))
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(&path, files)?;
        } else if path.extension().is_some_and(|e| e == "ts") && !path.to_string_lossy().ends_with(".d.ts") {
            files.push(path);
        }
    }
    Ok(())
}

/// The name `loadSpec` and the first word of a command line use for `file`: its path under `src` without `.ts`.
fn spec_name(src: &Path, file: &Path) -> Option<String> {
    let relative = file.strip_prefix(src).ok()?.with_extension("");
    let name = relative.to_string_lossy().replace('\\', "/");
    let skipped = name.starts_with('@')
        || name.contains("test")
        || SKIPPED_TREES.iter().any(|tree| name.starts_with(&format!("{tree}/")));
    (!skipped).then_some(name)
}

/// The value with only the keys `term-suggest` reads, names and arguments always as lists; `None` when nothing is
/// left. `role` is what the value is: a spec, an option, an argument or a suggestion.
fn clean(value: &Value, role: &str) -> Option<Value> {
    if role == "suggestion"
        && let Value::String(name) = value
    {
        return Some(Value::Object(Map::from_iter([(
            "name".to_string(),
            Value::Array(vec![Value::String(name.clone())]),
        )])));
    }
    let object = value.as_object()?;
    if object.get("hidden") == Some(&Value::Bool(true)) {
        return None;
    }
    let mut out = Map::new();
    for (key, value) in object {
        if !KEPT.contains(&key.as_str()) {
            continue;
        }
        let cleaned = match key.as_str() {
            "name" | "template" => strings(value),
            "subcommands" => list(value, "spec"),
            "options" => list(value, "option"),
            "args" => list(value, "arg"),
            "suggestions" => list(value, "suggestion"),
            "loadSpec" => value.as_str().map(|s| Value::String(s.to_string())),
            "description" => value.as_str().map(|s| Value::String(short(s))),
            "dynamic" => Some(value.clone()),
            _ => match value {
                Value::String(_) | Value::Bool(_) => Some(value.clone()),
                _ => None,
            },
        };
        if let Some(cleaned) = cleaned {
            out.insert(key.clone(), cleaned);
        }
    }
    if role == "arg"
        && let Some(generators) = object.get("generators")
    {
        take_generators(generators, &mut out);
    }
    if role != "arg" && !out.contains_key("name") {
        return None;
    }
    Some(Value::Object(out))
}

/// The templates of an argument's generators join its own; what they compute is kept as the names of the generators,
/// such as `gitGenerators.localBranches`, for the dynamic sources to recognise.
fn take_generators(generators: &Value, out: &mut Map<String, Value>) {
    let all = match generators {
        Value::Array(all) => all.clone(),
        one => vec![one.clone()],
    };
    for generator in all {
        if let Some(template) = generator.get("template").and_then(strings) {
            let entry = out.entry("template").or_insert_with(|| Value::Array(Vec::new()));
            if let (Value::Array(have), Value::Array(more)) = (entry, template) {
                have.extend(more);
            }
        }
        let dynamic = generator.get("dynamic").and_then(Value::as_str);
        if let Some(helper) = dynamic.filter(|name| name.starts_with("filepaths(") || name.starts_with("folders(")) {
            let only_folders = helper.starts_with("folders(") || helper.contains("showFolders: \"only\"");
            let template = if only_folders { "folders" } else { "filepaths" };
            let entry = out.entry("template").or_insert_with(|| Value::Array(Vec::new()));
            if let Value::Array(have) = entry {
                have.push(Value::String(template.to_string()));
            }
        } else if let Some(name) = dynamic {
            let entry = out.entry("dynamic").or_insert_with(|| Value::Array(Vec::new()));
            if let Value::Array(have) = entry {
                have.push(Value::String(name.to_string()));
            }
        }
    }
}

/// The first sentence of `text` on one line, cut at a word to at most `LONGEST_DESCRIPTION` characters.
fn short(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let sentence = line.find(". ").map_or(line.as_str(), |end| &line[..=end]);
    if sentence.chars().count() <= LONGEST_DESCRIPTION {
        return sentence.trim_end_matches('.').to_string();
    }
    let mut cut: String = sentence.chars().take(LONGEST_DESCRIPTION).collect();
    if let Some(space) = cut.rfind(' ') {
        cut.truncate(space);
    }
    format!("{cut}…")
}

fn strings(value: &Value) -> Option<Value> {
    match value {
        Value::String(s) => Some(Value::Array(vec![Value::String(s.clone())])),
        Value::Array(all) => {
            let kept: Vec<Value> = all.iter().filter(|v| v.is_string()).cloned().collect();
            (!kept.is_empty()).then_some(Value::Array(kept))
        }
        _ => None,
    }
}

fn list(value: &Value, role: &str) -> Option<Value> {
    let all = match value {
        Value::Array(all) => all.iter().filter_map(|item| clean(item, role)).collect::<Vec<_>>(),
        one => clean(one, role).into_iter().collect(),
    };
    (!all.is_empty()).then_some(Value::Array(all))
}
