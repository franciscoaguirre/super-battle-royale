use proc_macro::TokenStream;
use quote::quote;
use syn::parse::Parse;
use syn::{parse_macro_input, DeriveInput, Expr, Lit, Meta, MetaNameValue};

fn channel_from_attrs(attrs: &[syn::Attribute], default: &str) -> syn::Ident {
    for attr in attrs {
        if attr.path().is_ident("network") {
            if let Meta::List(list) = &attr.meta {
                let nested = list
                    .parse_args_with(|input: syn::parse::ParseStream| {
                        input.parse_terminated(Meta::parse, syn::Token![,])
                    })
                    .expect("invalid #[network(...)] attributes");
                for meta in nested {
                    if let Meta::NameValue(MetaNameValue { path, value, .. }) = meta {
                        if path.is_ident("channel") {
                            if let Expr::Lit(expr_lit) = value {
                                if let Lit::Str(s) = &expr_lit.lit {
                                    return syn::Ident::new(&s.value(), s.span());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    syn::Ident::new(default, proc_macro2::Span::call_site())
}

/// Derive macro for replicated gameplay components.
///
/// Use in addition to `Component`, `Serialize`, `Deserialize`, `Clone`, etc.:
///
/// ```ignore
/// #[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, Replicated)]
/// pub struct Player;
/// ```
///
/// It generates an `impl ReplicatedComponent` and an `impl NetworkRegistered`
/// that registers the component with `bevy_replicon`.
#[proc_macro_derive(Replicated)]
pub fn derive_replicated(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let expanded = quote! {
        impl #impl_generics crate::game::net::ReplicatedComponent for #name #ty_generics #where_clause {}

        impl #impl_generics crate::game::net::NetworkRegistered for #name #ty_generics #where_clause {
            fn register(app: &mut ::bevy::prelude::App) {
                use ::bevy_replicon::prelude::*;
                app.replicate::<#name>();
            }
        }
    };

    TokenStream::from(expanded)
}

/// Derive macro for client-to-server events.
///
/// Use in addition to `Event`, `Serialize`, `Deserialize`:
///
/// ```ignore
/// #[derive(Event, Serialize, Deserialize, ClientEvent)]
/// #[network(channel = "Unreliable")]
/// pub struct PlayerInput { ... }
/// ```
///
/// Valid channels: `Unreliable`, `Unordered`, `Ordered`.
#[proc_macro_derive(ClientEvent, attributes(network))]
pub fn derive_client_event(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let channel = channel_from_attrs(&input.attrs, "Ordered");
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let expanded = quote! {
        impl #impl_generics crate::game::net::NetworkEvent for #name #ty_generics #where_clause {}

        impl #impl_generics crate::game::net::NetworkRegistered for #name #ty_generics #where_clause {
            fn register(app: &mut ::bevy::prelude::App) {
                use ::bevy_replicon::prelude::*;
                app.add_client_event::<#name>(Channel::#channel);
            }
        }
    };

    TokenStream::from(expanded)
}

/// Derive macro for server-to-client events.
///
/// Use in addition to `Event`, `Serialize`, `Deserialize`:
///
/// ```ignore
/// #[derive(Event, Serialize, Deserialize, ServerEvent)]
/// #[network(channel = "Ordered")]
/// pub struct YouAreOwner;
/// ```
#[proc_macro_derive(ServerEvent, attributes(network))]
pub fn derive_server_event(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let channel = channel_from_attrs(&input.attrs, "Ordered");
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let expanded = quote! {
        impl #impl_generics crate::game::net::NetworkEvent for #name #ty_generics #where_clause {}

        impl #impl_generics crate::game::net::NetworkRegistered for #name #ty_generics #where_clause {
            fn register(app: &mut ::bevy::prelude::App) {
                use ::bevy_replicon::prelude::*;
                app.add_server_event::<#name>(Channel::#channel);
            }
        }
    };

    TokenStream::from(expanded)
}
