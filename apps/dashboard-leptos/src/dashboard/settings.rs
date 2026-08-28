//! Pure state for the Settings panel: parse, patch, and resolve one write.
//!
//! The panel this backs used to render three toggles over local `signal()`s, so
//! a click changed the pixels and nothing else. Everything here exists to make
//! that impossible: the panel can only show a value it parsed out of
//! `GET /api/settings`, and an optimistic flip is only allowed because
//! [`resolve`] can put the previous value back when the `PUT` is refused.
//!
//! Kept free of `leptos` and of `fetch` so it is unit-testable on the native
//! target.

use crate::api::ApiError;
use serde::Deserialize;
use std::collections::BTreeMap;

/// The endpoint that owns every field on this panel.
pub const SETTINGS_PATH: &str = "/api/settings";

/// The public `/api/settings` projection.
///
/// Mirrors `SettingsView` in `services/state-actix/src/store.rs`.
///
/// The booleans carry no `serde` default on purpose. They are access-control
/// state, and a missing `tunnelDashboardAccess` defaulted to `false` would
/// render as a claim the server never made. An absent boolean is a shape change,
/// so it fails the parse and surfaces as an error. The string fields do default:
/// the service serialises an unset tunnel, proxy, or SSO field as `""`, so
/// absent and empty mean the same thing.
///
/// There is no `requireLogin` here. Dashboard login is unconditional in
/// nullrouter, so it is not a setting and has no row — see
/// [`LOGIN_ALWAYS_REQUIRED`].
///
/// The two secrets are represented by `*_set` booleans, never by their values:
/// `SettingsView` does not project `oidcClientSecret` or `samlCert`, so the
/// panel can report "configured" without ever holding the secret.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshot {
    pub tunnel_dashboard_access: bool,
    #[serde(default)]
    pub tunnel_url: String,
    #[serde(default)]
    pub tailscale_url: String,
    pub outbound_proxy_enabled: bool,
    #[serde(default)]
    pub outbound_proxy_url: String,
    #[serde(default)]
    pub outbound_no_proxy: String,
    #[serde(default)]
    pub oidc_issuer_url: String,
    #[serde(default)]
    pub oidc_client_id: String,
    /// Whether an OIDC client secret is stored. Not the secret itself.
    pub oidc_client_secret_set: bool,
    #[serde(default)]
    pub oidc_scopes: String,
    #[serde(default)]
    pub oidc_login_label: String,
    #[serde(default)]
    pub saml_entry_point: String,
    #[serde(default)]
    pub saml_issuer: String,
    /// Whether an IdP certificate is stored. Not the certificate itself.
    pub saml_cert_set: bool,
    #[serde(default)]
    pub saml_attribute_email: String,
    #[serde(default)]
    pub saml_attribute_name: String,
}

/// One writable field of [`SettingsSnapshot`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SettingsField {
    TunnelDashboardAccess,
    TunnelUrl,
    TailscaleUrl,
    OutboundProxyEnabled,
    OutboundProxyUrl,
    OutboundNoProxy,
    OidcIssuerUrl,
    OidcClientId,
    OidcClientSecret,
    OidcScopes,
    OidcLoginLabel,
    SamlEntryPoint,
    SamlIssuer,
    SamlCert,
    SamlAttributeEmail,
    SamlAttributeName,
}

/// Which control a field is edited with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsControl {
    Toggle,
    Text,
    /// Write-only: the value is sent but never read back. Rendered as a masked
    /// input plus a configured/not-configured indicator driven by the field's
    /// [`SettingsField::readback_key`].
    Secret,
}

/// Which card of the panel a field belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsGroup {
    Access,
    Oidc,
    Saml,
}

impl SettingsGroup {
    pub const fn title(self) -> &'static str {
        match self {
            Self::Access => "Security Settings",
            Self::Oidc => "OIDC single sign-on",
            Self::Saml => "SAML single sign-on",
        }
    }

    pub const fn blurb(self) -> &'static str {
        match self {
            Self::Access => {
                "Read from the local router over GET /api/settings. Each row saves on its own with PUT /api/settings and shows what the router stored."
            }
            Self::Oidc => {
                "Configure an OIDC provider for dashboard sign-in. Validate the issuer with POST /api/auth/oidc/test before relying on it."
            }
            Self::Saml => {
                "Configure a SAML identity provider. Service-provider metadata is served from GET /api/auth/saml/metadata."
            }
        }
    }

    /// The fields in this group, in display order.
    pub fn fields(self) -> Vec<SettingsField> {
        SETTINGS_FIELDS
            .into_iter()
            .filter(|field| field.group() == self)
            .collect()
    }
}

/// Every group the panel renders, in display order.
pub const SETTINGS_GROUPS: [SettingsGroup; 3] = [
    SettingsGroup::Access,
    SettingsGroup::Oidc,
    SettingsGroup::Saml,
];

/// A field's value, in the shape its control produces.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettingsValue {
    Flag(bool),
    Text(String),
}

impl SettingsField {
    /// The JSON key this field is **written** with on `PUT /api/settings`.
    ///
    /// For a [`SettingsControl::Secret`] this is not a key the `GET` response
    /// carries; read [`Self::readback_key`] for that.
    pub const fn json_key(self) -> &'static str {
        match self {
            Self::TunnelDashboardAccess => "tunnelDashboardAccess",
            Self::TunnelUrl => "tunnelUrl",
            Self::TailscaleUrl => "tailscaleUrl",
            Self::OutboundProxyEnabled => "outboundProxyEnabled",
            Self::OutboundProxyUrl => "outboundProxyUrl",
            Self::OutboundNoProxy => "outboundNoProxy",
            Self::OidcIssuerUrl => "oidcIssuerUrl",
            Self::OidcClientId => "oidcClientId",
            Self::OidcClientSecret => "oidcClientSecret",
            Self::OidcScopes => "oidcScopes",
            Self::OidcLoginLabel => "oidcLoginLabel",
            Self::SamlEntryPoint => "samlEntryPoint",
            Self::SamlIssuer => "samlIssuer",
            Self::SamlCert => "samlCert",
            Self::SamlAttributeEmail => "samlAttributeEmail",
            Self::SamlAttributeName => "samlAttributeName",
        }
    }

    /// The `GET /api/settings` key that reports this field's state.
    ///
    /// Same as [`Self::json_key`] for everything the server echoes back. A
    /// secret is never echoed, so its readback key is the `…Set` boolean that
    /// says whether one is stored — the only thing the panel is allowed to know
    /// about it.
    pub const fn readback_key(self) -> &'static str {
        match self {
            Self::OidcClientSecret => "oidcClientSecretSet",
            Self::SamlCert => "samlCertSet",
            _ => self.json_key(),
        }
    }

    pub const fn control(self) -> SettingsControl {
        match self {
            Self::TunnelDashboardAccess | Self::OutboundProxyEnabled => SettingsControl::Toggle,
            Self::OidcClientSecret | Self::SamlCert => SettingsControl::Secret,
            Self::TunnelUrl
            | Self::TailscaleUrl
            | Self::OutboundProxyUrl
            | Self::OutboundNoProxy
            | Self::OidcIssuerUrl
            | Self::OidcClientId
            | Self::OidcScopes
            | Self::OidcLoginLabel
            | Self::SamlEntryPoint
            | Self::SamlIssuer
            | Self::SamlAttributeEmail
            | Self::SamlAttributeName => SettingsControl::Text,
        }
    }

    pub const fn group(self) -> SettingsGroup {
        match self {
            Self::TunnelDashboardAccess
            | Self::TunnelUrl
            | Self::TailscaleUrl
            | Self::OutboundProxyEnabled
            | Self::OutboundProxyUrl
            | Self::OutboundNoProxy => SettingsGroup::Access,
            Self::OidcIssuerUrl
            | Self::OidcClientId
            | Self::OidcClientSecret
            | Self::OidcScopes
            | Self::OidcLoginLabel => SettingsGroup::Oidc,
            Self::SamlEntryPoint
            | Self::SamlIssuer
            | Self::SamlCert
            | Self::SamlAttributeEmail
            | Self::SamlAttributeName => SettingsGroup::Saml,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::TunnelDashboardAccess => "Tunnel dashboard access",
            Self::TunnelUrl => "Tunnel URL",
            Self::TailscaleUrl => "Tailscale URL",
            Self::OutboundProxyEnabled => "Route outbound traffic through a proxy",
            Self::OutboundProxyUrl => "Outbound proxy URL",
            Self::OutboundNoProxy => "Proxy exceptions",
            Self::OidcIssuerUrl => "Issuer URL",
            Self::OidcClientId => "Client ID",
            Self::OidcClientSecret => "Client secret",
            Self::OidcScopes => "Scopes",
            Self::OidcLoginLabel => "Sign-in button label",
            Self::SamlEntryPoint => "IdP sign-on URL",
            Self::SamlIssuer => "Service provider entity ID",
            Self::SamlCert => "IdP signing certificate",
            Self::SamlAttributeEmail => "Email attribute",
            Self::SamlAttributeName => "Display-name attribute",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::TunnelDashboardAccess => {
                "Allow the dashboard to be reached through the configured tunnel."
            }
            Self::TunnelUrl => {
                "Public tunnel address published for this router. Leave empty when no tunnel is configured."
            }
            Self::TailscaleUrl => {
                "Tailnet address for this router. Leave empty when Tailscale is not in use."
            }
            Self::OutboundProxyEnabled => {
                "Send upstream provider requests through the configured proxy."
            }
            Self::OutboundProxyUrl => {
                "Proxy the router dials for upstream requests, for example http://127.0.0.1:8080."
            }
            Self::OutboundNoProxy => {
                "Comma-separated hosts that bypass the proxy, for example localhost,127.0.0.1."
            }
            Self::OidcIssuerUrl => {
                "Base URL of the provider. Discovery is read from its /.well-known/openid-configuration."
            }
            Self::OidcClientId => "Client identifier this router registers as with the provider.",
            Self::OidcClientSecret => {
                "Sent to the token endpoint and never read back. Saving another row does not clear it."
            }
            Self::OidcScopes => {
                "Space-separated scopes requested at sign-in. Defaults to openid profile email when empty."
            }
            Self::OidcLoginLabel => {
                "Text on the login page's SSO button. Defaults to Sign in with OIDC when empty."
            }
            Self::SamlEntryPoint => {
                "IdP endpoint that authentication requests are sent to, for example https://idp.example.com/sso."
            }
            Self::SamlIssuer => {
                "Entity ID this router presents in its metadata. Defaults to urn:9router:sp when empty."
            }
            Self::SamlCert => {
                "X.509 certificate used to verify assertions, and never read back. PEM or bare base64."
            }
            Self::SamlAttributeEmail => {
                "Assertion attribute holding the user's email. Common claims are tried when empty."
            }
            Self::SamlAttributeName => {
                "Assertion attribute holding the display name. Common claims are tried when empty."
            }
        }
    }

    /// A stable DOM id, used to tie a label and a status region to a control.
    pub fn dom_id(self) -> String {
        format!("nr-setting-{}", self.json_key())
    }

    /// The id of this field's save-status region.
    pub fn status_id(self) -> String {
        format!("nr-setting-status-{}", self.json_key())
    }
}

/// Every field the panel renders, in display order.
pub const SETTINGS_FIELDS: [SettingsField; 16] = [
    SettingsField::TunnelDashboardAccess,
    SettingsField::TunnelUrl,
    SettingsField::TailscaleUrl,
    SettingsField::OutboundProxyEnabled,
    SettingsField::OutboundProxyUrl,
    SettingsField::OutboundNoProxy,
    SettingsField::OidcIssuerUrl,
    SettingsField::OidcClientId,
    SettingsField::OidcClientSecret,
    SettingsField::OidcScopes,
    SettingsField::OidcLoginLabel,
    SettingsField::SamlEntryPoint,
    SettingsField::SamlIssuer,
    SettingsField::SamlCert,
    SettingsField::SamlAttributeEmail,
    SettingsField::SamlAttributeName,
];

impl SettingsValue {
    /// The boolean, when this is a toggle value.
    pub const fn flag(&self) -> Option<bool> {
        match self {
            Self::Flag(value) => Some(*value),
            Self::Text(_) => None,
        }
    }

    /// The text, when this is a text value.
    pub const fn text(&self) -> Option<&str> {
        match self {
            Self::Text(value) => Some(value.as_str()),
            Self::Flag(_) => None,
        }
    }
}

impl SettingsSnapshot {
    /// Read one field.
    ///
    /// A [`SettingsControl::Secret`] reads as empty text: the value is not in
    /// the snapshot and never will be. Use [`Self::is_set`] for whether one is
    /// stored.
    pub fn value(&self, field: SettingsField) -> SettingsValue {
        match field {
            SettingsField::TunnelDashboardAccess => {
                SettingsValue::Flag(self.tunnel_dashboard_access)
            }
            SettingsField::TunnelUrl => SettingsValue::Text(self.tunnel_url.clone()),
            SettingsField::TailscaleUrl => SettingsValue::Text(self.tailscale_url.clone()),
            SettingsField::OutboundProxyEnabled => SettingsValue::Flag(self.outbound_proxy_enabled),
            SettingsField::OutboundProxyUrl => SettingsValue::Text(self.outbound_proxy_url.clone()),
            SettingsField::OutboundNoProxy => SettingsValue::Text(self.outbound_no_proxy.clone()),
            SettingsField::OidcIssuerUrl => SettingsValue::Text(self.oidc_issuer_url.clone()),
            SettingsField::OidcClientId => SettingsValue::Text(self.oidc_client_id.clone()),
            SettingsField::OidcScopes => SettingsValue::Text(self.oidc_scopes.clone()),
            SettingsField::OidcLoginLabel => SettingsValue::Text(self.oidc_login_label.clone()),
            SettingsField::SamlEntryPoint => SettingsValue::Text(self.saml_entry_point.clone()),
            SettingsField::SamlIssuer => SettingsValue::Text(self.saml_issuer.clone()),
            SettingsField::SamlAttributeEmail => {
                SettingsValue::Text(self.saml_attribute_email.clone())
            }
            SettingsField::SamlAttributeName => {
                SettingsValue::Text(self.saml_attribute_name.clone())
            }
            // Write-only. The panel holds no secret to hand back.
            SettingsField::OidcClientSecret | SettingsField::SamlCert => {
                SettingsValue::Text(String::new())
            }
        }
    }

    /// Whether a secret field has a value stored on the router.
    ///
    /// `None` for a field that is not a secret, so a caller cannot mistake "not
    /// a secret" for "no secret stored".
    pub const fn is_set(&self, field: SettingsField) -> Option<bool> {
        match field {
            SettingsField::OidcClientSecret => Some(self.oidc_client_secret_set),
            SettingsField::SamlCert => Some(self.saml_cert_set),
            _ => None,
        }
    }

    /// Write one field, ignoring a value of the wrong kind.
    ///
    /// A kind mismatch cannot come from a control — each control only ever
    /// produces its own kind — so it is dropped rather than guessed at, which
    /// keeps this total without a `panic!`.
    ///
    /// A secret is also dropped: there is nowhere to put it. The `…Set` flag it
    /// affects is server state, so it is only ever adopted from a `PUT` reply,
    /// never predicted locally.
    pub fn set(&mut self, field: SettingsField, value: SettingsValue) {
        let SettingsValue::Text(text) = value else {
            match (field, value) {
                (SettingsField::TunnelDashboardAccess, SettingsValue::Flag(flag)) => {
                    self.tunnel_dashboard_access = flag;
                }
                (SettingsField::OutboundProxyEnabled, SettingsValue::Flag(flag)) => {
                    self.outbound_proxy_enabled = flag;
                }
                _ => {}
            }
            return;
        };
        match field {
            SettingsField::TunnelUrl => self.tunnel_url = text,
            SettingsField::TailscaleUrl => self.tailscale_url = text,
            SettingsField::OutboundProxyUrl => self.outbound_proxy_url = text,
            SettingsField::OutboundNoProxy => self.outbound_no_proxy = text,
            SettingsField::OidcIssuerUrl => self.oidc_issuer_url = text,
            SettingsField::OidcClientId => self.oidc_client_id = text,
            SettingsField::OidcScopes => self.oidc_scopes = text,
            SettingsField::OidcLoginLabel => self.oidc_login_label = text,
            SettingsField::SamlEntryPoint => self.saml_entry_point = text,
            SettingsField::SamlIssuer => self.saml_issuer = text,
            SettingsField::SamlAttributeEmail => self.saml_attribute_email = text,
            SettingsField::SamlAttributeName => self.saml_attribute_name = text,
            // A toggle handed text, or a secret handed anything: dropped.
            SettingsField::TunnelDashboardAccess
            | SettingsField::OutboundProxyEnabled
            | SettingsField::OidcClientSecret
            | SettingsField::SamlCert => {}
        }
    }

    /// The same snapshot with one field replaced.
    #[must_use]
    pub fn with(&self, field: SettingsField, value: SettingsValue) -> Self {
        let mut next = self.clone();
        next.set(field, value);
        next
    }
}

/// Parse a `GET`/`PUT /api/settings` body.
///
/// `None` on anything that is not a complete settings object, so the panel
/// reports a failure instead of rendering defaults as if the server sent them.
pub fn parse_settings(body: &str) -> Option<SettingsSnapshot> {
    serde_json::from_str::<SettingsSnapshot>(body).ok()
}

/// The `PUT` body that changes exactly one field.
///
/// `SettingsRequest` in the state service takes every field as `Option`, so a
/// single-key body leaves the rest untouched. That is also what makes saving any
/// row safe while a secret is stored: a body that never mentions
/// `oidcClientSecret` cannot clear it. Serialised through `serde_json` so a URL
/// or certificate containing a quote or a backslash cannot break out of the
/// payload.
pub fn patch_body(field: SettingsField, value: &SettingsValue) -> String {
    let json = match value {
        SettingsValue::Flag(flag) => serde_json::Value::Bool(*flag),
        SettingsValue::Text(text) => serde_json::Value::String(text.clone()),
    };
    let mut body = BTreeMap::new();
    body.insert(field.json_key(), json);
    serde_json::to_string(&body).unwrap_or_else(|_error| String::from("{}"))
}

/// How a `PUT /api/settings` ended.
///
/// The confirmed snapshot is boxed: [`SettingsSnapshot`] is 16 fields wide, so
/// carrying it inline would make every `WriteOutcome` — including the two empty
/// refusal arms — that large. One allocation per settled write is cheaper than
/// that, and this is a per-click path, not a hot one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteOutcome {
    /// 2xx with a body that parsed: the server told us its new state.
    Confirmed(Box<SettingsSnapshot>),
    /// 2xx with a body that did not parse.
    ///
    /// Distinct from [`Self::Rejected`] because the write probably landed, so
    /// rolling the row back would be its own lie.
    Unconfirmed,
    /// The request failed, or the server refused it.
    Rejected(ApiError),
}

/// What the panel should do with an in-flight write once it ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    /// The snapshot to display.
    pub snapshot: SettingsSnapshot,
    /// The error to show on the row, if any.
    pub error: Option<ApiError>,
    /// Whether the optimistic value survived.
    pub committed: bool,
}

/// Settle one optimistic write.
///
/// `optimistic` is the snapshot the panel is already showing (the flip has
/// happened), and `previous` is the value that field held before it. A rejection
/// restores only that field, so a concurrent write to another row is not undone
/// as a side effect.
pub fn resolve(
    optimistic: &SettingsSnapshot,
    field: SettingsField,
    previous: &SettingsValue,
    outcome: WriteOutcome,
) -> Resolution {
    match outcome {
        WriteOutcome::Confirmed(server) => Resolution {
            snapshot: *server,
            error: None,
            committed: true,
        },
        // The server accepted it but we cannot read back what it stored. Keep
        // the value on screen, and still report the error: the row is not
        // allowed to claim a clean "Saved".
        WriteOutcome::Unconfirmed => Resolution {
            snapshot: optimistic.clone(),
            error: Some(ApiError::Body),
            committed: true,
        },
        WriteOutcome::Rejected(error) => Resolution {
            snapshot: optimistic.with(field, previous.clone()),
            error: Some(error),
            committed: false,
        },
    }
}

/// Why `requireApiKey` has no control here.
///
/// The runtime enforces it, but it is deliberately absent from the public
/// `SettingsView` (its shape is pinned by a parity test), so the dashboard
/// cannot read it back. A toggle would be showing a state it invented; the row
/// is rendered as unavailable instead.
pub const REQUIRE_API_KEY_UNAVAILABLE: &str = "Enforced by the runtime. GET /api/settings does not report this value, so it cannot be shown or changed here.";

/// Why dashboard login has no control here.
///
/// There used to be a `requireLogin` toggle. It was removed rather than fixed:
/// `nullrouter-auth` never read the persisted value, so the toggle moved a knob
/// and changed nothing. Login is now unconditional — a deliberate departure from
/// 9Router, which lets an operator disable dashboard auth outright — so there is
/// no setting for a control to edit.
pub const LOGIN_ALWAYS_REQUIRED: &str = "Always on. Dashboard access requires a login in nullrouter and cannot be turned off, so there is nothing to configure here.";
