/**
 * Deployment lifecycle: list / status / remove.
 *
 * Read-only (plus remove). Install is deliberately NOT in the console yet —
 * the `POST /api/admin/deployments` path+vars flow will be reworked around
 * the registry (ideas/deployments.md, phase 5), which simplifies the UX.
 * Mirrors the CLI `pandemic-cli deployment ...` surface.
 */
import { apiRequest } from './api.js';

// Shared-variable values may be secrets (the record is 0600 root-only), so the
// browser only ever shows their *names* — never their values.
function variablesBlock(variables) {
    const names = Object.keys(variables || {});
    if (names.length === 0) return '';
    return `
        <div class="detail-block">
            <h4>Shared variables</h4>
            <div>${names.map(n => `<span class="version">${n}</span>`).join(' ')}</div>
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
