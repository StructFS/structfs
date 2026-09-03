//! Assembly definitions (`isotope/spec/02-assemblies.md`).
//!
//! An assembly definition is an immutable value: blocks, a public block,
//! wiring, config, failure policies, and imports. YAML and JSON are both
//! accepted; both deserialize to the same `Value`, which is the canonical
//! form.

use std::collections::BTreeMap;

use structfs_core_store::{Path, Value};

use crate::block::FailurePolicy;
use crate::error::{Result, RuntimeError};

/// One block reference within an assembly.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockDef {
    /// Artifact reference: `builtin:{name}`, a `.wasm` path, or a nested
    /// assembly definition file (`.json`/`.yaml`/`.yml`).
    pub artifact: String,
    /// Declared serialization format (default `application/json`).
    pub serialization: String,
    /// Optional content hash (accepted, not verified in the strawman).
    pub hash: Option<String>,
}

/// A wiring target.
#[derive(Debug, Clone, PartialEq)]
pub enum WireTarget {
    /// Another block in this assembly, by local name.
    Block(String),
    /// An import provided by the instantiating parent (`$name`).
    Import(String),
}

/// One wiring entry: `block:/prefix -> target`.
#[derive(Debug, Clone, PartialEq)]
pub struct WireDef {
    /// The block whose namespace gains the mount.
    pub block: String,
    /// The mount prefix in that block's namespace.
    pub prefix: Path,
    /// Where operations under the prefix go.
    pub target: WireTarget,
}

/// A parsed assembly definition.
#[derive(Debug, Clone, PartialEq)]
pub struct AssemblyDef {
    pub name: String,
    pub version: Option<String>,
    pub blocks: BTreeMap<String, BlockDef>,
    pub public: String,
    pub wiring: Vec<WireDef>,
    pub config: BTreeMap<String, Value>,
    pub failure: BTreeMap<String, FailurePolicy>,
    /// Imports the parent must provide: name -> description.
    pub imports: BTreeMap<String, String>,
}

fn expect_string(value: &Value, what: &str) -> Result<String> {
    match value {
        Value::String(s) => Ok(s.clone()),
        _ => Err(RuntimeError::assembly(format!("{what} must be a string"))),
    }
}

fn parse_wire_line(line: &str) -> Result<WireDef> {
    // Syntax: "block:/prefix -> target"
    let (left, right) = line
        .split_once("->")
        .ok_or_else(|| RuntimeError::assembly(format!("wiring line missing '->': {line}")))?;
    let (block, prefix) = left
        .trim()
        .split_once(':')
        .ok_or_else(|| RuntimeError::assembly(format!("wiring line missing ':': {line}")))?;
    let prefix = Path::parse(prefix.trim())
        .map_err(|e| RuntimeError::assembly(format!("bad wiring prefix in '{line}': {e}")))?;
    if prefix.is_empty() {
        return Err(RuntimeError::assembly(format!(
            "wiring prefix may not be the namespace root: {line}"
        )));
    }
    if prefix[0] == "iso" {
        return Err(RuntimeError::assembly(format!(
            "the iso/ prefix is reserved and cannot be wired: {line}"
        )));
    }
    let target = right.trim();
    let target = match target.strip_prefix('$') {
        Some(import) => WireTarget::Import(import.to_string()),
        None => WireTarget::Block(target.to_string()),
    };
    Ok(WireDef {
        block: block.trim().to_string(),
        prefix,
        target,
    })
}

impl AssemblyDef {
    /// Parse a definition from its canonical `Value` form.
    pub fn from_value(value: &Value) -> Result<Self> {
        let map = match value {
            Value::Map(map) => map,
            _ => return Err(RuntimeError::assembly("definition must be a map")),
        };

        let name = expect_string(
            map.get("assembly")
                .ok_or_else(|| RuntimeError::assembly("missing 'assembly' name"))?,
            "assembly",
        )?;
        let version = match map.get("version") {
            Some(v) => Some(expect_string(v, "version")?),
            None => None,
        };

        let mut blocks = BTreeMap::new();
        match map.get("blocks") {
            Some(Value::Map(entries)) => {
                for (block_name, entry) in entries {
                    let def = match entry {
                        // Short form: artifact string, JSON serialization.
                        Value::String(artifact) => BlockDef {
                            artifact: artifact.clone(),
                            serialization: "application/json".to_string(),
                            hash: None,
                        },
                        Value::Map(fields) => BlockDef {
                            artifact: expect_string(
                                fields.get("wasm").or_else(|| fields.get("artifact")).ok_or_else(
                                    || {
                                        RuntimeError::assembly(format!(
                                            "block '{block_name}' missing artifact"
                                        ))
                                    },
                                )?,
                                "artifact",
                            )?,
                            serialization: match fields.get("serialization") {
                                Some(v) => expect_string(v, "serialization")?,
                                None => "application/json".to_string(),
                            },
                            hash: match fields.get("hash") {
                                Some(v) => Some(expect_string(v, "hash")?),
                                None => None,
                            },
                        },
                        _ => {
                            return Err(RuntimeError::assembly(format!(
                                "block '{block_name}' must be a string or map"
                            )))
                        }
                    };
                    blocks.insert(block_name.clone(), def);
                }
            }
            _ => return Err(RuntimeError::assembly("missing 'blocks' map")),
        }

        let public = expect_string(
            map.get("public")
                .ok_or_else(|| RuntimeError::assembly("missing 'public' block name"))?,
            "public",
        )?;
        if !blocks.contains_key(&public) {
            return Err(RuntimeError::assembly(format!(
                "public block '{public}' is not in blocks"
            )));
        }

        let mut wiring = Vec::new();
        if let Some(Value::Array(lines)) = map.get("wiring") {
            for line in lines {
                wiring.push(parse_wire_line(&expect_string(line, "wiring entry")?)?);
            }
        }

        let mut imports = BTreeMap::new();
        if let Some(Value::Map(entries)) = map.get("imports") {
            for (import_name, description) in entries {
                imports.insert(
                    import_name.clone(),
                    expect_string(description, "import description")?,
                );
            }
        }

        // Validate wiring references.
        for wire in &wiring {
            if !blocks.contains_key(&wire.block) {
                return Err(RuntimeError::assembly(format!(
                    "wiring references unknown block '{}'",
                    wire.block
                )));
            }
            match &wire.target {
                WireTarget::Block(target) if !blocks.contains_key(target) => {
                    return Err(RuntimeError::assembly(format!(
                        "wiring references unknown target block '{target}'"
                    )));
                }
                WireTarget::Import(import) if !imports.contains_key(import) => {
                    return Err(RuntimeError::assembly(format!(
                        "wiring references undeclared import '${import}'"
                    )));
                }
                _ => {}
            }
        }

        let mut config = BTreeMap::new();
        if let Some(Value::Map(entries)) = map.get("config") {
            for (block_name, block_config) in entries {
                config.insert(block_name.clone(), block_config.clone());
            }
        }

        let mut failure = BTreeMap::new();
        if let Some(Value::Map(entries)) = map.get("failure") {
            for (block_name, policy) in entries {
                let policy = match expect_string(policy, "failure policy")?.as_str() {
                    "fail-fast" => FailurePolicy::FailFast,
                    "isolate" => FailurePolicy::Isolate,
                    other => {
                        return Err(RuntimeError::assembly(format!(
                            "unsupported failure policy '{other}' (strawman supports fail-fast, isolate)"
                        )))
                    }
                };
                failure.insert(block_name.clone(), policy);
            }
        }

        Ok(Self {
            name,
            version,
            blocks,
            public,
            wiring,
            config,
            failure,
            imports,
        })
    }

    /// Parse a definition from JSON or YAML source text.
    pub fn from_str(source: &str) -> Result<Self> {
        let json: serde_json::Value = if source.trim_start().starts_with('{') {
            serde_json::from_str(source)
                .map_err(|e| RuntimeError::assembly(format!("bad JSON definition: {e}")))?
        } else {
            serde_yaml::from_str(source)
                .map_err(|e| RuntimeError::assembly(format!("bad YAML definition: {e}")))?
        };
        Self::from_value(&structfs_serde_store::json_to_value(json))
    }

    /// The failure policy for a block (default fail-fast).
    pub fn failure_policy(&self, block: &str) -> FailurePolicy {
        self.failure.get(block).copied().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::path;

    const DEMO_YAML: &str = r#"
assembly: demo
version: "1.0.0"
blocks:
  shell: builtin:shell
  kv:
    artifact: builtin:kv
    serialization: application/json
public: shell
wiring:
  - "shell:/services/kv -> kv"
config:
  shell:
    prompt: "iso> "
failure:
  kv: isolate
"#;

    #[test]
    fn parses_yaml_definition() {
        let def = AssemblyDef::from_str(DEMO_YAML).unwrap();
        assert_eq!(def.name, "demo");
        assert_eq!(def.public, "shell");
        assert_eq!(def.blocks["shell"].artifact, "builtin:shell");
        assert_eq!(def.blocks["kv"].serialization, "application/json");
        assert_eq!(def.wiring.len(), 1);
        assert_eq!(def.wiring[0].prefix, path!("services/kv"));
        assert_eq!(def.wiring[0].target, WireTarget::Block("kv".to_string()));
        assert_eq!(def.failure_policy("kv"), FailurePolicy::Isolate);
        assert_eq!(def.failure_policy("shell"), FailurePolicy::FailFast);
    }

    #[test]
    fn parses_json_definition() {
        let def = AssemblyDef::from_str(
            r#"{"assembly": "j", "blocks": {"a": "builtin:echo"}, "public": "a"}"#,
        )
        .unwrap();
        assert_eq!(def.name, "j");
    }

    #[test]
    fn rejects_unknown_public() {
        let err = AssemblyDef::from_str(
            r#"{"assembly": "x", "blocks": {"a": "builtin:echo"}, "public": "nope"}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("public"));
    }

    #[test]
    fn rejects_wiring_to_unknown_block() {
        let err = AssemblyDef::from_str(
            r#"{"assembly": "x", "blocks": {"a": "builtin:echo"}, "public": "a",
                "wiring": ["a:/services/b -> b"]}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown target"));
    }

    #[test]
    fn rejects_wiring_iso_prefix() {
        let err = AssemblyDef::from_str(
            r#"{"assembly": "x", "blocks": {"a": "builtin:echo"}, "public": "a",
                "wiring": ["a:/iso/time -> a"]}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn imports_are_declared_and_referenced() {
        let def = AssemblyDef::from_str(
            r#"{"assembly": "x", "blocks": {"a": "builtin:echo"}, "public": "a",
                "imports": {"logger": "Logging service"},
                "wiring": ["a:/services/logger -> $logger"]}"#,
        )
        .unwrap();
        assert_eq!(
            def.wiring[0].target,
            WireTarget::Import("logger".to_string())
        );

        let err = AssemblyDef::from_str(
            r#"{"assembly": "x", "blocks": {"a": "builtin:echo"}, "public": "a",
                "wiring": ["a:/services/logger -> $logger"]}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("undeclared import"));
    }
}
