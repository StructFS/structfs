---
layout: base.njk
title: Unicode Identifiers
templateClass: doc-page
---

<div class="doc-page">

# Unicode Identifiers and UAX 31

Programming languages need rules for what constitutes a valid identifier — a variable name, function name, type name. Historically each language invented its own rules. [Unicode Standard Annex #31](https://unicode.org/reports/tr31/) (UAX 31) defines a common standard that modern languages now converge on.

Namecode encodes arbitrary strings into identifiers that conform to this standard.

## XID_Start and XID_Continue

UAX 31 defines identifiers using two Unicode character properties from [UAX #44](https://unicode.org/reports/tr44/#XID_Start):

**`XID_Start`** — characters that can *begin* an identifier:
- Letters (Latin, Greek, Cyrillic, CJK, etc.)
- Letter-like numbers (e.g. Roman numerals)
- Underscore `_`
- *Not* digits, punctuation, symbols, or whitespace

**`XID_Continue`** — characters that can appear *after* the first position:
- Everything in `XID_Start`, plus:
- Digits (`0`–`9`)
- Combining marks (accents, diacritics)
- Connector punctuation (underscore)

A valid identifier is: one `XID_Start` character, followed by zero or more `XID_Continue` characters.

The "XID" prefix stands for "eXtended IDentifier" — these are derived properties that remain stable across Unicode normalization forms (NFC/NFKC), which makes them safe for compilers and tooling.

## What this means in practice

| String | Valid? | Why |
|--------|--------|-----|
| `foo` | Yes | Letter, letters |
| `café` | Yes | Letters (including `é`, which is `XID_Continue`) |
| `名前` | Yes | CJK characters are `XID_Start` and `XID_Continue` |
| `_private` | Yes | Underscore is `XID_Start` |
| `foo123` | Yes | Digits are `XID_Continue` |
| `123foo` | **No** | Digit is not `XID_Start` — can't begin an identifier |
| `hello world` | **No** | Space is not `XID_Continue` |
| `foo-bar` | **No** | Hyphen is not `XID_Continue` |
| `foo@bar` | **No** | `@` is not `XID_Continue` |

## Language adoption

Most modern languages have converged on UAX 31 or a close subset:

| Language | Identifier rule | Notes |
|----------|----------------|-------|
| **Rust** | [UAX 31](https://doc.rust-lang.org/reference/identifiers.html) | Exact UAX 31 since Rust 1.53 (2021). NFC normalized. |
| **Python 3** | [UAX 31](https://docs.python.org/3/reference/lexical_analysis.html#identifiers) | Exact UAX 31 since Python 3.0 (2008). NFKC normalized. |
| **JavaScript** | [UAX 31](https://tc39.es/ecma262/#sec-names-and-keywords) | Via ECMAScript spec. Allows `$` as an extension. |
| **Go** | [UAX 31 subset](https://go.dev/ref/spec#Identifiers) | `letter` is Unicode letter or `_`; digits are Unicode digits. |
| **Swift** | [UAX 31](https://docs.swift.org/swift-book/documentation/the-swift-programming-language/lexicalstructure/#Identifiers) | With some operator-character extensions. |
| **C23** | [UAX 31](https://www.open-std.org/jtc1/sc22/wg14/www/docs/n3220.pdf) | C23 adopts UAX 31. Older C standards used a different Unicode range list. |
| **Java** | Custom | Uses `Character.isJavaIdentifierStart/Part`, which is similar but predates and differs from UAX 31. |

Because namecode output conforms to UAX 31, it produces valid identifiers in all of these languages (with the caveat that Java's rules are slightly different, though compatible in practice for the character set namecode uses).

## Why namecode uses UAX 31

Namecode needs a single encoding that works across languages. UAX 31 is the natural choice:

1. It's the actual standard that modern languages implement
2. The `XID_Start`/`XID_Continue` properties are stable across Unicode versions
3. The alphabet namecode uses for its encoded portion (`a`–`z`, `0`–`5`) is a strict subset of ASCII `XID_Continue`, so encoded output is valid everywhere — even in languages with more restrictive rules

## Further reading

- [UAX #31: Unicode Identifier and Pattern Syntax](https://unicode.org/reports/tr31/) — the standard itself
- [UAX #31 Table 1: Lexical Classes for Identifiers](https://unicode.org/reports/tr31/#Table_Lexical_Classes_for_Identifiers) — the formal grammar
- [UAX #44: Unicode Character Database](https://unicode.org/reports/tr44/#XID_Start) — where `XID_Start` and `XID_Continue` are defined
- [Namecode specification](/spec/) — how namecode uses these properties

</div>
