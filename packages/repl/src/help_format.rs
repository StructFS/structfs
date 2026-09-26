//! Rendering help documents and the built-in command summary.

use std::collections::BTreeMap;

use nu_ansi_term::{Color, Style};

use structfs_core_store::Value;

use crate::command_table::COMMANDS;
use crate::commands::format_ir_value;
use crate::mounts::DEFAULT_MOUNTS;

fn heading(text: &str) -> String {
    Style::new().bold().paint(text).to_string()
}

fn cmd_style() -> Style {
    Style::new().bold().fg(Color::Cyan)
}

fn arg_style() -> Style {
    Style::new().fg(Color::Yellow)
}

/// The built-in help: every command, the default mounts, register syntax.
pub fn format_help() -> String {
    let desc_style = Style::new().fg(Color::White);
    let mut help = format!("{}\n\n", heading("StructFS REPL"));

    for cmd in COMMANDS {
        let summary = if cmd.aliases.is_empty() {
            cmd.summary.to_string()
        } else {
            format!("{} (alias: {})", cmd.summary, cmd.aliases.join(", "))
        };
        help.push_str(&format!(
            "  {:<12} {:<20} {}\n",
            cmd_style().paint(cmd.name),
            arg_style().paint(cmd.usage),
            desc_style.paint(summary)
        ));
    }

    help.push_str(&format!("\n{}\n", heading("Default Mounts")));
    for mount in DEFAULT_MOUNTS {
        help.push_str(&format!(
            "  {:<24} {}\n",
            arg_style().paint(format!("/{}", mount.name)),
            mount.summary
        ));
    }

    help.push_str(&format!("\n{}\n", heading("Registers")));
    for (syntax, desc) in [
        ("@name <command>", "Capture output in register"),
        ("read @name", "Read register contents"),
        ("*@name", "Dereference register as path"),
    ] {
        help.push_str(&format!("  {:<24} {}\n", arg_style().paint(syntax), desc));
    }

    help
}

/// Root help: the built-in commands plus the topics the help store indexes.
pub fn format_help_root(topics: &Value) -> String {
    let mut output = format_help();

    if let Value::Array(topic_list) = topics {
        if !topic_list.is_empty() {
            output.push_str(&format!("\n{}\n", heading("Available Help Topics")));
            for topic in topic_list {
                if let Value::String(t) = topic {
                    output.push_str(&format!(
                        "  {} {}\n",
                        Color::Cyan.paint(format!("help {}", t)),
                        Color::White
                            .dimmed()
                            .paint(format!("(read /ctx/help/{})", t))
                    ));
                }
            }
        }
    }

    output
}

/// Render a help document, with `path` shown next to its title.
pub fn format_help_value_with_path(value: &Value, path: &str) -> String {
    match value {
        Value::Map(map) => {
            let title = match map.get("title") {
                Some(Value::String(title)) => format!(
                    "{} {}",
                    heading(title),
                    Color::Cyan.dimmed().paint(format!("({})", path))
                ),
                _ => Color::Cyan.dimmed().paint(path).to_string(),
            };
            format!("{title}\n\n{}", render_body(map, ""))
        }
        other => format_help_value(other, 0),
    }
}

/// Render a help document at an indent level (two spaces per level).
pub fn format_help_value(value: &Value, indent: usize) -> String {
    let pad = "  ".repeat(indent);
    match value {
        Value::Map(map) => {
            let mut output = String::new();
            if let Some(Value::String(title)) = map.get("title") {
                output.push_str(&format!("{}\n\n", heading(title)));
            }
            output.push_str(&render_body(map, &pad));
            output
        }
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr
            .iter()
            .map(|item| format!("{pad}• {}\n", format_help_value(item, indent)))
            .collect(),
        other => format_ir_value(other),
    }
}

fn str_field<'a>(map: &'a BTreeMap<String, Value>, key: &str) -> &'a str {
    match map.get(key) {
        Some(Value::String(s)) => s,
        _ => "",
    }
}

/// A `{key: description}` section: one aligned line per string entry.
fn key_value_section(
    out: &mut String,
    pad: &str,
    title: &str,
    entries: &BTreeMap<String, Value>,
    key_style: Style,
    width: usize,
) {
    out.push_str(&format!("{pad}{}\n", heading(title)));
    for (key, desc) in entries {
        if let Value::String(d) = desc {
            out.push_str(&format!("{pad}  {:<width$} {d}\n", key_style.paint(key)));
        }
    }
    out.push('\n');
}

/// A `{key: description}` section with the description on its own line.
fn two_line_section(out: &mut String, pad: &str, title: &str, entries: &BTreeMap<String, Value>) {
    out.push_str(&format!("{pad}{}\n", heading(title)));
    for (key, desc) in entries {
        if let Value::String(d) = desc {
            out.push_str(&format!(
                "{pad}  {}\n{pad}    {}\n",
                arg_style().paint(key),
                Color::White.dimmed().paint(d)
            ));
        }
    }
    out.push('\n');
}

/// Everything in a help map except its title.
fn render_body(map: &BTreeMap<String, Value>, pad: &str) -> String {
    let mut out = String::new();

    if let Some(Value::String(desc)) = map.get("description") {
        out.push_str(&format!("{pad}{desc}\n\n"));
    }
    // "not found" responses
    if let Some(Value::String(err)) = map.get("error") {
        out.push_str(&format!("{pad}{}\n", Color::Yellow.paint(err)));
    }
    if let Some(Value::String(hint)) = map.get("hint") {
        out.push_str(&format!("{pad}{hint}\n\n"));
    }

    if let Some(Value::Map(commands)) = map.get("commands") {
        let detailed = matches!(commands.values().next(), Some(Value::Map(_)));
        if detailed {
            // {cmd: {args, desc}}
            for (cmd, info) in commands {
                if let Value::Map(details) = info {
                    out.push_str(&format!(
                        "{pad}  {:<12} {:<20} {}\n",
                        cmd_style().paint(cmd),
                        arg_style().paint(str_field(details, "args")),
                        Style::new()
                            .fg(Color::White)
                            .paint(str_field(details, "desc"))
                    ));
                }
            }
            out.push('\n');
        } else {
            key_value_section(&mut out, pad, "Commands:", commands, cmd_style(), 24);
        }
    }

    if let Some(Value::Map(mounts)) = map.get("default_mounts") {
        key_value_section(&mut out, pad, "Default Mounts", mounts, arg_style(), 24);
    }
    if let Some(Value::Map(registers)) = map.get("registers") {
        key_value_section(&mut out, pad, "Registers", registers, arg_style(), 24);
    }

    if map.contains_key("topics") {
        out.push_str(&format!(
            "{pad}{}\n",
            Color::White
                .dimmed()
                .paint("More help: commands, mounts, http, paths, stores, examples")
        ));
    }

    // "not found" responses
    if let Some(Value::Array(topics)) = map.get("available_topics") {
        out.push_str(&format!("{pad}{}\n", heading("Available topics:")));
        for topic in topics {
            if let Value::String(t) = topic {
                out.push_str(&format!("{pad}  {}\n", Color::Cyan.paint(t)));
            }
        }
        out.push('\n');
    }

    if let Some(Value::Map(paths)) = map.get("paths") {
        two_line_section(&mut out, pad, "Paths", paths);
    }

    if let Some(Value::Array(steps)) = map.get("example") {
        out.push_str(&format!("{pad}{}\n", heading("Example")));
        for step in steps {
            if let Value::String(s) = step {
                let color = if s.starts_with('#') {
                    Color::White.dimmed()
                } else {
                    Color::Green.normal()
                };
                out.push_str(&format!("{pad}  {}\n", color.paint(s)));
            }
        }
        out.push('\n');
    }

    if let Some(Value::Map(ops)) = map.get("operations") {
        two_line_section(&mut out, pad, "Operations", ops);
    }
    if let Some(Value::Map(brokers)) = map.get("brokers") {
        key_value_section(&mut out, pad, "Brokers", brokers, arg_style(), 24);
    }
    if let Some(Value::Map(fmt)) = map.get("request_format") {
        key_value_section(&mut out, pad, "Request Format", fmt, cmd_style(), 16);
    }
    if let Some(Value::Map(syntax)) = map.get("syntax") {
        key_value_section(&mut out, pad, "Syntax", syntax, arg_style(), 16);
    }
    if let Some(Value::Map(configs)) = map.get("mount_configs") {
        key_value_section(&mut out, pad, "Mount Configs", configs, cmd_style(), 16);
    }

    if let Some(Value::Map(stores)) = map.get("stores") {
        out.push_str(&format!("{pad}{}\n", heading("Stores")));
        for (name, info) in stores {
            out.push_str(&format!("{pad}  {}\n", cmd_style().paint(name)));
            if let Value::Map(info) = info {
                if let Some(Value::String(d)) = info.get("description") {
                    out.push_str(&format!("{pad}    {d}\n"));
                }
                if let Some(Value::String(c)) = info.get("config") {
                    out.push_str(&format!("{pad}    Config: {}\n", Color::Green.paint(c)));
                }
            }
        }
        out.push('\n');
    }

    if let Some(Value::Array(examples)) = map.get("examples") {
        out.push_str(&format!("{pad}{}\n", heading("Examples")));
        for example in examples {
            if let Value::Map(ex) = example {
                if let Some(Value::String(title)) = ex.get("title") {
                    out.push_str(&format!("{pad}  {}\n", heading(title)));
                }
                if let Some(Value::Array(steps)) = ex.get("steps") {
                    for step in steps {
                        if let Value::String(s) = step {
                            out.push_str(&format!("{pad}    {}\n", Color::Green.paint(s)));
                        }
                    }
                }
            }
        }
        out.push('\n');
    }

    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::strip_ansi_codes;
    use collection_literals::btree;

    fn s(text: &str) -> Value {
        Value::from(text)
    }

    #[test]
    fn format_help_lists_every_command_and_default_mount() {
        let help = strip_ansi_codes(&format_help());
        for cmd in COMMANDS {
            assert!(help.contains(cmd.name), "{} missing", cmd.name);
        }
        for mount in DEFAULT_MOUNTS {
            assert!(help.contains(&format!("/{}", mount.name)));
        }
        assert!(help.contains("@name"));
        assert!(help.contains("*@"));
    }

    #[test]
    fn every_section_renders_at_every_indent() {
        let doc = Value::Map(btree! {
            "title".into() => s("Title"),
            "description".into() => s("Desc"),
            "error".into() => s("Err"),
            "hint".into() => s("Hint"),
            "commands".into() => Value::Map(btree! {"c".into() => s("cdesc")}),
            "default_mounts".into() => Value::Map(btree! {"/m".into() => s("mdesc")}),
            "registers".into() => Value::Map(btree! {"@r".into() => s("rdesc")}),
            "topics".into() => Value::Array(vec![]),
            "available_topics".into() => Value::Array(vec![s("t1")]),
            "paths".into() => Value::Map(btree! {"p".into() => s("pdesc")}),
            "example".into() => Value::Array(vec![s("# comment"), s("step")]),
            "operations".into() => Value::Map(btree! {"op".into() => s("opdesc")}),
            "brokers".into() => Value::Map(btree! {"b".into() => s("bdesc")}),
            "request_format".into() => Value::Map(btree! {"f".into() => s("fdesc")}),
            "syntax".into() => Value::Map(btree! {"x".into() => s("xdesc")}),
            "mount_configs".into() => Value::Map(btree! {"mc".into() => s("mcdesc")}),
            "stores".into() => Value::Map(btree! {"st".into() => Value::Map(btree! {
                "description".into() => s("stdesc"),
                "config".into() => s("stconfig"),
            })}),
            "examples".into() => Value::Array(vec![Value::Map(btree! {
                "title".into() => s("extitle"),
                "steps".into() => Value::Array(vec![s("exstep")]),
            })]),
        });
        let expected = [
            "Desc",
            "Err",
            "Hint",
            "cdesc",
            "mdesc",
            "rdesc",
            "More help",
            "t1",
            "pdesc",
            "# comment",
            "step",
            "opdesc",
            "bdesc",
            "fdesc",
            "xdesc",
            "mcdesc",
            "stdesc",
            "stconfig",
            "extitle",
            "exstep",
        ];
        for rendered in [
            format_help_value_with_path(&doc, "/ctx/help/x"),
            format_help_value(&doc, 0),
            format_help_value(&doc, 2),
        ] {
            let plain = strip_ansi_codes(&rendered);
            assert!(plain.starts_with("Title"), "{plain}");
            for text in expected {
                assert!(plain.contains(text), "{text} missing from {plain}");
            }
        }
        let indented = strip_ansi_codes(&format_help_value(&doc, 2));
        assert!(indented.contains("\n    Desc"), "{indented}");
        assert!(strip_ansi_codes(&format_help_value_with_path(&doc, "/p")).contains("(/p)"));
    }

    #[test]
    fn detailed_commands_render_args_and_desc() {
        let doc = Value::Map(btree! {
            "commands".into() => Value::Map(btree! {"read".into() => Value::Map(btree! {
                "args".into() => s("<path>"),
                "desc".into() => s("Read it"),
            })}),
        });
        let plain = strip_ansi_codes(&format_help_value(&doc, 0));
        assert!(
            plain.contains("<path>") && plain.contains("Read it"),
            "{plain}"
        );
    }

    #[test]
    fn untitled_maps_scalars_and_arrays_render() {
        let untitled = Value::Map(btree! {"description".into() => s("D")});
        assert!(strip_ansi_codes(&format_help_value_with_path(&untitled, "/p")).starts_with("/p"));
        assert_eq!(format_help_value_with_path(&s("plain"), "/p"), "plain");
        let list = format_help_value(&Value::Array(vec![s("a"), Value::Integer(1)]), 1);
        let plain = strip_ansi_codes(&list);
        assert!(plain.contains("  • a") && plain.contains("1"), "{plain}");
    }

    #[test]
    fn root_help_lists_topics() {
        let plain = strip_ansi_codes(&format_help_root(&Value::Array(vec![s("ctx/sys")])));
        assert!(plain.contains("Available Help Topics"));
        assert!(plain.contains("help ctx/sys"));
        let bare = strip_ansi_codes(&format_help_root(&Value::Array(vec![])));
        assert!(!bare.contains("Available Help Topics"));
    }
}
