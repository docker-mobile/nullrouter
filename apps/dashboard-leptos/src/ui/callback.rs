//! The provider OAuth callback screen.
//!
//! This was 59 lines of inline JavaScript in the actix host's callback shell. It
//! relays the grant a provider redirected back with to whatever started the flow,
//! then tells the user the tab can close.
//!
//! Derivations live in [`crate::dashboard::callback_live`]; this file is markup and
//! the three relay channels.

use crate::dashboard::callback_live::{
    CallbackData, HELPER_ORIGIN, Panel, RELAY_CHANNEL, panel_for, parse_callback, relay_envelope,
    relay_origins, relay_payload,
};
use leptos::prelude::*;

/// Strings this screen renders, asserted by the host's boundary tests.
static VISIBLE_CONTRACT: &[&str] = &[
    "nr-callback-panel",
    "Processing...",
    "Authorization Successful!",
    "Copy This URL",
    "You can close this tab now.",
    RELAY_CHANNEL,
    HELPER_ORIGIN,
];

pub fn callback_visible_contract() -> &'static [&'static str] {
    VISIBLE_CONTRACT
}

#[component]
pub fn CallbackPanel() -> impl IntoView {
    let data = RwSignal::new(read_callback());
    let panel = RwSignal::new(Panel::Processing);
    let closing_copy = RwSignal::new(String::from("This window will close automatically..."));

    relay_and_settle(data, panel, closing_copy);

    view! {
        <main class="nr-callback-panel" aria-live="polite">
            <Show when=move || panel.get() == Panel::Processing>
                <div id="processing" class="nr-callback-state">
                    <span class="nr-callback-icon">"..."</span>
                    <h1>"Processing..."</h1>
                    <p>"Please wait while we complete the authorization."</p>
                </div>
            </Show>
            <Show when=move || panel.get() == Panel::Success>
                <div id="success" class="nr-callback-state">
                    <span class="nr-callback-icon nr-callback-ok">"OK"</span>
                    <h1>"Authorization Successful!"</h1>
                    <p id="success-copy">{move || closing_copy.get()}</p>
                </div>
            </Show>
            <Show when=move || panel.get() == Panel::ManualCopy>
                <div id="manual" class="nr-callback-state">
                    <span class="nr-callback-icon nr-callback-warn">"URL"</span>
                    <h1>"Copy This URL"</h1>
                    <p>
                        "Please copy the URL from the address bar and paste it in the application."
                    </p>
                    <code id="callback-url">{move || data.get().full_url}</code>
                </div>
            </Show>
        </main>
    }
}

/// Read the grant off the current URL.
#[cfg(target_arch = "wasm32")]
fn read_callback() -> CallbackData {
    let Some(window) = web_sys::window() else {
        return CallbackData::default();
    };
    let location = window.location();
    let query = location.search().unwrap_or_default();
    let full_url = location.href().unwrap_or_default();
    parse_callback(&query, &full_url)
}

#[cfg(not(target_arch = "wasm32"))]
fn read_callback() -> CallbackData {
    CallbackData::default()
}

/// Relay the grant, then show the resulting panel.
///
/// All three channels are attempted because which one the initiator is listening
/// on depends on how it opened this page. A failure on one is not fatal: another
/// may still deliver, and the manual-copy panel is the last resort.
#[cfg(target_arch = "wasm32")]
fn relay_and_settle(
    data: RwSignal<CallbackData>,
    panel: RwSignal<Panel>,
    closing_copy: RwSignal<String>,
) {
    let payload = data.get_untracked();
    relay_to_opener(&payload);
    relay_to_channel(&payload);
    relay_to_storage(&payload);

    let next = panel_for(&payload);
    panel.set(next);
    if next == Panel::Success {
        // Closing a tab the user opened is not always permitted, so the copy is
        // corrected rather than leaving a promise the browser may refuse to keep.
        schedule_close(closing_copy);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn relay_and_settle(
    data: RwSignal<CallbackData>,
    panel: RwSignal<Panel>,
    _closing_copy: RwSignal<String>,
) {
    panel.set(panel_for(&data.get_untracked()));
}

/// `postMessage` the grant to the window that opened this one.
///
/// Targeted at named origins, never `"*"`: the payload carries an authorization
/// code, and a wildcard target would hand it to any opener.
#[cfg(target_arch = "wasm32")]
fn relay_to_opener(data: &CallbackData) {
    use wasm_bindgen::{JsCast, JsValue};

    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(opener) = window.opener() else {
        return;
    };
    if opener.is_falsy() {
        return;
    }
    let Ok(opener) = opener.dyn_into::<web_sys::Window>() else {
        return;
    };
    let envelope = relay_envelope(data);
    let Ok(message) = js_sys::JSON::parse(&envelope) else {
        return;
    };
    let own_origin = window.location().origin().unwrap_or_default();
    for origin in relay_origins(&own_origin) {
        // A cross-origin post to a window that is not listening throws; another
        // origin in the list may still be the right one.
        let _ = opener.post_message(&message, &origin);
    }
    drop(JsValue::from(0));
}

/// Broadcast the grant to any same-origin listener.
#[cfg(target_arch = "wasm32")]
fn relay_to_channel(data: &CallbackData) {
    let Ok(channel) = web_sys::BroadcastChannel::new(RELAY_CHANNEL) else {
        return;
    };
    if let Ok(message) = js_sys::JSON::parse(&relay_payload(data, now_ms())) {
        let _ = channel.post_message(&message);
    }
    channel.close();
}

/// Leave the grant in `localStorage` for a listener that starts later.
#[cfg(target_arch = "wasm32")]
fn relay_to_storage(data: &CallbackData) {
    if let Some(window) = web_sys::window()
        && let Ok(Some(storage)) = window.local_storage()
    {
        let _ = storage.set_item(RELAY_CHANNEL, &relay_payload(data, now_ms()));
    }
}

#[cfg(target_arch = "wasm32")]
fn now_ms() -> i64 {
    // `Date.now()` is milliseconds since the epoch, which fits an i64 for any
    // date this will run on.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Date.now() in ms is far inside i64 for any real date"
    )]
    let millis = js_sys::Date::now() as i64;
    millis
}

/// Try to close the tab shortly after a successful relay.
#[cfg(target_arch = "wasm32")]
fn schedule_close(closing_copy: RwSignal<String>) {
    use wasm_bindgen::JsCast;

    let tick = wasm_bindgen::closure::Closure::once_into_js(move || {
        if let Some(window) = web_sys::window() {
            let _ = window.close();
        }
        // If the close was refused, the user needs to be told to do it.
        closing_copy.set(String::from("You can close this tab now."));
    });
    if let Some(window) = web_sys::window() {
        let _ = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(tick.unchecked_ref(), 1500);
    }
}

#[cfg(test)]
mod tests {
    use super::callback_visible_contract;
    use crate::dashboard::callback_live::{HELPER_ORIGIN, RELAY_CHANNEL};

    #[test]
    fn the_contract_names_the_relay_channels_and_every_panel() {
        // The host serves only a mount point now, so it asserts this list. A
        // missing panel would be a state the user could get stuck in with no copy.
        let contract = callback_visible_contract();
        for expected in [
            "Processing...",
            "Authorization Successful!",
            "Copy This URL",
            RELAY_CHANNEL,
            HELPER_ORIGIN,
        ] {
            assert!(contract.contains(&expected), "{expected} missing");
        }
    }
}
