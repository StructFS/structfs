//! The REPL's command table.
//!
//! One table drives dispatch (`commands`), register capture, completion
//! (`completer`), highlighting (`highlighter`), the `help` listing, and the
//! `ctx/repl/docs/commands` document. Add a command here and to the
//! dispatcher; a test fails if an entry does not dispatch.

/// How a command's argument text is highlighted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ArgKind {
    /// No arguments, or free text.
    Text,
    /// A path.
    Path,
    /// A path followed by a JSON value.
    PathAndValue,
}

/// One REPL command.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct CommandSpec {
    /// Canonical name, used by dispatch.
    pub name: &'static str,
    /// Alternative spellings.
    pub aliases: &'static [&'static str],
    /// Whether `@reg <command>` may capture this command's output.
    pub capturable: bool,
    /// Argument synopsis, e.g. `[path]`.
    pub usage: &'static str,
    /// One-line description.
    pub summary: &'static str,
    /// How the arguments are highlighted.
    pub args: ArgKind,
}

impl CommandSpec {
    /// The name followed by every alias.
    pub fn spellings(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.name).chain(self.aliases.iter().copied())
    }
}

/// Every REPL command, in help order.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "read",
        aliases: &["get", "r"],
        capturable: true,
        usage: "[path|@reg]",
        summary: "Read the value at a path or register",
        args: ArgKind::Path,
    },
    CommandSpec {
        name: "write",
        aliases: &["set", "w"],
        capturable: true,
        usage: "<path> <json|@reg>",
        summary: "Write a value to a path or register",
        args: ArgKind::PathAndValue,
    },
    CommandSpec {
        name: "ls",
        aliases: &[],
        capturable: true,
        usage: "[path]",
        summary: "List the child names at a path",
        args: ArgKind::Path,
    },
    CommandSpec {
        name: "cd",
        aliases: &[],
        capturable: true,
        usage: "<path>",
        summary: "Change the current path",
        args: ArgKind::Path,
    },
    CommandSpec {
        name: "pwd",
        aliases: &[],
        capturable: true,
        usage: "",
        summary: "Print the current path",
        args: ArgKind::Text,
    },
    CommandSpec {
        name: "mounts",
        aliases: &[],
        capturable: true,
        usage: "",
        summary: "List mounts and their configs (read /ctx/mounts)",
        args: ArgKind::Text,
    },
    CommandSpec {
        name: "registers",
        aliases: &["regs"],
        capturable: false,
        usage: "",
        summary: "List all registers",
        args: ArgKind::Text,
    },
    CommandSpec {
        name: "help",
        aliases: &["?"],
        capturable: false,
        usage: "[topic]",
        summary: "Show help (try: help ctx/http)",
        args: ArgKind::Text,
    },
    CommandSpec {
        name: "exit",
        aliases: &["quit", "q"],
        capturable: false,
        usage: "",
        summary: "Exit the REPL",
        args: ArgKind::Text,
    },
];

/// Find the command a word names (case-insensitive), by name or alias.
pub fn lookup(word: &str) -> Option<&'static CommandSpec> {
    let word = word.to_lowercase();
    COMMANDS.iter().find(|c| c.spellings().any(|s| s == word))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spellings_are_unique() {
        let mut all: Vec<&str> = COMMANDS.iter().flat_map(|c| c.spellings()).collect();
        let total = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), total);
    }

    #[test]
    fn readme_documents_every_command() {
        let readme = include_str!("../README.md");
        for spec in COMMANDS {
            let row = readme
                .lines()
                .find(|line| line.starts_with(&format!("| `{}", spec.name)))
                .unwrap_or_else(|| panic!("README has no row for `{}`", spec.name));
            assert!(row.contains(spec.summary), "stale README row: {row}");
            for alias in spec.aliases {
                assert!(row.contains(&format!("`{alias}`")), "{row} lacks {alias}");
            }
        }
    }

    #[test]
    fn lookup_finds_names_and_aliases_case_insensitively() {
        assert_eq!(lookup("READ").unwrap().name, "read");
        assert_eq!(lookup("regs").unwrap().name, "registers");
        assert_eq!(lookup("?").unwrap().name, "help");
        assert!(lookup("nope").is_none());
    }
}
