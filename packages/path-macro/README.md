# structfs-path-macro

Compile-time validation for StructFS path literals.

## Usage

Applications normally use `structfs::path!` or `structfs_core_store::path!`.
Literal components are validated at compile time; expression components must
be validated `PathComponent` values. This crate shares the runtime grammar
through `structfs-path-validation`. There are no optional features.

## Version and support

0.4.0 is the published release; this checkout is the 0.5 development line.
Both support Rust 1.96+. See the [API documentation](https://docs.rs/structfs-path-macro)
for compiled examples and full contracts.
Read the [0.5 migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.5.md)
(or the [0.4 guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.4.md) from 0.3) before upgrading
code or persisted data. The [release procedure](https://github.com/StructFS/structfs/blob/main/docs/releasing.md)
describes package, platform and feature verification.

Import path! through structfs-core-store or structfs. The hygienic wrapper supports
renamed dependencies and facade reexports; the proc macro entry point is an
implementation detail. Runtime expressions remain validated PathComponent values.
