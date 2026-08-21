/**
 * Infection registry search and management.
 */

import { apiRequest } from './api.js';

/**
 * Search the infection registry and render results.
 */
export async function searchInfections(apiBase, apiKey, container) {
    const query = document.getElementById('registry-search').value.trim();
    if (!query) return;

    container.innerHTML = '<div class="loading">Searching infections...</div>';

    try {
        const result = await apiRequest(apiBase, apiKey, `/api/admin/registry/search?q=${encodeURIComponent(query)}`);
        const infections = result.data?.infections || [];

        if (infections.length === 0) {
            container.innerHTML = '<div class="empty">No infections found</div>';
            return;
        }

        container.innerHTML = infections.map(infection => `
            <div class="infection-item">
                <div class="infection-info">
                    <strong>${infection.name}</strong>
                    <span class="version">v${infection.latest_version}</span>
                </div>
                <div class="infection-description">${infection.description || 'No description'}</div>
                <div class="infection-meta">
                    <span>Type: ${infection.type || 'Unknown'}</span>
                    <span>Repository: <a href="${infection.manifest_url || 'N/A'}">[Link]</a></span>
                </div>
                <div class="infection-actions">
                    <button onclick="window.pandemicConsole.viewInfectionManifest('${infection.name}')">View Details</button>
                    <button onclick="window.pandemicConsole.installInfection('${infection.name}')" class="primary">Install</button>
                </div>
                <div id="manifest-${infection.name}" class="infection-manifest" style="display: none;"></div>
            </div>
        `).join('');
    } catch (error) {
        container.innerHTML = `<div class="error">Search failed: ${error.message}</div>`;
    }
}

/**
 * Fetch and display an infection's manifest.
 */
export async function viewInfectionManifest(infectionName, apiBase, apiKey) {
    const manifestDiv = document.getElementById(`manifest-${infectionName}`);
    const isVisible = manifestDiv.style.display !== 'none';

    if (isVisible) {
        manifestDiv.style.display = 'none';
        return;
    }

    manifestDiv.innerHTML = '<div class="loading">Loading manifest...</div>';
    manifestDiv.style.display = 'block';

    try {
        const result = await apiRequest(apiBase, apiKey, `/api/admin/registry/infections/${infectionName}`);
        const manifest = result.data;

        manifestDiv.innerHTML = `
            <div class="manifest-display">
                <h4>Infection Manifest:</h4>
                <div class="manifest-details">
                    <p><strong>Name:</strong> ${manifest.name}</p>
                    <p><strong>Version:</strong> ${manifest.version}</p>
                    <p><strong>Description:</strong> ${manifest.description || 'N/A'}</p>
                    <p><strong>Author:</strong> ${manifest.author || 'Unknown'}</p>
                    <p><strong>License:</strong> ${manifest.license || 'N/A'}</p>
                    ${manifest.keywords && manifest.keywords.length > 0 ?
                        `<p><strong>Keywords:</strong> ${manifest.keywords.map(k => `<span class="version">${k}</span>`).join(' ')}</p>` : ''}
                    ${manifest.dependencies && manifest.dependencies.length > 0 ?
                        `<p><strong>Dependencies:</strong> ${manifest.dependencies.join(', ')}</p>` : ''}
                    ${manifest.platforms && manifest.platforms.length > 0 ?
                        `<p><strong>Platforms:</strong> ${manifest.platforms.map(p => `<span class="version">${p.arch}</span>`).join(' ')}</p>` : ''}
                </div>
                ${manifest.readme ? `
                    <div class="manifest-readme">
                        <h5>README:</h5>
                        <pre>${manifest.readme}</pre>
                    </div>
                ` : ''}
            </div>
        `;
    } catch (error) {
        manifestDiv.innerHTML = `<div class="error">Failed to load manifest: ${error.message}</div>`;
    }
}

/**
 * Install an infection.
 */
export async function installInfection(infectionName, apiBase, apiKey, reloadPlugins) {
    if (!confirm(`Install infection '${infectionName}'?`)) return;

    try {
        const result = await apiRequest(apiBase, apiKey, `/api/admin/registry/infections/${infectionName}/install`, {
            method: 'POST',
            body: JSON.stringify({}),
        });

        if (result.status === 'Success') {
            alert(`Successfully installed ${infectionName}`);
            reloadPlugins();
        } else {
            alert(`Installation failed: ${result.message || 'Unknown error'}`);
        }
    } catch (error) {
        alert(`Installation failed: ${error.message}`);
    }
}
