//! The per-model rates the router bills usage against.
//!
//! `GET /api/pricing` answers a two-level map with no envelope -- provider, then model, then the
//! five rate fields -- so it is flattened into ordered rows by
//! [`crate::routes::types::price_rows`] rather than rendered from the map directly.
//!
//! What the endpoint returns is the *merged* catalogue: the router's built-in rates with any local
//! override applied on top. A `PATCH` answers with the overrides alone, which is a different shape
//! and a smaller set, so a save refetches instead of rendering the response. Reporting the reply as
//! though it were the whole catalogue would blank every rate the user had not overridden.
//!
//! An absent field is left absent everywhere. A model priced for input and output but not for cached
//! reads shows a dash for the rest, because a zero there is a claim that reading cache is free.

use leptos::prelude::*;

use crate::api::{Hydrate, Method, Save, encode, load_with, request_detailed, submit_reporting};
use crate::routes::types::{PriceFields, PriceRow, price_rows};
use crate::routes::{PageHeader, Panel};

/// The flattened table.
///
/// An alias rather than `Vec<PriceRow>` written out, because the `view!` attribute parser reads the
/// `<` in a generic type annotation as the start of a tag and fails on the rest of the line.
type PriceRows = Vec<PriceRow>;

/// One rate field as the user left it.
enum Rate {
    /// Blank. The field is not sent, so whatever is stored stays.
    Absent,
    /// A number the router will accept.
    Value(f64),
    /// Not a rate: unparseable, negative, or not finite.
    Invalid,
}

/// Read one rate field.
///
/// Negative and non-finite values are refused here as well as by the server, which turns a typo into
/// an immediate message instead of a round trip.
fn parse_rate(raw: &str) -> Rate {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Rate::Absent;
    }
    match trimmed.parse::<f64>() {
        Ok(value) if value.is_finite() && value >= 0.0 => Rate::Value(value),
        Ok(_) | Err(_) => Rate::Invalid,
    }
}

/// Build the body for a save, or report that a field is not a rate.
fn fields_from_entries(entries: [&str; 5]) -> Option<PriceFields> {
    let [input, output, cached, reasoning, cache_creation] = entries;
    let mut fields = PriceFields::default();
    for (raw, slot) in [
        (input, &mut fields.input),
        (output, &mut fields.output),
        (cached, &mut fields.cached),
        (reasoning, &mut fields.reasoning),
        (cache_creation, &mut fields.cache_creation),
    ] {
        match parse_rate(raw) {
            Rate::Absent => {}
            Rate::Value(value) => *slot = Some(value),
            Rate::Invalid => return None,
        }
    }
    Some(fields)
}

/// A rate for display. An absent one reads as unknown, not as free.
fn rate_label(rate: Option<f64>) -> String {
    rate.map_or_else(|| "—".to_owned(), |value| value.to_string())
}

/// A rate for an input field: blank when there is nothing stored.
fn rate_value(rate: Option<f64>) -> String {
    rate.map(|value| value.to_string()).unwrap_or_default()
}

/// Percent-encode a query parameter value.
///
/// The reset below identifies a model by query string, and the names are not this panel's to choose:
/// the override form accepts whatever the user types, and the endpoint stores it verbatim. A name
/// carrying a space or an `&` would otherwise build a request that resets the wrong entry, or none.
///
/// Only the unreserved set from RFC 3986 is passed through; everything else, non-ASCII included, is
/// encoded byte by byte.
fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(hex_digit(byte >> 4));
            out.push(hex_digit(byte & 0x0F));
        }
    }
    out
}

/// One uppercase hex digit for the low nibble of `value`.
fn hex_digit(value: u8) -> char {
    char::from_digit(u32::from(value & 0x0F), 16).map_or('0', |digit| digit.to_ascii_uppercase())
}

/// Serialize a one-model pricing update into the nested shape the endpoint takes.
fn patch_body(provider: &str, model: &str, fields: &PriceFields) -> Option<String> {
    let models: std::collections::BTreeMap<&str, &PriceFields> = [(model, fields)].into();
    let catalog: std::collections::BTreeMap<&str, _> = [(provider, models)].into();
    encode(&catalog).ok()
}

#[component]
pub fn Pricing() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (rows, set_rows) = signal(Hydrate::<PriceRows>::Loading);
    let reload = move || {
        set_rows.set(Hydrate::Loading);
        load_with("/api/pricing", set_rows, price_rows);
    };
    reload();

    view! {
        <PageHeader
            title=locale.get("nav.pricing").to_owned()
            description=locale.get("pricing.description").to_owned()
        />
        <AddOverride reload=reload />
        <Panel
            state=rows
            on_retry=Callback::new(move |()| reload())
            children=move |data: PriceRows| view! { <PriceTable rows=data reload=reload /> }
        />
    }
}

/// Price a model the catalogue does not carry a rate for.
///
/// The same `PATCH` the row editor uses. Kept separate because a provider and model that are not
/// listed yet have no row to edit, and pricing one is otherwise impossible from here.
#[component]
fn AddOverride(reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (provider, set_provider) = signal(String::new());
    let (model, set_model) = signal(String::new());
    let (input, set_input) = signal(String::new());
    let (output, set_output) = signal(String::new());
    let (save, set_save) = signal(Save::Idle);

    // Owned before the closure: `Locale` is not `Copy`, and the view below still needs it.
    let rate_invalid = StoredValue::new(locale.get("pricing.rate_invalid").to_owned());
    let nothing_entered = StoredValue::new(locale.get("pricing.nothing_entered").to_owned());
    let encode_failed = StoredValue::new(locale.get("pricing.encode_failed").to_owned());

    let apply = move || {
        let provider_name = provider.get().trim().to_owned();
        let model_name = model.get().trim().to_owned();
        if save.get().is_saving() || provider_name.is_empty() || model_name.is_empty() {
            return;
        }
        let Some(fields) = fields_from_entries([&input.get(), &output.get(), "", "", ""]) else {
            set_save.set(Save::Refused(rate_invalid.get_value()));
            return;
        };
        if fields == PriceFields::default() {
            set_save.set(Save::Refused(nothing_entered.get_value()));
            return;
        }
        let Some(body) = patch_body(&provider_name, &model_name, &fields) else {
            set_save.set(Save::Refused(encode_failed.get_value()));
            return;
        };

        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Patch, "/api/pricing", Some(&body)).await },
            move |_| {
                // The reply carries only the overrides, not the merged catalogue, so the table is
                // refetched rather than rendered from it.
                set_provider.set(String::new());
                set_model.set(String::new());
                set_input.set(String::new());
                set_output.set(String::new());
                reload();
            },
        );
    };

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4 mb-4">
            <p class="text-sm text-muted-foreground">{locale.get("pricing.add_hint").to_owned()}</p>
            <div class="grid gap-3 sm:grid-cols-4">
                <TextField
                    label=locale.get("pricing.col_provider").to_owned()
                    value=provider
                    set=set_provider
                />
                <TextField
                    label=locale.get("pricing.col_model").to_owned()
                    value=model
                    set=set_model
                />
                <RateField
                    label=locale.get("pricing.col_input").to_owned()
                    value=input
                    set=set_input
                />
                <RateField
                    label=locale.get("pricing.col_output").to_owned()
                    value=output
                    set=set_output
                />
            </div>
            <button
                type="button"
                class="rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
                disabled=move || {
                    save.get().is_saving() || provider.get().trim().is_empty()
                        || model.get().trim().is_empty()
                }
                on:click=move |_| apply()
            >
                {locale.get("pricing.add").to_owned()}
            </button>
            <SaveMessage save=save />
        </section>
    }
}

/// Whatever went wrong with the last write, in the server's words when it had any.
#[component]
fn SaveMessage(save: ReadSignal<Save>) -> impl IntoView {
    view! {
        {move || {
            save.get()
                .message()
                .map(|message| {
                    view! { <p class="text-sm text-destructive" role="alert">{message}</p> }
                })
        }}
    }
}

#[component]
fn TextField(label: String, value: ReadSignal<String>, set: WriteSignal<String>) -> impl IntoView {
    view! {
        <label class="space-y-1 text-sm">
            <span class="text-muted-foreground">{label}</span>
            <input
                type="text"
                class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                prop:value=move || value.get()
                on:input=move |ev| set.set(event_target_value(&ev))
            />
        </label>
    }
}

/// A rate input.
///
/// `inputmode="decimal"` rather than `type="number"`, whose spinners and browser-specific
/// validation get in the way of a field that is legitimately blank most of the time.
#[component]
fn RateField(label: String, value: ReadSignal<String>, set: WriteSignal<String>) -> impl IntoView {
    view! {
        <label class="space-y-1 text-sm">
            <span class="text-muted-foreground">{label}</span>
            <input
                type="text"
                inputmode="decimal"
                class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono"
                prop:value=move || value.get()
                on:input=move |ev| set.set(event_target_value(&ev))
            />
        </label>
    }
}

#[component]
fn PriceTable(
    rows: Vec<PriceRow>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if rows.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">{locale.get("pricing.empty").to_owned()}</p>
        }
        .into_any();
    }
    view! {
        <div class="rounded-lg border border-border overflow-x-auto">
            <table class="w-full text-sm">
                <thead class="bg-muted/50 text-muted-foreground">
                    <tr>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("pricing.col_provider").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("pricing.col_model").to_owned()}
                        </th>
                        <th class="text-right font-medium px-3 py-2">
                            {locale.get("pricing.col_input").to_owned()}
                        </th>
                        <th class="text-right font-medium px-3 py-2">
                            {locale.get("pricing.col_output").to_owned()}
                        </th>
                        <th class="text-right font-medium px-3 py-2">
                            {locale.get("pricing.col_cached").to_owned()}
                        </th>
                        <th class="text-right font-medium px-3 py-2">
                            {locale.get("pricing.col_reasoning").to_owned()}
                        </th>
                        <th class="text-right font-medium px-3 py-2">
                            {locale.get("pricing.col_cache_creation").to_owned()}
                        </th>
                        <th class="px-3 py-2"></th>
                    </tr>
                </thead>
                <tbody>
                    {rows
                        .into_iter()
                        .map(|row| view! { <PriceRowView row=row reload=reload /> })
                        .collect_view()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

/// One priced model, with an inline editor and a reset.
#[component]
fn PriceRowView(row: PriceRow, reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (editing, set_editing) = signal(false);
    let (save, set_save) = signal(Save::Idle);

    let (input, set_input) = signal(rate_value(row.fields.input));
    let (output, set_output) = signal(rate_value(row.fields.output));
    let (cached, set_cached) = signal(rate_value(row.fields.cached));
    let (reasoning, set_reasoning) = signal(rate_value(row.fields.reasoning));
    let (cache_creation, set_cache_creation) = signal(rate_value(row.fields.cache_creation));

    let provider = StoredValue::new(row.provider.clone());
    let model = StoredValue::new(row.model.clone());
    let stored = StoredValue::new(row.fields);
    let rate_invalid = StoredValue::new(locale.get("pricing.rate_invalid").to_owned());
    let encode_failed = StoredValue::new(locale.get("pricing.encode_failed").to_owned());
    let label_edit = locale.get("pricing.edit").to_owned();
    let label_cancel = locale.get("pricing.cancel").to_owned();

    let apply = move || {
        if save.get().is_saving() {
            return;
        }
        let Some(fields) = fields_from_entries([
            &input.get(),
            &output.get(),
            &cached.get(),
            &reasoning.get(),
            &cache_creation.get(),
        ]) else {
            set_save.set(Save::Refused(rate_invalid.get_value()));
            return;
        };
        let Some(body) = patch_body(&provider.get_value(), &model.get_value(), &fields) else {
            set_save.set(Save::Refused(encode_failed.get_value()));
            return;
        };

        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Patch, "/api/pricing", Some(&body)).await },
            move |_| {
                set_editing.set(false);
                reload();
            },
        );
    };

    // Scoped to this one model by query parameter. Both are required: a provider on its own drops
    // every model under it, and neither drops every override there is.
    let reset = move || {
        if save.get().is_saving() {
            return;
        }
        let path = format!(
            "/api/pricing?provider={}&model={}",
            encode_query(&provider.get_value()),
            encode_query(&model.get_value())
        );
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Delete, &path, None).await },
            move |_| {
                set_editing.set(false);
                reload();
            },
        );
    };

    view! {
        <tr class="border-t border-border align-top">
            <td class="px-3 py-2 text-muted-foreground">{row.provider}</td>
            <td class="px-3 py-2 font-mono text-xs">{row.model}</td>
            {move || {
                if editing.get() {
                    view! {
                        <td class="px-3 py-2"><RateCell value=input set=set_input /></td>
                        <td class="px-3 py-2"><RateCell value=output set=set_output /></td>
                        <td class="px-3 py-2"><RateCell value=cached set=set_cached /></td>
                        <td class="px-3 py-2"><RateCell value=reasoning set=set_reasoning /></td>
                        <td class="px-3 py-2">
                            <RateCell value=cache_creation set=set_cache_creation />
                        </td>
                    }
                        .into_any()
                } else {
                    let fields = stored.get_value();
                    view! {
                        <td class="px-3 py-2 text-right font-mono text-xs">
                            {rate_label(fields.input)}
                        </td>
                        <td class="px-3 py-2 text-right font-mono text-xs">
                            {rate_label(fields.output)}
                        </td>
                        <td class="px-3 py-2 text-right font-mono text-xs">
                            {rate_label(fields.cached)}
                        </td>
                        <td class="px-3 py-2 text-right font-mono text-xs">
                            {rate_label(fields.reasoning)}
                        </td>
                        <td class="px-3 py-2 text-right font-mono text-xs">
                            {rate_label(fields.cache_creation)}
                        </td>
                    }
                        .into_any()
                }
            }}
            <td class="px-3 py-2 text-right whitespace-nowrap">
                <div class="flex items-center justify-end gap-3">
                    <SaveButton editing=editing save=save on_click=Callback::new(move |()| apply()) />
                    <button
                        type="button"
                        class="text-sm underline-offset-4 hover:underline disabled:opacity-50"
                        disabled=move || save.get().is_saving()
                        on:click=move |_| {
                            // Reopening restores the stored rates, discarding an abandoned edit and
                            // any refusal left over from the previous attempt.
                            if !editing.get() {
                                let fields = stored.get_value();
                                set_input.set(rate_value(fields.input));
                                set_output.set(rate_value(fields.output));
                                set_cached.set(rate_value(fields.cached));
                                set_reasoning.set(rate_value(fields.reasoning));
                                set_cache_creation.set(rate_value(fields.cache_creation));
                                set_save.set(Save::Idle);
                            }
                            set_editing.update(|open| *open = !*open);
                        }
                    >
                        {move || {
                            if editing.get() { label_cancel.clone() } else { label_edit.clone() }
                        }}
                    </button>
                    <button
                        type="button"
                        class="text-sm text-muted-foreground underline-offset-4 hover:underline disabled:opacity-50"
                        disabled=move || save.get().is_saving()
                        on:click=move |_| reset()
                        title=locale.get("pricing.reset_hint").to_owned()
                    >
                        {locale.get("pricing.reset").to_owned()}
                    </button>
                </div>
                {move || {
                    save.get()
                        .message()
                        .map(|message| {
                            view! {
                                <p class="mt-1 text-xs text-destructive whitespace-normal text-right">
                                    {message}
                                </p>
                            }
                        })
                }}
            </td>
        </tr>
    }
}

/// Shown only while the row is open for editing.
#[component]
fn SaveButton(
    editing: ReadSignal<bool>,
    save: ReadSignal<Save>,
    on_click: Callback<()>,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let label = locale.get("pricing.save").to_owned();
    view! {
        {move || {
            let label = label.clone();
            editing
                .get()
                .then(|| {
                    view! {
                        <button
                            type="button"
                            class="text-sm font-medium underline-offset-4 hover:underline disabled:opacity-50"
                            disabled=move || save.get().is_saving()
                            on:click=move |_| on_click.run(())
                        >
                            {label}
                        </button>
                    }
                })
        }}
    }
}

#[component]
fn RateCell(value: ReadSignal<String>, set: WriteSignal<String>) -> impl IntoView {
    view! {
        <input
            type="text"
            inputmode="decimal"
            class="w-20 rounded-md border border-input bg-background px-2 py-1 text-right text-xs font-mono"
            prop:value=move || value.get()
            on:input=move |ev| set.set(event_target_value(&ev))
        />
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Rate, encode_query, fields_from_entries, parse_rate, patch_body, rate_label, rate_value,
    };
    use crate::routes::types::PriceFields;

    #[test]
    fn a_plain_model_name_needs_no_encoding() {
        assert_eq!(encode_query("gpt-5.3-codex"), "gpt-5.3-codex");
        assert_eq!(encode_query("openai"), "openai");
    }

    #[test]
    fn a_name_that_would_break_the_query_is_encoded() {
        // `&` would otherwise start a new parameter, and a slash or space would arrive mangled --
        // either way the reset would clear the wrong entry, or nothing at all.
        assert_eq!(encode_query("a b"), "a%20b");
        assert_eq!(encode_query("x&provider=y"), "x%26provider%3Dy");
        assert_eq!(encode_query("vendor/model"), "vendor%2Fmodel");
        assert_eq!(encode_query("50%"), "50%25");
    }

    #[test]
    fn non_ascii_is_encoded_per_byte() {
        assert_eq!(encode_query("é"), "%C3%A9");
    }

    #[test]
    fn a_blank_field_is_absent_rather_than_zero() {
        assert!(matches!(parse_rate(""), Rate::Absent));
        assert!(matches!(parse_rate("   "), Rate::Absent));
    }

    #[test]
    fn a_usable_rate_is_read() {
        assert!(
            matches!(parse_rate("1.75"), Rate::Value(value) if (value - 1.75).abs() < f64::EPSILON)
        );
        assert!(matches!(parse_rate("0"), Rate::Value(value) if value == 0.0));
        assert!(
            matches!(parse_rate(" 14 "), Rate::Value(value) if (value - 14.0).abs() < f64::EPSILON)
        );
    }

    #[test]
    fn a_rate_the_router_would_refuse_is_caught_before_the_request() {
        // The server refuses each of these too; catching them here turns a round trip into a
        // message beside the field.
        for raw in ["-1", "abc", "1.2.3", "NaN", "inf", "1e400"] {
            assert!(matches!(parse_rate(raw), Rate::Invalid), "{raw}");
        }
    }

    #[test]
    fn entered_fields_become_a_partial_update() {
        let fields = fields_from_entries(["1.75", "14", "", "", ""]).expect("valid");
        assert_eq!(fields.input, Some(1.75));
        assert_eq!(fields.output, Some(14.0));
        // Untouched fields stay absent, so the save does not overwrite them with zero.
        assert_eq!(fields.cached, None);
        assert_eq!(fields.reasoning, None);
        assert_eq!(fields.cache_creation, None);
    }

    #[test]
    fn one_bad_field_refuses_the_whole_update() {
        assert!(fields_from_entries(["1.75", "-2", "", "", ""]).is_none());
    }

    #[test]
    fn the_patch_body_is_nested_by_provider_then_model() {
        let fields = PriceFields {
            input: Some(2.5),
            output: Some(10.0),
            ..PriceFields::default()
        };
        let body = patch_body("openai", "gpt-5", &fields).expect("encodes");
        assert_eq!(body, r#"{"openai":{"gpt-5":{"input":2.5,"output":10.0}}}"#);
    }

    #[test]
    fn absent_fields_are_omitted_from_the_body_rather_than_sent_as_null() {
        // `{"input":null}` is not a number, and the endpoint refuses the whole update.
        let fields = PriceFields {
            input: Some(1.0),
            ..PriceFields::default()
        };
        let body = patch_body("gh", "m", &fields).expect("encodes");
        assert_eq!(body, r#"{"gh":{"m":{"input":1.0}}}"#);
        assert!(!body.contains("null"));
    }

    #[test]
    fn an_absent_rate_reads_as_unknown_not_as_free() {
        assert_eq!(rate_label(None), "—");
        assert_eq!(rate_label(Some(1.75)), "1.75");
        assert_eq!(rate_label(Some(0.0)), "0");
    }

    #[test]
    fn an_absent_rate_leaves_its_input_blank() {
        // A "0" here would turn opening the editor into a claim that the rate is zero.
        assert_eq!(rate_value(None), "");
        assert_eq!(rate_value(Some(0.175)), "0.175");
    }
}
