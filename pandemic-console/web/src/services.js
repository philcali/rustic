/**
 * Service management rendering and actions.
 */

import { apiRequest } from './api.js';

/**
 * Render the list of systemd services into the given container.
 */
export function renderServices(services, agentCapabilities, container) {
    if (services.length === 0) {
        container.innerHTML = '<div class="empty">No pandemic services found</div>';
        return;
    }

    container.innerHTML = services.map(service => `
        <div class="service-item">
            <div class="service-info">
                <strong>${service.name}</strong>
                <span class="status status-${service.status}">${service.status}</span>
            </div>
            <div class="service-description">${service.description}</div>
            <div class="service-actions">
                <button onclick="window.pandemicConsole.controlService('${service.name}', 'start')">Start</button>
                <button onclick="window.pandemicConsole.controlService('${service.name}', 'stop')">Stop</button>
                <button onclick="window.pandemicConsole.controlService('${service.name}', 'restart')">Restart</button>
                <button onclick="window.pandemicConsole.toggleServiceConfig('${service.name}')">Config</button>
            </div>
            <div id="config-${service.name}" class="service-config" style="display: none;">
                <div class="config-actions">
                    <button onclick="window.pandemicConsole.showServiceConfig('${service.name}')">Show</button>
                    <button onclick="window.pandemicConsole.resetServiceConfig('${service.name}')">Reset</button>
                </div>
                <div id="config-details-${service.name}" class="config-details"></div>
            </div>
        </div>
    `).join('');
}

/**
 * Load services from the API (if agent has systemd capability) and render them.
 */
export async function loadServices(apiBase, apiKey, agentCapabilities) {
    if (!agentCapabilities.includes('systemd')) return;

    try {
        const result = await apiRequest(apiBase, apiKey, '/api/admin/services');
        renderServices(result.data?.services || [], agentCapabilities, document.getElementById('services-list'));
    } catch (error) {
        document.getElementById('services-list').innerHTML =
            `<div class="error">Failed to load services: ${error.message}</div>`;
    }
}

/**
 * Toggle the visibility of a service config panel.
 */
export function toggleServiceConfig(serviceName) {
    const configDiv = document.getElementById(`config-${serviceName}`);
    const isVisible = configDiv.style.display !== 'none';
    configDiv.style.display = isVisible ? 'none' : 'block';
}

/**
 * Fetch and display a service's configuration override.
 */
export async function showServiceConfig(serviceName, agentCapabilities, apiBase, apiKey) {
    if (!agentCapabilities.includes('service_config')) return;

    try {
        const result = await apiRequest(apiBase, apiKey, `/api/admin/services/${serviceName}/config`);
        const configDetails = document.getElementById(`config-details-${serviceName}`);

        if (result.data && result.data.config) {
            const config = result.data.config;
            configDetails.innerHTML = `
                <div class="config-display">
                    <h4>Current Configuration:</h4>
                    <pre>${JSON.stringify(config, null, 2)}</pre>
                </div>
            `;
        } else {
            configDetails.innerHTML = '<div class="empty">No configuration overrides</div>';
        }
    } catch (error) {
        const configDetails = document.getElementById(`config-details-${serviceName}`);
        configDetails.innerHTML = `<div class="error">Failed to load config: ${error.message}</div>`;
    }
}

/**
 * Reset a service's configuration override.
 */
export async function resetServiceConfig(serviceName, apiBase, apiKey) {
    if (!confirm(`Reset configuration for ${serviceName}?`)) return;

    try {
        await apiRequest(apiBase, apiKey, `/api/admin/services/${serviceName}/config`, { method: 'DELETE' });
        const configDetails = document.getElementById(`config-details-${serviceName}`);
        configDetails.innerHTML = '<div class="success">Configuration reset successfully</div>';
    } catch (error) {
        alert(`Failed to reset config: ${error.message}`);
    }
}

/**
 * Control a service (start/stop/restart) and reload the list.
 */
export async function controlService(serviceName, action, apiBase, apiKey) {
    try {
        await apiRequest(apiBase, apiKey, `/api/admin/services/${serviceName}/action`, {
            method: 'POST',
            body: JSON.stringify({ action })
        });
        setTimeout(() => loadServices(apiBase, apiKey, []), 1000);
    } catch (error) {
        alert(`Failed to ${action} service: ${error.message}`);
    }
}
