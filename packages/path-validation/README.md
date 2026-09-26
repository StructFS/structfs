# structfs-path-validation

The shared grammar for StructFS runtime and compile-time path validation.

## Usage

`validate_component` accepts numeric strings and the documented Unicode
identifier grammar. Empty components, bare `_`, punctuation and slashes fail.
Use core-store `PathComponent::encode` when arbitrary names must become valid
components. There are no optional features.

## Version and support

0.4.0 is the published release; this checkout is the 0.5 development line.
Both support Rust 1.96+. See the [API documentation](https://docs.rs/structfs-path-validation)
for compiled examples and full contracts.
Read the [0.5 migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.5.md)
(or the [0.4 guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.4.md) from 0.3) before upgrading
code or persisted data. The [release procedure](https://github.com/StructFS/structfs/blob/main/docs/releasing.md)
describes package, platform and feature verification.
