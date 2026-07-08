//! Proc-macro support for `bashrs` commands.
//!
//! [`macro@category`] lets a category module declare its commands as plain functions, generating
//! the clap `Subcommand` enum and dispatch for them. [`macro@elevated`] turns a function into a
//! `sudo` self-re-exec round-trip, for the part of a command that must run as root.

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
        // `#[trailing_newline]` prints a blank line after the command runs: some terminals drop
        // the last printed row when the window is enlarged afterward, so the extra line takes that
        // hit instead of the command's real final line of output.
        let call = if trailing_newline {
            quote!({ #fn_ident(args); ::std::println!(); })
        } else {
            quote!(#fn_ident(args))
        };
        arms.push(quote!(#command::#variant(args) => #call,));
        kept.push(quote!(#func));
    }

    // Per-command shell suffixes (`#[after("…")]`) and input pipes (`#[piped("…")]`), each a
    // generated by-clap-name lookup.
    let wrapper_suffix = lookup_fn("wrapper_suffix", &suffixes);
    let wrapper_prefix = lookup_fn("wrapper_prefix", &prefixes);

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

/// A generated `pub fn <ident>(name: &str) -> Option<&'static str>` mapping each `pairs` key to its
/// value (a constant `None` when `pairs` is empty) — the shape of both per-command wrapper lookups.
fn lookup_fn(ident: &str, pairs: &[(String, String)]) -> TokenStream2 {
    let ident = format_ident!("{ident}");
    if pairs.is_empty() {
        return quote!(pub fn #ident(_name: &str) -> Option<&'static str> { None });
    }
    let arms = pairs.iter().map(|(name, value)| quote!(#name => Some(#value),));
    quote! {
        pub fn #ident(name: &str) -> Option<&'static str> {
            match name {
                #(#arms)*
                _ => None,
            }
        }
    }
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

/// Turn a function into a `sudo` self-re-exec routine — the parent/child round-trip that the
/// `internal_cli` module used to spell out by hand. Applied to a free function whose parameters are
/// the data to forward to the elevated run, it emits a sibling unit struct named in PascalCase after
/// the function, with two `pub(crate)` methods:
/// - `reexec(…)` — the *parent* side: spawn `<superuser> <self> <marker> <args-as-flags>`, then revoke.
/// - `try_handle() -> bool` — the *child* side: if `argv[1]` is the marker, parse the flags back with
///   `clap` and run the function as root; returns whether it handled the invocation.
///
/// The marker is the function name, kebab-cased. Each parameter becomes a `--flag`: a `bool` is a
/// switch, a `Vec<T>` repeats once per element, an `Option<T>` appears only when `Some` (absent
/// parses back as `None`), any other type takes a value.
///
/// ```ignore
/// #[elevated]
/// fn gg_elevated_rescan(paths: Vec<PathBuf>, context: usize, delve: bool, save: Option<PathBuf>) {
///     /* … the work to run as root … */
/// }
/// // generates `GgElevatedRescan`, with:
/// //   GgElevatedRescan::reexec(&paths, ctx, delve, save)   // parent, from `gg`
/// //   GgElevatedRescan::try_handle()                       // child, from the entry point
/// ```
#[proc_macro_attribute]
pub fn elevated(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = parse_macro_input!(item as ItemFn);
    expand_elevated(func).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// The last path-segment identifier of a type, e.g. `std::path::PathBuf` → `PathBuf`.
fn type_ident(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(tp) => tp.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

/// The `T` of a `wrapper<T>` (`Vec<T>`, `Option<T>`), if `ty` is one.
fn element_of(ty: &Type, wrapper: &str) -> Option<Type> {
    let Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    if seg.ident != wrapper {
        return None;
    }
    match &seg.arguments {
        syn::PathArguments::AngleBracketed(ab) => match ab.args.first()? {
            syn::GenericArgument::Type(inner) => Some(inner.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Whether values of this type go straight to `Command::arg` (they're `AsRef<OsStr>`), or need a
/// `to_string` first (a number, say).
fn is_osstr(ty: &Type) -> bool {
    matches!(type_ident(ty).as_deref(), Some("String" | "PathBuf" | "OsString"))
}

/// How a bound element value (`__v`) becomes a `Command::arg`: directly for `AsRef<OsStr>` types,
/// via `to_string` otherwise — shared by the `Vec` and `Option` parameter kinds.
fn arg_value(inner: &Type) -> TokenStream2 {
    if is_osstr(inner) {
        quote!(__v)
    } else {
        quote!(__v.to_string())
    }
}

/// The borrowed form of an `AsRef<OsStr>` scalar to take in `reexec` — `&Path` not `&PathBuf`, `&str`
/// not `&String` — so the generated signature stays clippy-clean (`clippy::ptr_arg`).
fn osstr_ref(ty: &Type) -> TokenStream2 {
    match type_ident(ty).as_deref() {
        Some("String") => quote!(&str),
        Some("PathBuf") => quote!(&::std::path::Path),
        Some("OsString") => quote!(&::std::ffi::OsStr),
        _ => quote!(&#ty),
    }
}

fn expand_elevated(func: ItemFn) -> syn::Result<TokenStream2> {
    let fn_ident = func.sig.ident.clone();
    let command = format_ident!("{}", to_pascal_case(&fn_ident.to_string()));
    let args_ty = format_ident!("{command}Args");
    let marker = fn_ident.to_string().replace('_', "-");

    let mut fields = Vec::new(); // clap parser fields (child side)
    let mut params = Vec::new(); // reexec signature (parent side)
    let mut pushes = Vec::new(); // reexec body: build the argv
    let mut forwards = Vec::new(); // try_handle: parsed fields passed to the function

    for input in &func.sig.inputs {
        let FnArg::Typed(PatType { pat, ty, .. }) = input else {
            return Err(syn::Error::new_spanned(input, "an #[elevated] function cannot take `self`"));
        };
        let syn::Pat::Ident(binding) = &**pat else {
            return Err(syn::Error::new_spanned(pat, "#[elevated] parameters must be plain identifiers"));
        };
        let name = &binding.ident;
        let flag = format!("--{}", name.to_string().replace('_', "-"));

        fields.push(quote!(#[arg(long)] #name: #ty));
        forwards.push(quote!(parsed.#name));

        // Each parameter becomes a `--flag`: a `bool` is a switch, a `Vec<T>` repeats, an
        // `Option<T>` is emitted only when present (absent parses back as `None` — no sentinel
        // values on the wire), and any other value is passed as text — directly if it's
        // `AsRef<OsStr>`, else via `to_string`.
        if type_ident(ty).as_deref() == Some("bool") {
            params.push(quote!(#name: bool));
            pushes.push(quote!(if #name { cmd.arg(#flag); }));
        } else if let Some(inner) = element_of(ty, "Vec") {
            let value = arg_value(&inner);
            params.push(quote!(#name: &[#inner]));
            pushes.push(quote!(for __v in #name { cmd.arg(#flag).arg(#value); }));
        } else if let Some(inner) = element_of(ty, "Option") {
            let value = arg_value(&inner);
            let param_inner = if is_osstr(&inner) { osstr_ref(&inner) } else { quote!(#inner) };
            params.push(quote!(#name: ::core::option::Option<#param_inner>));
            pushes.push(quote!(if let ::core::option::Option::Some(__v) = #name { cmd.arg(#flag).arg(#value); }));
        } else if is_osstr(ty) {
            let borrowed = osstr_ref(ty);
            params.push(quote!(#name: #borrowed));
            pushes.push(quote!(cmd.arg(#flag).arg(#name);));
        } else {
            params.push(quote!(#name: #ty));
            pushes.push(quote!(cmd.arg(#flag).arg(#name.to_string());));
        }
    }

    Ok(quote! {
        #func

        #[derive(::clap::Parser)]
        struct #args_ty {
            #(#fields,)*
        }

        /// The `sudo` self-re-exec round-trip for the routine above, generated by `#[elevated]`.
        pub(crate) struct #command;

        impl #command {
            const MARKER: &'static str = #marker;

            /// Parent side: re-exec this binary under the superuser, forwarding each argument as the
            /// flag(s) the child parses back, then drop the elevation.
            pub(crate) fn reexec(#(#params),*) {
                let exe = match ::std::env::current_exe() {
                    ::core::result::Result::Ok(exe) => exe,
                    ::core::result::Result::Err(err) => {
                        return ::std::eprintln!("cannot locate self to re-run as root: {}", err)
                    }
                };
                let mut cmd = crate::support::superuser::command();
                cmd.arg(exe).arg(Self::MARKER);
                #(#pushes)*
                let _ = cmd.status();
                crate::support::superuser::revoke();
            }

            /// Child side: if this process is the elevated re-exec (its `argv[1]` is the marker),
            /// parse the forwarded flags and run the routine as root. Returns whether it did.
            pub(crate) fn try_handle() -> bool {
                match ::std::env::args_os().nth(1).and_then(|a| a.into_string().ok()).as_deref() {
                    ::core::option::Option::Some(Self::MARKER) => {
                        let parsed = <#args_ty as ::clap::Parser>::parse_from(::std::env::args_os().skip(1));
                        #fn_ident(#(#forwards),*);
                        true
                    }
                    _ => false,
                }
            }
        }
    })
}
