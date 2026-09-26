/**
 * Epidemic (node / group / coordinator) read surface — increment 5b:
 * the roster groups (groups.toml) and the spread history with each node's
 * outcome. Read-only; triggering a spread from the console is 5c.
 */

import { apiRequest } from './api.js';

/**
 * HTML-escape a value for safe interpolation into innerHTML.
 */
function esc(value) {
    return String(value ?? '')
        .replaceAll('&', '&amp;')
        .replaceAll('<', '&lt;')
        .replaceAll('>', '&gt;')
        .replaceAll('"', '&quot;')
        .replaceAll("'", '&#39;');
}

/**
 * Render unix-seconds as a local timestamp string.
 */
function formatTime(unixSeconds) {
    if (!unixSeconds) return '-';
    return new Date(unixSeconds * 1000).toLocaleString();
}

/**
 * The canary cohort, as the CLI prints it (pct=25 / subset=KEY=VALUE).
 */
function canaryLabel(canary) {
    if (!canary) return '';
    if (canary.kind === 'percentage') return `canary=pct=${canary.pct}`;
    if (canary.kind === 'subset') return `canary=${canary.criterion}`;
    return '';
}

/**
 * Render the roster groups (GET /api/epidemic/groups) into the container.
 */
export function renderEpidemicGroups(data, container) {
    const groups = data?.groups || [];
    if (groups.length === 0) {
        const where = data?.path ? ` at ${esc(data.path)}` : '';
        container.innerHTML =
            `<div class="empty">No roster groups declared${where}.` +
            ` Declare them in groups.toml (see docs/epidemic.md).</div>`;
        return;
    }

    container.innerHTML = groups.map(group => {
        const nodes = group.nodes || [];
        const nodeList = nodes.length
            ? nodes.map(n =>
                `<li><code>${esc(n.name)}</code> <span class="epidemic-addr">@ ${esc(n.addr)}</span></li>`
            ).join('')
            : '<li class="epidemic-muted">no nodes in roster</li>';
        return `
            <div class="epidemic-group">
                <div class="epidemic-info">
                    <strong>${esc(group.name)}</strong>
                    <span class="version">${nodes.length} node${nodes.length === 1 ? '' : 's'}</span>
                    ${group.secret_path
                        ? `<span class="version" title="group epidemic secret path">${esc(group.secret_path)}</span>`
                        : '<span class="version" title="falls back to the default epidemic secret path">default secret</span>'}
                </div>
                <ul class="epidemic-nodes">${nodeList}</ul>
            </div>`;
    }).join('');
}

/**
 * Render one spread record's per-node outcomes (✓/✗ + error).
 */
function renderSpreadNodes(spread) {
    const nodes = spread.nodes || [];
    if (nodes.length === 0) {
        return '<div class="epidemic-muted">no per-node detail (legacy record)</div>';
    }
    return nodes.map(n => n.ok
        ? `<div class="epidemic-node ok"><span class="mark">✓</span> <code>${esc(n.name)}</code> <span class="epidemic-addr">${esc(n.addr)}</span></div>`
        : `<div class="epidemic-node fail"><span class="mark">✗</span> <code>${esc(n.name)}</code> <span class="epidemic-addr">${esc(n.addr)}</span> <span class="epidemic-error">${esc(n.error || 'failed')}</span></div>`
    ).join('');
}

/**
 * Render the spread history (GET /api/epidemic/spreads) into the container.
 */
export function renderEpidemicSpreads(data, container) {
    const spreads = data?.spreads || [];
    if (spreads.length === 0) {
        const where = data?.path ? ` (${esc(data.path)})` : '';
        container.innerHTML =
            `<div class="empty">No spreads recorded yet${where}. Run ` +
            `<code>epidemic spread &lt;target&gt;</code> to create one.</div>`;
        return;
    }

    container.innerHTML = spreads.map(spread => {
        const meta = [
            spread.group ? `group=${esc(spread.group)}` : '',
            spread.origin ? `origin=${esc(spread.origin)}` : '',
            (spread.criteria || []).length ? `criteria=${esc(spread.criteria.join(';'))}` : '',
            canaryLabel(spread.canary),
            `spread_id=${esc(spread.spread_id)}`,
        ].filter(Boolean).join('&nbsp;&nbsp;');

        return `
            <div class="epidemic-spread ${spread.ok ? 'ok' : 'fail'}">
                <div class="epidemic-info">
                    <span class="status ${spread.ok ? 'status-active' : 'status-failed'}"
                        title="${spread.ok ? 'every reached node applied' : 'one or more nodes failed'}">${spread.ok ? '✓' : '✗'}</span>
                    <strong>${esc(spread.name)}</strong>
                    <span class="version">v${esc(spread.version)}</span>
                    <span class="version">${esc(spread.mode)}</span>
                    <span class="version">${esc(spread.stage)}</span>
                    <span class="version" title="nodes applied / nodes failed">applied=${spread.applied} failed=${spread.failed}</span>
                    <span class="epidemic-time">${formatTime(spread.timestamp)}</span>
                    <code class="epidemic-sha" title="${esc(spread.sha256)}">${esc(spread.sha256.slice(0, 12))}…</code>
                </div>
                ${meta ? `<div class="epidemic-meta">${meta}</div>` : ''}
                <div class="epidemic-spread-nodes">${renderSpreadNodes(spread)}</div>
            </div>`;
    }).join('');
}

/**
 * Load the epidemic read surface (groups + spread history) and render it.
 */
export async function loadEpidemic(apiBase, apiKey) {
    const groupsEl = document.getElementById('epidemic-groups');
    const spreadsEl = document.getElementById('epidemic-spreads');
    if (!groupsEl || !spreadsEl) return;

    try {
        const [groupsResult, spreadsResult] = await Promise.all([
            apiRequest(apiBase, apiKey, '/api/epidemic/groups'),
            apiRequest(apiBase, apiKey, '/api/epidemic/spreads?limit=50'),
        ]);
        renderEpidemicGroups(groupsResult.data, groupsEl);
        renderEpidemicSpreads(spreadsResult.data, spreadsEl);
    } catch (error) {
        const message = `<div class="error">Failed to load epidemic data: ${esc(error.message)}</div>`;
        groupsEl.innerHTML = message;
        spreadsEl.innerHTML = message;
    }
}
