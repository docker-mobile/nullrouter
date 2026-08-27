//! Import an existing 9Router installation.
//!
//! This is the one panel in the dashboard that writes a user's provider
//! credentials, so it is built around two guarantees.
//!
//! *Preview before write.* [`ImportGate`] decides whether Import is pressable,
//! and it only opens after a dry run reported records it would write. The rule
//! lives in `dashboard::migrate` where it is unit-tested; the button only reads
//! it.
//!
//! *No invented numbers.* Every count on screen comes out of a server
//! [`ImportReport`]. While a scan is open the table is `.nr-skeleton` rows, and
//! a failure renders as itself — there is no branch that fills the table with
//! zeros, because "0 found" and "the request failed" mean opposite things to
//! someone deciding whether to migrate.
//!
//! The API-key caveat sits above the controls rather than in the warnings list:
//! keys cannot be imported at all, and a user who only skims would otherwise
//! finish the import believing their keys came across.

use crate::api::{ApiError, Hydrate, Save};
use crate::dashboard::migrate::{
    ImportGate, ImportReport, MissingInstall, Outcome, Phase, Rejected, ReportRow, import_gate,
    run_migrate, status_line,
};
use leptos::prelude::*;

/// The panel's stylesheet, shared verbatim with the actix host.
///
/// The CSR build links no stylesheet, so the same file the host serves is
/// inlined here. One source, two delivery paths — the alternative was a second
/// copy that would drift.
const MIGRATE_STYLES: &str =
    include_str!("../../../../services/dashboard-actix/static/assets/dashboard/migrate.css");

/// Signals shared by the panel and its dispatcher.
#[derive(Clone, Copy)]
struct MigrateSignals {
    /// Last dry-run result.
    preview: RwSignal<Hydrate<Outcome>>,
    /// Result of the last real import.
    outcome: RwSignal<Option<Outcome>>,
    /// Which request is open, if any.
    phase: RwSignal<Option<Phase>>,
    /// How the last import ended.
    save: RwSignal<Save>,
    /// `DATA_DIR` override, blank for server-side discovery.
    data_dir: RwSignal<String>,
}

/// Start a scan or an import.
///
/// A scan clears the previous preview *and* the previous import result: once the
/// source is being re-read, the old numbers describe a different scan and must
/// not stay on screen next to the new ones.
fn dispatch(signals: MigrateSignals, phase: Phase) {
    signals.phase.set(Some(phase));
    match phase {
        Phase::Scan => {
            signals.preview.set(Hydrate::Loading);
            signals.outcome.set(None);
            signals.save.set(Save::Idle);
        }
        Phase::Import => signals.save.set(Save::Saving),
    }

    let data_dir = signals.data_dir.get_untracked();
    spawn(async move {
        let result = run_migrate(data_dir, matches!(phase, Phase::Scan)).await;
        signals.phase.set(None);
        match phase {
            Phase::Scan => signals
                .preview
                .set(result.map_or_else(Hydrate::Failed, Hydrate::Ready)),
            Phase::Import => match result {
                Ok(outcome) => {
                    // A refusal is not a completed import, so it must not be
                    // reported as saved.
                    signals.save.set(if outcome.report().is_some() {
                        Save::Saved
                    } else {
                        Save::Failed(ApiError::Body)
                    });
                    signals.outcome.set(Some(outcome));
                }
                Err(error) => signals.save.set(Save::Failed(error)),
            },
        }
    });
}

#[cfg(target_arch = "wasm32")]
fn spawn<F: std::future::Future<Output = ()> + 'static>(task: F) {
    wasm_bindgen_futures::spawn_local(task);
}

/// Native builds have no executor and no browser to fetch from.
#[cfg(not(target_arch = "wasm32"))]
fn spawn<F: std::future::Future<Output = ()> + 'static>(task: F) {
    drop(task);
}

#[component]
pub(super) fn MigratePanel() -> impl IntoView {
    let signals = MigrateSignals {
        preview: RwSignal::new(Hydrate::Loading),
        outcome: RwSignal::new(None),
        phase: RwSignal::new(None),
        save: RwSignal::new(Save::Idle),
        data_dir: RwSignal::new(String::new()),
    };

    let gate = Memo::new(move |_| {
        let preview = signals.preview.get();
        // An import already consumed this preview only if it produced a report.
        let imported = signals
            .outcome
            .with(|outcome| outcome.as_ref().is_some_and(|done| done.report().is_some()));
        import_gate(preview.ready(), signals.phase.get().is_some(), imported)
    });

    // Land on the page already knowing whether an install exists: a user should
    // not have to press anything to find out.
    dispatch(signals, Phase::Scan);

    view! {
        <style>{MIGRATE_STYLES}</style>
        <div class="nr-panel-stack">
            <article class="nr-card nr-card-hero nr-anim-rise">
                <div>
                    <p class="nr-eyebrow">"Migration"</p>
                    <h2>"Migrate from 9Router"</h2>
                    <p>
                        "Reads an existing 9Router installation and copies its provider connections, \
                         combos, proxy pools, and settings into nullrouter. The import is additive: \
                         existing records are kept and duplicates are skipped, so running it twice \
                         is safe."
                    </p>
                </div>
            </article>
            <ApiKeyCaveat />
            <ScanControls signals gate />
            <PreviewCard signals />
            <ImportResultCard signals />
        </div>
    }
}

/// The one consequence of migrating that a warnings list would bury.
#[component]
fn ApiKeyCaveat() -> impl IntoView {
    view! {
        <aside class="nr-migrate-caveat nr-anim-rise" aria-label="Before you import">
            <span class="nr-migrate-caveat-mark" aria-hidden="true">"!"</span>
            <div class="nr-migrate-caveat-body">
                <strong>"Your 9Router API keys will not be imported."</strong>
                <span>
                    "nullrouter stores only a one-way digest of each API key, so an existing \
                     9Router key cannot be turned back into a usable key here. Any key you issued \
                     in 9Router must be re-issued from this dashboard, and clients using the old \
                     keys will be rejected until you do. Everything else — connections, combos, \
                     proxy pools, and settings — imports normally."
                </span>
            </div>
        </aside>
    }
}

/// Data-dir override, the two actions, and the live status line.
#[component]
fn ScanControls(signals: MigrateSignals, gate: Memo<ImportGate>) -> impl IntoView {
    let busy = move || signals.phase.get().is_some();

    view! {
        <article class="nr-card nr-anim-rise">
            <div class="nr-card-head">
                <div>
                    <h2>"Source and actions"</h2>
                    <p>"Preview first, then import. Import stays disabled until a preview succeeds."</p>
                </div>
            </div>
            <div class="nr-migrate-dir">
                <label for="migrate-data-dir">"9Router data directory"</label>
                <div class="nr-migrate-dir-row">
                    <input
                        id="migrate-data-dir"
                        type="text"
                        class="nr-preview-input"
                        placeholder="Leave blank to search DATA_DIR and ~/.9router"
                        autocomplete="off"
                        spellcheck="false"
                        prop:value=move || signals.data_dir.get()
                        on:input=move |event| signals.data_dir.set(event_target_value(&event))
                    />
                    <button
                        type="button"
                        class="nr-button secondary"
                        disabled=busy
                        on:click=move |_| dispatch(signals, Phase::Scan)
                    >
                        "Re-scan"
                    </button>
                </div>
                <small>
                    "Point this at a 9Router DATA_DIR to read an installation outside the default \
                     location. Re-scanning replaces the preview below."
                </small>
            </div>
            <div class="nr-migrate-actions">
                <button
                    type="button"
                    class="nr-button secondary"
                    disabled=busy
                    on:click=move |_| dispatch(signals, Phase::Scan)
                >
                    <Show when=move || signals.phase.get() == Some(Phase::Scan)>
                        <span class="nr-spinner" aria-hidden="true"></span>
                    </Show>
                    "Preview (dry run)"
                </button>
                <button
                    type="button"
                    class="nr-button primary"
                    disabled=move || !gate.get().allows_import()
                    title=move || gate.get().blocked_reason().unwrap_or("Import from 9Router")
                    on:click=move |_| dispatch(signals, Phase::Import)
                >
                    <Show when=move || signals.phase.get() == Some(Phase::Import)>
                        <span class="nr-spinner" aria-hidden="true"></span>
                    </Show>
                    "Import"
                </button>
                <Show when=move || gate.get().blocked_reason().is_some()>
                    <span class="nr-migrate-gate-note">
                        {move || gate.get().blocked_reason().unwrap_or_default()}
                    </span>
                </Show>
            </div>
            // A bar rather than only a spinner: an import has no progress to
            // report, but the sweep makes it clear the request is still open.
            <Show when=busy>
                <div class="nr-progress-indeterminate" aria-hidden="true"></div>
            </Show>
            <p class="nr-migrate-status" role="status" aria-live="polite">
                {move || {
                    let preview = signals.preview.get();
                    let phase = signals.phase.get();
                    match (phase, preview.failure()) {
                        (None, Some(error)) => format!("Scan failed: {}", error.message()),
                        _ => status_line(phase, preview.ready()),
                    }
                }}
            </p>
        </article>
    }
}

/// The dry-run result: skeletons, a table, a not-found notice, or a failure.
#[component]
fn PreviewCard(signals: MigrateSignals) -> impl IntoView {
    view! {
        <article class="nr-card nr-anim-rise">
            <div class="nr-card-head">
                <div>
                    <h2>"Preview"</h2>
                    <p>"What a dry run found at the source, and how much of it would be written."</p>
                </div>
            </div>
            {move || match signals.preview.get() {
                Hydrate::Loading => view! { <SkeletonRows /> }.into_any(),
                Hydrate::Failed(error) => view! { <FailureNotice error /> }.into_any(),
                Hydrate::Ready(Outcome::Missing(missing)) => {
                    view! { <MissingNotice missing /> }.into_any()
                }
                Hydrate::Ready(Outcome::Refused(rejected)) => {
                    view! { <RefusedNotice rejected /> }.into_any()
                }
                Hydrate::Ready(Outcome::Completed { report, .. }) => {
                    view! { <ReportBody report heading="Would import" /> }.into_any()
                }
            }}
        </article>
    }
}

/// The result of a real import, shown only once one has run.
#[component]
fn ImportResultCard(signals: MigrateSignals) -> impl IntoView {
    view! {
        <Show when=move || {
            signals.outcome.with(Option::is_some) || signals.save.with(|save| {
                matches!(save, Save::Failed(_))
            })
        }>
            <article class="nr-card nr-anim-rise">
                <div class="nr-card-head">
                    <div>
                        <h2>"Import report"</h2>
                        <p>{move || signals.save.get().status().unwrap_or("Import finished.")}</p>
                    </div>
                </div>
                {move || match (signals.outcome.get(), signals.save.get()) {
                    (Some(Outcome::Completed { report, .. }), _) => {
                        view! { <ReportBody report heading="Imported" /> }.into_any()
                    }
                    (Some(Outcome::Missing(missing)), _) => {
                        view! { <MissingNotice missing /> }.into_any()
                    }
                    (Some(Outcome::Refused(rejected)), _) => {
                        view! { <RefusedNotice rejected /> }.into_any()
                    }
                    (None, Save::Failed(error)) => view! { <FailureNotice error /> }.into_any(),
                    (None, _) => ().into_any(),
                }}
            </article>
        </Show>
    }
}

/// Source, per-kind counts, and every warning.
#[component]
fn ReportBody(report: ImportReport, heading: &'static str) -> impl IntoView {
    let source = report.source.clone();
    let format = report.format.clone();
    let rows = report.rows();
    let warnings = report.warnings.clone();
    let empty = report.found_nothing();

    view! {
        <dl class="nr-migrate-source">
            <dt>"Source"</dt>
            <dd>{source}</dd>
            <dt>"Format"</dt>
            <dd>{format}</dd>
        </dl>
        <div class="nr-migrate-table-wrap">
            <table class="nr-migrate-table">
                <caption class="nr-migrate-sr-only">
                    {format!("Records found at the source and how many were {}", heading.to_lowercase())}
                </caption>
                <thead>
                    <tr>
                        <th scope="col">"Record"</th>
                        <th scope="col">"Found"</th>
                        <th scope="col">{heading}</th>
                        <th scope="col">"Skipped"</th>
                    </tr>
                </thead>
                <tbody class="nr-stagger">
                    <For
                        each=move || rows.to_vec()
                        key=|row| row.label
                        children=|row| view! { <CountRow row /> }
                    />
                    <tr>
                        <th scope="row">
                            "Settings"
                            <span class="nr-migrate-note">"Counted as one record"</span>
                        </th>
                        // The server reports only whether settings were written,
                        // never how many were present, so Found stays blank
                        // rather than guessing a number.
                        <td class="nr-migrate-cell-zero" aria-label="not reported">"—"</td>
                        <td class:nr-migrate-cell-zero=!report.settings_imported>
                            {if report.settings_imported { "yes" } else { "no" }}
                        </td>
                        <td class="nr-migrate-cell-zero" aria-label="not reported">"—"</td>
                    </tr>
                </tbody>
            </table>
        </div>
        <Show when=move || empty>
            <div class="nr-empty-state">
                <strong>"The source held no records"</strong>
                <span>
                    "The installation was read successfully but contained no connections, combos, \
                     proxy pools, or API keys. There is nothing to import from it."
                </span>
            </div>
        </Show>
        <WarningList warnings />
    }
}

/// One per-kind row.
#[component]
fn CountRow(row: ReportRow) -> impl IntoView {
    let zero = |count: usize| {
        if count == 0 {
            "nr-migrate-cell-zero"
        } else {
            ""
        }
    };

    view! {
        <tr>
            <th scope="row">
                {row.label}
                <Show when=move || row.note.is_some()>
                    <span class="nr-migrate-note">{row.note.unwrap_or_default()}</span>
                </Show>
            </th>
            <td class=zero(row.found)>{row.found}</td>
            <td class=zero(row.importable)>{row.importable}</td>
            <td class=zero(row.skipped())>{row.skipped()}</td>
        </tr>
    }
}

/// Every warning, verbatim.
///
/// Warnings name the exact record that was skipped, so they are the only place
/// a user can see *which* connection did not come across. None is dropped and
/// none is summarised.
#[component]
fn WarningList(warnings: Vec<String>) -> impl IntoView {
    let count = warnings.len();
    let numbered: Vec<(usize, String)> = warnings.into_iter().enumerate().collect();

    view! {
        <Show when=move || count != 0>
            <div class="nr-migrate-warnings-block">
                <p class="nr-migrate-status">
                    {format!("{count} warning(s). Each names a record that was skipped or failed.")}
                </p>
                <ul class="nr-migrate-warnings">
                    <For
                        // Keyed by position as well as text: two records can
                        // produce the identical warning and both must render.
                        each={
                            let numbered = numbered.clone();
                            move || numbered.clone()
                        }
                        key=|(index, warning)| format!("{index}:{warning}")
                        children=|(_, warning)| view! { <li>{warning}</li> }
                    />
                </ul>
            </div>
        </Show>
    }
}

/// No installation was found: expected for a fresh user, not a crash.
#[component]
fn MissingNotice(missing: MissingInstall) -> impl IntoView {
    let message = missing.message;
    let searched = missing.searched;
    let has_paths = !searched.is_empty();

    view! {
        <div class="nr-empty-state">
            <strong>"No 9Router installation found"</strong>
            <span class="nr-migrate-message">
                {if message.is_empty() {
                    "The server reported no installation and gave no detail.".to_owned()
                } else {
                    message
                }}
            </span>
            <Show when=move || has_paths>
                <p class="nr-migrate-status">"Searched:"</p>
                <ul class="nr-migrate-paths">
                    <For
                        each={
                            // Cloned per render: `Show` re-runs its children, so
                            // the list cannot be moved out of the closure.
                            let searched = searched.clone();
                            move || searched.clone()
                        }
                        key=|path| path.clone()
                        children=|path| view! { <li>{path}</li> }
                    />
                </ul>
            </Show>
            <span class="nr-migrate-message">
                "If 9Router is installed somewhere else, put its data directory in the field above \
                 and press Re-scan. That is the directory holding db/data.sqlite, or db.json on \
                 older installs — usually the value of 9Router's own DATA_DIR. If you never ran \
                 9Router, there is nothing to migrate and you can configure providers directly."
            </span>
        </div>
    }
}

/// The server declined the request.
#[component]
fn RefusedNotice(rejected: Rejected) -> impl IntoView {
    let code = rejected.error;
    let message = rejected.message;

    view! {
        <div class="nr-empty-state">
            <strong>"The import did not run"</strong>
            <span class="nr-migrate-message">
                {if message.is_empty() {
                    "The server declined the request without detail.".to_owned()
                } else {
                    message
                }}
            </span>
            <Show when={
                let code = code.clone();
                move || !code.is_empty()
            }>
                <span class="nr-migrate-status">{format!("Reported as: {code}")}</span>
            </Show>
        </div>
    }
}

/// The request itself did not produce a usable answer.
#[component]
fn FailureNotice(error: ApiError) -> impl IntoView {
    view! {
        <div class="nr-empty-state">
            <strong>"Could not read the 9Router source"</strong>
            <span class="nr-migrate-message">{error.message()}</span>
            <span class="nr-migrate-message">
                "Nothing was imported. No counts are shown because none were received — press \
                 Preview to try again."
            </span>
        </div>
    }
}

/// Placeholder rows while a scan is open.
///
/// Shaped like the table it replaces so the panel does not jump, and labelled so
/// a screen reader hears "loading" rather than silence.
#[component]
fn SkeletonRows() -> impl IntoView {
    view! {
        <div class="nr-migrate-skeletons" role="status" aria-label="Scanning for a 9Router installation">
            <span class="nr-skeleton nr-skeleton-text-short">"loading"</span>
            <span class="nr-skeleton nr-skeleton-row">"loading"</span>
            <span class="nr-skeleton nr-skeleton-row">"loading"</span>
            <span class="nr-skeleton nr-skeleton-row">"loading"</span>
            <span class="nr-skeleton nr-skeleton-row">"loading"</span>
        </div>
    }
}
