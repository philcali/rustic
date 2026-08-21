/**
 * Group management rendering and actions.
 */

import { apiRequest } from './api.js';

/**
 * Render the list of system groups into the given container.
 */
export function renderGroups(groups, agentCapabilities, container) {
    if (groups.length === 0) {
        container.innerHTML = '<div class="empty">No groups found</div>';
        return;
    }

    container.innerHTML = groups.map(group => `
        <div class="group-item">
            <div class="group-info">
                <strong>${group}</strong>
            </div>
            <div class="group-actions">
                <button onclick="window.pandemicConsole.deleteGroup('${group}')" class="danger">Delete</button>
            </div>
        </div>
    `).join('');
}

/**
 * Load groups from the API (if agent has group_management capability) and render them.
 */
export async function loadGroups(apiBase, apiKey, agentCapabilities) {
    if (!agentCapabilities.includes('group_management')) return;

    try {
        const result = await apiRequest(apiBase, apiKey, '/api/admin/groups');
        renderGroups(result.data?.groups || [], agentCapabilities, document.getElementById('groups-list'));
    } catch (error) {
        document.getElementById('groups-list').innerHTML =
            `<div class="error">Failed to load groups: ${error.message}</div>`;
    }
}

/**
 * Delete a system group after confirmation.
 */
export async function deleteGroup(groupname, apiBase, apiKey, reloadFn) {
    if (!confirm(`Delete group ${groupname}?`)) return;

    try {
        await apiRequest(apiBase, apiKey, `/api/admin/groups/${groupname}`, { method: 'DELETE' });
        reloadFn();
    } catch (error) {
        alert(`Failed to delete group: ${error.message}`);
    }
}
