//! Proxy Pools: the outbound proxies this router holds.
//!
//! This panel used to render `proxy_pools_dashboard_state()`. Its `entries` were
//! always empty, so it drew an empty-state card — and then, directly underneath,
//! a fabricated row labelled "Cloudflare edge relay" pointing at
//! `cloudflare-relay.example.workers.dev`, tested "Jul 12, 2026, 09:10", with "2
//! bound". None of it existed. Below that sat five modal previews captioned
//! "Save disabled: /api/proxy-pools persistence is unavailable here", which was
//! also untrue: `GET/POST /api/proxy-pools` and `GET/PUT/DELETE
//! /api/proxy-pools/{id}` were all implemented.
//!
//! Now every row comes from `GET /api/proxy-pools?includeUsage=true` and nothing
//! else: `.nr-skeleton` while loading, an explicit failure with a retry, and an
//! invitation to add one when the router genuinely holds none. Create, edit,
//! activate, delete, test, batch import, and bulk actions all perform real
//! requests.
//!
//! Two things are still honestly unavailable, and say so rather than pretending:
//!
//! * **Relay deployment.** `/api/proxy-pools/{cloudflare,deno,vercel}-deploy`
//!   answer `501 unsupported` in this build, so the menu is inert and captioned.
//! * **Proxy testing.** The extra path segment routes `{id}/test` to
//!   `nullrouter-api`, which answers `501`. The action runs, and reports that
//!   nothing was tested — not that the proxy failed.
//!
//! Parsing, ordering, and rollback live in [`crate::dashboard::pools_live`], where
//! they are unit-tested on the native target. This file holds signals and markup.

use crate::api::{ApiError, Hydrate, Save};
use crate::dashboard::pools_live::{
    DeleteSettlement, ImportPlan, Pool, PoolDraft, PoolList, TestOutcome, ToggleSettlement,
    create_pool, delete_pool, load_pools, parse_import, plural, set_pool_active, test_pool,
    update_pool,
};
use leptos::prelude::*;

/// Panel styles, shared verbatim with the actix host.
///
/// The CSR build links no stylesheet of its own, so the same file the host serves
/// from `/assets/dashboard/panels-live.css` is inlined here. One source, two
/// delivery paths — the alternative was a second copy that would drift.
const PANELS_LIVE_STYLES: &str =
    include_str!("../../../../services/dashboard-actix/static/assets/dashboard/panels-live.css");

/// The class hooks and user-facing strings this route can render.
///
/// Asserted by `tests/proxy_pools_boundary.rs`. Every entry is still rendered by
/// the live panel below — some unconditionally (the title, the totals, the create
/// button), some only when the data calls for it (`"cloudflare relay"` needs a
/// Cloudflare pool; `"Last tested:"` needs a tested one). What changed in the
/// rewrite is where the strings get their values: `"Total:"` now counts rows the
/// router returned instead of a hardcoded `0`.
const VISIBLE_CONTRACT: [&str; 32] = [
    "nr-proxy-pools-panel",
    "nr-proxy-relay-menu",
    "nr-proxy-bulk-bar",
    "nr-proxy-empty",
    "nr-proxy-row",
    "nr-proxy-modal-grid",
    "Proxy Pools",
    "Deploy Relay",
    "Cloudflare Relay",
    "Vercel Relay",
    "Deno Relay",
    "Batch Import",
    "Add Proxy Pool",
    "Total:",
    "Active:",
    "Select all",
    "0 selected",
    "Health Check",
    "Checking 0/0",
    "No proxy pool entries yet",
    "Create a proxy pool entry, then assign it to connections.",
    "cloudflare relay",
    "No proxy:",
    "Last tested:",
    "Test proxy",
    "Edit",
    "Delete",
    "Batch Import Proxies",
    "Add/Edit Proxy Pool",
    "Deploy Vercel Relay",
    "Deploy Cloudflare Relay",
    "Deploy Deno Relay",
];

pub const fn proxy_pools_visible_contract() -> &'static [&'static str] {
    &VISIBLE_CONTRACT
}

/// One relay target upstream can deploy and this build cannot.
struct RelayTarget {
    label: &'static str,
    action: &'static str,
}

/// The three relay deployers, kept visible and inert.
///
/// `/api/proxy-pools/*-deploy` answers `501 unsupported` for all three
/// (`deploy_unsupported` in `services/api-actix/src/proxy_pool_tools.rs`). They are
/// listed so the page describes what upstream offers, and disabled so it does not
/// offer a button that cannot work. A relay URL can still be pasted into the form
/// as an ordinary proxy.
const RELAY_TARGETS: [RelayTarget; 3] = [
    RelayTarget {
        label: "Cloudflare Relay",
        action: "Deploy Cloudflare Relay",
    },
    RelayTarget {
        label: "Vercel Relay",
        action: "Deploy Vercel Relay",
    },
    RelayTarget {
        label: "Deno Relay",
        action: "Deploy Deno Relay",
    },
];

/// Everything this panel reads and writes.
///
/// One `Copy` struct so the row components take a single handle instead of ten
/// signals, and so a write always updates the list and its status together.
#[derive(Clone, Copy)]
struct PanelState {
    /// The configured pools, or why they could not be read.
    list: RwSignal<Hydrate<PoolList>>,
    /// The open create/edit form, if any. `None` means closed.
    draft: RwSignal<Option<PoolDraft>>,
    /// The pasted batch-import text.
    import_text: RwSignal<String>,
    /// Ids ticked for a bulk action.
    selected: RwSignal<Vec<String>>,
    /// Which row has an armed delete confirmation.
    ///
    /// One at a time: a page of primed destructive buttons is a trap.
    confirming: RwSignal<Option<String>>,
    /// Whether the bulk delete is armed.
    bulk_confirming: RwSignal<bool>,
    /// Row ids with a request in flight.
    busy: RwSignal<Vec<String>>,
    /// Per-row status text, announced politely.
    notes: RwSignal<Vec<(String, Note)>>,
    /// Panel-level status, for create, import, and bulk work.
    save: RwSignal<Save>,
    /// Health-check progress as `(finished, total)`.
    health: RwSignal<(usize, usize)>,
}

/// A result to announce on one row.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Note {
    text: String,
    tone: Tone,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tone {
    Ok,
    Error,
    Neutral,
}

impl Tone {
    const fn class_name(self) -> &'static str {
        match self {
            Self::Ok => "nr-live-status is-ok",
            Self::Error => "nr-live-status is-error",
            Self::Neutral => "nr-live-status",
        }
    }
}

impl PanelState {
    fn new() -> Self {
        Self {
            list: RwSignal::new(Hydrate::Loading),
            draft: RwSignal::new(None),
            import_text: RwSignal::new(String::new()),
            selected: RwSignal::new(Vec::new()),
            confirming: RwSignal::new(None),
            bulk_confirming: RwSignal::new(false),
            busy: RwSignal::new(Vec::new()),
            notes: RwSignal::new(Vec::new()),
            save: RwSignal::new(Save::Idle),
            health: RwSignal::new((0, 0)),
        }
    }

    fn is_busy(self, id: &str) -> bool {
        self.busy
            .with(|busy| busy.iter().any(|busy_id| busy_id == id))
    }

    fn set_busy(self, id: &str) {
        self.busy.update(|busy| {
            busy.retain(|busy_id| busy_id != id);
            busy.push(id.to_owned());
        });
    }

    fn clear_busy(self, id: &str) {
        self.busy
            .update(|busy| busy.retain(|busy_id| busy_id != id));
    }

    fn note(self, id: &str) -> Option<Note> {
        self.notes.with(|notes| {
            notes
                .iter()
                .find(|(note_id, _note)| note_id == id)
                .map(|(_id, note)| note.clone())
        })
    }

    fn set_note(self, id: &str, text: String, tone: Tone) {
        self.notes.update(|notes| {
            notes.retain(|(note_id, _note)| note_id != id);
            notes.push((id.to_owned(), Note { text, tone }));
        });
    }

    fn clear_note(self, id: &str) {
        self.notes
            .update(|notes| notes.retain(|(note_id, _note)| note_id != id));
    }

    fn is_selected(self, id: &str) -> bool {
        self.selected
            .with(|selected| selected.iter().any(|selected_id| selected_id == id))
    }

    fn toggle_selected(self, id: &str) {
        self.selected.update(|selected| {
            if let Some(index) = selected.iter().position(|selected_id| selected_id == id) {
                selected.remove(index);
            } else {
                selected.push(id.to_owned());
            }
        });
        // A selection that shrank cannot leave a primed bulk delete behind.
        self.bulk_confirming.set(false);
    }

    /// Selected ids that are still in the list.
    ///
    /// Filtered against the current rows so a reload that removed a pool cannot
    /// leave its id in a bulk action.
    fn live_selection(self) -> Vec<String> {
        let ids = self
            .list
            .with(|list| list.ready().map(PoolList::ids).unwrap_or_default());
        self.selected.with(|selected| {
            selected
                .iter()
                .filter(|id| ids.iter().any(|live| live == *id))
                .cloned()
                .collect()
        })
    }

    /// Drop ids that no longer exist, so counts match what is on screen.
    fn prune_selection(self) {
        let live = self.live_selection();
        self.selected.set(live);
    }
}

/// Spawn a task on the browser's executor.
#[cfg(target_arch = "wasm32")]
fn spawn<F: std::future::Future<Output = ()> + 'static>(task: F) {
    wasm_bindgen_futures::spawn_local(task);
}

/// Native builds have no executor and no browser to fetch from.
///
/// Dropping the future is the honest outcome: the panel stays in whatever state the
/// caller set before spawning (`Loading`, `Saving`), and no fabricated success
/// appears.
#[cfg(not(target_arch = "wasm32"))]
fn spawn<F: std::future::Future<Output = ()> + 'static>(task: F) {
    drop(task);
}

/// Load, or reload, the pool list.
///
/// `reset` is false for a refresh after a write: the rows already on screen stay
/// put rather than flashing back to skeletons.
fn reload(state: PanelState, reset: bool) {
    if reset {
        state.list.set(Hydrate::Loading);
    }
    spawn(async move {
        let next = load_pools()
            .await
            .map_or_else(Hydrate::Failed, Hydrate::Ready);
        state.list.set(next);
        state.prune_selection();
    });
}

/// Delete one pool: remove it now, put it back if the router refuses.
fn dispatch_delete(state: PanelState, id: String) {
    // Taking the row out of the list *is* the optimistic update. If the id is gone
    // (a refresh landed first) there is nothing to delete and nothing to roll back.
    let Some(pending) = state
        .list
        .try_update(|list| match list {
            Hydrate::Ready(ready) => ready.take(&id),
            Hydrate::Loading | Hydrate::Failed(_) => None,
        })
        .flatten()
    else {
        return;
    };

    state.confirming.set(None);
    state.clear_note(&id);
    state.set_busy(&id);
    state.save.set(Save::Saving);

    spawn(async move {
        let outcome = delete_pool(&id).await;
        state.clear_busy(&id);
        match crate::dashboard::pools_live::settle_delete(pending, outcome) {
            DeleteSettlement::Removed => {
                state.save.set(Save::Saved);
                state.clear_note(&id);
                state.prune_selection();
            }
            DeleteSettlement::RolledBack {
                pending,
                error,
                message,
            } => {
                state.list.update(|list| {
                    if let Hydrate::Ready(ready) = list {
                        ready.restore(pending);
                    }
                });
                state.save.set(Save::Failed(error));
                state.set_note(&id, message, Tone::Error);
            }
        }
    });
}

/// Flip one pool's routing flag now, and put it back if the `PUT` is refused.
fn dispatch_toggle(state: PanelState, id: String, name: String, next_active: bool) {
    let Some(_previous) = state
        .list
        .try_update(|list| match list {
            Hydrate::Ready(ready) => ready.set_active(&id, next_active),
            Hydrate::Loading | Hydrate::Failed(_) => None,
        })
        .flatten()
    else {
        return;
    };

    state.set_busy(&id);
    state.clear_note(&id);

    spawn(async move {
        let settlement = set_pool_active(&name, &id, next_active).await;
        state.clear_busy(&id);
        match settlement {
            ToggleSettlement::Applied(pool) => {
                // Take the router's row, not the optimistic one: it also carries the
                // new `updatedAt`.
                state.list.update(|list| {
                    if let Hydrate::Ready(ready) = list {
                        ready.upsert(*pool);
                    }
                });
                state.save.set(Save::Saved);
            }
            ToggleSettlement::RolledBack {
                previous,
                error,
                message,
            } => {
                state.list.update(|list| {
                    if let Hydrate::Ready(ready) = list {
                        ready.set_active(&id, previous);
                    }
                });
                state.save.set(Save::Failed(error));
                state.set_note(&id, message, Tone::Error);
            }
        }
    });
}

/// Test one pool and record what came back.
fn dispatch_test(state: PanelState, id: String) {
    state.set_busy(&id);
    state.set_note(&id, String::from("Testing proxy…"), Tone::Neutral);

    spawn(async move {
        let outcome = test_pool(&id).await;
        state.clear_busy(&id);
        let tone = match outcome {
            TestOutcome::Passed { .. } => Tone::Ok,
            TestOutcome::Failed(_) | TestOutcome::Rejected(_) => Tone::Error,
            // Nothing was tested, so this is neither good nor bad news.
            TestOutcome::Unsupported => Tone::Neutral,
        };
        // Only a real verdict updates the row's recorded status.
        if let Some(status) = outcome.recorded_status() {
            let error = outcome.recorded_error();
            state.list.update(|list| {
                if let Hydrate::Ready(ready) = list {
                    ready.set_test_result(&id, Some(status.to_owned()), error);
                }
            });
        }
        state.set_note(&id, outcome.message(), tone);
    });
}

/// Save the open draft: `POST` for a new pool, `PUT` for an edit.
fn dispatch_save(state: PanelState) {
    let Some(draft) = state.draft.get_untracked() else {
        return;
    };
    let body = match draft.body() {
        Ok(body) => body,
        // The form already shows this; a click on a disabled control cannot get
        // here, so there is nothing further to report.
        Err(_error) => return,
    };
    state.save.set(Save::Saving);

    spawn(async move {
        let result = match draft.id.as_deref() {
            Some(id) => update_pool(id, body).await,
            None => create_pool(body).await,
        };
        match result {
            Ok(pool) => {
                // Show the row the router just confirmed, then re-read the list so
                // ordering and any server-applied defaults are the server's.
                state.list.update(|list| {
                    if let Hydrate::Ready(ready) = list {
                        ready.upsert(pool);
                    }
                });
                state.draft.set(None);
                state.save.set(Save::Saved);
                reload(state, false);
            }
            Err(error) => state.save.set(Save::Failed(error)),
        }
    });
}

/// Create every parsed line from the batch-import box.
///
/// One `POST` per entry, because the endpoint takes one pool at a time. Failures
/// are counted rather than aborting the run: with twenty pasted proxies, stopping
/// at the first rejection would leave the user unable to tell which ones landed.
fn dispatch_import(state: PanelState) {
    let plan = parse_import(&state.import_text.get_untracked());
    if plan.is_empty() {
        return;
    }
    state.save.set(Save::Saving);

    spawn(async move {
        let total = plan.drafts.len();
        let mut created = 0_usize;
        let mut failure: Option<ApiError> = None;
        for draft in plan.drafts {
            let Ok(body) = draft.body() else {
                continue;
            };
            match create_pool(body).await {
                Ok(pool) => {
                    created += 1;
                    state.list.update(|list| {
                        if let Hydrate::Ready(ready) = list {
                            ready.upsert(pool);
                        }
                    });
                }
                Err(error) => failure = Some(error),
            }
        }
        // The panel-level status names both halves: how many landed, and that some
        // did not. A bare "Saved" after 3 of 20 would be a lie.
        match failure {
            // Nothing landed: the panel status is the transport error itself.
            Some(error) if created == 0 => state.save.set(Save::Failed(error)),
            // Some landed. Neither "Saved" nor a single error describes that, so the
            // panel status goes quiet and the count below carries the outcome.
            Some(_error) => state.save.set(Save::Idle),
            None => state.save.set(Save::Saved),
        }
        state.import_text.set(String::new());
        state.set_note(
            IMPORT_STATUS_KEY,
            format!(
                "Imported {} of {}.",
                created,
                plural(total, "pasted proxy entry"),
            ),
            if created == total {
                Tone::Ok
            } else {
                Tone::Error
            },
        );
        reload(state, false);
    });
}

/// Status key for the import card's live region.
///
/// Notes are keyed by pool id; the import card has no id of its own, so it uses a
/// reserved key that no pool can collide with (ids are `proxy_pool_<millis>_<n>`).
const IMPORT_STATUS_KEY: &str = "\0import";

/// Status key for the bulk bar's live region.
const BULK_STATUS_KEY: &str = "\0bulk";

/// Activate or deactivate every selected pool.
fn dispatch_bulk_active(state: PanelState, next_active: bool) {
    let ids = state.live_selection();
    if ids.is_empty() {
        return;
    }
    state.save.set(Save::Saving);
    state.set_note(
        BULK_STATUS_KEY,
        format!("Updating {}…", plural(ids.len(), "proxy pool"),),
        Tone::Neutral,
    );

    spawn(async move {
        let total = ids.len();
        let mut applied = 0_usize;
        for id in ids {
            let name = state.list.with_untracked(|list| {
                list.ready()
                    .and_then(|ready| ready.get(&id))
                    .map_or_else(String::new, |pool| pool.name.clone())
            });
            state.set_busy(&id);
            let settlement = set_pool_active(&name, &id, next_active).await;
            state.clear_busy(&id);
            match settlement {
                ToggleSettlement::Applied(pool) => {
                    applied += 1;
                    state.list.update(|list| {
                        if let Hydrate::Ready(ready) = list {
                            ready.upsert(*pool);
                        }
                    });
                }
                ToggleSettlement::RolledBack { message, .. } => {
                    state.set_note(&id, message, Tone::Error);
                }
            }
        }
        report_bulk(state, "Updated", applied, total);
    });
}

/// Delete every selected pool.
///
/// Each one goes through the same optimistic-remove-and-settle path as a single
/// delete, so a pool the router refuses to drop (still bound to a connection)
/// reappears with its own explanation while the rest stay gone.
fn dispatch_bulk_delete(state: PanelState) {
    let ids = state.live_selection();
    if ids.is_empty() {
        return;
    }
    state.bulk_confirming.set(false);
    state.save.set(Save::Saving);
    state.set_note(
        BULK_STATUS_KEY,
        format!("Deleting {}…", plural(ids.len(), "proxy pool")),
        Tone::Neutral,
    );

    spawn(async move {
        let total = ids.len();
        let mut removed = 0_usize;
        for id in ids {
            let Some(pending) = state
                .list
                .try_update(|list| match list {
                    Hydrate::Ready(ready) => ready.take(&id),
                    Hydrate::Loading | Hydrate::Failed(_) => None,
                })
                .flatten()
            else {
                continue;
            };
            state.set_busy(&id);
            let outcome = delete_pool(&id).await;
            state.clear_busy(&id);
            match crate::dashboard::pools_live::settle_delete(pending, outcome) {
                DeleteSettlement::Removed => removed += 1,
                DeleteSettlement::RolledBack {
                    pending, message, ..
                } => {
                    state.list.update(|list| {
                        if let Hydrate::Ready(ready) = list {
                            ready.restore(pending);
                        }
                    });
                    state.set_note(&id, message, Tone::Error);
                }
            }
        }
        report_bulk(state, "Deleted", removed, total);
        state.prune_selection();
    });
}

/// Test every selected pool, reporting progress as it goes.
fn dispatch_health_check(state: PanelState) {
    let ids = state.live_selection();
    if ids.is_empty() {
        return;
    }
    let total = ids.len();
    state.health.set((0, total));

    spawn(async move {
        for (index, id) in ids.into_iter().enumerate() {
            state.set_busy(&id);
            let outcome = test_pool(&id).await;
            state.clear_busy(&id);
            if let Some(status) = outcome.recorded_status() {
                let error = outcome.recorded_error();
                state.list.update(|list| {
                    if let Hydrate::Ready(ready) = list {
                        ready.set_test_result(&id, Some(status.to_owned()), error);
                    }
                });
            }
            let tone = match outcome {
                TestOutcome::Passed { .. } => Tone::Ok,
                TestOutcome::Failed(_) | TestOutcome::Rejected(_) => Tone::Error,
                TestOutcome::Unsupported => Tone::Neutral,
            };
            state.set_note(&id, outcome.message(), tone);
            state.health.set((index + 1, total));
        }
        // Back to idle: a finished run must not leave "Checking 4/4" on screen as if
        // it were still working.
        state.health.set((0, 0));
    });
}

/// Report how a bulk run ended, naming the shortfall when there was one.
fn report_bulk(state: PanelState, verb: &str, done: usize, total: usize) {
    if done == total {
        state.save.set(Save::Saved);
        state.set_note(
            BULK_STATUS_KEY,
            format!("{verb} {}.", plural(total, "proxy pool")),
            Tone::Ok,
        );
    } else {
        // A partial run is neither "Saved" nor one error. The panel status goes quiet
        // and this line carries the shortfall; each refused row explains itself.
        state.save.set(Save::Idle);
        state.set_note(
            BULK_STATUS_KEY,
            format!("{verb} {done} of {total}. The rows that did not change say why."),
            Tone::Error,
        );
    }
}

#[component]
pub(super) fn ProxyPoolsPanel() -> impl IntoView {
    let state = PanelState::new();
    // Land on the page already knowing what is configured: a user should not have to
    // press anything to find out.
    reload(state, true);

    view! {
        <style>{PANELS_LIVE_STYLES}</style>
        <div class="nr-panel-stack nr-proxy-pools-panel">
            <PoolsCard state />
            <Show when=move || state.draft.with(Option::is_some)>
                <PoolForm state />
            </Show>
            <ImportCard state />
        </div>
    }
}

/// The configured pools, in whichever of its four states applies.
#[component]
fn PoolsCard(state: PanelState) -> impl IntoView {
    let totals = move || {
        state.list.with(|list| {
            list.ready()
                .map(|ready| (ready.len(), ready.active_count()))
        })
    };

    view! {
        <article class="nr-card nr-anim-rise">
            <div class="nr-card-head between">
                <div>
                    <p class="nr-eyebrow">"Proxy routing"</p>
                    <h2>"Proxy Pools"</h2>
                    <p>
                        "Outbound proxies this router holds, read from the local state service. \
                         Assign one to a provider connection to route that upstream through it."
                    </p>
                </div>
                <div class="nr-live-actions">
                    <Show when=move || state.list.with(Hydrate::is_loading)>
                        <span class="nr-spinner" aria-hidden="true"></span>
                    </Show>
                    <button
                        type="button"
                        class="nr-button secondary small"
                        disabled=move || state.list.with(Hydrate::is_loading)
                        on:click=move |_event| reload(state, true)
                    >
                        "Refresh"
                    </button>
                    <a class="nr-button secondary small" href="#nr-pool-import">"Batch Import"</a>
                    <button
                        type="button"
                        class="nr-button primary small"
                        on:click=move |_event| state.draft.set(Some(PoolDraft::default()))
                    >
                        "Add Proxy Pool"
                    </button>
                </div>
            </div>

            <p class="nr-live-status" role="status" aria-live="polite">
                {move || {
                    totals()
                        .map(|(total, active)| {
                            format!("{} configured, {active} active.", plural(total, "proxy pool"))
                        })
                        .or_else(|| state.save.with(|save| save.status().map(str::to_owned)))
                }}
            </p>

            <Show when=move || totals().is_some()>
                <PoolsToolbar state />
            </Show>

            {move || match state.list.get() {
                Hydrate::Loading => view! { <PoolsSkeleton /> }.into_any(),
                Hydrate::Failed(error) => view! { <PoolsFailure state error /> }.into_any(),
                Hydrate::Ready(ready) if ready.is_empty() => view! { <PoolsEmpty state /> }.into_any(),
                Hydrate::Ready(ready) => view! { <PoolRows state pools=ready.pools().to_vec() /> }
                    .into_any(),
            }}

            <RelayMenu />
        </article>
    }
}

/// Totals and selection, above the rows.
///
/// Rendered only once the list has loaded: a "Total: 0" beside a skeleton would be
/// a count of rows nobody has read yet.
#[component]
fn PoolsToolbar(state: PanelState) -> impl IntoView {
    let total = move || {
        state
            .list
            .with(|list| list.ready().map_or(0, PoolList::len))
    };
    let active = move || {
        state
            .list
            .with(|list| list.ready().map_or(0, PoolList::active_count))
    };
    let selected_count = move || state.selected.with(Vec::len);
    let all_selected = move || total() != 0 && selected_count() == total();

    view! {
        <div class="nr-live-toolbar">
            <div class="nr-live-totals">
                <label class="nr-live-check">
                    <input
                        type="checkbox"
                        prop:checked=all_selected
                        disabled=move || total() == 0
                        on:change=move |_event| {
                            if all_selected() {
                                state.selected.set(Vec::new());
                            } else {
                                let ids = state
                                    .list
                                    .with(|list| {
                                        list.ready().map(PoolList::ids).unwrap_or_default()
                                    });
                                state.selected.set(ids);
                            }
                            state.bulk_confirming.set(false);
                        }
                    />
                    "Select all"
                </label>
                <span class="nr-status-pill is-idle">
                    <span></span>"Total: "{move || total().to_string()}
                </span>
                <span class="nr-status-pill is-connected">
                    <span></span>"Active: "{move || active().to_string()}
                </span>
            </div>
            <Show when=move || selected_count() != 0>
                <BulkBar state />
            </Show>
        </div>
    }
}

/// Actions for the current selection.
///
/// Present only when something is selected, so this is never a row of disabled
/// buttons implying capability the panel does not have.
#[component]
fn BulkBar(state: PanelState) -> impl IntoView {
    let selected_count = move || state.selected.with(Vec::len);
    let health = move || state.health.get();
    let checking = move || health().1 != 0;
    let note = Memo::new(move |_previous| state.note(BULK_STATUS_KEY));

    view! {
        <div class="nr-proxy-bulk-bar nr-live-bulk-bar">
            <span class="nr-status-pill is-idle">
                <span></span>{move || format!("{} selected", selected_count())}
            </span>
            <div class="nr-live-bulk-actions">
                <button
                    type="button"
                    class="nr-button primary small"
                    disabled=checking
                    aria-label="Run a proxy test on every selected pool"
                    on:click=move |_event| dispatch_health_check(state)
                >
                    <Show when=checking>
                        <span class="nr-spinner" aria-hidden="true"></span>
                    </Show>
                    "Health Check"
                </button>
                <span class="nr-status-pill is-idle" role="status" aria-live="polite">
                    <span></span>
                    {move || {
                        let (done, total) = health();
                        format!("Checking {done}/{total}")
                    }}
                </span>
                <button
                    type="button"
                    class="nr-button secondary small"
                    on:click=move |_event| dispatch_bulk_active(state, true)
                >
                    "Activate"
                </button>
                <button
                    type="button"
                    class="nr-button secondary small"
                    on:click=move |_event| dispatch_bulk_active(state, false)
                >
                    "Deactivate"
                </button>
                <Show
                    when=move || state.bulk_confirming.get()
                    fallback=move || {
                        view! {
                            <button
                                type="button"
                                class="nr-button danger small"
                                aria-label="Delete every selected proxy pool"
                                on:click=move |_event| state.bulk_confirming.set(true)
                            >
                                "Delete"
                            </button>
                        }
                    }
                >
                    <button
                        type="button"
                        class="nr-button danger small"
                        on:click=move |_event| dispatch_bulk_delete(state)
                    >
                        {move || format!("Delete {} permanently", plural(selected_count(), "pool"))}
                    </button>
                    <button
                        type="button"
                        class="nr-button secondary small"
                        on:click=move |_event| state.bulk_confirming.set(false)
                    >
                        "Keep them"
                    </button>
                </Show>
                <button
                    type="button"
                    class="nr-button secondary small"
                    on:click=move |_event| {
                        state.selected.set(Vec::new());
                        state.bulk_confirming.set(false);
                    }
                >
                    "Clear"
                </button>
            </div>
            <p
                class=move || {
                    note.with(|note| {
                        note.as_ref().map_or_else(
                            || String::from("nr-live-status"),
                            |note| note.tone.class_name().to_owned(),
                        )
                    })
                }
                role="status"
                aria-live="polite"
            >
                {move || note.with(|note| note.as_ref().map(|note| note.text.clone()))}
            </p>
        </div>
    }
}

/// Relay deployers upstream offers and this build does not run.
///
/// Listed rather than hidden so the page describes the full surface, and disabled
/// rather than clickable because every one of these endpoints answers `501`.
#[component]
fn RelayMenu() -> impl IntoView {
    view! {
        <section aria-labelledby="nr-relay-heading">
            <div class="nr-card-head">
                <div>
                    <h3 id="nr-relay-heading">"Deploy Relay"</h3>
                    <p>
                        "Upstream can deploy an edge relay and add it as a pool. This build answers \
                         501 for all three deployers, so they are listed and inactive. A relay you \
                         deploy yourself works as an ordinary proxy URL above."
                    </p>
                </div>
            </div>
            <div class="nr-proxy-relay-menu nr-relay-menu">
                {RELAY_TARGETS
                    .iter()
                    .map(|target| {
                        view! {
                            <span class="nr-relay-item" title=target.action>
                                <span>{target.label}</span>
                                <small>"Not available in this build"</small>
                            </span>
                        }
                    })
                    .collect_view()}
            </div>
        </section>
    }
}

/// Placeholder rows, labelled so the wait is announced rather than only shown.
#[component]
fn PoolsSkeleton() -> impl IntoView {
    view! {
        <div class="nr-pool-list" role="status" aria-label="Loading your proxy pools">
            {(0..2)
                .map(|_index| {
                    view! {
                        <div class="nr-pool-skeleton" aria-hidden="true">
                            <span class="nr-skeleton nr-skeleton-text-short"></span>
                            <span class="nr-skeleton nr-skeleton-text"></span>
                            <span class="nr-skeleton nr-skeleton-row"></span>
                        </div>
                    }
                })
                .collect_view()}
        </div>
    }
}

/// The request failed. Say so, and offer the only useful action.
#[component]
fn PoolsFailure(state: PanelState, error: ApiError) -> impl IntoView {
    view! {
        <div class="nr-panel-notice is-error" role="alert">
            <strong>"Could not read your proxy pools"</strong>
            <span>
                {error.message()}
                " Nothing is listed below, because this page cannot tell whether you have proxy \
                 pools configured."
            </span>
            <button
                type="button"
                class="nr-button secondary small"
                on:click=move |_event| reload(state, true)
            >
                "Try again"
            </button>
        </div>
    }
}

/// The router holds nothing. The one state the old panel could not express.
#[component]
fn PoolsEmpty(state: PanelState) -> impl IntoView {
    view! {
        <div class="nr-proxy-empty nr-panel-notice">
            <strong>"No proxy pool entries yet"</strong>
            <span>"Create a proxy pool entry, then assign it to connections."</span>
            <button
                type="button"
                class="nr-button primary small"
                on:click=move |_event| state.draft.set(Some(PoolDraft::default()))
            >
                "Add Proxy Pool"
            </button>
        </div>
    }
}

/// The rows, in list order.
#[component]
fn PoolRows(state: PanelState, pools: Vec<Pool>) -> impl IntoView {
    view! {
        <div class="nr-pool-list nr-stagger" role="list" aria-label="Configured proxy pools">
            <For
                each=move || pools.clone()
                key=|pool| {
                    // Keyed on the fields the row renders, so a toggle or a test
                    // result re-renders the row instead of leaving stale text.
                    (
                        pool.id.clone(),
                        pool.is_active,
                        pool.test_status.clone(),
                        pool.last_error.clone(),
                    )
                }
                children=move |pool| view! { <PoolRow state pool /> }
            />
        </div>
    }
}

/// One pool: what it is, what the router last learned about it, and the four things
/// you can do to it.
#[component]
#[allow(
    clippy::too_many_lines,
    reason = "one row view: the markup is flat and splitting it would hide the row's shape"
)]
fn PoolRow(state: PanelState, pool: Pool) -> impl IntoView {
    let id = pool.id.clone();
    let heading_id = pool.heading_id();
    let labelled_by = heading_id.clone();
    let status_id = pool.status_id();
    let status = pool.status();
    let badge = pool.kind().badge_label().map(str::to_owned);
    let bound = pool.bound_label();
    let no_proxy = pool.no_proxy_label().map(str::to_owned);
    let last_tested = pool.last_tested_label();
    let last_error = pool.last_error.clone();
    let strict = pool.strict_label();
    let active = pool.active_label();
    let is_active = pool.is_active;
    let toggle_text = pool.toggle_text();
    let toggle_aria = pool.toggle_label();
    let test_aria = pool.test_label();
    let edit_aria = pool.edit_label();
    let delete_aria = pool.delete_label();
    let name = pool.name.clone();
    let url = pool.proxy_url.clone();
    let select_label = format!("Select proxy pool {}", pool.name);
    let edit_source = pool.clone();

    let busy = {
        let id = id.clone();
        Memo::new(move |_previous| state.is_busy(&id))
    };
    let confirming = {
        let id = id.clone();
        Memo::new(move |_previous| {
            state
                .confirming
                .with(|target| target.as_deref() == Some(id.as_str()))
        })
    };
    let note = {
        let id = id.clone();
        Memo::new(move |_previous| state.note(&id))
    };
    let selected = {
        let id = id.clone();
        Memo::new(move |_previous| state.is_selected(&id))
    };

    let select_id = id.clone();
    let toggle_id = id.clone();
    let toggle_name = name.clone();
    let test_id = id.clone();
    let arm_id = id.clone();
    let confirm_id = id.clone();
    let confirm_name = name.clone();

    view! {
        <article
            class="nr-proxy-row nr-pool-row"
            class:is-inactive=move || !is_active
            class:is-busy=move || busy.get()
            role="listitem"
            aria-labelledby=labelled_by
        >
            <label class="nr-live-check">
                <input
                    type="checkbox"
                    aria-label=select_label
                    prop:checked=move || selected.get()
                    on:change=move |_event| state.toggle_selected(&select_id)
                />
            </label>

            <div class="nr-pool-copy">
                <div class="nr-pool-badges">
                    <h4 id=heading_id>{name.clone()}</h4>
                    <span class=format!("nr-status-pill {}", status.class_name())>
                        <span></span>{status.label()}
                    </span>
                    <span class=if is_active {
                        "nr-status-pill is-connected"
                    } else {
                        "nr-status-pill is-idle"
                    }>
                        <span></span>{active}
                    </span>
                    {badge
                        .map(|label| {
                            view! {
                                <span class="nr-status-pill is-idle"><span></span>{label}</span>
                            }
                        })}
                    {bound
                        .map(|label| {
                            view! {
                                <span class="nr-status-pill is-idle"><span></span>{label}</span>
                            }
                        })}
                    <span class="nr-status-pill is-idle"><span></span>{strict}</span>
                </div>
                <code>{url}</code>
                {no_proxy.map(|value| view! { <small>"No proxy: "{value}</small> })}
                <small>"Last tested: "{last_tested}</small>
                {last_error
                    .map(|error| view! { <p class="nr-pool-error">"Last error: "{error}</p> })}
            </div>

            <div class="nr-live-actions">
                <Show
                    when=move || confirming.get()
                    fallback=move || {
                        let toggle_id = toggle_id.clone();
                        let toggle_name = toggle_name.clone();
                        let toggle_aria = toggle_aria.clone();
                        let test_id = test_id.clone();
                        let test_aria = test_aria.clone();
                        let edit_source = edit_source.clone();
                        let edit_aria = edit_aria.clone();
                        let arm_id = arm_id.clone();
                        let delete_aria = delete_aria.clone();
                        view! {
                            <button
                                type="button"
                                class="nr-button secondary small"
                                aria-label=toggle_aria
                                disabled=move || busy.get()
                                on:click={
                                    let toggle_id = toggle_id.clone();
                                    let toggle_name = toggle_name.clone();
                                    move |_event| {
                                        dispatch_toggle(
                                            state,
                                            toggle_id.clone(),
                                            toggle_name.clone(),
                                            !is_active,
                                        );
                                    }
                                }
                            >
                                {toggle_text}
                            </button>
                            <button
                                type="button"
                                class="nr-button secondary small"
                                aria-label=test_aria
                                disabled=move || busy.get()
                                on:click={
                                    let test_id = test_id.clone();
                                    move |_event| dispatch_test(state, test_id.clone())
                                }
                            >
                                <Show when=move || busy.get()>
                                    <span class="nr-spinner" aria-hidden="true"></span>
                                </Show>
                                "Test proxy"
                            </button>
                            <button
                                type="button"
                                class="nr-button secondary small"
                                aria-label=edit_aria
                                on:click={
                                    let edit_source = edit_source.clone();
                                    move |_event| {
                                        state.draft.set(Some(PoolDraft::for_edit(&edit_source)));
                                    }
                                }
                            >
                                "Edit"
                            </button>
                            <button
                                type="button"
                                class="nr-button danger small"
                                aria-label=delete_aria
                                disabled=move || busy.get()
                                on:click={
                                    let arm_id = arm_id.clone();
                                    move |_event| state.confirming.set(Some(arm_id.clone()))
                                }
                            >
                                "Delete"
                            </button>
                        }
                    }
                >
                    <PoolDeleteConfirm
                        state
                        id=confirm_id.clone()
                        name=confirm_name.clone()
                    />
                </Show>
            </div>

            <p
                id=status_id
                class=move || {
                    note.with(|note| {
                        note.as_ref().map_or_else(
                            || String::from("nr-live-status"),
                            |note| note.tone.class_name().to_owned(),
                        )
                    })
                }
                role="status"
                aria-live="polite"
            >
                {move || note.with(|note| note.as_ref().map(|note| note.text.clone()))}
            </p>
        </article>
    }
}

/// The armed state of a row's delete action.
///
/// Deleting a pool is irreversible from here, so the first press only ever gets you
/// this far, and the confirming button says what it will do.
#[component]
fn PoolDeleteConfirm(state: PanelState, id: String, name: String) -> impl IntoView {
    view! {
        <div class="nr-connection-confirm" role="group" aria-label="Confirm deletion">
            <p>
                "Delete "<strong>{name}</strong>
                "? Connections using this proxy will fall back to their own settings. This cannot \
                 be undone from the dashboard."
            </p>
            <div class="nr-live-actions">
                <button
                    type="button"
                    class="nr-button danger small"
                    on:click=move |_event| dispatch_delete(state, id.clone())
                >
                    "Delete permanently"
                </button>
                <button
                    type="button"
                    class="nr-button secondary small"
                    on:click=move |_event| state.confirming.set(None)
                >
                    "Keep it"
                </button>
            </div>
        </div>
    }
}

/// The proxy kinds the endpoint accepts as `type`.
///
/// `normalize_proxy_type` coerces anything else to `http`, so offering a fifth
/// option would silently store something different from what was picked.
const POOL_KINDS: [(&str, &str); 4] = [
    ("http", "Direct proxy (HTTP, HTTPS, or SOCKS URL)"),
    ("cloudflare", "Cloudflare Worker relay"),
    ("vercel", "Vercel edge relay"),
    ("deno", "Deno Deploy relay"),
];

/// Create or edit one pool.
///
/// The same fields for both, because `POST /api/proxy-pools` and `PUT
/// /api/proxy-pools/{id}` take the same body. This used to be a disabled preview
/// card captioned "Save disabled: /api/proxy-pools persistence is unavailable here";
/// both endpoints were live at the time.
#[component]
#[allow(
    clippy::too_many_lines,
    reason = "one form: six controls and their help text, flat by nature"
)]
fn PoolForm(state: PanelState) -> impl IntoView {
    let draft = move || state.draft.get().unwrap_or_default();
    let is_edit = move || {
        state
            .draft
            .with(|draft| draft.as_ref().is_some_and(PoolDraft::is_edit))
    };
    let blocking = move || {
        state
            .draft
            .with(|draft| draft.as_ref().and_then(PoolDraft::validation_error))
    };
    let saving = move || state.save.with(Save::is_saving);
    let update = move |apply: &dyn Fn(&mut PoolDraft)| {
        state.draft.update(|draft| {
            if let Some(draft) = draft.as_mut() {
                apply(draft);
            }
        });
    };

    view! {
        <article class="nr-card nr-anim-rise" id="nr-pool-form">
            <div class="nr-card-head between">
                <div>
                    <h2><span class="nr-card-icon">"pxy"</span>"Add/Edit Proxy Pool"</h2>
                    <p>
                        {move || {
                            if is_edit() {
                                "Changes are written to the local state service when you save. \
                                 Fields you leave alone are not sent."
                            } else {
                                "Stored by the local state service. A pool does nothing until you \
                                 assign it to a provider connection."
                            }
                        }}
                    </p>
                </div>
                <button
                    type="button"
                    class="nr-button secondary small"
                    on:click=move |_event| state.draft.set(None)
                >
                    "Cancel"
                </button>
            </div>

            <div class="nr-live-form nr-proxy-modal-grid">
                <div class="nr-live-form-grid">
                    <div class="nr-live-field">
                        <label for="nr-pool-name">"Name"</label>
                        <input
                            id="nr-pool-name"
                            class="nr-preview-input"
                            type="text"
                            autocomplete="off"
                            placeholder="Office Proxy"
                            disabled=saving
                            prop:value=move || draft().name
                            on:input=move |event| {
                                let value = event_target_value(&event);
                                update(&|draft: &mut PoolDraft| draft.name.clone_from(&value));
                            }
                        />
                        <small>"Display name for this proxy pool entry."</small>
                    </div>
                    <div class="nr-live-field">
                        <label for="nr-pool-kind">"Type"</label>
                        <select
                            id="nr-pool-kind"
                            class="nr-preview-input"
                            disabled=saving
                            on:change=move |event| {
                                let value = event_target_value(&event);
                                update(&|draft: &mut PoolDraft| draft.kind.clone_from(&value));
                            }
                        >
                            {POOL_KINDS
                                .iter()
                                .map(|(value, label)| {
                                    view! {
                                        <option
                                            value=*value
                                            selected=move || draft().kind == *value
                                        >
                                            {*label}
                                        </option>
                                    }
                                })
                                .collect_view()}
                        </select>
                        <small>"How the router labels this pool. It does not rewrite the URL."</small>
                    </div>
                </div>

                <div class="nr-live-field">
                    <label for="nr-pool-url">"Proxy URL"</label>
                    <input
                        id="nr-pool-url"
                        class="nr-preview-input"
                        type="text"
                        autocomplete="off"
                        spellcheck="false"
                        placeholder="http://127.0.0.1:7897"
                        disabled=saving
                        prop:value=move || draft().proxy_url
                        on:input=move |event| {
                            let value = event_target_value(&event);
                            update(&|draft: &mut PoolDraft| draft.proxy_url.clone_from(&value));
                        }
                    />
                    <small>
                        {move || {
                            state
                                .draft
                                .with(|draft| draft.as_ref().and_then(PoolDraft::url_hint))
                                .unwrap_or(
                                    "HTTP, HTTPS, SOCKS, or relay URL used for upstream requests. \
                                     Credentials in the URL are stored as written.",
                                )
                        }}
                    </small>
                </div>

                <div class="nr-live-field">
                    <label for="nr-pool-no-proxy">"No Proxy"</label>
                    <input
                        id="nr-pool-no-proxy"
                        class="nr-preview-input"
                        type="text"
                        autocomplete="off"
                        placeholder="localhost,127.0.0.1,.internal"
                        disabled=saving
                        prop:value=move || draft().no_proxy
                        on:input=move |event| {
                            let value = event_target_value(&event);
                            update(&|draft: &mut PoolDraft| draft.no_proxy.clone_from(&value));
                        }
                    />
                    <small>"Comma-separated hosts and domains to reach directly."</small>
                </div>

                <div class="nr-live-form-grid">
                    <label class="nr-live-switch">
                        <input
                            type="checkbox"
                            disabled=saving
                            prop:checked=move || draft().is_active
                            on:change=move |event| {
                                let checked = event_target_checked(&event);
                                update(&|draft: &mut PoolDraft| draft.is_active = checked);
                            }
                        />
                        <span>
                            <strong>"Active"</strong>
                            <small>"Inactive pools are ignored when the router resolves a proxy."</small>
                        </span>
                    </label>
                    <label class="nr-live-switch">
                        <input
                            type="checkbox"
                            disabled=saving
                            prop:checked=move || draft().strict_proxy
                            on:change=move |event| {
                                let checked = event_target_checked(&event);
                                update(&|draft: &mut PoolDraft| draft.strict_proxy = checked);
                            }
                        />
                        <span>
                            <strong>"Strict Proxy"</strong>
                            <small>"Fail the request if the proxy is unreachable, instead of going direct."</small>
                        </span>
                    </label>
                </div>

                <div class="nr-live-actions">
                    <span class="nr-form-error" role="status" aria-live="polite">
                        {move || {
                            blocking()
                                .map(|error| error.message().to_owned())
                                .or_else(|| {
                                    state.save.with(|save| save.status().map(str::to_owned))
                                })
                        }}
                    </span>
                    <button
                        type="button"
                        class="nr-button primary small"
                        disabled=move || blocking().is_some() || saving()
                        on:click=move |_event| dispatch_save(state)
                    >
                        <Show when=saving>
                            <span class="nr-spinner" aria-hidden="true"></span>
                        </Show>
                        {move || if is_edit() { "Save changes" } else { "Create proxy pool" }}
                    </button>
                </div>
            </div>
        </article>
    }
}

/// Paste a proxy list and create every line.
///
/// The plan is shown before anything is sent, including the lines that could not be
/// read and their line numbers. Upstream's import dropped those silently.
#[component]
fn ImportCard(state: PanelState) -> impl IntoView {
    let plan = Memo::new(move |_previous| parse_import(&state.import_text.get()));
    let saving = move || state.save.with(Save::is_saving);
    let note = Memo::new(move |_previous| state.note(IMPORT_STATUS_KEY));

    view! {
        <article class="nr-card nr-anim-rise" id="nr-pool-import">
            <div class="nr-card-head">
                <div>
                    <h2><span class="nr-card-icon">"imp"</span>"Batch Import Proxies"</h2>
                    <p>
                        "One proxy per line. Each line becomes a pool named after its host and \
                         port, created inactive-free with this build's defaults. Nothing is sent \
                         until you press Import."
                    </p>
                </div>
            </div>

            <div class="nr-live-form">
                <div class="nr-live-field">
                    <label for="nr-pool-import-text">"Paste Proxy List (One per line)"</label>
                    <textarea
                        id="nr-pool-import-text"
                        class="nr-preview-input"
                        rows="5"
                        spellcheck="false"
                        disabled=saving
                        prop:value=move || state.import_text.get()
                        on:input=move |event| state.import_text.set(event_target_value(&event))
                    ></textarea>
                    <small>
                        "Supported formats: protocol://user:pass@host:port, host:port:user:pass, \
                         host:port. Blank lines and lines starting with # are skipped."
                    </small>
                </div>

                <Show when=move || !state.import_text.with(|text| text.trim().is_empty())>
                    <p class="nr-live-status" role="status" aria-live="polite">
                        {move || plan.with(ImportPlan::summary)}
                    </p>
                    <Show when=move || plan.with(|plan| !plan.rejected.is_empty())>
                        <div class="nr-panel-notice is-error" role="alert">
                            <strong>"Some lines could not be read"</strong>
                            <span>
                                "These are not imported. Fix or remove them; the rest can still be \
                                 created."
                            </span>
                            <ul class="nr-combo-members">
                                {move || {
                                    plan.with(|plan| {
                                        plan.rejected
                                            .iter()
                                            .map(|rejection| {
                                                let text = format!(
                                                    "line {}: {}",
                                                    rejection.line,
                                                    rejection.text,
                                                );
                                                view! { <li><code>{text}</code></li> }
                                            })
                                            .collect_view()
                                    })
                                }}
                            </ul>
                        </div>
                    </Show>
                </Show>

                <div class="nr-live-actions">
                    <p
                        class=move || {
                            note.with(|note| {
                                note.as_ref().map_or_else(
                                    || String::from("nr-live-status"),
                                    |note| note.tone.class_name().to_owned(),
                                )
                            })
                        }
                        role="status"
                        aria-live="polite"
                    >
                        {move || note.with(|note| note.as_ref().map(|note| note.text.clone()))}
                    </p>
                    <button
                        type="button"
                        class="nr-button primary small"
                        disabled=move || plan.with(ImportPlan::is_empty) || saving()
                        on:click=move |_event| dispatch_import(state)
                    >
                        <Show when=saving>
                            <span class="nr-spinner" aria-hidden="true"></span>
                        </Show>
                        "Import"
                    </button>
                </div>
            </div>
        </article>
    }
}
