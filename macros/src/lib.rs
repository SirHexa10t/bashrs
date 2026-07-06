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
/// A command's clap name is `<prefix><fn-name>` by default. Helper attributes
/// adjust that, per command:
/// - `#[unprefixed]` — expose it under its bare name only (no prefix).
/// - `#[prefixed] #[unprefixed]` — expose it under both names.
/// - `#[name("cmd")]` — give it this exact name, overriding the prefix logic.
/// - `#[alias("cmd")]` — add a visible alias (repeatable); e.g. `recho` gains `echor`,
///   so the command also completes after typing `echo`.
/// - `#[after("cmd")]` — append `&& cmd` to the generated shell wrapper, e.g.
///   `#[after("exec bash")]` to restart the shell after the command runs.
/// - `#[piped("cmd")]` — prepend `cmd |` to the generated wrapper, feeding `cmd`'s output in
///   as the command's stdin, e.g. `#[piped("history")]` to search the shell history (a
///   builtin only the shell can produce).
/// - `#[trailing_newline]` — print a blank line after the command runs, sparing the last row
///   from terminals that drop it when the window is enlarged.
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
    let mut prefixes: Vec<(String, String)> = Vec::new();

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
        let CommandAttrs {
            docs,
            prefixed,
            unprefixed,
            after,
            piped,
            name: custom_name,
            aliases,
            trailing_newline,
        } = take_command_attrs(&mut func.attrs)?;

        let variant = format_ident!("{}", to_pascal_case(&fn_name));
        let fn_ident = func.sig.ident.clone();

        // An explicit `#[name("…")]` wins outright. Otherwise the command is prefixed by
        // default; `#[unprefixed]` drops the prefix, and `#[prefixed] #[unprefixed]`
        // together expose the prefixed name plus the bare name as a visible alias.
        // `#[alias("…")]` adds further visible aliases in any case.
        let emit_prefixed = prefixed || !unprefixed;
        let mut visible_aliases = aliases;
        let name = match custom_name {
            Some(name) => name,
            None if emit_prefixed && unprefixed => {
                visible_aliases.insert(0, fn_name.clone());
                format!("{prefix}{fn_name}")
            }
            None if emit_prefixed => format!("{prefix}{fn_name}"),
            None => fn_name.clone(),
        };
        let command_attr = if visible_aliases.is_empty() {
            quote!(#[command(name = #name)])
        } else {
            quote!(#[command(name = #name, visible_aliases = [#(#visible_aliases),*])])
        };

        if let Some(after) = after {
            suffixes.push((name.clone(), after));
        }
        if let Some(piped) = piped {
            prefixes.push((name.clone(), piped));
        }

        variants.push(quote! {
            #(#docs)*
            #command_attr
            #variant(#arg_ty)
        });
        let call = if trailing_newline {
            quote!({ #fn_ident(args); ::std::println!(); })
        } else {
            quote!(#fn_ident(args))
        };
        arms.push(quote!(#command::#variant(args) => #call,));
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

    // Per-command shell input pipes (from `#[piped("…")]`), looked up by clap name.
    let wrapper_prefix = if prefixes.is_empty() {
        quote!(pub fn wrapper_prefix(_name: &str) -> Option<&'static str> { None })
    } else {
        let prefix_arms = prefixes.iter().map(|(cmd_name, piped)| quote!(#cmd_name => Some(#piped),));
        quote! {
            pub fn wrapper_prefix(name: &str) -> Option<&'static str> {
                match name {
                    #(#prefix_arms)*
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

            /// Command piped into a command's generated wrapper (from `#[piped("…")]`).
            #wrapper_prefix
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
    /// `#[piped("…")]` — a command piped into the wrapper's stdin.
    piped: Option<String>,
    /// `#[name("…")]` — an explicit clap name, overriding the prefix/bare logic.
    name: Option<String>,
    /// `#[alias("…")]` — extra visible aliases (repeatable).
    aliases: Vec<String>,
    /// `#[trailing_newline]` — print a blank line after the command, to spare the last row.
    trailing_newline: bool,
}

/// Split a command function's attributes into [`CommandAttrs`]. The `prefixed` /
/// `unprefixed` / `after` / `name` / `alias` helper attributes are consumed (not
/// re-emitted); everything else (including docs) stays on the function.
fn take_command_attrs(attrs: &mut Vec<Attribute>) -> syn::Result<CommandAttrs> {
    let mut parsed = CommandAttrs {
        docs: Vec::new(),
        prefixed: false,
        unprefixed: false,
        after: None,
        piped: None,
        name: None,
        aliases: Vec::new(),
        trailing_newline: false,
    };
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
        } else if attr.path().is_ident("piped") {
            parsed.piped = Some(attr.parse_args::<syn::LitStr>()?.value());
        } else if attr.path().is_ident("name") {
            parsed.name = Some(attr.parse_args::<syn::LitStr>()?.value());
        } else if attr.path().is_ident("alias") {
            parsed.aliases.push(attr.parse_args::<syn::LitStr>()?.value());
        } else if attr.path().is_ident("trailing_newline") {
            parsed.trailing_newline = true;
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
