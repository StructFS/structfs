//! Featherweight: a strawman Isotope runtime.
//!
//! Blocks are pico-processes whose entire world is StructFS reads and
//! writes; assemblies compose them with capability wiring. See
//! `featherweight-runtime` for the runtime library and the `fw` binary
//! for the CLI (`fw shell`, `fw run <assembly>`).
