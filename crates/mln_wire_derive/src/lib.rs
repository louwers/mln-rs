//! `#[derive(Wire)]` for `mln_wire`: a struct's fields in order, or an
//! enum's variant index and then its fields.
//!
//! Attributes:
//!
//! - `#[wire(bound = "...")]` on the type replaces the `T: Wire` bound that
//!   each type parameter gets otherwise.
//! - `#[wire(crate = path)]` on the type names the `mln_wire` crate by
//!   `path` instead of `::mln_wire`, for a macro that derives `Wire` in a
//!   crate that doesn't depend on `mln_wire` itself.
//! - `#[wire(default)]` on a struct that implements `Default` reads it into
//!   its default, one field after another. A struct literal of the fields
//!   read drops those read so far at each place reading can fail, which
//!   for a struct of many fields compiles to much more code. Its skipped
//!   fields keep the struct's default.
//! - `#[wire(with = "path")]` on a field writes it with `path::write(&field,
//!   writer)` and reads it with `path::read(reader)`.
//! - `#[wire(skip)]` on a field leaves it out; it reads as its `Default`, or
//!   with `#[wire(skip, default = "path")]` as what `path()` returns.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, Index, LitStr, Path, WherePredicate, parse_macro_input, parse_quote};

#[proc_macro_derive(Wire, attributes(wire))]
pub fn derive_wire(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[derive(Default)]
struct FieldAttrs {
    with: Option<Path>,
    skip: bool,
    default: Option<Path>,
}

fn field_attrs(attrs: &[syn::Attribute]) -> syn::Result<FieldAttrs> {
    let mut result = FieldAttrs::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("wire")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("with") {
                let path: LitStr = meta.value()?.parse()?;
                result.with = Some(path.parse()?);
                Ok(())
            } else if meta.path.is_ident("skip") {
                result.skip = true;
                Ok(())
            } else if meta.path.is_ident("default") {
                let path: LitStr = meta.value()?.parse()?;
                result.default = Some(path.parse()?);
                Ok(())
            } else {
                Err(meta.error("expected `with`, `skip` or `default`"))
            }
        })?;
    }
    Ok(result)
}

#[derive(Default)]
struct ContainerAttrs {
    bound: Option<Vec<WherePredicate>>,
    default: bool,
    krate: Option<Path>,
}

fn container_attrs(attrs: &[syn::Attribute]) -> syn::Result<ContainerAttrs> {
    let mut result = ContainerAttrs::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("wire")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("default") {
                result.default = true;
                Ok(())
            } else if meta.path.is_ident("crate") {
                result.krate = Some(meta.value()?.parse()?);
                Ok(())
            } else if meta.path.is_ident("bound") {
                let bound: LitStr = meta.value()?.parse()?;
                let predicates = if bound.value().trim().is_empty() {
                    Vec::new()
                } else {
                    bound
                        .parse_with(syn::punctuated::Punctuated::<WherePredicate, syn::Token![,]>::parse_terminated)?
                        .into_iter()
                        .collect()
                };
                result.bound = Some(predicates);
                Ok(())
            } else {
                Err(meta.error("expected `bound`, `crate` or `default`"))
            }
        })?;
    }
    Ok(result)
}

/// How a field is written, given an expression that borrows it.
fn write_field(wire: &TokenStream2, attrs: &FieldAttrs, field: &TokenStream2) -> TokenStream2 {
    if attrs.skip {
        return quote! {};
    }
    if let Some(path) = &attrs.with {
        quote! { #path::write(#field, w); }
    } else {
        quote! { #wire::Wire::write(#field, w); }
    }
}

/// How a field is read: an expression of its value.
fn read_field(wire: &TokenStream2, attrs: &FieldAttrs) -> TokenStream2 {
    if attrs.skip {
        return if let Some(default) = &attrs.default {
            quote! { #default() }
        } else {
            quote! { ::core::default::Default::default() }
        };
    }
    if let Some(path) = &attrs.with {
        quote! { #path::read(r)? }
    } else {
        quote! { #wire::Wire::read(r)? }
    }
}

/// The write statements and the read constructor of `fields`, whose values
/// the write statements borrow through `bindings`.
fn fields_code(
    wire: &TokenStream2,
    fields: &Fields,
    bindings: &[TokenStream2],
) -> syn::Result<(TokenStream2, TokenStream2)> {
    let mut writes = Vec::new();
    let mut reads = Vec::new();
    for (field, binding) in fields.iter().zip(bindings) {
        let attrs = field_attrs(&field.attrs)?;
        writes.push(write_field(wire, &attrs, binding));
        let read = read_field(wire, &attrs);
        reads.push(match &field.ident {
            Some(name) => quote! { #name: #read },
            None => read,
        });
    }
    let read = match fields {
        Fields::Named(_) => quote! { { #(#reads,)* } },
        Fields::Unnamed(_) => quote! { ( #(#reads,)* ) },
        Fields::Unit => quote! {},
    };
    Ok((quote! { #(#writes)* }, read))
}

fn bindings(fields: &Fields) -> Vec<syn::Ident> {
    fields.iter().enumerate().map(|(i, field)| field.ident.clone().unwrap_or_else(|| format_ident!("f{i}"))).collect()
}

fn pattern(fields: &Fields, bindings: &[syn::Ident]) -> TokenStream2 {
    match fields {
        Fields::Named(_) => quote! { { #(#bindings,)* } },
        Fields::Unnamed(_) => quote! { ( #(#bindings,)* ) },
        Fields::Unit => quote! {},
    }
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let mut generics = input.generics.clone();
    let container = container_attrs(&input.attrs)?;
    let wire = container.krate.as_ref().map_or_else(|| quote! { ::mln_wire }, |path| quote! { #path });
    if let Some(predicates) = container.bound {
        generics.make_where_clause().predicates.extend(predicates);
    } else {
        let params: Vec<_> = generics.type_params().map(|param| param.ident.clone()).collect();
        let where_clause = generics.make_where_clause();
        for param in params {
            where_clause.predicates.push(parse_quote! { #param: #wire::Wire });
        }
    }
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();

    let (write, read) = match &input.data {
        Data::Struct(data) => {
            let bindings: Vec<TokenStream2> = data
                .fields
                .iter()
                .enumerate()
                .map(|(i, field)| {
                    if let Some(name) = &field.ident {
                        quote! { &self.#name }
                    } else {
                        let index = Index::from(i);
                        quote! { &self.#index }
                    }
                })
                .collect();
            let (write, read) = fields_code(&wire, &data.fields, &bindings)?;
            if container.default {
                let mut reads = Vec::new();
                for (i, field) in data.fields.iter().enumerate() {
                    let attrs = field_attrs(&field.attrs)?;
                    if attrs.skip {
                        continue;
                    }
                    let member = if let Some(name) = &field.ident {
                        quote! { #name }
                    } else {
                        let index = Index::from(i);
                        quote! { #index }
                    };
                    reads.push(if attrs.with.is_some() {
                        let read = read_field(&wire, &attrs);
                        quote! { value.#member = #read; }
                    } else {
                        quote! { #wire::read_into(&mut value.#member, r)?; }
                    });
                }
                let read = quote! {
                    let mut value: Self = ::core::default::Default::default();
                    #(#reads)*
                    ::core::option::Option::Some(value)
                };
                (write, read)
            } else {
                (write, quote! { ::core::option::Option::Some(Self #read) })
            }
        }
        Data::Enum(data) => {
            let mut tag_arms = Vec::new();
            let mut write_arms = Vec::new();
            let mut read_arms = Vec::new();
            let unit_only = data.variants.iter().all(|variant| matches!(variant.fields, Fields::Unit));
            for (index, variant) in data.variants.iter().enumerate() {
                let index =
                    u32::try_from(index).map_err(|_| syn::Error::new(Span::call_site(), "too many variants"))?;
                let variant_name = &variant.ident;
                let names = bindings(&variant.fields);
                let pattern = pattern(&variant.fields, &names);
                let borrowed: Vec<TokenStream2> = names.iter().map(|name| quote! { #name }).collect();
                let (write, read) = fields_code(&wire, &variant.fields, &borrowed)?;
                let ignore = match &variant.fields {
                    Fields::Named(_) => quote! { { .. } },
                    Fields::Unnamed(_) => quote! { (..) },
                    Fields::Unit => quote! {},
                };
                tag_arms.push(quote! { Self::#variant_name #ignore => #index, });
                if !write.is_empty() {
                    write_arms.push(quote! { Self::#variant_name #pattern => { #write } });
                }
                read_arms.push(quote! { #index => ::core::option::Option::Some(Self::#variant_name #read), });
            }
            // The tag is written once, after a match that only finds it.
            let write = if data.variants.is_empty() {
                quote! { match *self {} }
            } else if write_arms.is_empty() {
                quote! { #wire::write_tag(w, match self { #(#tag_arms)* }); }
            } else {
                quote! {
                    #wire::write_tag(w, match self { #(#tag_arms)* });
                    #[allow(unreachable_patterns)]
                    match self { #(#write_arms)* _ => {} }
                }
            };
            // A fieldless enum is read from a table of its variants.
            let read = if unit_only && !data.variants.is_empty() && input.generics.params.is_empty() {
                let variants = data.variants.iter().map(|variant| &variant.ident);
                quote! {
                    const VARIANTS: &[#name] = &[#(#name::#variants),*];
                    match VARIANTS.get(r.tag()? as usize) {
                        ::core::option::Option::Some(variant) => ::core::option::Option::Some(::core::clone::Clone::clone(variant)),
                        ::core::option::Option::None => r.fail_tag(),
                    }
                }
            } else {
                quote! {
                    match r.tag()? {
                        #(#read_arms)*
                        _ => r.fail_tag(),
                    }
                }
            };
            (write, read)
        }
        Data::Union(_) => return Err(syn::Error::new(Span::call_site(), "unions can't derive Wire")),
    };

    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics #wire::Wire for #name #type_generics #where_clause {
            #[allow(unused_variables)]
            fn write(&self, w: &mut #wire::Writer) {
                #write
            }

            #[allow(unused_variables)]
            fn read(r: &mut #wire::Reader<'_>) -> ::core::option::Option<Self> {
                #read
            }
        }
    })
}
