# StructFS Plans

Core principle: **Everything is a store.**

## Remaining Work

| Plan | Description |
|------|-------------|
| [01-document-mutability](./01-document-mutability.md) | Document why `Reader::read` takes `&mut self` |

## 01: Document Mutability

`Reader::read(&mut self)` is intentional but undocumented. Some stores (HTTP broker, filesystem) mutate on read.

**Work:**
- Add trait-level documentation explaining the design
- Add note to CLAUDE.md architecture section
