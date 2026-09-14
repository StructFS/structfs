# structfs-path-validation

The shared grammar for StructFS runtime and compile-time path validation.

## Usage

`validate_component` accepts numeric strings and the documented Unicode
identifier grammar. Empty components, bare `_`, punctuation and slashes fail.
Use core-store `PathComponent::encode` when arbitrary names must become valid
components. There are no optional features.

## Version and support

The 0.3 release line supports Rust 1.96+. See the [API documentation](https://docs.rs/structfs-path-validation)
for compiled examples and full contracts.
Read the [migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.3.md) before upgrading
code or persisted data. The [release procedure](https://github.com/StructFS/structfs/blob/main/docs/releasing.md)
describes package, platform and feature verification.
