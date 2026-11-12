// Copyright The OpenTelemetry Authors
// SPDX-License-Identifier: Apache-2.0

//! Application build-time code generation using the OpenTelemetry Weaver CLI.
//!
//! Weaver resolves and validates the registry; this crate turns selected groups
//! into typed Rust declarations. No Weaver resolver or logging runtime is linked
//! into this build helper.

#![doc = include_str!("../README.md")]

use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const OUTPUT_FILE: &str = "slogr_attributes.rs";
const RESOLVED_FILE: &str = "attributes.json";

/// A registry, configuration, subprocess, or output error.
#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Result of generating application attributes.
pub type Result<T> = std::result::Result<T, Error>;

/// Generate a public `attributes` module from selected Weaver registry groups.
///
/// The output is `slogr_attributes.rs` in Cargo's `OUT_DIR`, unless overridden.
/// The Weaver executable is selected by [`Self::weaver`], `SLOGR_WEAVER`, or
/// `weaver` on `PATH`, in that order. Use the patched Weaver in `tools/weaver`;
/// it separates `--project-config` from the template `--config` option.
#[derive(Debug)]
pub struct Builder {
    registry: PathBuf,
    groups: BTreeSet<String>,
    weaver: Option<OsString>,
    config: Option<PathBuf>,
    out_dir: Option<PathBuf>,
    watched: Vec<PathBuf>,
}

impl Builder {
    /// Read a local Weaver registry directory or manifest.
    ///
    /// Relative paths are relative to the application build script's working
    /// directory, normally `CARGO_MANIFEST_DIR`. Dependencies in the manifest
    /// may be local registries or pinned remote semantic-convention registries.
    #[must_use]
    pub fn new(registry: impl Into<PathBuf>) -> Self {
        Self {
            registry: registry.into(),
            groups: BTreeSet::new(),
            weaver: None,
            config: None,
            out_dir: None,
            watched: Vec::new(),
        }
    }

    /// Select a group containing attributes declared or referenced by the application.
    ///
    /// Call repeatedly to select multiple groups. Missing groups and conflicting
    /// declarations fail generation instead of silently omitting attributes.
    #[must_use]
    pub fn group(mut self, id: impl Into<String>) -> Self {
        self.groups.insert(id.into());
        self
    }

    /// Override the Weaver CLI executable.
    #[must_use]
    pub fn weaver(mut self, executable: impl Into<OsString>) -> Self {
        self.weaver = Some(executable.into());
        self
    }

    /// Use and watch an explicit Weaver project configuration file.
    ///
    /// By default an empty configuration disables Weaver's upward-directory
    /// discovery, preventing untracked ambient configuration from affecting builds.
    #[must_use]
    pub fn config(mut self, path: impl Into<PathBuf>) -> Self {
        self.config = Some(path.into());
        self
    }

    /// Override Cargo's output directory, for example when running outside build.rs.
    #[must_use]
    pub fn out_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.out_dir = Some(path.into());
        self
    }

    /// Watch another Cargo input, such as a local dependency registry or policy
    /// outside the main registry directory.
    #[must_use]
    pub fn watch(mut self, path: impl Into<PathBuf>) -> Self {
        self.watched.push(path.into());
        self
    }

    /// Resolve the registry and generate Rust declarations.
    ///
    /// Emits Cargo rerun directives, propagates Weaver failures with diagnostics,
    /// and never falls back to stale output. Successful subprocess diagnostics
    /// are surfaced as Cargo warnings.
    pub fn generate(self) -> Result<PathBuf> {
        if self.groups.is_empty() || self.groups.iter().any(|id| id.is_empty()) {
            return Err(Error(
                "select at least one non-empty Weaver group with .group(...)".into(),
            ));
        }
        let registry = fs::canonicalize(&self.registry)
            .map_err(|error| io_error("opening registry", &self.registry, error))?;
        let watched_registry = if registry.is_dir() {
            registry.clone()
        } else {
            registry
                .parent()
                .expect("canonical file has a parent")
                .to_owned()
        };
        let out_dir = self
            .out_dir
            .or_else(|| env::var_os("OUT_DIR").map(PathBuf::from))
            .ok_or_else(|| Error("OUT_DIR is not set; use build.rs or .out_dir(...)".into()))?;
        fs::create_dir_all(&out_dir)
            .map_err(|error| io_error("creating output directory", &out_dir, error))?;
        let out_dir = fs::canonicalize(&out_dir)
            .map_err(|error| io_error("opening output directory", &out_dir, error))?;
        if out_dir.starts_with(&watched_registry) {
            return Err(Error(
                "the output directory must be outside the registry directory".into(),
            ));
        }

        println!("cargo:rerun-if-changed={}", watched_registry.display());
        println!("cargo:rerun-if-env-changed=SLOGR_WEAVER");
        println!("cargo:rerun-if-env-changed=PATH");
        for path in &self.watched {
            println!("cargo:rerun-if-changed={}", path.display());
        }
        let executable = self
            .weaver
            .or_else(|| env::var_os("SLOGR_WEAVER"))
            .unwrap_or_else(|| OsString::from("weaver"));
        let executable = locate_executable(executable);
        if Path::new(&executable).is_file() {
            println!(
                "cargo:rerun-if-changed={}",
                Path::new(&executable).display()
            );
        }

        let work = out_dir.join("slogr-weaver");
        let templates = work.join("templates").join("slogr");
        let resolved = work.join("resolved");
        fs::create_dir_all(&templates)
            .map_err(|error| io_error("creating templates", &templates, error))?;
        fs::create_dir_all(&resolved)
            .map_err(|error| io_error("creating resolved output", &resolved, error))?;
        let project_config = if let Some(path) = self.config {
            let path = fs::canonicalize(&path)
                .map_err(|error| io_error("opening Weaver project configuration", &path, error))?;
            println!("cargo:rerun-if-changed={}", path.display());
            path
        } else {
            let path = work.join("project.toml");
            write(&path, b"")?;
            path
        };

        // JSON is valid YAML. Serialize the group IDs rather than interpolating
        // unescaped strings into the JQ expression or template configuration.
        let selection = serde_json::to_string(&self.groups)
            .map_err(|error| Error(format!("serializing group selection: {error}")))?;
        let config = json!({
            "templates": [{
                "template": "attributes.json.j2",
                "filter": format!(".groups | map(select(.id as $id | {selection} | index($id)))"),
                "application_mode": "single",
                "file_name": RESOLVED_FILE
            }]
        });
        write(
            &templates.join("weaver.yaml"),
            config.to_string().as_bytes(),
        )?;
        write(
            &templates.join("attributes.json.j2"),
            b"{{ ctx | tojson }}\n",
        )?;
        let resolved_file = resolved.join(RESOLVED_FILE);
        match fs::remove_file(&resolved_file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(io_error(
                    "removing stale resolved output",
                    &resolved_file,
                    error,
                ));
            }
        }

        let output = Command::new(&executable)
            .arg("--project-config")
            .arg(&project_config)
            .args(["--quiet", "registry", "generate", "slogr"])
            .arg(&resolved)
            .arg("--registry")
            .arg(&registry)
            .arg("--templates")
            .arg(work.join("templates"))
            .args([
                "--v2=false",
                "--include-unreferenced=false",
                "--skip-policies=false",
                "--diagnostic-format",
                "json",
            ])
            .output()
            .map_err(|error| {
                Error(format!(
                    "could not run Weaver {:?}: {error}; build tools/weaver and set SLOGR_WEAVER",
                    executable
                ))
            })?;
        if !output.status.success() {
            return Err(Error(format!(
                "Weaver generation failed ({}):\n{}{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            )));
        }
        for stream in [&output.stdout, &output.stderr] {
            for line in String::from_utf8_lossy(stream)
                .lines()
                .filter(|line| !line.is_empty())
            {
                println!("cargo:warning=Weaver: {line}");
            }
        }
        let bytes = fs::read(&resolved_file).map_err(|error| {
            io_error(
                "reading Weaver's resolved attributes",
                &resolved_file,
                error,
            )
        })?;
        let source = render(&bytes, &self.groups)?;
        let destination = out_dir.join(OUTPUT_FILE);
        write(&destination, source.as_bytes())?;
        Ok(destination)
    }
}

fn locate_executable(executable: OsString) -> OsString {
    let path = Path::new(&executable);
    if path.components().count() > 1 {
        return executable;
    }
    if let Some(paths) = env::var_os("PATH") {
        for directory in env::split_paths(&paths) {
            let candidate = directory.join(path);
            if candidate.is_file() {
                return candidate.into_os_string();
            }
            #[cfg(windows)]
            {
                let candidate = candidate.with_extension("exe");
                if candidate.is_file() {
                    return candidate.into_os_string();
                }
            }
        }
    }
    executable
}

fn io_error(action: &str, path: &Path, error: std::io::Error) -> Error {
    Error(format!("{action} {}: {error}", path.display()))
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).map_err(|error| io_error("writing", path, error))
}

#[derive(Deserialize)]
struct Group {
    id: String,
    #[serde(default)]
    attributes: Vec<ResolvedAttribute>,
}

#[derive(Deserialize)]
struct ResolvedAttribute {
    name: String,
    #[serde(default)]
    brief: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    annotations: Option<BTreeMap<String, Value>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SlogrAnnotation {
    #[serde(rename = "type", default = "string_type")]
    rust_type: String,
}

fn string_type() -> String {
    "str".into()
}

#[derive(Debug, PartialEq)]
struct Declaration {
    rust_type: String,
    description: String,
}

fn render(bytes: &[u8], selected: &BTreeSet<String>) -> Result<String> {
    let groups: Vec<Group> = serde_json::from_slice(bytes)
        .map_err(|error| Error(format!("invalid Weaver v1 resolved group output: {error}")))?;
    let mut found = BTreeSet::new();
    let mut attributes = BTreeMap::new();
    for group in groups {
        if !selected.contains(&group.id) || !found.insert(group.id.clone()) {
            return Err(Error(format!(
                "unexpected or duplicate resolved group {:?}",
                group.id
            )));
        }
        for attribute in group.attributes {
            let mut rust_type = string_type();
            if let Some(annotations) = &attribute.annotations {
                if let Some(key) = annotations.keys().find(|key| key.starts_with("slogr.")) {
                    return Err(Error(format!(
                        "attribute {:?}: use nested annotations.slogr.type, not annotation {key:?}",
                        attribute.name
                    )));
                }
                if let Some(value) = annotations.get("slogr") {
                    rust_type = serde_json::from_value::<SlogrAnnotation>(value.clone())
                        .map_err(|error| {
                            Error(format!(
                                "attribute {:?}: invalid slogr annotation: {error}",
                                attribute.name
                            ))
                        })?
                        .rust_type;
                }
            }
            if !matches!(
                rust_type.as_str(),
                "str"
                    | "bool"
                    | "i8"
                    | "i16"
                    | "i32"
                    | "i64"
                    | "isize"
                    | "u8"
                    | "u16"
                    | "u32"
                    | "u64"
                    | "usize"
                    | "f32"
                    | "f64"
            ) {
                return Err(Error(format!(
                    "attribute {:?}: unsupported slogr type {rust_type:?}; expected str, bool, a native integer up to 64 bits, or f32/f64",
                    attribute.name
                )));
            }
            let description = match (attribute.brief.is_empty(), attribute.note.is_empty()) {
                (true, _) => attribute.note,
                (_, true) => attribute.brief,
                _ => format!("{}\n\n{}", attribute.brief, attribute.note),
            };
            let declaration = Declaration {
                rust_type,
                description,
            };
            if let Some(previous) = attributes.get(&attribute.name) {
                if previous != &declaration {
                    return Err(Error(format!(
                        "conflicting type or description for attribute {:?}",
                        attribute.name
                    )));
                }
            } else {
                attributes.insert(attribute.name, declaration);
            }
        }
    }
    let missing = selected.difference(&found).cloned().collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(Error(format!(
            "Weaver registry groups not found: {}",
            missing.join(", ")
        )));
    }

    let mut symbols = BTreeMap::new();
    let mut source = String::from(
        "// Generated by slogr-build from an OpenTelemetry Weaver registry. DO NOT EDIT.\n\
         pub mod attributes {\n    #![allow(non_upper_case_globals)]\n",
    );
    for (name, declaration) in attributes {
        let symbol = rust_identifier(&name)?;
        if let Some(previous) = symbols.insert(symbol.clone(), name.clone()) {
            return Err(Error(format!(
                "attributes {previous:?} and {name:?} both generate Rust identifier {symbol:?}"
            )));
        }
        let description = rust_string(&declaration.description);
        source.push_str(&format!(
            "\n    #[doc = {description}]\n    pub static {symbol}: ::slogr::Attribute<{}> =\n        ::slogr::Attribute::new({}, {description});\n",
            declaration.rust_type, rust_string(&name),
        ));
    }
    source.push_str("}\n");
    Ok(source)
}

fn rust_identifier(name: &str) -> Result<String> {
    let mut symbol = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            symbol.push(character.to_ascii_lowercase());
        } else if !symbol.is_empty() && !symbol.ends_with('_') {
            symbol.push('_');
        }
    }
    let mut symbol = symbol.trim_end_matches('_').to_owned();
    if symbol.is_empty() {
        return Err(Error(format!(
            "attribute {name:?} has no usable Rust identifier"
        )));
    }
    if symbol.as_bytes()[0].is_ascii_digit()
        || matches!(symbol.as_str(), "self" | "super" | "crate")
    {
        symbol.insert_str(0, "attr_");
    }
    if matches!(
        symbol.as_str(),
        "as" | "break"
            | "const"
            | "continue"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "static"
            | "struct"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "dyn"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
            | "try"
            | "gen"
    ) {
        symbol.insert_str(0, "r#");
    }
    Ok(symbol)
}

fn rust_string(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .chars()
            .flat_map(char::escape_default)
            .collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate(attributes: Value) -> Result<String> {
        render(
            &serde_json::to_vec(&json!([{"id": "app", "attributes": attributes}])).unwrap(),
            &BTreeSet::from(["app".into()]),
        )
    }

    /// Scenario: resolved attributes have standard types and optional slogr extensions.
    /// Guarantees: string is the default even for semantic integers, explicit types win, and descriptions are escaped Rust literals.
    #[test]
    fn default_strings_and_explicit_types() -> Result<()> {
        let source = generate(json!([
            {"name": "server.port", "type": "int", "brief": "Port"},
            {"name": "app.count", "type": "int", "annotations": {"slogr": {"type": "u64"}}},
            {"name": "app.ready", "annotations": {"slogr": {"type": "bool"}}},
            {"name": "app.ratio", "annotations": {"slogr": {"type": "f64"}}},
            {"name": "app.delta", "annotations": {"slogr": {"type": "i64"}}},
            {"name": "type", "brief": "A \"quote\" and \u{0008}", "note": "line\n\u{00e9}"},
            {"name": "self"},
            {"name": "1st.value"}
        ]))?;
        assert!(source.contains("#![allow(non_upper_case_globals)]"));
        assert!(source.contains("server_port: ::slogr::Attribute<str>"));
        assert!(source.contains("app_count: ::slogr::Attribute<u64>"));
        assert!(source.contains("app_ready: ::slogr::Attribute<bool>"));
        assert!(source.contains("app_ratio: ::slogr::Attribute<f64>"));
        assert!(source.contains("app_delta: ::slogr::Attribute<i64>"));
        assert!(source.contains("pub static r#type:"));
        assert!(source.contains("pub static attr_self:"));
        assert!(source.contains("pub static attr_1st_value:"));
        assert!(source.contains(r#"A \"quote\" and \u{8}\n\nline\n\u{e9}"#));
        assert!(source.is_ascii());
        Ok::<_, Error>(())
    }

    /// Scenario: different wire names normalize to one symbol, or the same key has conflicting declarations.
    /// Guarantees: no attribute is silently overwritten, and identical declarations deduplicate deterministically.
    #[test]
    fn rejects_collisions_and_conflicts() {
        let error = generate(json!([{"name": "a.b"}, {"name": "a_b"}])).unwrap_err();
        assert!(error.to_string().contains("both generate Rust identifier"));
        let error = generate(json!([
            {"name": "a"}, {"name": "a", "annotations": {"slogr": {"type": "u64"}}}
        ]))
        .unwrap_err();
        assert!(error.to_string().contains("conflicting type"));
        let first = generate(json!([{"name": "b"}, {"name": "a"}, {"name": "a"}])).unwrap();
        let second = generate(json!([{"name": "a"}, {"name": "b"}])).unwrap();
        assert_eq!(first, second);
    }

    /// Scenario: a slogr extension is malformed, misspelled, or asks for an unsupported type.
    /// Guarantees: invalid explicit configuration never quietly falls back to string.
    #[test]
    fn rejects_invalid_annotations_and_missing_groups() {
        for annotations in [
            json!({"slogr": {"type": "u128"}}),
            json!({"slogr": {"type": null}}),
            json!({"slogr": {"typo": "u64"}}),
            json!({"slogr": "u64"}),
            json!({"slogr.type": "u64"}),
        ] {
            assert!(generate(json!([{"name": "app.count", "annotations": annotations}])).is_err());
        }
        assert!(
            render(b"[]", &BTreeSet::from(["missing".into()]))
                .unwrap_err()
                .to_string()
                .contains("not found")
        );
    }

    /// Scenario: a build has valid local inputs but its configured Weaver executable is missing.
    /// Guarantees: generation fails with an actionable diagnostic instead of reusing prior generated code.
    #[test]
    fn missing_weaver_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let registry = temp.path().join("registry");
        let output = temp.path().join("out");
        fs::create_dir(&registry).unwrap();
        fs::create_dir(&output).unwrap();
        fs::write(output.join(OUTPUT_FILE), "stale output").unwrap();
        let error = Builder::new(registry)
            .group("app")
            .weaver(temp.path().join("no-such-weaver"))
            .out_dir(&output)
            .generate()
            .unwrap_err();
        assert!(error.to_string().contains("could not run Weaver"));
        assert!(error.to_string().contains("SLOGR_WEAVER"));
        assert_eq!(
            fs::read_to_string(output.join(OUTPUT_FILE)).unwrap(),
            "stale output"
        );
    }

    /// Scenario: Weaver reports a failure or claims success without writing its resolved output.
    /// Guarantees: diagnostics propagate and stale intermediate data cannot turn either failure into success.
    #[cfg(unix)]
    #[test]
    fn subprocess_failures_do_not_reuse_stale_data() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let registry = temp.path().join("registry");
        let output = temp.path().join("out");
        let fake = temp.path().join("weaver");
        fs::create_dir(&registry).unwrap();
        for (script, expected) in [
            (
                "#!/bin/sh\nprintf 'registry is invalid' >&2\nexit 7\n",
                "registry is invalid",
            ),
            (
                "#!/bin/sh\nexit 0\n",
                "reading Weaver's resolved attributes",
            ),
        ] {
            let intermediate = output.join("slogr-weaver/resolved");
            fs::create_dir_all(&intermediate).unwrap();
            fs::write(
                intermediate.join(RESOLVED_FILE),
                r#"[{"id":"app","attributes":[]}]"#,
            )
            .unwrap();
            fs::write(&fake, script).unwrap();
            fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
            let error = Builder::new(&registry)
                .group("app")
                .weaver(fake.as_os_str())
                .out_dir(&output)
                .generate()
                .unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
            assert!(!output.join(OUTPUT_FILE).exists());
        }
    }
}
