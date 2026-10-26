//! Pointing the router at an identity provider.
//!
//! The backend for this has existed the whole time -- settings persist ten SSO fields, the callback
//! verifies an `id_token` against the provider's JWKS, and there are test endpoints for both halves --
//! with no way to configure any of it from the dashboard. An operator had to PATCH `/api/settings` by
//! hand.
//!
//! Presets are the part worth explaining. They are not integrations: every provider here speaks the
//! same OIDC discovery protocol, and what differs is the shape of the issuer URL and which scopes the
//! provider expects to be asked for. A preset fills those two in and nothing else, so "Okta" is a
//! shortcut for a URL pattern rather than a code path. That keeps the list additive -- a provider not
//! named here works through `Custom` with no change to this file.
//!
//! Both secrets are write-only. `GET /api/settings` reports whether a client secret and a certificate
//! are *stored*, never their values, so the fields start blank and an empty submit leaves the stored
//! value alone. Sending the blank would clear a working secret, which is a way to lock everyone out
//! by pressing Save.
//!
//! # Why each protocol is its own card
//!
//! OIDC and SAML are two separate configurations with separate writes; the previous version put them
//! in consecutive bordered `<section>`s that read as one long form, so a failure under the SAML fields
//! looked like a failure of the OIDC save above it. Each protocol is now a `Card` with one footer
//! action, and the refusal lands inside the card that produced it.

use leptos::prelude::*;
use serde::Deserialize;

use crate::api::{Hydrate, Method, Save, encode, load, request_detailed, submit_reporting};
use crate::routes::types::{SettingsPatch, SettingsView};
use crate::routes::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Button, ButtonSize, ButtonVariant, Card,
    CardContent, CardDescription, CardFooter, CardHeader, CardTitle, ErrorNote, Field, Icon,
    IconName, Input, Mono, PageBody, PageHeader, Panel, Select, Textarea,
};
use crate::t;

/// A provider's OIDC shape: the issuer pattern it publishes discovery under, and the scopes it wants.
///
/// `issuer_hint` is a pattern with a placeholder, not a URL. It cannot be a working default -- every
/// one of these is per-tenant -- so it goes in the field as a starting point the operator edits.
struct Preset {
    id: &'static str,
    label: &'static str,
    issuer_hint: &'static str,
    scopes: &'static str,
}

/// The providers whose issuer shape is worth pre-filling.
///
/// Ordered by how often they turn up in an enterprise. `Custom` is last and is the one that does
/// nothing, which is also what makes this list safe to extend: nothing else in the codebase reads it.
const PRESETS: &[Preset] = &[
    Preset {
        id: "entra",
        label: "Microsoft Entra ID",
        issuer_hint: "https://login.microsoftonline.com/<tenant-id>/v2.0",
        scopes: "openid email profile",
    },
    Preset {
        id: "okta",
        label: "Okta",
        issuer_hint: "https://<your-org>.okta.com",
        scopes: "openid email profile",
    },
    Preset {
        id: "google",
        label: "Google Workspace",
        issuer_hint: "https://accounts.google.com",
        scopes: "openid email profile",
    },
    Preset {
        id: "auth0",
        label: "Auth0",
        issuer_hint: "https://<your-tenant>.us.auth0.com",
        scopes: "openid email profile",
    },
    Preset {
        id: "keycloak",
        label: "Keycloak",
        issuer_hint: "https://<host>/realms/<realm>",
        scopes: "openid email profile",
    },
    Preset {
        id: "onelogin",
        label: "OneLogin",
        issuer_hint: "https://<your-org>.onelogin.com/oidc/2",
        scopes: "openid email profile",
    },
    Preset {
        id: "jumpcloud",
        label: "JumpCloud",
        issuer_hint: "https://oauth.id.jumpcloud.com/",
        scopes: "openid email profile",
    },
];

/// What a test endpoint answers.
///
/// `ok` is the verdict, not the status: both test routes answer `200` with `ok: false` when the
/// provider could not be reached, so treating the status as the answer reports a broken configuration
/// as working.
#[derive(Clone, Debug, Default, Deserialize)]
struct TestOutcome {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    message: String,
}

impl TestOutcome {
    /// The provider's own wording, whichever field carried it.
    fn detail(&self) -> Option<String> {
        [self.error.as_str(), self.message.as_str()]
            .into_iter()
            .map(str::trim)
            .find(|text| !text.is_empty())
            .map(str::to_owned)
    }
}

#[component]
pub fn Sso() -> impl IntoView {
    let (settings, set_settings) = signal(Hydrate::<SettingsView>::Loading);
    let reload = move || {
        set_settings.set(Hydrate::Loading);
        load("/api/settings", set_settings);
    };
    reload();

    view! {
        <PageHeader title=t!("nav.sso") description=t!("sso.description") />

        <PageBody>
            <Panel
                state=settings
                on_retry=Callback::new(move |()| reload())
                children=move |data: SettingsView| {
                    view! {
                        <div class="grid gap-4 xl:grid-cols-2">
                            <OidcForm data=data.clone() reload=reload />
                            <SamlForm data=data reload=reload />
                        </div>
                    }
                }
            />
        </PageBody>
    }
}

/// The origin an IdP has to redirect back to.
///
/// Read from the browser rather than configured, because it is whatever the operator is reaching the
/// dashboard on right now -- which is exactly what has to be registered with the provider. A
/// hardcoded `localhost` would be wrong for every deployment that is not one.
#[cfg(target_arch = "wasm32")]
fn public_origin() -> String {
    web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
const fn public_origin() -> String {
    String::new()
}

#[component]
fn OidcForm(data: SettingsView, reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let (issuer, set_issuer) = signal(data.oidc_issuer_url.clone());
    let (client_id, set_client_id) = signal(data.oidc_client_id.clone());
    let (secret, set_secret) = signal(String::new());
    let (scopes, set_scopes) = signal(data.oidc_scopes.clone());
    let (label, set_label) = signal(data.oidc_login_label.clone());
    let (save, set_save) = signal(Save::Idle);
    let (test, set_test) = signal(Save::Idle);
    let (outcome, set_outcome) = signal(None::<TestOutcome>);

    let secret_stored = data.oidc_client_secret_set;
    let encode_failed = StoredValue::new(t!("sso.encode_failed"));
    let redirect_uri = format!("{}/api/auth/oidc/callback", public_origin());

    // Only the fields that changed are sent. The store treats a present key as "set this", so
    // submitting the blank secret field would clear a working secret.
    let patch = move || SettingsPatch {
        oidc_issuer_url: Some(issuer.get().trim().to_owned()),
        oidc_client_id: Some(client_id.get().trim().to_owned()),
        oidc_client_secret: {
            let value = secret.get();
            (!value.is_empty()).then_some(value)
        },
        oidc_scopes: Some(scopes.get().trim().to_owned()),
        oidc_login_label: Some(label.get().trim().to_owned()),
        ..SettingsPatch::default()
    };

    let store = move || {
        if save.get().is_saving() {
            return;
        }
        let Ok(encoded) = encode(&patch()) else {
            set_save.set(Save::Refused(encode_failed.get_value()));
            return;
        };
        submit_reporting(
            set_save,
            move || async move {
                request_detailed(Method::Patch, "/api/settings", Some(&encoded)).await
            },
            move |_| {
                set_secret.set(String::new());
                reload();
            },
        );
    };

    let probe = move || {
        if test.get().is_saving() {
            return;
        }
        set_outcome.set(None);
        // Tested with the values on screen rather than the stored ones, so a change can be checked
        // before it is saved. That is the whole point of a test button here.
        let body = serde_json::json!({
            "issuerUrl": issuer.get().trim(),
            "clientId": client_id.get().trim(),
            "clientSecret": secret.get(),
            "scopes": scopes.get().trim(),
        });
        let Ok(encoded) = encode(&body) else {
            set_test.set(Save::Refused(encode_failed.get_value()));
            return;
        };
        submit_reporting(
            set_test,
            move || async move {
                request_detailed(Method::Post, "/api/auth/oidc/test", Some(&encoded)).await
            },
            move |response| {
                set_outcome.set(crate::api::decode::<TestOutcome>(&response).ok());
            },
        );
    };

    // Whether a secret is on file is the one fact the API will tell us about it, so it becomes the
    // field's description rather than a sentence elsewhere in the card.
    let secret_state = if secret_stored {
        t!("sso.secret_stored")
    } else {
        t!("sso.secret_not_stored")
    };

    view! {
        <Card>
            <CardHeader>
                <div class="space-y-1">
                    <CardTitle>{t!("sso.oidc")}</CardTitle>
                    <CardDescription>{t!("sso.oidc_hint")}</CardDescription>
                </div>
                <StoredMark stored=secret_stored />
            </CardHeader>

            <CardContent class="space-y-3">
                <PresetPicker set_issuer=set_issuer set_scopes=set_scopes />

                <div class="grid gap-3 sm:grid-cols-2">
                    <Field
                        label=t!("sso.issuer_url")
                        description=t!("sso.issuer_url_hint")
                        required=true
                    >
                        <Input
                            value=Signal::derive(move || issuer.get())
                            on_input=Callback::new(move |value| set_issuer.set(value))
                            placeholder=t!("sso.issuer_placeholder")
                            autocomplete="off"
                            class="font-mono text-xs"
                        />
                    </Field>
                    <Field label=t!("sso.client_id") required=true>
                        <Input
                            value=Signal::derive(move || client_id.get())
                            on_input=Callback::new(move |value| set_client_id.set(value))
                            placeholder=t!("sso.client_id_placeholder")
                            autocomplete="off"
                            class="font-mono text-xs"
                        />
                    </Field>
                    <Field label=t!("sso.client_secret") description=secret_state>
                        <Input
                            input_type="password"
                            value=Signal::derive(move || secret.get())
                            on_input=Callback::new(move |value| set_secret.set(value))
                            placeholder=t!("sso.secret_placeholder")
                            autocomplete="off"
                        />
                    </Field>
                    <Field label=t!("sso.scopes")>
                        <Input
                            value=Signal::derive(move || scopes.get())
                            on_input=Callback::new(move |value| set_scopes.set(value))
                            placeholder=t!("sso.scopes_placeholder")
                            autocomplete="off"
                            class="font-mono text-xs"
                        />
                    </Field>
                    <Field
                        label=t!("sso.login_label")
                        description=t!("sso.login_label_hint")
                        class="sm:col-span-2"
                    >
                        <Input
                            value=Signal::derive(move || label.get())
                            on_input=Callback::new(move |value| set_label.set(value))
                            placeholder=t!("sso.login_label_placeholder")
                            autocomplete="off"
                        />
                    </Field>
                </div>

                <Readback label=t!("sso.redirect_uri") value=redirect_uri />

                {move || {
                    save.get().message().map(|message| view! { <ErrorNote message=message /> })
                }}
                {move || {
                    test.get().message().map(|message| view! { <ErrorNote message=message /> })
                }}
                <TestVerdict outcome=outcome />
            </CardContent>

            <CardFooter>
                <Button
                    label=t!("sso.save")
                    size=ButtonSize::Sm
                    disabled=Signal::derive(move || save.get().is_saving())
                    busy=Signal::derive(move || save.get().is_saving())
                    on_click=Callback::new(move |()| store())
                />
                <Button
                    label=t!("sso.test")
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    disabled=Signal::derive(move || {
                        test.get().is_saving() || issuer.get().trim().is_empty()
                    })
                    busy=Signal::derive(move || test.get().is_saving())
                    on_click=Callback::new(move |()| probe())
                />
            </CardFooter>
        </Card>
    }
}

/// A small mark in a protocol card's header saying whether its secret is on file.
///
/// Separate from the sentence beside the field because the card's header is what an operator scans
/// first: "is this actually configured?" should be answerable without reading the form. Not a
/// `Badge` with a word, because the sentence under the field already carries the wording and two
/// copies of it in one card read as two different facts.
#[component]
fn StoredMark(stored: bool) -> impl IntoView {
    if stored {
        view! { <Icon name=IconName::Check class="size-4 shrink-0 text-success" /> }.into_any()
    } else {
        view! { <Icon name=IconName::Info class="size-4 shrink-0 text-muted-foreground" /> }
            .into_any()
    }
}

#[component]
fn SamlForm(data: SettingsView, reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let (entry, set_entry) = signal(data.saml_entry_point.clone());
    let (issuer, set_issuer) = signal(data.saml_issuer.clone());
    let (cert, set_cert) = signal(String::new());
    let (email_attr, set_email_attr) = signal(data.saml_attribute_email.clone());
    let (name_attr, set_name_attr) = signal(data.saml_attribute_name.clone());
    let (save, set_save) = signal(Save::Idle);

    let cert_stored = data.saml_cert_set;
    let encode_failed = StoredValue::new(t!("sso.encode_failed"));
    let acs_url = format!("{}/api/auth/saml/acs", public_origin());

    let store = move || {
        if save.get().is_saving() {
            return;
        }
        let patch = SettingsPatch {
            saml_entry_point: Some(entry.get().trim().to_owned()),
            saml_issuer: Some(issuer.get().trim().to_owned()),
            // As with the client secret: blank means "leave the stored one alone".
            saml_cert: {
                let value = cert.get();
                (!value.trim().is_empty()).then_some(value)
            },
            saml_attribute_email: Some(email_attr.get().trim().to_owned()),
            saml_attribute_name: Some(name_attr.get().trim().to_owned()),
            ..SettingsPatch::default()
        };
        let Ok(encoded) = encode(&patch) else {
            set_save.set(Save::Refused(encode_failed.get_value()));
            return;
        };
        submit_reporting(
            set_save,
            move || async move {
                request_detailed(Method::Patch, "/api/settings", Some(&encoded)).await
            },
            move |_| {
                set_cert.set(String::new());
                reload();
            },
        );
    };

    let cert_state = if cert_stored {
        t!("sso.cert_stored")
    } else {
        t!("sso.cert_not_stored")
    };

    view! {
        <Card>
            <CardHeader>
                <div class="space-y-1">
                    <CardTitle>{t!("sso.saml")}</CardTitle>
                    <CardDescription>{t!("sso.saml_hint")}</CardDescription>
                </div>
                <StoredMark stored=cert_stored />
            </CardHeader>

            <CardContent class="space-y-3">
                // Assertions are recorded and refused on this build; an operator needs to know that
                // before spending an afternoon registering the router with their IdP.
                <Alert variant=AlertVariant::Warning>
                    <AlertTitle>{t!("sso.saml_unsupported_title")}</AlertTitle>
                    <AlertDescription>{t!("sso.saml_unsupported")}</AlertDescription>
                </Alert>

                <div class="grid gap-3 sm:grid-cols-2">
                    <Field label=t!("sso.entry_point") required=true>
                        <Input
                            value=Signal::derive(move || entry.get())
                            on_input=Callback::new(move |value| set_entry.set(value))
                            placeholder=t!("sso.entry_point_placeholder")
                            autocomplete="off"
                            class="font-mono text-xs"
                        />
                    </Field>
                    <Field label=t!("sso.saml_issuer") required=true>
                        <Input
                            value=Signal::derive(move || issuer.get())
                            on_input=Callback::new(move |value| set_issuer.set(value))
                            placeholder=t!("sso.saml_issuer_placeholder")
                            autocomplete="off"
                            class="font-mono text-xs"
                        />
                    </Field>
                    <Field label=t!("sso.attr_email")>
                        <Input
                            value=Signal::derive(move || email_attr.get())
                            on_input=Callback::new(move |value| set_email_attr.set(value))
                            placeholder=t!("sso.email_placeholder")
                            autocomplete="off"
                            class="font-mono text-xs"
                        />
                    </Field>
                    <Field label=t!("sso.attr_name")>
                        <Input
                            value=Signal::derive(move || name_attr.get())
                            on_input=Callback::new(move |value| set_name_attr.set(value))
                            placeholder=t!("sso.display_name_placeholder")
                            autocomplete="off"
                            class="font-mono text-xs"
                        />
                    </Field>
                </div>

                <Field label=t!("sso.cert") description=cert_state class="sm:col-span-2">
                    <Textarea
                        value=Signal::derive(move || cert.get())
                        on_input=Callback::new(move |value| set_cert.set(value))
                        placeholder=t!("sso.cert_placeholder")
                        rows=4
                        class="font-mono text-xs"
                    />
                </Field>

                <Readback label=t!("sso.acs_url") value=acs_url />

                {move || {
                    save.get().message().map(|message| view! { <ErrorNote message=message /> })
                }}
            </CardContent>

            <CardFooter>
                <Button
                    label=t!("sso.save")
                    size=ButtonSize::Sm
                    disabled=Signal::derive(move || save.get().is_saving())
                    busy=Signal::derive(move || save.get().is_saving())
                    on_click=Callback::new(move |()| store())
                />
                <a
                    href="/api/auth/saml/metadata"
                    target="_blank"
                    rel="noreferrer noopener"
                    class=crate::ui::button_class(ButtonVariant::Outline, ButtonSize::Sm)
                >
                    <Icon name=IconName::Download class="size-4" />
                    {t!("sso.metadata_link")}
                </a>
            </CardFooter>
        </Card>
    }
}

/// Fills the issuer pattern and scopes for a named provider.
#[component]
fn PresetPicker(set_issuer: WriteSignal<String>, set_scopes: WriteSignal<String>) -> impl IntoView {
    let (chosen, set_chosen) = signal(String::new());

    // `(value, label)` pairs for `Select`. `Custom` leads and is what a blank selection means, so the
    // list reads as "start from a named provider, or type your own".
    let options = Signal::derive(|| {
        let mut options = vec![(String::new(), t!("sso.preset_custom"))];
        options.extend(
            PRESETS
                .iter()
                .map(|preset| (preset.id.to_owned(), preset.label.to_owned())),
        );
        options
    });

    view! {
        <Field
            label=t!("sso.preset")
            description=t!("sso.preset_hint")
            class="sm:max-w-xs"
        >
            <Select
                value=Signal::derive(move || chosen.get())
                options=options
                on_change=Callback::new(move |picked: String| {
                    if let Some(preset) = PRESETS.iter().find(|preset| preset.id == picked) {
                        set_issuer.set(preset.issuer_hint.to_owned());
                        set_scopes.set(preset.scopes.to_owned());
                    }
                    set_chosen.set(picked);
                })
                aria_label=t!("sso.preset")
            />
        </Field>
    }
}

/// A value the operator has to copy into their IdP, shown read-only.
#[component]
fn Readback(label: String, value: String) -> impl IntoView {
    view! {
        <div class="space-y-1">
            <p class="text-[0.8125rem] font-medium text-foreground">{label}</p>
            <Mono
                value=value
                class="block rounded-md border border-border bg-muted/40 px-2.5 py-1.5 text-xs"
            />
        </div>
    }
}

/// What a finished test said.
///
/// The success case is a sentence with an icon rather than a coloured word: "the provider answered"
/// is information, and a green adjective on its own does not say *what* answered.
#[component]
fn TestVerdict(outcome: ReadSignal<Option<TestOutcome>>) -> impl IntoView {
    view! {
        {move || {
            outcome
                .get()
                .map(|result| {
                    if result.ok {
                        view! {
                            <p class="flex items-center gap-1.5 text-[0.8125rem] text-success-emphasis">
                                <Icon name=IconName::Check class="size-3.5 shrink-0 text-success" />
                                {t!("sso.test_ok")}
                            </p>
                        }
                            .into_any()
                    } else {
                        // The provider's own wording. "Failed" names nothing to fix.
                        let detail = result
                            .detail()
                            .unwrap_or_else(|| t!("providers.test_failed"));
                        view! { <ErrorNote message=detail /> }.into_any()
                    }
                })
        }}
    }
}
