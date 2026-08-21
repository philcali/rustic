/**
 * User management rendering and actions.
 */

import { apiRequest } from './api.js';

/**
 * Render the list of system users into the given container.
 */
export function renderUsers(users, agentCapabilities, container) {
    if (users.length === 0) {
        container.innerHTML = '<div class="empty">No users found</div>';
        return;
    }

    container.innerHTML = users.map(user => `
        <div class="user-item">
            <div class="user-info">
                <strong>${user}</strong>
            </div>
            <div class="user-actions">
                <button onclick="window.pandemicConsole.deleteUser('${user}')" class="danger">Delete</button>
            </div>
        </div>
    `).join('');
}

/**
 * Load users from the API (if agent has user_management capability) and render them.
 */
export async function loadUsers(apiBase, apiKey, agentCapabilities) {
    if (!agentCapabilities.includes('user_management')) return;

    try {
        const result = await apiRequest(apiBase, apiKey, '/api/admin/users');
        renderUsers(result.data?.users || [], agentCapabilities, document.getElementById('users-list'));
    } catch (error) {
        document.getElementById('users-list').innerHTML =
            `<div class="error">Failed to load users: ${error.message}</div>`;
    }
}

/**
 * Delete a system user after confirmation.
 */
export async function deleteUser(username, apiBase, apiKey, reloadFn) {
    if (!confirm(`Delete user ${username}?`)) return;

    try {
        await apiRequest(apiBase, apiKey, `/api/admin/users/${username}`, { method: 'DELETE' });
        reloadFn();
    } catch (error) {
        alert(`Failed to delete user: ${error.message}`);
    }
}
