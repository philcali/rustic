/**
 * Plugin list rendering and loading.
 */

import { apiRequest } from './api.js';

/**
 * Render the list of registered plugins into the given container.
 */
export function renderPlugins(plugins, container) {
    if (plugins.length === 0) {
        container.innerHTML = '<div class="empty">No plugins registered</div>';
        return;
    }

    container.innerHTML = plugins.map(plugin => `
        <div class="plugin-item">
            <div class="plugin-info">
                <strong>${plugin.name}</strong>
                <span class="version">v${plugin.version}</span>
            </div>
            <div class="plugin-description">${plugin.description || 'No description'}</div>
        </div>
    `).join('');
}

/**
 * Load plugins from the API and render them.
 */
export async function loadPlugins(apiBase, apiKey) {
    try {
        const result = await apiRequest(apiBase, apiKey, '/api/plugins');
        renderPlugins(result.data || [], document.getElementById('plugins-list'));
    } catch (error) {
        document.getElementById('plugins-list').innerHTML =
            `<div class="error">Failed to load plugins: ${error.message}</div>`;
    }
}
