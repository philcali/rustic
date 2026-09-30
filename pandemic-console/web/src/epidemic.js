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
/**
 * A "(×N attempts)" badge when a node needed more than one apply attempt
 * (increment 4b retries). Omitted for the single-attempt (common) case.
 */
function attemptsBadge(n) {
    return (n && n.attempts > 1)
        ? ` <span class="epidemic-attempts" title="retried ${n.attempts} times">${n.attempts}×</span>`
        : '';
}

function renderSpreadNodes(spread) {
    const nodes = spread.nodes || [];
    if (nodes.length === 0) {
        return '<div class="epidemic-muted">no per-node detail (legacy record)</div>';
    }
    return nodes.map(n => n.ok
        ? `<div class="epidemic-node ok"><span class="mark">✓</span> <code>${esc(n.name)}</code> <span class="epidemic-addr">${esc(n.addr)}</span>${attemptsBadge(n)}</div>`
        : `<div class="epidemic-node fail"><span class="mark">✗</span> <code>${esc(n.name)}</code> <span class="epidemic-addr">${esc(n.addr)}</span>${attemptsBadge(n)} <span class="epidemic-error">${esc(n.error || 'failed')}</span></div>`
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
 * Load the epidemic surface (groups + spread history + trigger form) and
 * render it. The trigger form is rendered once; on later loads (e.g. after
 * a spread finishes) only the roster-group <select> is refreshed.
 */
export async function loadEpidemic(apiBase, apiKey) {
    const groupsEl = document.getElementById('epidemic-groups');
    const spreadsEl = document.getElementById('epidemic-spreads');
    const triggerEl = document.getElementById('epidemic-trigger');
    if (!groupsEl || !spreadsEl) return;
    apiContext = { base: apiBase, key: apiKey };

    try {
        const [groupsResult, spreadsResult] = await Promise.all([
            apiRequest(apiBase, apiKey, '/api/epidemic/groups'),
            apiRequest(apiBase, apiKey, '/api/epidemic/spreads?limit=50'),
        ]);
        renderEpidemicGroups(groupsResult.data, groupsEl);
        renderEpidemicSpreads(spreadsResult.data, spreadsEl);
        renderEpidemicTrigger(triggerEl, groupsResult.data?.groups || []);
    } catch (error) {
        const message = `<div class="error">Failed to load epidemic data: ${esc(error.message)}</div>`;
        groupsEl.innerHTML = message;
        spreadsEl.innerHTML = message;
        if (triggerEl && !triggerEl.dataset.rendered) {
            triggerEl.innerHTML = message;
        }
    }
}

// ── Start a spread (increment 5c) ────────────────────────────────────────
//
// POST /api/epidemic/spread runs the shared coordinator; its progress
// events arrive over the event websocket on topic epidemic.spread, shaped
// {"spread_progress": {phase: started|node|finished, ...}}. The live panel
// below follows the most recent spread_id it sees (console-triggered or
// CLI-triggered), then history reloads when it finishes.

// The console's current API base/key (set by loadEpidemic), so the trigger
// button can POST without re-plumbing the values through the DOM.
let apiContext = { base: null, key: null };

const liveProgress = {
    spreadId: null,
    started: null,      // the phase=started event
    nodes: new Map(),   // addr -> {name, addr, ok, error}
    finished: null,     // the phase=finished event
};

/**
 * Render the live progress panel for the tracked spread.
 */
export function renderEpidemicProgress(container) {
    if (!container) return;
    if (!liveProgress.spreadId && !liveProgress.started && liveProgress.nodes.size === 0 && !liveProgress.finished) {
        container.innerHTML = '';
        return;
    }

    const s = liveProgress.started;
    const nodes = [...liveProgress.nodes.values()];
    const f = liveProgress.finished;
    const okCount = nodes.filter(n => n.ok).length;
    const failCount = nodes.filter(n => !n.ok).length;

    const head = s
        ? `<div class="epidemic-live-head">
                <strong>${esc(s.name)}</strong>
                <span class="version">v${esc(s.version)}</span>
                <span class="version">${esc(s.mode)}</span>
                ${s.group ? `<span class="version">group=${esc(s.group)}</span>` : ''}
                <span class="version" title="spread id">id=${esc(liveProgress.spreadId)}</span>
                <span class="version">${s.nodes.length} node${s.nodes.length === 1 ? '' : 's'}</span>
            </div>`
        : `<div class="epidemic-live-head"><span class="version">spread id=${esc(liveProgress.spreadId)}</span></div>`;

    const nodeList = nodes.length
        ? nodes.map(n => n.ok
            ? `<div class="epidemic-node ok"><span class="mark">✓</span> <code>${esc(n.name)}</code> <span class="epidemic-addr">${esc(n.addr)}</span>${attemptsBadge(n)}</div>`
            : `<div class="epidemic-node fail"><span class="mark">✗</span> <code>${esc(n.name)}</code> <span class="epidemic-addr">${esc(n.addr)}</span>${attemptsBadge(n)} <span class="epidemic-error">${esc(n.error || 'failed')}</span></div>`
        ).join('')
        : '<div class="epidemic-muted">waiting for node results…</div>';

    const foot = f
        ? `<div class="epidemic-live-foot ${f.ok ? 'ok' : 'fail'}">${f.ok ? '✓' : '✗'} finished — applied=${f.applied} failed=${f.failed}</div>`
        : '<div class="epidemic-muted">in progress…</div>';

    container.innerHTML = `
        <div class="epidemic-live">
            ${head}
            <div class="epidemic-live-nodes">${nodeList}</div>
            ${foot}
        </div>`;
}

/**
 * Ingest one epidemic.spread progress event (from the websocket, or from
 * the POST response record) and re-render the live panel.
 *
 * `event` is the SpreadProgress payload: {phase: started|node|finished, …}.
 * `onFinished` (optional) is called once when the spread completes — the
 * console uses it to reload the spread history.
 */
export function handleEpidemicProgress(payload, onFinished) {
    // The daemon bus wraps the event as {"spread_progress": {...}}; accept
    // a bare event too (e.g. direct calls / tests).
    const event = payload?.spread_progress ?? payload;
    if (!event || !event.phase || !event.spread_id) return;

    // A different spread started: the panel follows the newest one.
    if (liveProgress.spreadId && event.spread_id !== liveProgress.spreadId) {
        liveProgress.spreadId = null;
        liveProgress.started = null;
        liveProgress.nodes = new Map();
        liveProgress.finished = null;
    }
    liveProgress.spreadId = event.spread_id;

    switch (event.phase) {
        case 'started':
            liveProgress.started = event;
            break;
        case 'node': {
            const key = event.addr || event.name;
            liveProgress.nodes.set(key, {
                name: event.name,
                addr: event.addr,
                ok: event.ok,
                error: event.error,
                attempts: event.attempts,
            });
            break;
        }
        case 'finished':
            liveProgress.finished = event;
            if (onFinished) onFinished();
            break;
        default:
            break;
    }

    renderEpidemicProgress(document.getElementById('epidemic-progress'));
}

/**
 * Render the "Start a spread" form into the container. Rendered once
 * (flagged in dataset); on later loads only the roster-group <select>
 * options are refreshed (preserving the current selection).
 */
export function renderEpidemicTrigger(container, groups) {
    if (!container) return;
    const groupNames = groups.map(g => g.name).filter(Boolean).sort();

    if (container.dataset.rendered) {
        const select = container.querySelector('#ep-group');
        if (select) {
            const current = select.value;
            select.innerHTML =
                `<option value="">(ad-hoc nodes only)</option>` +
                groupNames.map(n => `<option value="${esc(n)}">${esc(n)}</option>`).join('');
            select.value = groupNames.includes(current) ? current : '';
        }
        return;
    }
    container.dataset.rendered = '1';

    container.innerHTML = `
        <div class="trigger-grid">
            <fieldset>
                <legend>Deployment</legend>
                <label class="trigger-radio"><input type="radio" name="ep-target" value="name" checked> Registry name</label>
                <input id="ep-name" type="text" placeholder="e.g. rest-mqtt" autocomplete="off">
                <label class="trigger-radio"><input type="radio" name="ep-target" value="path"> Spec path</label>
                <input id="ep-path" type="text" placeholder="/etc/pandemic/deployments/web.toml" autocomplete="off">
                <label>Vars <span class="epidemic-hint">(KEY=VALUE, one per line — like --set)</span></label>
                <textarea id="ep-vars" rows="2" placeholder="REPLICAS=3"></textarea>
            </fieldset>

            <fieldset id="ep-roster-fields">
                <legend>Targeting (roster)</legend>
                <label>Group <span class="epidemic-hint">(from groups.toml)</span></label>
                <select id="ep-group">
                    <option value="">(ad-hoc nodes only)</option>
                    ${groupNames.map(n => `<option value="${esc(n)}">${esc(n)}</option>`).join('')}
                </select>
                <label>Nodes <span class="epidemic-hint">(host:port, comma-separated)</span></label>
                <input id="ep-nodes" type="text" placeholder="10.0.0.5:7711, 10.0.0.6:7711" autocomplete="off">
            </fieldset>

            <fieldset>
                <legend>Epidemic secret</legend>
                <label>Secret</label>
                <input id="ep-secret" type="password" placeholder="(falls back to group/default/generated)" autocomplete="off">
                <label>Secret path</label>
                <input id="ep-secret-path" type="text" placeholder="/etc/pandemic/epidemic.key" autocomplete="off">
                <label>TLS <span class="epidemic-hint">(coordinator→node hop)</span></label>
                <label class="trigger-check"><input id="ep-tls" type="checkbox"> encrypt the hop</label>
                <input id="ep-tls-ca" type="text" placeholder="root CA (PEM file path)" autocomplete="off">
                <input id="ep-tls-server-name" type="text" placeholder="server name (default: node host)" autocomplete="off">
            </fieldset>

            <fieldset id="ep-broadcast-fields">
                <legend>Broadcast <span class="epidemic-hint">(multicast instead of roster)</span></legend>
                <label class="trigger-check"><input id="ep-broadcast" type="checkbox"> broadcast mode</label>
                <label>Criteria <span class="epidemic-hint">(KEY=VALUE, one per line)</span></label>
                <textarea id="ep-criteria" rows="2" placeholder="tier=web"></textarea>
                <label>Canary</label>
                <input id="ep-canary" type="text" placeholder="25  (pct)  or  zone=us  (subset)" autocomplete="off">
                <label>Wait (s)</label>
                <input id="ep-wait" type="number" min="1" value="15">
                <label class="trigger-check"><input id="ep-promote" type="checkbox"> promote (requires spread id)</label>
                <input id="ep-spread-id" type="text" placeholder="(auto-generated)" autocomplete="off">
            </fieldset>
        </div>
        <div class="trigger-actions">
            <button id="ep-trigger-btn" class="primary">🦠 Trigger spread</button>
            <span id="ep-trigger-status" class="epidemic-muted"></span>
        </div>`;

    // Checkbox toggles show/hide the dependent fields.
    const broadcastBox = container.querySelector('#ep-broadcast');
    const rosterFields = container.querySelector('#ep-roster-fields');
    const setBroadcast = () => {
        rosterFields.classList.toggle('disabled', broadcastBox.checked);
    };
    broadcastBox.addEventListener('change', setBroadcast);
    setBroadcast();

    container.querySelector('#ep-trigger-btn')
        .addEventListener('click', () => triggerEpidemicSpread(container));
}

/**
 * Parse "KEY=VALUE" lines (like the CLI --set) into an object. Blank lines
 * and lines without '=' are skipped.
 */
function parseKvLines(text) {
    const out = {};
    for (const raw of String(text || '').split('\n')) {
        const line = raw.trim();
        if (!line) continue;
        const eq = line.indexOf('=');
        if (eq <= 0) continue;
        out[line.slice(0, eq).trim()] = line.slice(eq + 1).trim();
    }
    return out;
}

/**
 * A spread id for live tracking: console-<unix-ms>-<random4>.
 */
function newSpreadId() {
    const rand = Math.random().toString(36).slice(2, 6);
    return `console-${Date.now()}-${rand}`;
}

/**
 * Collect the trigger form into a POST /api/epidemic/spread payload and
 * fire it. The response carries the final SpreadRecord; live progress
 * arrives via the websocket in the meantime.
 */
export async function triggerEpidemicSpread(container) {
    const statusEl = container.querySelector('#ep-trigger-status');
    const btn = container.querySelector('#ep-trigger-btn');
    const value = id => container.querySelector(id)?.value?.trim() || '';
    const checked = id => container.querySelector(id)?.checked || false;

    const targetName = value('#ep-name');
    const targetPath = value('#ep-path');
    const group = value('#ep-group');
    const nodes = value('#ep-nodes').split(',').map(s => s.trim()).filter(Boolean);
    const broadcast = checked('#ep-broadcast');
    const tls = checked('#ep-tls');

    // Client-side validation mirroring the handler's 400s.
    if (!targetName && !targetPath) {
        statusEl.textContent = '✗ provide a registry name or a spec path';
        return;
    }
    if (broadcast) {
        if (group || nodes.length) {
            statusEl.textContent = '✗ broadcast mode is mutually exclusive with group/nodes';
            return;
        }
    } else if (!group && nodes.length === 0) {
        statusEl.textContent = '✗ pick a group or list at least one node (host:port)';
        return;
    }
    if (tls && !value('#ep-tls-ca')) {
        statusEl.textContent = '✗ TLS requires the root CA path';
        return;
    }

    const payload = {
        vars: parseKvLines(value('#ep-vars')),
    };
    if (targetName) payload.name = targetName;
    if (targetPath) payload.path = targetPath;

    if (!broadcast) {
        if (group) payload.group = group;
        if (nodes.length) payload.nodes = nodes;
        if (value('#ep-secret')) payload.secret = value('#ep-secret');
        if (value('#ep-secret-path')) payload.secret_path = value('#ep-secret-path');
        if (tls) {
            payload.tls = true;
            payload.tls_ca = value('#ep-tls-ca');
            if (value('#ep-tls-server-name')) payload.tls_server_name = value('#ep-tls-server-name');
        }
    } else {
        payload.broadcast = true;
        const criteria = value('#ep-criteria').split('\n').map(s => s.trim()).filter(Boolean);
        if (criteria.length) payload.criteria = criteria;
        if (value('#ep-canary')) payload.canary = value('#ep-canary');
        const wait = parseInt(value('#ep-wait'), 10);
        if (wait > 0) payload.wait = wait;
        if (checked('#ep-promote')) payload.promote = true;
    }

    if (payload.broadcast) {
        // Broadcast honors a caller id (required for promote). Roster ids
        // are generated server-side — the live panel adopts the id from
        // the `started` event instead.
        payload.spread_id = value('#ep-spread-id') || newSpreadId();
        if (checked('#ep-promote') && !payload.spread_id) {
            statusEl.textContent = '✗ promote requires a spread id';
            return;
        }
    }

    if (!apiContext.base || !apiContext.key) {
        statusEl.textContent = '✗ save an API key first (header)';
        return;
    }

    btn.disabled = true;
    const trackedId = payload.spread_id || null;
    statusEl.textContent = trackedId
        ? `… running ${trackedId} — watching live progress`
        : '… running — watching live progress';
    // Reset the live panel; if the id is known in advance (broadcast) adopt
    // it now, otherwise the `started` event adopts it.
    liveProgress.spreadId = trackedId;
    liveProgress.started = null;
    liveProgress.nodes = new Map();
    liveProgress.finished = null;
    renderEpidemicProgress(document.getElementById('epidemic-progress'));

    try {
        const result = await apiRequest(apiContext.base, apiContext.key, '/api/epidemic/spread', {
            method: 'POST',
            body: JSON.stringify(payload),
        });
        const record = result?.data?.spread;
        if (record) {
            // Reflect the final record in the live panel (idempotent with
            // the phase=finished websocket event, whichever lands last).
            for (const n of record.nodes || []) {
                if (n.addr || n.name) {
                    liveProgress.nodes.set(n.addr || n.name, {
                        name: n.name, addr: n.addr, ok: n.ok, error: n.error, attempts: n.attempts,
                    });
                }
            }
            liveProgress.finished = {
                spread_id: record.spread_id, applied: record.applied, failed: record.failed, ok: record.ok,
            };
            renderEpidemicProgress(document.getElementById('epidemic-progress'));
        }
        const spreadId = record?.spread_id || payload.spread_id || '(spread)';
        statusEl.textContent = record?.ok
            ? `✓ ${spreadId} — applied=${record.applied} failed=${record.failed}`
            : `✗ ${spreadId} — applied=${record?.applied ?? 0} failed=${record?.failed ?? 0}`;
        if (result?.data?.generated_secret) {
            statusEl.textContent += '  (no epidemic secret was configured — a generated one was used)';
        }
        loadEpidemic(apiContext.base, apiContext.key);
    } catch (error) {
        statusEl.textContent = `✗ ${error.message}`;
    } finally {
        btn.disabled = false;
    }
}
