/**
 * Deployment lifecycle: install (preview → apply), list, status, remove.
 *
 * Install is two-step by design: `POST /api/admin/deployments` with
 * `dry_run: true` returns the rendered plan; the user reviews it, then the
 * identical payload is sent with `dry_run: false`. Mirrors the CLI
 * `pandemic-cli deployment ...` surface (ideas/deployments.md, phase 6).
 *
 * Variable *values*, rendered file contents, and the health-check command
 * are never shown in the browser (values may be secrets; the state record
 * is 0600 root-only) — the API only sends names, target paths, owners,
 * modes, and content hashes (phase 8 server-side masking).
 */
import { apiRequest } from './api.js';

/** Escape a value for safe interpolation into an HTML string. */
function esc(value) {
    return String(value).replace(/[&<>"']/g, c => (
        { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]
    ));
}

// Shared-variable values may be secrets (the record is 0600 root-only), so the
// browser only ever shows their *names* — never their values.
function variablesBlock(variables) {
    const names = Object.keys(variables || {});
    if (names.length === 0) return '';
    return `
        <div class="detail-block">
            <h4>Shared variables</h4>
            <div>${names.map(n => `<span class="version">${esc(n)}</span>`).join(' ')}</div>
        </div>`;
}

function activeLabel(active) {
    if (active === true) return 'active';
    if (active === false) return 'inactive';
    return 'n/a';
}

function infectionDetail(inf) {
    if (!inf.present) {
        return `<div class="detail-infection">
            <strong>${inf.name}</strong>
            <span class="status status-inactive">not installed on host</span>
        </div>`;
    }
    const st = inf.status || {};
    const state = st.state || {};
    const files = (st.files || []).map(f => {
        const mark = f.exists && f.hash_ok ? '✓' : (f.exists ? 'modified' : 'missing');
        return `<div class="file-row">[${mark}] ${f.target}</div>`;
    }).join('');
    const unit = state.unit
        ? `<div>unit: ${state.unit}
            <span class="status ${st.unit_active ? 'status-active' : 'status-inactive'}">${activeLabel(st.unit_active)}</span></div>`
        : '';
    const attach = state.attach
        ? `<div>attach: ${state.attach}
            <span class="status ${st.target_active ? 'status-active' : 'status-inactive'}">${activeLabel(st.target_active)}</span></div>`
        : '';
    return `<div class="detail-infection">
        <div class="deployment-info">
            <strong>${inf.name}</strong>
            <span class="version">v${state.version || '?'}</span>
            <span class="status status-active">installed</span>
        </div>
        ${unit}
        ${attach}
        ${files ? `<div class="detail-block">${files}</div>` : ''}
    </div>`;
}

/**
 * List installed deployments into `container`.
 */
export async function listDeployments(apiBase, apiKey, container) {
    container.innerHTML = '<div class="loading">Loading deployments...</div>';
    try {
        const result = await apiRequest(apiBase, apiKey, '/api/admin/deployments');
        const deployments = result.data?.deployments || [];
        if (deployments.length === 0) {
            container.innerHTML = '<div class="empty">No deployments installed.</div>';
            return;
        }
        container.innerHTML = deployments.map(d => `
            <div class="deployment-item">
                <div class="deployment-info">
                    <strong>${d.name}</strong>
                    <span class="version">v${d.version}</span>
                    <span class="muted">installed ${d.installed_at || '-'}</span>
                </div>
                <div class="deployment-meta">
                    ${(d.infections || []).map(i => `<span class="version">${i.name} (order ${i.order})</span>`).join(' ')}
                </div>
                <div class="infection-actions">
                    <button onclick="window.pandemicConsole.viewDeployment('${d.name}')">Status</button>
                    <button class="danger" onclick="window.pandemicConsole.removeDeployment('${d.name}')">Remove</button>
                </div>
                <div id="deployment-detail-${d.name}" style="display:none;"></div>
            </div>
        `).join('');
    } catch (error) {
        container.innerHTML = `<div class="error">Failed to load deployments: ${error.message}</div>`;
    }
}

/**
 * Toggle one deployment's status detail into its panel.
 */
export async function viewDeployment(name, apiBase, apiKey) {
    const panel = document.getElementById(`deployment-detail-${name}`);
    if (!panel) return;
    if (panel.style.display !== 'none') {
        panel.style.display = 'none';
        return;
    }
    panel.innerHTML = '<div class="loading">Loading status...</div>';
    panel.style.display = 'block';
    try {
        const result = await apiRequest(apiBase, apiKey, `/api/admin/deployments/${encodeURIComponent(name)}`);
        const data = result.data || {};
        const state = data.state || {};
        panel.innerHTML = `
            <div class="deployment-detail">
                <div class="detail-block">
                    <h4>${state.name || name} <span class="version">v${state.version || '?'}</span></h4>
                    <div class="infection-meta">
                        <span>Installed: ${state.installed_at || '-'}</span>
                    </div>
                </div>
                ${variablesBlock(state.variables)}
                <div class="detail-block">
                    <h4>Infections (install order)</h4>
                    ${(data.infections || []).map(infectionDetail).join('')}
                </div>
            </div>`;
    } catch (error) {
        panel.innerHTML = `<div class="error">Failed to load status: ${error.message}</div>`;
    }
}

/**
 * Remove a deployment (uninstalls the infections it owns).
 */
export async function removeDeployment(name, apiBase, apiKey, reloadDeployments) {
    if (!confirm(`Remove deployment '${name}'? This uninstalls the infections it owns.`)) return;
    try {
        const result = await apiRequest(apiBase, apiKey, `/api/admin/deployments/${encodeURIComponent(name)}`, {
            method: 'DELETE',
        });
        const d = result.data || {};
        const list = (k) => (d[k] || []).join(', ');
        const parts = [
            d.record_removed
                ? `Removed deployment '${name}'.`
                : `Removal of '${name}' partially failed — record kept.`,
        ];
        if ((d.removed || []).length) parts.push(`Removed: ${list('removed')}`);
        if ((d.skipped || []).length) parts.push(`Skipped: ${list('skipped')}`);
        if ((d.failed || []).length) parts.push(`Failed: ${list('failed')}`);
        (d.notes || []).forEach(n => parts.push(n));
        alert(parts.join('\n'));
        reloadDeployments();
    } catch (error) {
        alert(`Removal failed: ${error.message}`);
    }
}

// ---------------------------------------------------------------------------
// Install — always two-step: preview the rendered plan, then apply it.
// The payload of the last successful preview is remembered; Apply re-sends
// exactly that payload with `dry_run: false`. Any form edit invalidates the
// preview so the plan can never drift from what gets applied.
// ---------------------------------------------------------------------------

let pendingInstall = null; // { name?, path?, vars } from the last good preview

/**
 * Wire up the install panel (source radios, preview invalidation).
 * Called once from the console after render.
 */
export function setupDeploymentInstall() {
    const showSourceInput = () => {
        const mode = document.querySelector('input[name="deployment-source"]:checked').value;
        document.getElementById('deployment-name').style.display = mode === 'name' ? '' : 'none';
        document.getElementById('deployment-path').style.display = mode === 'path' ? '' : 'none';
    };
    document.querySelectorAll('input[name="deployment-source"]').forEach(r =>
        r.addEventListener('change', () => { showSourceInput(); invalidateInstallPreview(); }));

    ['deployment-name', 'deployment-path'].forEach(id =>
        document.getElementById(id).addEventListener('input', invalidateInstallPreview));
    document.getElementById('deployment-vars').addEventListener('input', invalidateInstallPreview);
}

/**
 * Add an empty variable override row to the install form.
 */
export function addDeploymentVar() {
    const row = document.createElement('div');
    row.className = 'var-row';
    row.innerHTML = `
        <input class="var-key" type="text" placeholder="variable name" autocomplete="off">
        <input class="var-value" type="text" placeholder="value" autocomplete="off">
        <button class="danger" type="button" title="Remove variable"
                onclick="this.closest('.var-row').remove()">×</button>`;
    document.getElementById('deployment-vars').appendChild(row);
    row.querySelector('.var-key').focus();
}

/**
 * Reset the whole install panel (form + preview).
 */
export function clearDeploymentInstall() {
    invalidateInstallPreview();
    document.getElementById('deployment-name').value = '';
    document.getElementById('deployment-path').value = '';
    document.getElementById('deployment-vars').innerHTML = '';
    document.querySelector('input[name="deployment-source"][value="name"]').checked = true;
    document.getElementById('deployment-name').style.display = '';
    document.getElementById('deployment-path').style.display = 'none';
}

function invalidateInstallPreview() {
    pendingInstall = null;
    document.getElementById('deployment-preview').innerHTML = '';
    document.getElementById('deployment-apply-btn').style.display = 'none';
    document.getElementById('deployment-clear-btn').style.display = 'none';
}

function readInstallForm() {
    const vars = {};
    document.querySelectorAll('#deployment-vars .var-row').forEach(row => {
        const key = row.querySelector('.var-key').value.trim();
        if (!key) return;
        vars[key] = row.querySelector('.var-value').value;
    });
    const mode = document.querySelector('input[name="deployment-source"]:checked').value;
    if (mode === 'name') {
        const name = document.getElementById('deployment-name').value.trim();
        if (!name) throw new Error('Enter a registry deployment name (e.g. rest-mqtt)');
        return { name, vars };
    }
    const path = document.getElementById('deployment-path').value.trim();
    if (!path) throw new Error('Enter a local deployment spec path (…/deployment.toml)');
    return { path, vars };
}

function errorBlock(message) {
    return `<div class="error">${esc(message)}</div>`;
}

/**
 * Step 1 — resolve + render the plan (`dry_run: true`) and display it.
 */
export async function previewDeploymentInstall(apiBase, apiKey) {
    const previewEl = document.getElementById('deployment-preview');
    const btn = document.getElementById('deployment-preview-btn');
    let payload;
    try {
        payload = readInstallForm();
    } catch (e) {
        previewEl.innerHTML = errorBlock(e.message);
        return;
    }
    btn.disabled = true;
    previewEl.innerHTML = '<div class="loading">Resolving and rendering plan...</div>';
    try {
        const result = await apiRequest(apiBase, apiKey, '/api/admin/deployments', {
            method: 'POST',
            body: JSON.stringify({ ...payload, dry_run: true }),
        });
        pendingInstall = payload;
        previewEl.innerHTML = renderPlanPreview(result.data || {});
        document.getElementById('deployment-apply-btn').style.display = '';
        document.getElementById('deployment-clear-btn').style.display = '';
    } catch (error) {
        pendingInstall = null;
        document.getElementById('deployment-apply-btn').style.display = 'none';
        document.getElementById('deployment-clear-btn').style.display = '';
        previewEl.innerHTML = errorBlock(`Preview failed: ${error.message}`);
    } finally {
        btn.disabled = false;
    }
}

/**
 * Step 2 — apply the previewed plan (same payload, `dry_run: false`).
 */
export async function applyDeploymentInstall(apiBase, apiKey, reloadDeployments) {
    if (!pendingInstall) return;
    const label = pendingInstall.name || pendingInstall.path;
    if (!confirm(
        `Apply deployment '${label}'?\n\n` +
        `This installs every infection in the previewed plan (as root, via the agent).`,
    )) return;
    const btn = document.getElementById('deployment-apply-btn');
    const previewEl = document.getElementById('deployment-preview');
    btn.disabled = true;
    previewEl.innerHTML = '<div class="loading">Applying deployment...</div>';
    try {
        const result = await apiRequest(apiBase, apiKey, '/api/admin/deployments', {
            method: 'POST',
            body: JSON.stringify({ ...pendingInstall, dry_run: false }),
        });
        const d = result.data || {};
        previewEl.innerHTML = `<div class="success">
            Deployment '${esc(d.name || label)}' applied.
            ${(d.applied || []).length ? ` Infections installed: ${(d.applied || []).map(esc).join(', ')}.` : ''}
        </div>`;
        pendingInstall = null;
        document.getElementById('deployment-apply-btn').style.display = 'none';
        document.getElementById('deployment-clear-btn').style.display = '';
        reloadDeployments();
    } catch (error) {
        previewEl.innerHTML = errorBlock(`Apply failed: ${error.message}\n\nYou can retry the apply, or clear the panel to start over.`);
    } finally {
        btn.disabled = false;
    }
}

// --- plan preview rendering (names/targets/modes only, never values or contents) ---

function variableNamesBlock(names) {
    const list = Array.isArray(names) ? names : Object.keys(names || {});
    if (list.length === 0) return '';
    return `<div class="plan-row">
        <span class="plan-label">variables</span>
        <span>${list.map(n => `<span class="version">${esc(n)}</span>`).join(' ')}
            <span class="muted">(values hidden)</span></span>
    </div>`;
}

/** Short content hash for display: first 12 hex chars, full hash in the tooltip. */
function hashSpan(sha256) {
    if (!sha256) return '';
    return ` <span class="muted" title="sha256 ${esc(sha256)}">sha ${esc(sha256.slice(0, 12))}</span>`;
}

function planRow(label, inner) {
    if (!inner) return '';
    return `<div class="plan-row"><span class="plan-label">${label}</span><span>${inner}</span></div>`;
}

function fileDiffLabel(state) {
    switch (state) {
        case 'absent': return 'will be created';
        case 'unchanged': return 'no change';
        case 'modified': return 'will be replaced';
        default: return String(state);
    }
}

function guDiff(label, g) {
    if (!g) return '';
    const bits = [];
    if ((g.present || []).length) bits.push(`existing: ${(g.present || []).map(esc).join(', ')}`);
    if ((g.missing || []).length) bits.push(`to create: ${(g.missing || []).map(esc).join(', ')}`);
    return bits.length ? `${label}: ${bits.join(' · ')}` : '';
}

/** Host diff block (phase 8): what applying would change on this host. */
function renderDiff(d) {
    if (!d) return '';
    const rows = [];
    if (d.already_recorded) rows.push('<span class="status status-inactive">already installed on host — applying re-applies / upgrades it</span>');
    if (d.selected_package_manager) rows.push(`package manager: <span class="version">${esc(d.selected_package_manager)}</span>`);
    if (d.unit) rows.push(`unit: file ${d.unit.file_exists ? 'present' : 'absent'} · ${d.unit.active ? 'active' : 'inactive'}`);
    if (d.attach) rows.push(`attach target: ${d.attach.active ? 'active' : 'inactive'}`);
    const g = [guDiff('groups', d.groups), guDiff('users', d.users)].filter(Boolean).join(' · ');
    if (g) rows.push(g);
    if (rows.length === 0) return '';
    return `<div class="plan-diff">
        <div class="plan-diff-head">On this host:</div>
        ${rows.map(r => `<div class="plan-diff-row">${r}</div>`).join('')}
    </div>`;
}

function renderPlanInfection(r) {
    const p = r.preview || {};
    const d = r.diff || null;
    const fileDiff = (target) => {
        const f = (d && d.files || []).find(x => x.target === target);
        return f ? ` <span class="muted">→ ${esc(fileDiffLabel(f.state))}</span>` : '';
    };
    const packages = Object.entries(p.declared_packages || {})
        .map(([mgr, list]) => `${esc(mgr)}: ${list.map(esc).join(', ')}`)
        .join(' · ');
    const files = (p.files || [])
        .map(f => `${esc(f.target)} <span class="muted">${esc(f.owner)}:${esc(f.mode)}</span>${hashSpan(f.sha256)}${fileDiff(f.target)}`)
        .join('<br>');
    const users = (p.users || []).map(u => `<span class="version">${esc(u)}</span>`).join(' ');
    const unit = p.unit
        ? `${esc(p.unit.name)}${p.unit.enable ? ' <span class="muted">(enabled at boot)</span>' : ''}${hashSpan(p.unit.sha256)}`
        : '';
    const health = p.health && p.health.configured
        ? `every ${p.health.interval}s <span class="muted">(command hidden)</span>`
        : '';
    return `<div class="plan-infection">
        <div class="plan-infection-head">
            <strong>${esc(r.name)}</strong>
            <span class="version">v${esc(r.version || '?')}</span>
            <span class="muted">order ${r.order} · source: ${esc(r.source)}</span>
        </div>
        ${p.description ? `<div class="muted">${esc(p.description)}</div>` : ''}
        ${planRow('packages', packages)}
        ${planRow('files', files)}
        ${planRow('unit', unit)}
        ${planRow('attach', p.attach ? esc(p.attach) : '')}
        ${planRow('health check', health)}
        ${planRow('groups', (p.groups || []).map(g => `<span class="version">${esc(g)}</span>`).join(' '))}
        ${planRow('users', users)}
        ${variableNamesBlock(p.variable_names)}
        ${renderDiff(d)}
    </div>`;
}

function renderPlanPreview(data) {
    const infections = data.infections || [];
    return `<div class="plan-preview">
        <div class="plan-head">
            <strong>${esc(data.name || '?')}</strong>
            <span class="version">v${esc(data.version || '?')}</span>
            <span class="muted">${infections.length} infection${infections.length === 1 ? '' : 's'}, installed in order</span>
        </div>
        ${variableNamesBlock(data.shared_variable_names)}
        ${infections.map(renderPlanInfection).join('') || '<div class="empty">No infections in this deployment.</div>'}
        <div class="muted">File contents, variable values, and health-check commands are hidden in the preview. When the agent is reachable, each infection also shows what applying would change on the host. Applying sends the fully rendered plan to the agent.</div>
    </div>`;
}
