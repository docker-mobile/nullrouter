use super::super::super::{HeaderPanel, ShellSignals};
#[cfg(target_arch = "wasm32")]
use crate::account::logout_response_succeeded;
use crate::ui::navigation::AccountActionKind;
use crate::{
    account::AccountState,
    ui::{AccountAction, dashboard_account_actions, dashboard_icon_glyph},
};
use leptos::prelude::*;

#[derive(Clone, Copy)]
struct AccountSignals {
    account: ReadSignal<AccountState>,
    set_account: WriteSignal<AccountState>,
}

#[component]
pub(crate) fn HeaderAccount(shell: ShellSignals) -> impl IntoView {
    let (account, set_account) = signal(AccountState::checking());
    let signals = AccountSignals {
        account,
        set_account,
    };
    hydrate_account(set_account);

    view! {
        <>
            <Show when=move || shell.header_panel.get() == HeaderPanel::Account>
                <div
                    id="nr-header-account"
                    class="nr-header-popover nr-account-popover"
                    role="menu"
                    aria-label="Account menu"
                >
                    <div class="nr-popover-heading">
                        <strong>"Menu"</strong>
                        <span>{move || account.with(|state| state.display_name().to_owned())}</span>
                    </div>
                    <For
                        each=move || dashboard_account_actions().iter().copied()
                        key=|action| action.label
                        children=move |action| view! {
                            <AccountMenuItem action shell signals />
                        }
                    />
                </div>
            </Show>
            <span
                role="status"
                aria-live="polite"
                aria-atomic="true"
                style="position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0"
            >
                {move || account.with(AccountState::logout_announcement)}
            </span>
        </>
    }
}

#[component]
fn AccountMenuItem(
    action: AccountAction,
    shell: ShellSignals,
    signals: AccountSignals,
) -> impl IntoView {
    match action.kind {
        AccountActionKind::Unsupported => view! {
            <button
                type="button"
                class="nr-account-item"
                role="menuitem"
                disabled=true
                aria-disabled="true"
                title="Unavailable until the matching Rust host service is implemented"
            >
                <span class="material-symbols-outlined" aria-hidden="true">
                    {dashboard_icon_glyph(action.icon)}
                </span>
                <span>{action.label}</span>
                <small>"Unavailable"</small>
            </button>
        }
        .into_any(),
        AccountActionKind::Logout => view! {
            <button
                type="button"
                class="nr-account-item"
                role="menuitem"
                disabled=move || !signals.account.with(AccountState::can_logout)
                aria-disabled=move || (!signals.account.with(AccountState::can_logout)).to_string()
                aria-busy=move || signals.account.with(AccountState::is_logout_pending).to_string()
                title=move || signals.account.with(AccountState::logout_status)
                on:click=move |_| begin_logout(shell, signals)
            >
                <span class="material-symbols-outlined" aria-hidden="true">
                    {dashboard_icon_glyph(action.icon)}
                </span>
                <span>{action.label}</span>
                <small>
                    {move || signals.account.with(AccountState::logout_status)}
                </small>
            </button>
        }
        .into_any(),
    }
}

fn begin_logout(shell: ShellSignals, signals: AccountSignals) {
    let mut started = false;
    signals
        .set_account
        .update(|state| started = state.begin_logout());
    if started {
        shell.set_header_panel.set(HeaderPanel::Closed);
        submit_logout(shell, signals.set_account);
    }
}

#[cfg(target_arch = "wasm32")]
fn hydrate_account(set_account: WriteSignal<AccountState>) {
    wasm_bindgen_futures::spawn_local(async move {
        match request_json(AccountRequest::Status).await {
            Ok(body) => set_account.update(|state| {
                if state.apply_status_json(&body).is_err() {
                    state.status_failed();
                }
            }),
            Err(_error) => set_account.update(AccountState::status_failed),
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
const fn hydrate_account(_set_account: WriteSignal<AccountState>) {}

#[cfg(target_arch = "wasm32")]
fn submit_logout(shell: ShellSignals, set_account: WriteSignal<AccountState>) {
    wasm_bindgen_futures::spawn_local(async move {
        match request_json(AccountRequest::Logout).await {
            Ok(body) if logout_response_succeeded(&body) => redirect_to_login(shell, set_account),
            Ok(_) | Err(_) => set_account.update(AccountState::logout_failed),
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn submit_logout(_shell: ShellSignals, set_account: WriteSignal<AccountState>) {
    set_account.update(AccountState::logout_failed);
}

#[cfg(target_arch = "wasm32")]
async fn request_json(request_kind: AccountRequest) -> Result<String, AccountRequestError> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{Request, Response};

    let init = request_kind.init();
    let request = Request::new_with_str_and_init(request_kind.path(), &init)
        .map_err(|_error| AccountRequestError::RequestBuild)?;
    let window = web_sys::window().ok_or(AccountRequestError::WindowUnavailable)?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_error| AccountRequestError::Network)?
        .dyn_into::<Response>()
        .map_err(|_error| AccountRequestError::ResponseType)?;
    if !response.ok() {
        return Err(AccountRequestError::HttpStatus);
    }
    let body = JsFuture::from(
        response
            .text()
            .map_err(|_error| AccountRequestError::ResponseBody)?,
    )
    .await
    .map_err(|_error| AccountRequestError::ResponseBody)?;
    body.as_string().ok_or(AccountRequestError::ResponseBody)
}

#[cfg(target_arch = "wasm32")]
fn redirect_to_login(shell: ShellSignals, set_account: WriteSignal<AccountState>) {
    let Some(window) = web_sys::window() else {
        set_account.update(AccountState::logout_failed);
        return;
    };
    shell.set_header_panel.set(HeaderPanel::Closed);
    if window.location().set_href("/login").is_err() {
        set_account.update(AccountState::logout_failed);
    }
}

#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccountRequest {
    Status,
    Logout,
}

#[cfg(target_arch = "wasm32")]
impl AccountRequest {
    const fn path(self) -> &'static str {
        match self {
            Self::Status => "/api/auth/status",
            Self::Logout => "/api/auth/logout",
        }
    }

    fn init(self) -> web_sys::RequestInit {
        use web_sys::{RequestCache, RequestCredentials, RequestInit};

        let init = RequestInit::new();
        match self {
            Self::Status => init.set_method("GET"),
            Self::Logout => init.set_method("POST"),
        }
        init.set_credentials(RequestCredentials::SameOrigin);
        init.set_cache(RequestCache::NoStore);
        init
    }
}

#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccountRequestError {
    WindowUnavailable,
    RequestBuild,
    Network,
    ResponseType,
    HttpStatus,
    ResponseBody,
}
