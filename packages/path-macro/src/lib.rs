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

struct Input {
    root: syn::Path,
    args: Punctuated<Expr, Token![,]>,
}
impl syn::parse::Parse for Input {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let root = input.parse()?;
        input.parse::<Token![;]>()?;
        Ok(Self {
            root,
            args: Punctuated::parse_terminated(input)?,
        })
    }
}

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
///
/// Implementation detail: invoke the hygienic `structfs_core_store::path!` facade.
#[proc_macro]
pub fn path(input: TokenStream) -> TokenStream {
    let Input { root, args } = parse_macro_input!(input as Input);

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
                // An explicit borrow enforces the validated type without consuming it.
                component_exprs.push(quote! {{
                    let component: &#root::PathComponent = &(#other);
                    ::std::string::String::from(component.validated_str())
                }});
            }
        }
    }

    // All components are validated: literals here at compile time,
    // PathComponent values at their construction site. The constructor
    // also validates in release builds to protect direct callers.
    quote! {
        #root::Path::from_validated_components(
            ::std::vec![#(#component_exprs),*]
        )
    }
    .into()
}
