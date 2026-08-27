pub(super) const MITM_STYLES: &str = r"
.nr-mitm-panel{max-width:var(--mitm-max-width);margin:0 auto;gap:var(--space-page)}
.nr-mitm-risk,.nr-mitm-server-head,.nr-mitm-server-title,.nr-mitm-checks,.nr-mitm-check,.nr-mitm-tool-head,.nr-mitm-tool-title,.nr-mitm-tool-heading,.nr-mitm-tool-status,.nr-mitm-action-row{display:flex;align-items:center;gap:var(--space-tight)}
.nr-mitm-risk{align-items:flex-start;border:1px solid color-mix(in srgb,var(--warn) 35%,var(--border-dark));border-radius:var(--radius-control);background:color-mix(in srgb,var(--warn) 10%,transparent);padding:8px 12px;color:var(--warn)}
.nr-mitm-risk p{margin:0;color:var(--warn);font-size:12px;line-height:1.4}
.nr-mitm-risk-mark{width:18px;height:18px;flex:0 0 18px;font-size:18px}
.nr-mitm-server{display:grid;gap:12px;border-radius:var(--radius-card);border-color:color-mix(in srgb,var(--brand) 20%,var(--border-dark));background:color-mix(in srgb,var(--brand) 5%,var(--surface-dark))}
.nr-mitm-server-head{justify-content:space-between;flex-wrap:wrap}
.nr-mitm-server-title h2{font-size:14px;font-weight:600}
.nr-mitm-server-icon{color:var(--brand);font-size:20px}
.nr-mitm-checks{flex-wrap:wrap}
.nr-mitm-badge{display:inline-flex;align-items:center;gap:6px;border:0;border-radius:999px;background:var(--surface-dark-2);color:var(--text-muted-dark);padding:2px 8px;font-size:var(--mitm-badge-font);font-weight:600;line-height:1.5}
.nr-mitm-check{gap:2px;border-radius:var(--mitm-control-radius);padding:2px 6px;color:var(--text-muted-dark);font-size:12px}
.nr-mitm-check-mark{font-size:12px}
.nr-mitm-explain,.nr-mitm-hosts,.nr-mitm-unsupported{display:grid;gap:var(--space-tight);border:1px solid var(--border-dark);border-radius:var(--radius-control);background:var(--surface-dark-2);padding:8px}
.nr-mitm-explain p,.nr-mitm-unsupported{margin:0;color:var(--text-muted-dark);font-size:var(--mitm-copy-server);line-height:1.4}
.nr-mitm-explain strong,.nr-mitm-hosts strong{color:var(--text-main-dark)}
.nr-mitm-fields{display:grid;gap:var(--space-row)}
.nr-mitm-field{display:grid;grid-template-columns:var(--mitm-server-label) auto minmax(0,1fr);align-items:center;gap:var(--space-tight)}
.nr-mitm-field label{color:var(--text-main-dark);font-size:14px;font-weight:600;text-align:right}
.nr-mitm-model-label{color:var(--text-main-dark);font-size:12px;font-weight:600;text-align:right}
.nr-mitm-field-arrow,.nr-mitm-model-arrow{color:var(--text-muted-dark);font-size:14px}
.nr-mitm-field input,.nr-mitm-model-row input{min-width:0;width:100%;border:1px solid var(--border-dark);border-radius:var(--mitm-control-radius);background:var(--surface-dark-2);padding:6px 8px;color:var(--text-main-dark);font-size:12px}
.nr-mitm-field input:focus-visible,.nr-mitm-model-row input:focus-visible,.nr-mitm-tool-head:focus-visible,.nr-mitm-model-row button:focus-visible,.nr-mitm-action-row button:focus-visible{outline:2px solid var(--brand);outline-offset:2px}
.nr-mitm-field input:disabled,.nr-mitm-model-row input:disabled,.nr-mitm-model-row button:disabled,.nr-mitm-action-row button:disabled{cursor:not-allowed;opacity:.55}
.nr-mitm-action-row{align-items:flex-start;flex-wrap:wrap}
.nr-mitm-unsupported{flex:1;color:var(--warn)}
.nr-mitm-tool-list{display:grid;gap:var(--space-card)}
.nr-mitm-tool{overflow:hidden;border:1px solid var(--border-dark);border-radius:var(--radius-card);background:var(--surface-dark);padding:12px}
.nr-mitm-tool-head{width:100%;justify-content:space-between;gap:12px;border:0;background:transparent;padding:0;color:var(--text-main-dark);text-align:left;cursor:pointer}
.nr-mitm-tool-title{min-width:0;gap:12px;align-items:center}
.nr-mitm-tool-title img{width:2rem;height:2rem;flex:0 0 2rem;border-radius:var(--radius-control);object-fit:contain}
.nr-mitm-tool-copy{min-width:0;display:grid;gap:4px}
.nr-mitm-tool-heading,.nr-mitm-tool-status{flex-wrap:wrap}
.nr-mitm-tool-heading{gap:var(--space-tight)}
.nr-mitm-tool-name{font-size:14px;font-weight:500}
.nr-mitm-tool-copy p{margin:0;color:var(--text-muted-dark);font-size:12px;line-height:1.4}
.nr-mitm-chevron{color:var(--text-muted-dark);font-size:20px;transition:transform var(--mitm-motion) ease-out}
.nr-mitm-chevron.is-open{transform:rotate(180deg)}
.nr-mitm-tool-body{display:grid;gap:var(--space-card);margin-top:var(--space-card);border-top:1px solid var(--border-dark);padding-top:var(--space-card)}
.nr-mitm-hosts{gap:2px;margin-top:8px;border-radius:6px;padding:6px 8px}
.nr-mitm-hosts p{margin:0 0 2px;color:var(--text-main-dark);font-size:var(--mitm-copy-host);line-height:1.4}
.nr-mitm-hosts code{display:block;overflow-wrap:anywhere;color:var(--text-muted-dark);font-size:var(--mitm-copy-host);line-height:1.4}
.nr-mitm-tool-info{display:grid;gap:2px;padding:0 4px;color:var(--text-muted-dark);font-size:var(--mitm-copy-info)}
.nr-mitm-tool-info p{margin:0}
.nr-mitm-mapping-note{margin-top:4px!important;color:var(--warn);font-size:var(--mitm-copy-warning)}
.nr-mitm-model-list{display:grid;gap:var(--space-tight)}
.nr-mitm-model-row{display:grid;grid-template-columns:var(--mitm-model-label) auto minmax(0,1fr) auto;align-items:center;gap:var(--space-tight)}
.nr-mitm-model-label{overflow-wrap:anywhere}
.nr-mitm-action-icon{margin-right:4px;font-size:16px}
.nr-mitm-model-row .nr-button{min-height:0;border-radius:var(--mitm-control-radius);padding:6px 8px;font-size:12px}
.nr-mitm-action-row .nr-button{min-height:0;border-radius:var(--radius-control);padding:6px 16px;font-size:12px;font-weight:500}
.nr-mitm-action-row .nr-button.primary{border-color:color-mix(in srgb,var(--brand) 30%,var(--border-dark));background:color-mix(in srgb,var(--brand) 10%,transparent);color:var(--brand);box-shadow:none}
.nr-mitm-unported{display:grid;gap:4px;border:1px dashed color-mix(in srgb,var(--warn) 45%,#464646);border-radius:var(--radius-control);background:color-mix(in srgb,var(--warn) 7%,transparent);padding:12px;color:var(--text-muted-dark);font-size:var(--mitm-copy-server);line-height:1.45}
.nr-mitm-unported strong{color:var(--text-main-dark)}
.nr-mitm-privilege{margin:0;color:var(--text-muted-dark);font-size:var(--mitm-copy-info)}
.nr-mitm-status-failure{display:grid;justify-items:start;gap:6px;border:1px solid color-mix(in srgb,var(--warn) 40%,var(--border-dark));border-radius:var(--radius-control);background:color-mix(in srgb,var(--warn) 8%,transparent);padding:10px;color:var(--text-muted-dark);font-size:var(--mitm-copy-server);line-height:1.45}
.nr-mitm-status-failure strong{color:var(--text-main-dark)}
.nr-mitm-check.is-ok{color:var(--ok)}
.nr-mitm-check.is-ok .nr-mitm-check-mark{color:var(--ok)}
.nr-mitm-badge.is-connected{background:color-mix(in srgb,var(--ok) 18%,transparent);color:var(--ok)}
.nr-mitm-badge.is-degraded{background:color-mix(in srgb,var(--warn) 18%,transparent);color:var(--warn)}
.nr-mitm-model-saved{overflow-wrap:anywhere;color:var(--text-muted-dark);font-size:var(--mitm-copy-host);white-space:nowrap}
.nr-visually-hidden{position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0}
@media (max-width:639px){.nr-mitm-server-head,.nr-mitm-tool-head{align-items:flex-start}.nr-mitm-field,.nr-mitm-model-row{grid-template-columns:1fr;gap:6px}.nr-mitm-field label,.nr-mitm-model-label{font-size:12px;text-align:left}.nr-mitm-field-arrow,.nr-mitm-model-arrow{display:none}.nr-mitm-field input,.nr-mitm-model-row input{padding-block:8px}.nr-mitm-model-row .nr-button,.nr-mitm-action-row .nr-button{width:100%;padding-block:8px}.nr-mitm-action-row{align-items:stretch;flex-direction:column}.nr-mitm-tool-list{gap:12px}.nr-mitm-tool-title{align-items:flex-start}}
";
