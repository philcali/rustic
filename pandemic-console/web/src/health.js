/**
 * System health metrics rendering.
 */

import { apiRequest } from './api.js';

/**
 * Format a duration in seconds to a human-readable string.
 */
export function formatUptime(seconds) {
    const days = Math.floor(seconds / 86400);
    const hours = Math.floor((seconds % 86400) / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);

    if (days > 0) {
        return `${days}d ${hours}h ${minutes}m`;
    } else if (hours > 0) {
        return `${hours}h ${minutes}m`;
    } else {
        return `${minutes}m`;
    }
}

/**
 * Render health metrics into the given container.
 */
export function renderHealth(health, container) {
    container.innerHTML = `
        <div class="health-grid">
            <div class="health-metric">
                <div class="metric-label">Active Plugins</div>
                <div class="metric-value">${health.active_plugins}</div>
            </div>
            <div class="health-metric">
                <div class="metric-label">Total Connections</div>
                <div class="metric-value">${health.total_connections}</div>
            </div>
            <div class="health-metric">
                <div class="metric-label">Memory Usage</div>
                <div class="metric-value">${health.memory_used_mb}MB / ${health.memory_total_mb}MB</div>
            </div>
            <div class="health-metric">
                <div class="metric-label">CPU Usage</div>
                <div class="metric-value">${health.cpu_usage_percent.toFixed(1)}%</div>
            </div>
            <div class="health-metric">
                <div class="metric-label">Uptime</div>
                <div class="metric-value">${formatUptime(health.uptime_seconds)}</div>
            </div>
            <div class="health-metric">
                <div class="metric-label">Event Subscribers</div>
                <div class="metric-value">${health.event_bus_subscribers}</div>
            </div>
        </div>
    `;
}

/**
 * Load health metrics from the API and render them.
 */
export async function loadHealth(apiBase, apiKey) {
    try {
        const result = await apiRequest(apiBase, apiKey, '/api/health');
        renderHealth(result.data, document.getElementById('health-metrics'));
    } catch (error) {
        document.getElementById('health-metrics').innerHTML =
            `<div class="error">Failed to load health metrics: ${error.message}</div>`;
    }
}
