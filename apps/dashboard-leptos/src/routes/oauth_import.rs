//! Importing an existing provider credential.
//!
//! Every field on this page is a live credential, and the handling follows from that rather than
//! from convenience:
//!
//! - Credential inputs are `type="password"`. Non-secret fields alongside them -- a region, a base
//!   URL, a label -- are plain text, because masking a value the user has to *check* trades real
//!   safety for the appearance of it.
//! - A submitted value is never rendered back. What a result shows is the server's own sentence and,
//!   on success, the identity it derived from the credential's claims; nothing that was typed in.
//! - Fields clear on success, so a token does not sit in the DOM after it has been stored.
//! - Nothing is logged anywhere in this crate, so there is no path by which one of these reaches a
//!   console or a log buffer.
//!
//! The two auto-import routes are the sharp edge: they *answer* with a credential read off this
//! host's disk. Those responses are held in a signal and submitted from there. The token itself is
//! never put in an input, an attribute, or a text node -- what the panel shows is that one was
//! found, and the non-secret details identifying which install it came from.

use std::collections::BTreeMap;

use leptos::prelude::*;
use serde_json::Value;

use crate::api::{Hydrate, Method, Save, load};
use crate::routes::types::{ProviderRow, ProvidersList};
use crate::routes::{PageHeader, Panel, write_reporting};

/// One input in an import form.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FieldSpec {
    /// The JSON key the route reads.
    key: &'static str,
    /// Message key for the label.
    label: &'static str,
    /// Whether the value is a credential. Decides masking, and nothing else.
    secret: bool,
    /// Whether the route refuses the request without it.
    required: bool,
}

/// One import route, and the form that feeds it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ImportSpec {
    path: &'static str,
    /// Message key for the section heading.
    title: &'static str,
    /// Message key for the sentence under it.
    hint: &'static str,
    fields: &'static [FieldSpec],
}

/// `POST /api/oauth/gitlab/pat`. The token goes to GitLab in `Private-Token`, not as a bearer.
const GITLAB: ImportSpec = ImportSpec {
    path: "/api/oauth/gitlab/pat",
    title: "import.gitlab_title",
    hint: "import.gitlab_hint",
    fields: &[
        FieldSpec {
            key: "token",
            label: "import.field_pat",
            secret: true,
            required: true,
        },
        // Omitted when blank rather than sent empty: the route reads an absent `baseUrl` as
        // gitlab.com, and an empty string as a base URL it then refuses.
        FieldSpec {
            key: "baseUrl",
            label: "import.field_base_url",
            secret: false,
            required: false,
        },
    ],
};

/// `POST /api/oauth/kiro/api-key`. Verified against Amazon Q before the connection is recorded.
const KIRO_API_KEY: ImportSpec = ImportSpec {
    path: "/api/oauth/kiro/api-key",
    title: "import.kiro_key_title",
    hint: "import.kiro_key_hint",
    fields: &[
        FieldSpec {
            key: "apiKey",
            label: "import.field_api_key",
            secret: true,
            required: true,
        },
        // Not secret, and it must be checkable: the region becomes the first label of the hostname
        // the key is sent to, and the route refuses anything that is not `xx-yyyy-N`.
        FieldSpec {
            key: "region",
            label: "import.field_region",
            secret: false,
            required: false,
        },
    ],
};

/// `POST /api/oauth/kiro/import`.
///
/// The check here *is* a refresh -- Kiro has no read-only endpoint that would accept a refresh token
/// -- so a submit spends the credential. `clientId` and `clientSecret` are both-or-neither: with one
/// of them the route cannot tell which refresh protocol the token belongs to, and refuses.
const KIRO_OAUTH: ImportSpec = ImportSpec {
    path: "/api/oauth/kiro/import",
    title: "import.kiro_oauth_title",
    hint: "import.kiro_oauth_hint",
    fields: &[
        FieldSpec {
            key: "refreshToken",
            label: "import.field_refresh_token",
            secret: true,
            required: true,
        },
        // A public client identifier, and the field that decides the protocol. Left legible so the
        // both-or-neither pairing is something the user can see they have satisfied.
        FieldSpec {
            key: "clientId",
            label: "import.field_client_id",
            secret: false,
            required: false,
        },
        FieldSpec {
            key: "clientSecret",
            label: "import.field_client_secret",
            secret: true,
            required: false,
        },
        FieldSpec {
            key: "region",
            label: "import.field_region",
            secret: false,
            required: false,
        },
        // An ARN names an account and a role. Not a secret, and worth reading back: the route
        // rewrites its region, so seeing what was sent is how a surprise there gets noticed.
        FieldSpec {
            key: "profileArn",
            label: "import.field_profile_arn",
            secret: false,
            required: false,
        },
    ],
};

/// `POST /api/oauth/codex/import-token`. `name` is a label; the route falls back to the email claim.
const CODEX: ImportSpec = ImportSpec {
    path: "/api/oauth/codex/import-token",
    title: "import.codex_title",
    hint: "import.codex_hint",
    fields: &[
        FieldSpec {
            key: "accessToken",
            label: "import.field_access_token",
            secret: true,
            required: true,
        },
        FieldSpec {
            key: "name",
            label: "import.field_name",
            secret: false,
            required: false,
        },
    ],
};

/// `POST /api/oauth/cursor/import`.
///
/// Both values are checked for shape only. Cursor speaks protobuf and publishes nothing that would
/// accept this token in a probe, so a successful import here is not evidence the token works.
const CURSOR: ImportSpec = ImportSpec {
    path: "/api/oauth/cursor/import",
    title: "import.cursor_title",
    hint: "import.cursor_hint",
    fields: &[
        FieldSpec {
            key: "accessToken",
            label: "import.field_access_token",
            secret: true,
            required: true,
        },
        // Masked with the token it accompanies: it is an installation identifier that travels with
        // every later request, and the route's own message names a bad one clearly enough that
        // nothing is lost by not showing it.
        FieldSpec {
            key: "machineId",
            label: "import.field_machine_id",
            secret: true,
            required: true,
        },
    ],
};

/// `POST /api/oauth/iflow/cookie`. A whole browser cookie header; the route narrows it to the
/// session field itself.
const IFLOW: ImportSpec = ImportSpec {
    path: "/api/oauth/iflow/cookie",
    title: "import.iflow_title",
    hint: "import.iflow_hint",
    fields: &[FieldSpec {
        key: "cookie",
        label: "import.field_cookie",
        secret: true,
        required: true,
    }],
};

/// Every form-driven import, in the order the page shows them.
const FORMS: [ImportSpec; 6] = [GITLAB, KIRO_API_KEY, KIRO_OAUTH, CODEX, CURSOR, IFLOW];

/// A single import's answer.
///
/// `{success: true}` with an optional `connection` on a `200`; `{success: false, error}` on a
/// refusal. Both spellings decode here so the refusal's own sentence is what the panel shows.
#[derive(Clone, Debug, Default, serde::Deserialize)]
struct ImportReply {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    connection: Option<Connection>,
}

/// The identity a route derived from the credential it accepted.
///
/// Derived from the token's own claims, not from anything typed into the form -- which is why it is
/// safe to render. `name` is deliberately absent: at the codex route it is a submitted field, and
/// echoing a submitted value back is the habit this panel does not want to be in.
#[derive(Clone, Debug, Default, serde::Deserialize)]
struct Connection {
    #[serde(default)]
    provider: String,
    #[serde(default)]
    email: Option<String>,
}

/// `POST /api/oauth/codex/bulk-import`'s answer.
///
/// `success` is a **count** here, not a flag, and the route answers `200` even when every item
/// failed -- because the per-item report is the answer. Decoding this with the single-import shape
/// would read `success: 0` as `false` and `success: 3` as a type error, so it has its own.
#[derive(Clone, Debug, Default, serde::Deserialize)]
struct BulkReply {
    #[serde(default)]
    success: u32,
    #[serde(default)]
    failed: u32,
    #[serde(default)]
    results: Vec<BulkResult>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
struct BulkResult {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    error: String,
}

/// `GET /api/oauth/cursor/auto-import`'s answer.
///
/// `access_token` is a live credential this host read off local disk. It is never rendered; it is
/// held so the panel can submit it, and dropped when the panel does.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorFound {
    #[serde(default)]
    found: bool,
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    machine_id: String,
    #[serde(default)]
    error: String,
    /// Set when the database exists but could not be read here, so the manual form is the way in.
    #[serde(default)]
    manual: bool,
    #[serde(default)]
    db_path: String,
}

/// `GET /api/oauth/kiro/auto-import`'s answer.
///
/// `refresh_token` and `client_secret` are live credentials, held and never rendered. The rest is
/// what identifies which login on this host it came from, and is shown.
///
/// The optional fields arrive present-and-null rather than absent, and the distinction matters: a
/// `clientId`/`clientSecret` pair means an SSO-OIDC refresh, and their absence means a social login.
/// Sending one protocol's token to the other endpoint burns it, so neither is inferred here -- both
/// are forwarded exactly as found.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct KiroFound {
    #[serde(default)]
    found: bool,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    auth_method: Option<String>,
    #[serde(default)]
    profile_arn: Option<String>,
    /// Which file on this host the credential came out of.
    #[serde(default)]
    source: String,
    #[serde(default)]
    error: String,
}

/// One line of feedback, and whether it reports success.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Notice {
    ok: bool,
    text: String,
}

/// The two sentences a submit needs when the reply carries none of its own.
///
/// Grouped rather than passed as two more parameters, which keeps [`submit_import`] inside the
/// workspace's argument limit and makes the pair harder to swap by accident.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Wording {
    /// Shown when the router stored the connection but named nothing about it.
    stored: String,
    /// Shown when a `200` did not decode, or decoded with no error text.
    unreadable: String,
}

/// The success line: the identity the router derived from the credential, when it returned one.
///
/// Derived, never submitted. A provider and an email address come out of the token's own claims or
/// the provider's user endpoint, so showing them confirms *which account* was stored without
/// repeating any part of the credential that stored it.
fn describe(reply: &ImportReply, fallback: &str) -> String {
    let Some(connection) = reply.connection.as_ref() else {
        return fallback.to_owned();
    };
    let email = connection.email.as_deref().unwrap_or_default();
    if !connection.provider.is_empty() && !email.is_empty() {
        return format!("{fallback} \u{2014} {} ({email})", connection.provider);
    }
    if !connection.provider.is_empty() {
        return format!("{fallback} \u{2014} {}", connection.provider);
    }
    fallback.to_owned()
}

/// Post one credential payload, then report the outcome in the server's words.
///
/// `on_stored` runs only when the router said it stored the connection, which is where a form clears
/// itself. It does not run on a refusal, so a rejected import leaves the values in place to be fixed
/// rather than making the user re-paste a token.
fn submit_import<S>(
    path: &'static str,
    payload: String,
    busy: WriteSignal<Save>,
    notice: WriteSignal<Option<Notice>>,
    wording: Wording,
    on_stored: S,
) where
    S: FnOnce() + 'static,
{
    let Wording { stored, unreadable } = wording;
    busy.set(Save::Saving);
    notice.set(None);
    leptos::task::spawn_local(async move {
        match write_reporting(Method::Post, path, Some(&payload)).await {
            Ok(body) => {
                let reply = serde_json::from_str::<ImportReply>(&body).unwrap_or_default();
                if reply.success {
                    busy.set(Save::Saved);
                    notice.set(Some(Notice {
                        ok: true,
                        text: describe(&reply, &stored),
                    }));
                    on_stored();
                } else {
                    // A `200` whose `success` is false, or a body that did not decode. Neither is a
                    // stored connection, and neither is reported as one.
                    busy.set(Save::Idle);
                    notice.set(Some(Notice {
                        ok: false,
                        text: if reply.error.is_empty() {
                            unreadable
                        } else {
                            reply.error
                        },
                    }));
                }
            }
            Err(text) => {
                busy.set(Save::Idle);
                notice.set(Some(Notice { ok: false, text }));
            }
        }
    });
}

/// One import route as a form.
#[component]
fn ImportForm(spec: ImportSpec, refresh: Callback<()>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let values = RwSignal::new(BTreeMap::<&'static str, String>::new());
    let (busy, set_busy) = signal(Save::Idle);
    let (notice, set_notice) = signal(Option::<Notice>::None);

    let stored = locale.get("import.imported").to_owned();
    let unreadable = locale.get("import.unreadable").to_owned();
    let required = locale.get("import.required").to_owned();

    let submit = move || {
        let mut payload = serde_json::Map::new();
        let mut missing = false;
        for field in spec.fields {
            let value = values.with(|map| map.get(field.key).cloned().unwrap_or_default());
            let value = value.trim().to_owned();
            if value.is_empty() {
                // Omitted, not sent empty: several routes read an absent field as "use the default"
                // and an empty one as a value they then refuse.
                missing |= field.required;
                continue;
            }
            payload.insert(field.key.to_owned(), Value::String(value));
        }
        if missing {
            set_busy.set(Save::Idle);
            set_notice.set(Some(Notice {
                ok: false,
                text: required.clone(),
            }));
            return;
        }
        let Ok(body) = serde_json::to_string(&Value::Object(payload)) else {
            return;
        };
        submit_import(
            spec.path,
            body,
            set_busy,
            set_notice,
            Wording {
                stored: stored.clone(),
                unreadable: unreadable.clone(),
            },
            move || {
                values.set(BTreeMap::new());
                refresh.run(());
            },
        );
    };

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4">
            <SectionHead title=locale.get(spec.title).to_owned() hint=locale.get(spec.hint).to_owned() path=spec.path />
            <div class="grid gap-3 sm:grid-cols-2">
                {spec
                    .fields
                    .iter()
                    .map(|field| view! { <CredentialInput field=*field values=values /> })
                    .collect_view()}
            </div>
            <SubmitButton
                label=locale.get("import.submit").to_owned()
                busy=busy
                on_run=Callback::new(move |()| submit())
            />
            <NoticeLine notice=notice />
        </section>
    }
}

const CLI_PROXY_PATH: &str = "/api/oauth/kiro/import-cli-proxy";
const BULK_PATH: &str = "/api/oauth/codex/bulk-import";
const CURSOR_AUTO_PATH: &str = "/api/oauth/cursor/auto-import";
const KIRO_AUTO_PATH: &str = "/api/oauth/kiro/auto-import";
const KIRO_IMPORT_PATH: &str = "/api/oauth/kiro/import";
const CURSOR_IMPORT_PATH: &str = "/api/oauth/cursor/import";

/// A pasted `CLIProxyAPI` auth document.
///
/// Masked like any other credential field, because the document contains a refresh token. That costs
/// the ability to eyeball the JSON, which the local parse below gives back: a paste that is not JSON
/// is named as one here rather than coming back as a complaint about a missing field.
#[component]
fn CliProxyForm(refresh: Callback<()>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (document, set_document) = signal(String::new());
    let (busy, set_busy) = signal(Save::Idle);
    let (notice, set_notice) = signal(Option::<Notice>::None);

    let stored = locale.get("import.imported").to_owned();
    let unreadable = locale.get("import.unreadable").to_owned();
    let invalid = locale.get("import.invalid_json").to_owned();

    let submit = move || {
        let text = document.get().trim().to_owned();
        if serde_json::from_str::<Value>(&text).is_err() {
            set_notice.set(Some(Notice {
                ok: false,
                text: invalid.clone(),
            }));
            return;
        }
        submit_import(
            CLI_PROXY_PATH,
            text,
            set_busy,
            set_notice,
            Wording {
                stored: stored.clone(),
                unreadable: unreadable.clone(),
            },
            move || {
                set_document.set(String::new());
                refresh.run(());
            },
        );
    };

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4">
            <SectionHead
                title=locale.get("import.kiro_cli_title").to_owned()
                hint=locale.get("import.kiro_cli_hint").to_owned()
                path=CLI_PROXY_PATH
            />
            <SecretLine
                label=locale.get("import.field_document").to_owned()
                value=document
                set=set_document
            />
            <SubmitButton
                label=locale.get("import.submit").to_owned()
                busy=busy
                on_run=Callback::new(move |()| submit())
            />
            <NoticeLine notice=notice />
        </section>
    }
}

/// Several codex accounts in one paste.
///
/// This route reports per item and answers `200` even when every item failed, so the summary below is
/// the result -- not the status code. The field is cleared only when nothing failed: a partial import
/// leaves the list in place, because the entries that need fixing are in it.
#[component]
fn BulkImportForm(refresh: Callback<()>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (document, set_document) = signal(String::new());
    let (busy, set_busy) = signal(Save::Idle);
    let (notice, set_notice) = signal(Option::<Notice>::None);
    let (report, set_report) = signal(Option::<BulkReply>::None);

    let unreadable = locale.get("import.unreadable").to_owned();
    let invalid = locale.get("import.invalid_json").to_owned();

    let submit = move || {
        let text = document.get().trim().to_owned();
        if serde_json::from_str::<Value>(&text).is_err() {
            set_notice.set(Some(Notice {
                ok: false,
                text: invalid.clone(),
            }));
            return;
        }
        let undecodable = unreadable.clone();
        set_busy.set(Save::Saving);
        set_notice.set(None);
        set_report.set(None);
        leptos::task::spawn_local(async move {
            match write_reporting(Method::Post, BULK_PATH, Some(&text)).await {
                Ok(body) => match serde_json::from_str::<BulkReply>(&body) {
                    Ok(reply) => {
                        let (recorded, failed) = (reply.success, reply.failed);
                        set_busy.set(Save::Saved);
                        set_report.set(Some(reply));
                        if recorded > 0 {
                            refresh.run(());
                        }
                        // Cleared only when nothing failed: a partial import leaves the list, because
                        // the entries that need fixing are in it.
                        if failed == 0 && recorded > 0 {
                            set_document.set(String::new());
                        }
                    }
                    Err(_undecodable) => {
                        set_busy.set(Save::Idle);
                        set_notice.set(Some(Notice {
                            ok: false,
                            text: undecodable,
                        }));
                    }
                },
                Err(text) => {
                    set_busy.set(Save::Idle);
                    set_notice.set(Some(Notice { ok: false, text }));
                }
            }
        });
    };

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4">
            <SectionHead
                title=locale.get("import.codex_bulk_title").to_owned()
                hint=locale.get("import.codex_bulk_hint").to_owned()
                path=BULK_PATH
            />
            <SecretLine
                label=locale.get("import.field_document").to_owned()
                value=document
                set=set_document
            />
            <SubmitButton
                label=locale.get("import.submit").to_owned()
                busy=busy
                on_run=Callback::new(move |()| submit())
            />
            <NoticeLine notice=notice />
            {move || report.get().map(|reply| view! { <BulkReport reply=reply /> })}
        </section>
    }
}

/// The per-item outcome of a bulk import.
#[component]
fn BulkReport(reply: BulkReply) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let recorded = reply.success.to_string();
    let failed = reply.failed.to_string();
    let summary = locale.fmt(
        "import.bulk_summary",
        &[("imported", &recorded), ("failed", &failed)],
    );
    let problems: Vec<BulkResult> = reply
        .results
        .into_iter()
        .filter(|result| !result.ok)
        .collect();

    view! {
        <div class="space-y-2">
            <p class=if reply.failed == 0 {
                "text-sm text-foreground"
            } else {
                "text-sm text-warning"
            }>{summary}</p>
            {(!problems.is_empty())
                .then(|| {
                    view! {
                        <ul class="space-y-1">
                            {problems
                                .into_iter()
                                .map(|problem| {
                                    let index = problem.index.to_string();
                                    view! {
                                        <li class="text-xs text-destructive">
                                            {format!("#{index}: {}", problem.error)}
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                    }
                })}
        </div>
    }
}

/// Read a Cursor credential off this host's own disk.
///
/// The response *contains* the access token. It is held in a signal and posted from there; it never
/// reaches an input, an attribute, or a text node, so what is on screen is that a credential was
/// found -- not the credential. That is also why there is no "reveal" control here.
#[component]
fn CursorAutoPanel(refresh: Callback<()>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (busy, set_busy) = signal(Save::Idle);
    let (notice, set_notice) = signal(Option::<Notice>::None);
    let found = RwSignal::new(Option::<CursorFound>::None);

    let stored = locale.get("import.imported").to_owned();
    let unreadable = locale.get("import.unreadable").to_owned();
    let none_found = locale.get("import.auto_none").to_owned();

    let check = {
        let unreadable = unreadable.clone();
        move || {
            let (none_found, unreadable) = (none_found.clone(), unreadable.clone());
            set_busy.set(Save::Saving);
            set_notice.set(None);
            found.set(None);
            leptos::task::spawn_local(async move {
                match write_reporting(Method::Get, CURSOR_AUTO_PATH, None).await {
                    Ok(body) => match serde_json::from_str::<CursorFound>(&body) {
                        Ok(reply) if reply.found => {
                            set_busy.set(Save::Saved);
                            found.set(Some(reply));
                        }
                        // A `200` with `found: false` is an answer, not a failure, and the route's
                        // own text names the locations it checked -- which is the useful part.
                        Ok(reply) => {
                            set_busy.set(Save::Idle);
                            let mut text = if reply.error.is_empty() {
                                none_found
                            } else {
                                reply.error
                            };
                            // Named when the database was there but unreadable here, because the
                            // manual form above is then the way in.
                            if reply.manual && !reply.db_path.is_empty() {
                                text.push('\n');
                                text.push_str(&reply.db_path);
                            }
                            set_notice.set(Some(Notice { ok: false, text }));
                        }
                        Err(_undecodable) => {
                            set_busy.set(Save::Idle);
                            set_notice.set(Some(Notice {
                                ok: false,
                                text: unreadable,
                            }));
                        }
                    },
                    Err(text) => {
                        set_busy.set(Save::Idle);
                        set_notice.set(Some(Notice { ok: false, text }));
                    }
                }
            });
        }
    };

    let import = move || {
        let Some(credential) = found.get() else {
            return;
        };
        let Ok(payload) = serde_json::to_string(&serde_json::json!({
            "accessToken": credential.access_token,
            "machineId": credential.machine_id,
        })) else {
            return;
        };
        submit_import(
            CURSOR_IMPORT_PATH,
            payload,
            set_busy,
            set_notice,
            Wording {
                stored: stored.clone(),
                unreadable: unreadable.clone(),
            },
            // Dropped as soon as the router has it. Holding a live token in a signal for the rest of
            // the session buys nothing once the connection exists.
            move || {
                found.set(None);
                refresh.run(());
            },
        );
    };

    // Owned before the view: both are needed inside reactive closures, and a `move` closure cannot
    // borrow the locale that the eager parts of this view also read.
    // Built before the view, not inside it: `Callback::new(move |()| import())` inside a reactive
    // closure moves `import` into it, which makes that closure `FnOnce` where the view needs
    // `FnMut`. A `Callback` is `Copy`, so hoisting it lets the closure run on every re-render.
    let run_import = Callback::new(move |()| import());
    let label_use = locale.get("import.auto_use").to_owned();
    let text_found = locale.get("import.auto_found").to_owned();

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4">
            <SectionHead
                title=locale.get("import.auto_cursor_title").to_owned()
                hint=locale.get("import.auto_cursor_hint").to_owned()
                path=CURSOR_AUTO_PATH
            />
            <div class="flex flex-wrap gap-2">
                <SubmitButton
                    label=locale.get("import.auto_check").to_owned()
                    busy=busy
                    on_run=Callback::new(move |()| check())
                />
                {move || {
                    found
                        .get()
                        .map(|_credential| {
                            view! {
                                <SubmitButton
                                    label=label_use.clone()
                                    busy=busy
                                    on_run=run_import
                                />
                            }
                        })
                }}
            </div>
            {move || {
                found
                    .get()
                    .map(|_credential| {
                        view! {
                            // Deliberately says only that one was found: the token itself is held in
                            // a signal and posted from there.
                            <p class="text-sm text-foreground">{text_found.clone()}</p>
                        }
                    })
            }}
            <NoticeLine notice=notice />
        </section>
    }
}

/// Read a Kiro credential out of this host's own AWS SSO cache.
///
/// Same handling as the Cursor panel: the refresh token and client secret are held in a signal and
/// posted from there, never rendered. What is shown is which login on this host it came from, which
/// is the part that needs checking before spending the token -- and spending it is what an import
/// here does, because Kiro's only way to verify a refresh token is to use it.
#[component]
fn KiroAutoPanel(refresh: Callback<()>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (busy, set_busy) = signal(Save::Idle);
    let (notice, set_notice) = signal(Option::<Notice>::None);
    let found = RwSignal::new(Option::<KiroFound>::None);

    let stored = locale.get("import.imported").to_owned();
    let unreadable = locale.get("import.unreadable").to_owned();
    let none_found = locale.get("import.auto_none").to_owned();

    let check = {
        let unreadable = unreadable.clone();
        move || {
            let (none_found, unreadable) = (none_found.clone(), unreadable.clone());
            set_busy.set(Save::Saving);
            set_notice.set(None);
            found.set(None);
            leptos::task::spawn_local(async move {
                match write_reporting(Method::Get, KIRO_AUTO_PATH, None).await {
                    Ok(body) => match serde_json::from_str::<KiroFound>(&body) {
                        Ok(reply) if reply.found => {
                            set_busy.set(Save::Saved);
                            found.set(Some(reply));
                        }
                        Ok(reply) => {
                            set_busy.set(Save::Idle);
                            set_notice.set(Some(Notice {
                                ok: false,
                                text: if reply.error.is_empty() {
                                    none_found
                                } else {
                                    reply.error
                                },
                            }));
                        }
                        Err(_undecodable) => {
                            set_busy.set(Save::Idle);
                            set_notice.set(Some(Notice {
                                ok: false,
                                text: unreadable,
                            }));
                        }
                    },
                    Err(text) => {
                        set_busy.set(Save::Idle);
                        set_notice.set(Some(Notice { ok: false, text }));
                    }
                }
            });
        }
    };

    let import = move || {
        let Some(credential) = found.get() else {
            return;
        };
        let mut payload = serde_json::Map::new();
        payload.insert(
            "refreshToken".to_owned(),
            Value::String(credential.refresh_token),
        );
        // Forwarded exactly as found, nulls omitted. The route decides which refresh protocol to run
        // from whether the client pair is present, so filling in a blank here -- or sending an empty
        // string for one of them -- would pick a protocol on the user's behalf and burn the token.
        // `authMethod` is deliberately not sent for the same reason: it is a label, not a decision.
        for (key, value) in [
            ("clientId", credential.client_id),
            ("clientSecret", credential.client_secret),
            ("region", credential.region),
            ("profileArn", credential.profile_arn),
        ] {
            if let Some(value) = value.filter(|found| !found.trim().is_empty()) {
                payload.insert(key.to_owned(), Value::String(value));
            }
        }
        let Ok(body) = serde_json::to_string(&Value::Object(payload)) else {
            return;
        };
        submit_import(
            KIRO_IMPORT_PATH,
            body,
            set_busy,
            set_notice,
            Wording {
                stored: stored.clone(),
                unreadable: unreadable.clone(),
            },
            move || {
                found.set(None);
                refresh.run(());
            },
        );
    };

    // Built before the view, not inside it: `Callback::new(move |()| import())` inside a reactive
    // closure moves `import` into it, which makes that closure `FnOnce` where the view needs
    // `FnMut`. A `Callback` is `Copy`, so hoisting it lets the closure run on every re-render.
    let run_import = Callback::new(move |()| import());
    let label_use = locale.get("import.auto_use").to_owned();

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4">
            <SectionHead
                title=locale.get("import.auto_kiro_title").to_owned()
                hint=locale.get("import.auto_kiro_hint").to_owned()
                path=KIRO_AUTO_PATH
            />
            <div class="flex flex-wrap gap-2">
                <SubmitButton
                    label=locale.get("import.auto_check").to_owned()
                    busy=busy
                    on_run=Callback::new(move |()| check())
                />
                {move || {
                    found
                        .get()
                        .map(|_credential| {
                            view! {
                                <SubmitButton
                                    label=label_use.clone()
                                    busy=busy
                                    on_run=run_import
                                />
                            }
                        })
                }}
            </div>
            {move || {
                found
                    .get()
                    .map(|credential| {
                        // Only the non-secret half is passed on, so there is no path by which the
                        // token or the client secret could reach this markup.
                        view! {
                            <KiroDetails
                                source=credential.source
                                region=credential.region.unwrap_or_default()
                                method=credential.auth_method.unwrap_or_default()
                                profile=credential.profile_arn.unwrap_or_default()
                                idc=credential.client_id.is_some()
                            />
                        }
                    })
            }}
            <NoticeLine notice=notice />
        </section>
    }
}

/// The non-secret half of a discovered Kiro credential.
///
/// Takes the individual values rather than the whole response, so the credential cannot reach this
/// markup even by accident.
#[component]
fn KiroDetails(
    source: String,
    region: String,
    method: String,
    profile: String,
    /// Whether a `clientId`/`clientSecret` pair was found, which is what decides the refresh
    /// protocol. Shown because it is the difference between an organisation's login and a social one.
    idc: bool,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let protocol = if idc {
        locale.get("import.auto_idc").to_owned()
    } else {
        locale.get("import.auto_social").to_owned()
    };
    let rows = [
        (locale.get("import.auto_source").to_owned(), source),
        (locale.get("import.auto_region").to_owned(), region),
        (locale.get("import.auto_method").to_owned(), method),
        (locale.get("import.auto_profile").to_owned(), profile),
        (locale.get("import.auto_protocol").to_owned(), protocol),
    ];

    view! {
        <dl class="grid gap-3 sm:grid-cols-2 rounded-md border border-border p-3">
            {rows
                .into_iter()
                .filter(|(_label, value)| !value.is_empty())
                .map(|(label, value)| {
                    view! {
                        <div class="min-w-0">
                            <dt class="text-xs text-muted-foreground">{label}</dt>
                            <dd class="font-mono text-xs break-all">{value}</dd>
                        </div>
                    }
                })
                .collect_view()}
        </dl>
    }
}

#[component]
fn SectionHead(title: String, hint: String, path: &'static str) -> impl IntoView {
    view! {
        <div class="space-y-1">
            <h2 class="text-sm font-semibold tracking-tight">{title}</h2>
            <p class="text-sm text-muted-foreground">{hint}</p>
            // The route being called, shown because this page's whole subject is which endpoint a
            // credential is being handed to.
            <code class="text-xs text-muted-foreground">{path}</code>
        </div>
    }
}

#[component]
fn SubmitButton(label: String, busy: ReadSignal<Save>, on_run: Callback<()>) -> impl IntoView {
    view! {
        <button
            type="button"
            class="rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground \
                   disabled:opacity-50"
            disabled=move || busy.get().is_saving()
            on:click=move |_| on_run.run(())
        >
            {label}
        </button>
    }
}

/// The last thing an action reported, in the server's own words.
#[component]
fn NoticeLine(notice: ReadSignal<Option<Notice>>) -> impl IntoView {
    view! {
        {move || {
            notice
                .get()
                .map(|shown| {
                    let class = if shown.ok {
                        "text-sm text-foreground whitespace-pre-line"
                    } else {
                        "text-sm text-destructive whitespace-pre-line"
                    };
                    let role = if shown.ok { "status" } else { "alert" };
                    view! {
                        <p class=class role=role>
                            {shown.text}
                        </p>
                    }
                })
        }}
    }
}

/// One field in an import form.
///
/// `type` is decided by [`FieldSpec::secret`] and by nothing else -- not by a toggle, and not by
/// whether the value happens to look like a secret.
#[component]
fn CredentialInput(
    field: FieldSpec,
    values: RwSignal<BTreeMap<&'static str, String>>,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let label = if field.required {
        format!("{} *", locale.get(field.label))
    } else {
        locale.get(field.label).to_owned()
    };
    let key = field.key;

    view! {
        <label class="block space-y-1 text-sm">
            <span class="text-muted-foreground">{label}</span>
            <input
                type=if field.secret { "password" } else { "text" }
                // Off rather than a password-manager hint: these are other services' credentials
                // being handed over once, not this site's password to be remembered.
                autocomplete="off"
                spellcheck="false"
                class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                prop:value=move || values.with(|map| map.get(key).cloned().unwrap_or_default())
                on:input=move |ev| {
                    values
                        .update(|map| {
                            map.insert(key, event_target_value(&ev));
                        });
                }
            />
        </label>
    }
}

/// A masked single-line field for a pasted document.
///
/// A `textarea` would be the natural control for multi-line JSON and there is no way to mask one, so
/// this stays an `input`: the documents these take carry refresh tokens, and masking them matters
/// more than being able to read them back. JSON survives the round trip because a browser strips the
/// line breaks and JSON is whitespace-insensitive between tokens.
#[component]
fn SecretLine(label: String, value: ReadSignal<String>, set: WriteSignal<String>) -> impl IntoView {
    view! {
        <label class="block space-y-1 text-sm">
            <span class="text-muted-foreground">{label}</span>
            <input
                type="password"
                autocomplete="off"
                spellcheck="false"
                class="w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-sm"
                prop:value=move || value.get()
                on:input=move |ev| set.set(event_target_value(&ev))
            />
        </label>
    }
}

/// What is already stored, so a successful import can be seen to have landed.
///
/// Refetched by every form that succeeds. Without that this list would be a snapshot from page load
/// that stays unchanged after an import -- which reads as "the import did nothing", the one wrong
/// answer a page like this must not give.
#[component]
fn StoredList(rows: Vec<ProviderRow>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if rows.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">{locale.get("import.none_stored").to_owned()}</p>
        }
        .into_any();
    }
    view! {
        <ul class="flex flex-wrap gap-1.5">
            {rows
                .into_iter()
                .map(|row| {
                    let label = if row.name.is_empty() { row.provider.clone() } else { row.name };
                    view! {
                        <li class="rounded border border-border px-2 py-1 text-xs">
                            <span>{label}</span>
                            <code class="ml-1.5 text-muted-foreground">{row.provider}</code>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
    .into_any()
}

#[component]
pub fn OauthImport() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (stored, set_stored) = signal(Hydrate::<ProvidersList>::Loading);
    let reload = move || {
        set_stored.set(Hydrate::Loading);
        load("/api/providers", set_stored);
    };
    reload();
    let refresh = Callback::new(move |()| reload());

    view! {
        <PageHeader
            title=locale.get("nav.import").to_owned()
            description=locale.get("import.description").to_owned()
        />
        <section class="rounded-lg border border-border bg-card p-5 space-y-3">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("import.stored").to_owned()}
            </h2>
            <Panel
                state=stored
                on_retry=Callback::new(move |()| reload())
                children=move |data: ProvidersList| {
                    view! { <StoredList rows=data.connections /> }
                }
            />
        </section>
        <p class="mt-4 text-sm text-muted-foreground">
            {locale.get("import.security_note").to_owned()}
        </p>
        <div class="mt-4 space-y-4">
            {FORMS
                .into_iter()
                .map(|spec| view! { <ImportForm spec=spec refresh=refresh /> })
                .collect_view()}
            <CliProxyForm refresh=refresh />
            <BulkImportForm refresh=refresh />
            <CursorAutoPanel refresh=refresh />
            <KiroAutoPanel refresh=refresh />
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::{BulkReply, Connection, CursorFound, FORMS, ImportReply, KiroFound, describe};

    /// Every field key on this page whose value is a credential.
    ///
    /// The masking test below reads both directions off this list, so a new field is either named
    /// here and masked, or not named here and not masked. There is no third outcome that compiles
    /// and passes.
    const CREDENTIAL_KEYS: [&str; 7] = [
        "token",
        "apiKey",
        "refreshToken",
        "clientSecret",
        "accessToken",
        "machineId",
        "cookie",
    ];

    #[test]
    fn every_credential_field_is_masked() {
        for spec in FORMS {
            for field in spec.fields {
                if CREDENTIAL_KEYS.contains(&field.key) {
                    assert!(
                        field.secret,
                        "{} at {} carries a credential and must be masked",
                        field.key, spec.path
                    );
                }
            }
        }
    }

    #[test]
    fn nothing_that_is_not_a_credential_is_masked() {
        // The other direction matters too: masking a region or a base URL hides a value the user has
        // to check, which buys the look of security and none of it.
        for spec in FORMS {
            for field in spec.fields {
                if field.secret {
                    assert!(
                        CREDENTIAL_KEYS.contains(&field.key),
                        "{} at {} is masked but is not a credential",
                        field.key,
                        spec.path
                    );
                }
            }
        }
    }

    #[test]
    fn the_required_fields_are_exactly_the_ones_the_routes_refuse_without() {
        // Each expectation was read back from the running router: submitting `{}` answers `400`
        // naming precisely these fields.
        let expected: [(&str, &[&str]); 6] = [
            ("/api/oauth/gitlab/pat", &["token"]),
            ("/api/oauth/kiro/api-key", &["apiKey"]),
            ("/api/oauth/kiro/import", &["refreshToken"]),
            ("/api/oauth/codex/import-token", &["accessToken"]),
            ("/api/oauth/cursor/import", &["accessToken", "machineId"]),
            ("/api/oauth/iflow/cookie", &["cookie"]),
        ];
        for (path, required) in expected {
            let matching = FORMS.into_iter().filter(|spec| spec.path == path).count();
            assert_eq!(matching, 1, "{path} should be listed exactly once");
            for spec in FORMS.into_iter().filter(|spec| spec.path == path) {
                let actual: Vec<&str> = spec
                    .fields
                    .iter()
                    .filter(|field| field.required)
                    .map(|field| field.key)
                    .collect();
                assert_eq!(actual, required, "{path}");
            }
        }
    }

    #[test]
    fn every_form_has_a_distinct_route_and_a_namespaced_message_key() {
        for (index, spec) in FORMS.iter().enumerate() {
            assert!(spec.path.starts_with("/api/oauth/"), "{:?}", spec.path);
            assert!(spec.title.starts_with("import."), "{:?}", spec.title);
            assert!(spec.hint.starts_with("import."), "{:?}", spec.hint);
            assert!(!spec.fields.is_empty(), "{:?} has no fields", spec.path);
            let duplicate = FORMS
                .iter()
                .skip(index + 1)
                .find(|other| other.path == spec.path);
            assert!(duplicate.is_none(), "{:?} is listed twice", spec.path);
        }
    }

    #[test]
    fn a_refusal_carries_the_routes_own_sentence() {
        for (body, expected) in [
            (
                r#"{"error":"Personal Access Token is required","success":false}"#,
                "Personal Access Token is required",
            ),
            (
                r#"{"error":"Invalid machine ID format. Expected UUID format.","success":false}"#,
                "Invalid machine ID format. Expected UUID format.",
            ),
            (
                r#"{"error":"Invalid region","success":false}"#,
                "Invalid region",
            ),
        ] {
            let reply: ImportReply = serde_json::from_str(body).expect("decodes");
            assert!(!reply.success);
            assert_eq!(reply.error, expected);
        }
    }

    #[test]
    fn a_bare_success_decodes_without_a_connection_block() {
        // `gitlab/pat` and `kiro/api-key` answer exactly this.
        let reply: ImportReply = serde_json::from_str(r#"{"success":true}"#).expect("decodes");
        assert!(reply.success);
        assert!(reply.connection.is_none());
        assert_eq!(describe(&reply, "Imported"), "Imported");
    }

    #[test]
    fn a_success_naming_the_account_says_which_one_was_stored() {
        let body = r#"{"success":true,"connection":{"provider":"kiro","email":"a@example.com"}}"#;
        let reply: ImportReply = serde_json::from_str(body).expect("decodes");
        let line = describe(&reply, "Imported");
        assert!(line.contains("kiro"), "{line}");
        assert!(line.contains("a@example.com"), "{line}");
    }

    #[test]
    fn a_connection_with_a_null_email_does_not_render_an_empty_pair_of_brackets() {
        let reply = ImportReply {
            success: true,
            error: String::new(),
            connection: Some(Connection {
                provider: "cursor".to_owned(),
                email: None,
            }),
        };
        let line = describe(&reply, "Imported");
        assert_eq!(line, "Imported \u{2014} cursor");
    }

    #[test]
    fn the_bulk_route_reports_counts_not_a_flag() {
        // `success` is a number here. Decoding it as a bool would fail outright, and reading `0` as
        // `false` would report a wholly failed import as a wholly successful one.
        let body = r#"{"failed":1,"results":[{"error":"Missing accessToken","index":0,"ok":false},
            {"index":1,"ok":true}],"success":1}"#;
        let reply: BulkReply = serde_json::from_str(body).expect("decodes");
        assert_eq!(reply.success, 1);
        assert_eq!(reply.failed, 1);
        let failures: Vec<&super::BulkResult> =
            reply.results.iter().filter(|item| !item.ok).collect();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures.first().map(|item| item.index), Some(0));
    }

    #[test]
    fn a_cursor_auto_import_that_found_nothing_is_an_answer_rather_than_a_failure() {
        // The route answers `200` for this, and its text names the locations it checked.
        let body = r#"{"error":"Cursor database not found. Checked locations:\n/root/x",
            "found":false}"#;
        let reply: CursorFound = serde_json::from_str(body).expect("decodes");
        assert!(!reply.found);
        assert!(reply.access_token.is_empty());
        assert!(reply.error.contains("Checked locations"));
    }

    #[test]
    fn a_cursor_auto_import_that_found_a_credential_carries_it_for_submission_only() {
        let body = r#"{"found":true,"accessToken":"live-token","machineId":"abc"}"#;
        let reply: CursorFound = serde_json::from_str(body).expect("decodes");
        assert!(reply.found);
        assert_eq!(reply.access_token, "live-token");
        // Nothing in this module formats `access_token` into a view; the panel renders only that a
        // credential was found. This assertion is the reminder attached to that decision.
        assert!(!reply.machine_id.is_empty());
    }

    #[test]
    fn a_kiro_auto_import_keeps_a_null_client_pair_distinct_from_an_empty_one() {
        // Present-and-null, which is what tells the import it is a social login rather than an IDC
        // one. An empty string here would pick the wrong refresh protocol and spend the token.
        let body = r#"{"found":true,"refreshToken":"live","clientId":null,"clientSecret":null,
            "region":null,"authMethod":"social","profileArn":null,"source":"/root/.aws/sso/x.json"}"#;
        let reply: KiroFound = serde_json::from_str(body).expect("decodes");
        assert!(reply.found);
        assert!(reply.client_id.is_none());
        assert!(reply.client_secret.is_none());
        assert_eq!(reply.auth_method.as_deref(), Some("social"));
        assert_eq!(reply.source, "/root/.aws/sso/x.json");
    }

    #[test]
    fn a_kiro_auto_import_with_a_client_pair_reports_the_idc_protocol() {
        let body = r#"{"found":true,"refreshToken":"live","clientId":"id","clientSecret":"secret",
            "region":"us-east-1","authMethod":"idc","profileArn":"arn:aws:codewhisperer:us-east-1:1:x"}"#;
        let reply: KiroFound = serde_json::from_str(body).expect("decodes");
        assert!(reply.client_id.is_some());
        assert!(reply.client_secret.is_some());
        assert_eq!(reply.region.as_deref(), Some("us-east-1"));
    }
}
