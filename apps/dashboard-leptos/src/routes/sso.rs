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

use leptos::prelude::*;
use serde::Deserialize;

use crate::api::{Hydrate, Method, Save, encode, load, request_detailed, submit_reporting};
use crate::routes::types::{SettingsPatch, SettingsView};
use crate::routes::{PageHeader, Panel};

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
    let locale = crate::i18n::use_locale();
    let (settings, set_settings) = signal(Hydrate::<SettingsView>::Loading);
    let reload = move || {
        set_settings.set(Hydrate::Loading);
        load("/api/settings", set_settings);
    };
    reload();

    view! {
        <PageHeader
            title=locale.get("nav.sso").to_owned()
            description=locale.get("sso.description").to_owned()
        />
        <Panel
            state=settings
            on_retry=Callback::new(move |()| reload())
            children=move |data: SettingsView| {
                view! {
                    <div class="space-y-4">
                        <OidcForm data=data.clone() reload=reload />
                        <SamlForm data=data reload=reload />
                    </div>
                }
            }
        />
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
#[allow(clippy::missing_const_for_fn)]
fn public_origin() -> String {
    String::new()
}

#[component]
fn OidcForm(data: SettingsView, reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (issuer, set_issuer) = signal(data.oidc_issuer_url.clone());
    let (client_id, set_client_id) = signal(data.oidc_client_id.clone());
    let (secret, set_secret) = signal(String::new());
    let (scopes, set_scopes) = signal(data.oidc_scopes.clone());
    let (label, set_label) = signal(data.oidc_login_label.clone());
    let (save, set_save) = signal(Save::Idle);
    let (test, set_test) = signal(Save::Idle);
    let (outcome, set_outcome) = signal(None::<TestOutcome>);

    let secret_stored = data.oidc_client_secret_set;
    let encode_failed = StoredValue::new(locale.get("sso.encode_failed").to_owned());
    let label_save = locale.get("sso.save").to_owned();
    let label_test = locale.get("sso.test").to_owned();
    let label_testing = locale.get("sso.testing").to_owned();
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

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("sso.oidc").to_owned()}
            </h2>
            <PresetPicker set_issuer=set_issuer set_scopes=set_scopes />
            <div class="grid gap-3 sm:grid-cols-2">
                <Field
                    label=locale.get("sso.issuer_url").to_owned()
                    value=issuer
                    set_value=set_issuer
                    placeholder=locale.get("sso.issuer_placeholder").to_owned()
                    mono=true
                />
                <Field
                    label=locale.get("sso.client_id").to_owned()
                    value=client_id
                    set_value=set_client_id
                    placeholder=locale.get("sso.client_id_placeholder").to_owned()
                    mono=true
                />
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("sso.client_secret").to_owned()}
                    </span>
                    <input
                        type="password"
                        autocomplete="off"
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                        prop:value=move || secret.get()
                        on:input=move |ev| set_secret.set(event_target_value(&ev))
                        placeholder=locale.get("sso.secret_placeholder").to_owned()
                    />
                    <span class="block text-xs text-muted-foreground">
                        {if secret_stored {
                            locale.get("sso.secret_stored").to_owned()
                        } else {
                            locale.get("sso.secret_not_stored").to_owned()
                        }}
                    </span>
                </label>
                <Field
                    label=locale.get("sso.scopes").to_owned()
                    value=scopes
                    set_value=set_scopes
                    placeholder=locale.get("sso.scopes_placeholder").to_owned()
                    mono=true
                />
                <Field
                    label=locale.get("sso.login_label").to_owned()
                    value=label
                    set_value=set_label
                    placeholder=locale.get("sso.login_label_placeholder").to_owned()
                    mono=false
                />
            </div>
            <Readback label=locale.get("sso.redirect_uri").to_owned() value=redirect_uri />
            <div class="flex items-center gap-3">
                <button
                    type="button"
                    class="rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
                    disabled=move || save.get().is_saving()
                    on:click=move |_| store()
                >
                    {label_save}
                </button>
                <button
                    type="button"
                    class="rounded-md border border-border px-3 py-2 text-sm disabled:opacity-50"
                    disabled=move || {
                        test.get().is_saving() || issuer.get().trim().is_empty()
                    }
                    on:click=move |_| probe()
                >
                    {move || {
                        if test.get().is_saving() {
                            label_testing.clone()
                        } else {
                            label_test.clone()
                        }
                    }}
                </button>
            </div>
            <Outcome state=save />
            <Outcome state=test />
            <TestVerdict outcome=outcome />
        </section>
    }
}

#[component]
fn SamlForm(data: SettingsView, reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (entry, set_entry) = signal(data.saml_entry_point.clone());
    let (issuer, set_issuer) = signal(data.saml_issuer.clone());
    let (cert, set_cert) = signal(String::new());
    let (email_attr, set_email_attr) = signal(data.saml_attribute_email.clone());
    let (name_attr, set_name_attr) = signal(data.saml_attribute_name.clone());
    let (save, set_save) = signal(Save::Idle);

    let cert_stored = data.saml_cert_set;
    let encode_failed = StoredValue::new(locale.get("sso.encode_failed").to_owned());
    let label_save = locale.get("sso.save").to_owned();
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

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("sso.saml").to_owned()}
            </h2>
            <p class="rounded-md border border-warning/40 bg-warning/10 px-3 py-2 text-sm">
                {locale.get("sso.saml_unsupported").to_owned()}
            </p>
            <div class="grid gap-3 sm:grid-cols-2">
                <Field
                    label=locale.get("sso.entry_point").to_owned()
                    value=entry
                    set_value=set_entry
                    placeholder=locale.get("sso.entry_point_placeholder").to_owned()
                    mono=true
                />
                <Field
                    label=locale.get("sso.saml_issuer").to_owned()
                    value=issuer
                    set_value=set_issuer
                    placeholder=locale.get("sso.saml_issuer_placeholder").to_owned()
                    mono=true
                />
                <Field
                    label=locale.get("sso.attr_email").to_owned()
                    value=email_attr
                    set_value=set_email_attr
                    placeholder=locale.get("sso.email_placeholder").to_owned()
                    mono=true
                />
                <Field
                    label=locale.get("sso.attr_name").to_owned()
                    value=name_attr
                    set_value=set_name_attr
                    placeholder=locale.get("sso.display_name_placeholder").to_owned()
                    mono=true
                />
            </div>
            <label class="block space-y-1 text-sm">
                <span class="text-muted-foreground">{locale.get("sso.cert").to_owned()}</span>
                <textarea
                    rows="4"
                    class="w-full rounded-md border border-input bg-background px-3 py-2 text-xs font-mono"
                    prop:value=move || cert.get()
                    on:input=move |ev| set_cert.set(event_target_value(&ev))
                    placeholder=locale.get("sso.cert_placeholder").to_owned()
                ></textarea>
                <span class="block text-xs text-muted-foreground">
                    {if cert_stored {
                        locale.get("sso.cert_stored").to_owned()
                    } else {
                        locale.get("sso.cert_not_stored").to_owned()
                    }}
                </span>
            </label>
            <Readback label=locale.get("sso.acs_url").to_owned() value=acs_url />
            <div class="flex items-center gap-3">
                <button
                    type="button"
                    class="rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
                    disabled=move || save.get().is_saving()
                    on:click=move |_| store()
                >
                    {label_save}
                </button>
                <a
                    href="/api/auth/saml/metadata"
                    target="_blank"
                    rel="noreferrer noopener"
                    class="text-sm underline-offset-4 hover:underline"
                >
                    {locale.get("sso.metadata_link").to_owned()}
                </a>
            </div>
            <Outcome state=save />
        </section>
    }
}

/// Fills the issuer pattern and scopes for a named provider.
#[component]
fn PresetPicker(set_issuer: WriteSignal<String>, set_scopes: WriteSignal<String>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    view! {
        <label class="block space-y-1 text-sm">
            <span class="text-muted-foreground">{locale.get("sso.preset").to_owned()}</span>
            <select
                class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                on:change=move |ev| {
                    let chosen = event_target_value(&ev);
                    if let Some(preset) = PRESETS.iter().find(|preset| preset.id == chosen) {
                        set_issuer.set(preset.issuer_hint.to_owned());
                        set_scopes.set(preset.scopes.to_owned());
                    }
                }
            >
                <option value="">{locale.get("sso.preset_custom").to_owned()}</option>
                {PRESETS
                    .iter()
                    .map(|preset| {
                        view! { <option value=preset.id>{preset.label}</option> }
                    })
                    .collect_view()}
            </select>
            <span class="block text-xs text-muted-foreground">
                {locale.get("sso.preset_hint").to_owned()}
            </span>
        </label>
    }
}

/// A labelled text input bound to a signal.
#[component]
fn Field(
    label: String,
    value: ReadSignal<String>,
    set_value: WriteSignal<String>,
    placeholder: String,
    mono: bool,
) -> impl IntoView {
    let class = if mono {
        "w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono text-xs"
    } else {
        "w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
    };
    view! {
        <label class="space-y-1 text-sm">
            <span class="text-muted-foreground">{label}</span>
            <input
                type="text"
                class=class
                prop:value=move || value.get()
                on:input=move |ev| set_value.set(event_target_value(&ev))
                placeholder=placeholder
            />
        </label>
    }
}

/// A value the operator has to copy into their IdP, shown read-only.
#[component]
fn Readback(label: String, value: String) -> impl IntoView {
    view! {
        <div class="space-y-1">
            <span class="block text-xs text-muted-foreground">{label}</span>
            <code class="block truncate rounded-md border border-border bg-muted/40 px-3 py-2 text-xs">
                {value}
            </code>
        </div>
    }
}

/// The refusal from a save or a test, if there was one.
#[component]
fn Outcome(state: ReadSignal<Save>) -> impl IntoView {
    view! {
        {move || {
            state
                .get()
                .message()
                .map(|message| {
                    view! {
                        <p class="text-sm text-destructive whitespace-normal break-words">
                            {message}
                        </p>
                    }
                })
        }}
    }
}

/// What a finished test said.
#[component]
fn TestVerdict(outcome: ReadSignal<Option<TestOutcome>>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    view! {
        {move || {
            outcome
                .get()
                .map(|result| {
                    if result.ok {
                        view! {
                            <p class="text-sm text-success">
                                {locale.get("sso.test_ok").to_owned()}
                            </p>
                        }
                            .into_any()
                    } else {
                        // The provider's own wording. "Failed" names nothing to fix.
                        let detail = result
                            .detail()
                            .unwrap_or_else(|| locale.get("providers.test_failed").to_owned());
                        view! {
                            <p class="text-sm text-destructive whitespace-normal break-words">
                                {detail}
                            </p>
                        }
                            .into_any()
                    }
                })
        }}
    }
}
