//! Proc-macro support for defining `bashrs` command categories.
//!
//! See [`macro@category`] for the details; in short, it lets a category module
//! declare its commands as plain functions and generates the clap `Subcommand`
//! enum and dispatch for them.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    Attribute, Expr, ExprLit, FnArg, Item, ItemFn, ItemMod, Lit, MetaNameValue, PatType, Token,
    Type,
};

/// Turn a module of plain functions into a clap subcommand category.
///
/// Every free function in the module whose name does **not** start with `_`
/// becomes a command; its single parameter is the command's clap `Args` struct,
/// and its clap name is `<prefix><fn-name>`. Underscore-prefixed functions are
/// left untouched (helpers), as is everything that isn't a free function.
///
/// The macro generates a `#[derive(clap::Subcommand)]` enum (named by
/// `command = ...`) plus an inherent `run(self)` that dispatches to the
/// functions, and hoists the module's contents into the surrounding scope.
///
/// A command's clap name is `<prefix><fn-name>` by default. Two helper
/// attributes adjust that, per command:
/// - `#[unprefixed]` — expose it under its bare name only (no prefix).
/// - `#[prefixed] #[unprefixed]` — expose it under both names.
/// - `#[after("cmd")]` — append `&& cmd` to the generated shell wrapper, e.g.
///   `#[after("exec bash")]` to restart the shell after the command runs.
///
/// ```ignore
/// #[category(command = MediaCommand, prefix = "media_")]
/// mod commands {
///     use clap::Args;
///
///     /// Convert a media file
///     #[prefixed] #[unprefixed]                 // exposed as `media_conv` and `conv`
///     pub fn conv(args: ConvArgs) { /* ... */ }
///     #[derive(Args)] pub struct ConvArgs { /* ... */ }
///
///     fn _helper() { /* ignored: starts with `_` */ }
/// }
/// ```
#[proc_macro_attribute]
pub fn category(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as CategoryArgs);
    let module = parse_macro_input!(item as ItemMod);
    expand(args, module)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Parsed `#[category(command = ..., prefix = "...")]` arguments.
struct CategoryArgs {
    command: syn::Ident,
    prefix: String,
}

impl Parse for CategoryArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let pairs = Punctuated::<MetaNameValue, Token![,]>::parse_terminated(input)?;
        let mut command = None;
        let mut prefix = None;
        for pair in pairs {
            let key = pair.path.get_ident().map(ToString::to_string).unwrap_or_default();
            match key.as_str() {
                "command" => match pair.value {
                    Expr::Path(path) => command = path.path.get_ident().cloned(),
                    other => return Err(syn::Error::new_spanned(other, "`command` must be an identifier")),
                },
                "prefix" => match pair.value {
                    Expr::Lit(ExprLit { lit: Lit::Str(s), .. }) => prefix = Some(s.value()),
                    other => return Err(syn::Error::new_spanned(other, "`prefix` must be a string literal")),
                },
                _ => return Err(syn::Error::new_spanned(pair.path, "expected `command` or `prefix`")),
            }
        }
        Ok(CategoryArgs {
            command: command.ok_or_else(|| syn::Error::new(input.span(), "missing `command = TypeName`"))?,
            prefix: prefix.ok_or_else(|| syn::Error::new(input.span(), "missing `prefix = \"...\"`"))?,
        })
    }
}

fn expand(args: CategoryArgs, module: ItemMod) -> syn::Result<TokenStream2> {
    let CategoryArgs { command, prefix } = args;
    let items = match module.content {
        Some((_, items)) => items,
        None => {
            return Err(syn::Error::new_spanned(
                &module.ident,
                "#[category] must be applied to an inline module with a body",
            ))
        }
    };

    let mut kept = Vec::new();
    let mut variants = Vec::new();
    let mut arms = Vec::new();
    let mut suffixes: Vec<(String, String)> = Vec::new();

    for item in items {
        let Item::Fn(mut func) = item else {
            kept.push(quote!(#item));
            continue;
        };

        let fn_name = func.sig.ident.to_string();
        if fn_name.starts_with('_') {
            kept.push(quote!(#func)); // helper: leave it alone
            continue;
        }

        let arg_ty = command_arg_type(&func)?;
        let CommandAttrs { docs, prefixed, unprefixed, after } = take_command_attrs(&mut func.attrs)?;

        let variant = format_ident!("{}", to_pascal_case(&fn_name));
        let fn_ident = func.sig.ident.clone();

        // Prefixed by default; `#[unprefixed]` drops it unless `#[prefixed]` also
        // asks to keep it (in which case both names are exposed).
        let emit_prefixed = prefixed || !unprefixed;
        let name = if emit_prefixed { format!("{prefix}{fn_name}") } else { fn_name.clone() };
        let command_attr = if emit_prefixed && unprefixed {
            quote!(#[command(name = #name, visible_alias = #fn_name)])
        } else {
            quote!(#[command(name = #name)])
        };

        if let Some(after) = after {
            suffixes.push((name.clone(), after));
        }

        variants.push(quote! {
            #(#docs)*
            #command_attr
            #variant(#arg_ty)
        });
        arms.push(quote!(#command::#variant(args) => #fn_ident(args),));
        kept.push(quote!(#func));
    }

    // Per-command shell suffixes (from `#[after("…")]`), looked up by clap name.
    let wrapper_suffix = if suffixes.is_empty() {
        quote!(pub fn wrapper_suffix(_name: &str) -> Option<&'static str> { None })
    } else {
        let suffix_arms = suffixes.iter().map(|(cmd_name, after)| quote!(#cmd_name => Some(#after),));
        quote! {
            pub fn wrapper_suffix(name: &str) -> Option<&'static str> {
                match name {
                    #(#suffix_arms)*
                    _ => None,
                }
            }
        }
    };

    Ok(quote! {
        #(#kept)*

        #[derive(::clap::Subcommand)]
        pub enum #command {
            #(#variants),*
        }

        impl #command {
            pub fn run(self) {
                match self {
                    #(#arms)*
                }
            }

            /// Shell appended (after `&&`) to a command's generated wrapper.
            #wrapper_suffix
        }
    })
}

/// The type of a command function's single parameter (its clap `Args` struct).
fn command_arg_type(func: &ItemFn) -> syn::Result<Type> {
    let mut inputs = func.sig.inputs.iter();
    match (inputs.next(), inputs.next()) {
        (Some(FnArg::Typed(PatType { ty, .. })), None) => Ok((**ty).clone()),
        (Some(FnArg::Receiver(recv)), _) => {
            Err(syn::Error::new_spanned(recv, "a command function cannot take `self`"))
        }
        _ => Err(syn::Error::new_spanned(
            &func.sig,
            format!(
                "command `{}` must take exactly one argument: its `#[derive(Args)]` struct",
                func.sig.ident
            ),
        )),
    }
}

/// The helper attributes extracted from a command function.
struct CommandAttrs {
    /// `#[doc]` comments, copied onto the generated variant (kept on the fn too).
    docs: Vec<Attribute>,
    /// `#[prefixed]` — keep the prefixed name (alongside a bare one).
    prefixed: bool,
    /// `#[unprefixed]` — expose the bare (prefix-free) name.
    unprefixed: bool,
    /// `#[after("…")]` — shell appended (after `&&`) to the command's wrapper.
    after: Option<String>,
}

/// Split a command function's attributes into [`CommandAttrs`]. The `prefixed` /
/// `unprefixed` / `after` helper attributes are consumed (not re-emitted);
/// everything else (including docs) stays on the function.
fn take_command_attrs(attrs: &mut Vec<Attribute>) -> syn::Result<CommandAttrs> {
    let mut parsed = CommandAttrs { docs: Vec::new(), prefixed: false, unprefixed: false, after: None };
    let mut kept = Vec::new();
    for attr in std::mem::take(attrs) {
        if attr.path().is_ident("doc") {
            parsed.docs.push(attr.clone());
            kept.push(attr);
        } else if attr.path().is_ident("prefixed") {
            parsed.prefixed = true;
        } else if attr.path().is_ident("unprefixed") {
            parsed.unprefixed = true;
        } else if attr.path().is_ident("after") {
            parsed.after = Some(attr.parse_args::<syn::LitStr>()?.value());
        } else {
            kept.push(attr);
        }
    }
    *attrs = kept;
    Ok(parsed)
}

/// `hmerge_imgs` -> `HmergeImgs`.
fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}
