//! Proc macro for compile-time validated StructFS paths.
//!
//! `path!` validates string literal components against the StructFS path
//! grammar (UAX#31 identifiers or numeric strings) at compile time.
//! Expression arguments must be `PathComponent` values, which are validated
//! at construction time.
//!
//! ```ignore
//! // A single literal path — components validated at compile time
//! let p = path!("users/123/name");
//!
//! // Component style — equivalent to the above
//! let p = path!("users", 123, "name");
//!
//! // Mixed — literals validated at compile time, expressions must be
//! // PathComponent (bare String/&str fail to compile)
//! let name = PathComponent::try_new("alice")?;
//! let p = path!("users", name, "profile");
//!
//! // Compile error:
//! // let p = path!("users/bad-name");
//! //                ^^^^^^^^^^^^^^^ invalid character '-'
//! ```

use proc_macro::TokenStream;

use quote::quote;
use syn::punctuated::Punctuated;
use syn::{parse_macro_input, Expr, Lit, Token};

/// Build a `Path` from a mix of literal and runtime components.
///
/// - **String literals** are split on `/` and each component is validated
///   at compile time against the StructFS path grammar.
/// - **Integer literals** become numeric components (array indexing).
/// - **Expressions** must be of type `PathComponent` (runtime-validated at
///   construction). Bare `String`/`&str` values do not compile; validate
///   them first with `PathComponent::try_new` or `PathComponent::encode`.
///
/// Returns a `structfs_core_store::Path`.
#[proc_macro]
pub fn path(input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(input with Punctuated::<Expr, Token![,]>::parse_terminated);

    let mut component_exprs = Vec::new();

    for expr in &args {
        match expr {
            Expr::Lit(expr_lit) => match &expr_lit.lit {
                Lit::Str(s) => {
                    // Split on '/' like Path::parse: empty segments are
                    // ignored, so "a//b/" and "" behave identically to the
                    // runtime parser.
                    for component in s.value().split('/').filter(|c| !c.is_empty()) {
                        if let Err(msg) = structfs_path_validation::validate_component(component) {
                            return syn::Error::new(
                                s.span(),
                                format!("invalid path component '{component}': {msg}"),
                            )
                            .to_compile_error()
                            .into();
                        }
                        component_exprs.push(quote! { ::std::string::String::from(#component) });
                    }
                }
                Lit::Int(n) => {
                    // Numeric literals are valid components (array indexing)
                    let s = n.base10_digits();
                    if let Err(msg) = structfs_path_validation::validate_component(s) {
                        return syn::Error::new(
                            n.span(),
                            format!("invalid path component '{s}': {msg}"),
                        )
                        .to_compile_error()
                        .into();
                    }
                    component_exprs.push(quote! { ::std::string::String::from(#s) });
                }
                other => {
                    return syn::Error::new(
                        other.span(),
                        "expected string literal, integer literal, or PathComponent expression",
                    )
                    .to_compile_error()
                    .into();
                }
            },
            other => {
                // Runtime expression — must be a PathComponent (pre-validated).
                // We call .validated_str(), a method only PathComponent has, so
                // bare String/&str produce a compile error. Borrows rather than
                // consumes, so the same component can be reused across calls.
                component_exprs.push(quote! {
                    ::std::string::String::from((#other).validated_str())
                });
            }
        }
    }

    // All components are validated: literals here at compile time,
    // PathComponent values at their construction site. The constructor
    // re-checks with debug_assert as a safety net.
    quote! {
        ::structfs_core_store::Path::from_validated_components(
            ::std::vec![#(#component_exprs),*]
        )
    }
    .into()
}
