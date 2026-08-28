use crate::dashboard::{SkillSummary, quota_tracker_state, skill_summaries, token_saver_state};
use leptos::prelude::*;

const PARITY_STYLES: &str = r"
.nr-parity-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:10px}
.nr-parity-grid.three{grid-template-columns:repeat(3,minmax(0,1fr))}
.nr-parity-tile{min-width:0;display:grid;gap:9px;border:1px solid var(--border-dark);border-radius:8px;background:var(--surface-dark-2);padding:12px}
.nr-parity-top,.nr-parity-meta,.nr-parity-members,.nr-parity-actions{display:flex;align-items:center;gap:8px}
.nr-parity-top,.nr-parity-actions{justify-content:space-between}
.nr-parity-top h3{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:.95rem}
.nr-parity-tile p,.nr-parity-muted{color:var(--text-muted-dark);font-size:.86rem;line-height:1.45}
.nr-parity-meta,.nr-parity-members{flex-wrap:wrap}
.nr-parity-chip{border:1px solid var(--border-dark);border-radius:999px;background:rgba(255,255,255,.04);color:var(--text-main-dark);padding:4px 8px;font-size:.75rem}
.nr-parity-code{overflow-wrap:anywhere;color:#ffb199;font-family:ui-monospace,SFMono-Regular,Menlo,Monaco,Consolas,monospace;font-size:.82rem}
.nr-parity-band{display:grid;gap:4px;border:1px dashed #464646;border-radius:8px;background:rgba(255,255,255,.025);padding:12px}
.nr-parity-band strong{color:var(--text-main-dark)}
.nr-parity-list{display:grid;gap:8px}
.nr-parity-row{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:10px;align-items:center;border:1px solid var(--border-dark);border-radius:8px;background:var(--surface-dark-2);padding:10px 12px}
.nr-parity-row>span{display:grid;gap:2px;min-width:0}
.nr-parity-row strong{overflow-wrap:anywhere}
.nr-parity-row small{color:var(--text-muted-dark)}
.nr-parity-status{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:10px}
.nr-cli-detail-head,.nr-cli-detail-actions{display:flex;align-items:center;gap:10px;flex-wrap:wrap}
.nr-cli-detail-head{justify-content:space-between}
.nr-cli-detail-grid{display:grid;grid-template-columns:minmax(0,.95fr) minmax(0,1.05fr);gap:10px;align-items:start}
.nr-cli-section-list{display:grid;gap:8px}
.nr-cli-section-row{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:10px;align-items:center;border:1px solid var(--border-dark);border-radius:8px;background:var(--surface-dark-2);padding:10px 12px}
.nr-cli-section-row span{display:grid;gap:2px}
.nr-cli-section-row small,.nr-cli-command span{color:var(--text-muted-dark);font-size:.82rem;line-height:1.45}
.nr-cli-command{display:grid;gap:6px;border:1px solid var(--border-dark);border-radius:8px;background:var(--surface-dark-2);padding:12px}
@media (max-width:1180px){.nr-parity-grid.three{grid-template-columns:repeat(2,minmax(0,1fr))}.nr-cli-detail-grid{grid-template-columns:1fr}}
@media (max-width:680px){.nr-parity-grid,.nr-parity-grid.three,.nr-parity-status{grid-template-columns:1fr}.nr-parity-row,.nr-cli-section-row{grid-template-columns:1fr}.nr-parity-top,.nr-parity-actions,.nr-cli-detail-head,.nr-cli-detail-actions{align-items:flex-start;flex-direction:column}.nr-parity-top h3{white-space:normal}}
";

#[component]
pub(super) fn QuotaTrackerPanel() -> impl IntoView {
    let state = quota_tracker_state();
    let row_count = state.rows.len().to_string();

    view! {
        <ParityStyles />
        <div class="nr-panel-stack">
            <div class="nr-parity-status">
                <ParityMetric label="Live limits" value="Offline".to_owned() detail=state.source_label tone="warn" />
                <ParityMetric label="Quota rows" value=row_count detail="Hydrated rows" tone="info" />
            </div>
            <article class="nr-card">
                <div class="nr-card-head between">
                    <div>
                        <h2><span class="nr-card-icon">"quo"</span>"Quota Tracker"</h2>
                        <p>"Quota tables will show provider limit windows after gateway telemetry and provider quota APIs are connected."</p>
                    </div>
                    <span class="nr-status-pill is-idle"><span></span>"No quota feed"</span>
                </div>
                <div class="nr-empty-state">
                    <strong>"No provider limits loaded"</strong>
                    <span>"The dashboard keeps this table empty instead of inventing usage, reset times, or account capacity."</span>
                </div>
            </article>
        </div>
    }
}

#[component]
pub(super) fn TokenSaverPanel() -> impl IntoView {
    let state = token_saver_state();

    view! {
        <ParityStyles />
        <div class="nr-panel-stack">
            <article class="nr-card">
                <div class="nr-card-head between">
                    <div>
                        <h2><span class="nr-card-icon">"sav"</span>"Token Saver"</h2>
                        <p>"RTK and headroom controls mirror upstream settings, but persistence is not wired in this WASM slice."</p>
                    </div>
                    <span class="nr-status-pill is-degraded"><span></span>{state.status_label}</span>
                </div>
                <div class="nr-parity-grid">
                    <TokenSaverTile label="RTK compression" enabled=state.rtk_wired detail="Request token compaction setting awaits host settings API." />
                    <TokenSaverTile label="Headroom service" enabled=state.headroom_wired detail="External headroom endpoint toggle is visible but disabled." />
                </div>
            </article>
        </div>
    }
}

#[component]
pub(super) fn SkillsPanel() -> impl IntoView {
    view! {
        <ParityStyles />
        <div class="nr-panel-stack">
            <article class="nr-card">
                <div class="nr-card-head between">
                    <div>
                        <h2><span class="nr-card-icon">"ext"</span>"Skills"</h2>
                        <p>"Skill metadata mirrors the upstream install guide. Copy/install actions stay disabled until the host endpoint is available."</p>
                    </div>
                    <button type="button" class="nr-button secondary small" disabled>"Copy URL"</button>
                </div>
                <div class="nr-parity-list">
                    <For
                        each=|| skill_summaries().to_vec()
                        key=|skill| skill.id
                        children=|skill| view! { <SkillRow skill /> }
                    />
                </div>
            </article>
        </div>
    }
}

#[component]
fn TokenSaverTile(label: &'static str, enabled: bool, detail: &'static str) -> impl IntoView {
    view! {
        <button type="button" class="nr-setting-row" disabled>
            <span>
                <strong>{label}</strong>
                <small>{detail}</small>
            </span>
            <span class="nr-toggle" class:is-on=move || enabled aria-hidden="true">
                <span></span>
            </span>
        </button>
    }
}

#[component]
fn SkillRow(skill: SkillSummary) -> impl IntoView {
    view! {
        <div class="nr-parity-row">
            <span>
                <strong>{skill.name}</strong>
                <small>{skill.description}</small>
            </span>
            <code class="nr-parity-code">{endpoint_label(skill.endpoint)}</code>
        </div>
    }
}

#[component]
fn ParityMetric(
    label: &'static str,
    value: String,
    detail: &'static str,
    tone: &'static str,
) -> impl IntoView {
    view! {
        <article class=format!("nr-card nr-metric-card {}", tone)>
            <span class="nr-metric-label">{label}</span>
            <strong>{value}</strong>
            <small>{detail}</small>
        </article>
    }
}

#[component]
fn ParityStyles() -> impl IntoView {
    view! { <style>{PARITY_STYLES}</style> }
}

const fn endpoint_label(endpoint: Option<&'static str>) -> &'static str {
    match endpoint {
        Some(value) => value,
        None => "Index",
    }
}
