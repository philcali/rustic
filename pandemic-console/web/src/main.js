import './style.css'

class PandemicConsole {
    constructor() {
        this.apiBase = localStorage.getItem('pandemic-api-url') || 'http://localhost:8080';
        this.apiKey = localStorage.getItem('pandemic-api-key') || '';
        this.agentCapabilities = [];
        this.websocket = null;
        this.init();
    }

    async init() {
        this.render();
        await this.checkAgentCapabilities();
        this.setupEventListeners();
        this.loadHealth();
        this.setupWebSocket();
        this.loadPlugins();
        this.loadServices();
    }

    render() {
        document.querySelector('#app').innerHTML = `
            <div class="pandemic-console">
                <header>
                    <h1>🦠 Pandemic Console</h1>
                    <div class="auth-section">
                        <input type="password" id="api-key" placeholder="API Key" value="${this.apiKey}">
                        <button id="save-key">Save</button>
                    </div>
                </header>

                <main>
                    <section class="health-section">
                        <h2>📊 System Health</h2>
                        <div id="health-metrics" class="health-container">
                            <div class="loading">Loading health metrics...</div>
                        </div>
                    </section>

                    <section class="plugins-section">
                        <h2>Registered Plugins</h2>
                        <div id="plugins-list" class="list-container">
                            <div class="loading">Loading plugins...</div>
                        </div>
                    </section>

                    <section class="admin-section" id="admin-section" style="display: none;">
                        <h2>🔧 System Administration</h2>
                        <div class="admin-capabilities">
                            <span>Agent Status: <span id="agent-status">Unknown</span></span>
                            <span>Capabilities: <span id="agent-capabilities">None</span></span>
                        </div>

                        <div class="admin-tabs">
                            <button class="tab-button active" data-tab="services">Services</button>
                            <button class="tab-button" data-tab="users">Users</button>
                            <button class="tab-button" data-tab="groups">Groups</button>
                            <button class="tab-button" data-tab="registry">Registry</button>
                        </div>

                        <div class="tab-content">
                            <div id="services-tab" class="tab-panel active">
                                <div id="services-list" class="list-container">
                                    <div class="loading">Loading services...</div>
                                </div>
                            </div>

                            <div id="users-tab" class="tab-panel">
                                <div id="users-list" class="list-container">
                                    <div class="loading">Loading users...</div>
                                </div>
                            </div>

                            <div id="groups-tab" class="tab-panel">
                                <div id="groups-list" class="list-container">
                                    <div class="loading">Loading groups...</div>
                                </div>
                            </div>

                            <div id="registry-tab" class="tab-panel">
                                <div class="registry-container">
                                    <div class="registry-search">
                                        <input type="text" id="registry-search" placeholder="Search infections...">
                                        <button id="search-button">Search</button>
                                    </div>
                                    <div id="registry-results" class="list-container">
                                        <div class="empty">Enter a search term to find infections</div>
                                    </div>
                                </div>
                            </div>

                        </div>
                    </section>
                </main>
            </div>
        `;
    }

    setupEventListeners() {
        document.getElementById('save-key').addEventListener('click', () => {
            this.apiKey = document.getElementById('api-key').value;
            localStorage.setItem('pandemic-api-key', this.apiKey);
            this.loadHealth();
            this.loadPlugins();
            this.checkAgentCapabilities();
            this.setupWebSocket();
        });

        // Tab switching
        document.querySelectorAll('.tab-button').forEach(button => {
            button.addEventListener('click', (e) => {
                const tabName = e.target.dataset.tab;
                this.switchTab(tabName);
            });
        });

        // Registry search
        document.getElementById('search-button').addEventListener('click', () => {
            this.searchInfections();
        });
        
        document.getElementById('registry-search').addEventListener('keypress', (e) => {
            if (e.key === 'Enter') {
                this.searchInfections();
            }
        });
    }

    async apiRequest(endpoint, options = {}) {
        const response = await fetch(`${this.apiBase}${endpoint}`, {
            headers: {
                'Authorization': `Bearer ${this.apiKey}`,
                'Content-Type': 'application/json',
                ...options.headers
            },
            ...options
        });
        
        if (!response.ok) {
            throw new Error(`API Error: ${response.status}`);
        }
        
        return response.json();
    }

    async checkAgentCapabilities() {
        try {
            const result = await this.apiRequest('/api/admin/capabilities');
            const data = result.data;
            
            document.getElementById('agent-status').textContent = 
                data.agent_available ? 'Available' : 'Unavailable';
            document.getElementById('agent-capabilities').textContent = 
                data.capabilities.join(', ') || 'None';
            
            this.agentCapabilities = data.capabilities;
            
            // Show/hide admin section based on agent availability
            const adminSection = document.getElementById('admin-section');
            if (data.agent_available && data.capabilities.length > 0) {
                adminSection.style.display = 'block';
                this.loadServices();
                this.loadUsers();
                this.loadGroups();
            } else {
                adminSection.style.display = 'none';
            }
        } catch (error) {
            console.log('Agent capabilities check failed:', error.message);
            document.getElementById('admin-section').style.display = 'none';
        }
    }

    async loadHealth() {
        try {
            const result = await this.apiRequest('/api/health');
            const health = result.data;
            
            const container = document.getElementById('health-metrics');
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
                        <div class="metric-value">${this.formatUptime(health.uptime_seconds)}</div>
                    </div>
                    <div class="health-metric">
                        <div class="metric-label">Event Subscribers</div>
                        <div class="metric-value">${health.event_bus_subscribers}</div>
                    </div>
                </div>
            `;
        } catch (error) {
            document.getElementById('health-metrics').innerHTML = 
                `<div class="error">Failed to load health metrics: ${error.message}</div>`;
        }
    }

    setupWebSocket() {
        if (this.websocket) {
            this.websocket.close();
        }
        
        if (!this.apiKey) return;

        const parsedUrl = new URL(this.apiBase);
        const wsProtocol = parsedUrl.protocol === 'https' ? 'wss' : 'ws';
        const wsPort = parsedUrl.port ? `:${parsedUrl.port}` : '';
        console.log('Setting up WebSocket connection...');
        const wsUrl = `${wsProtocol}://${parsedUrl.hostname}${wsPort}/api/events/stream?token=${this.apiKey}`;
        this.websocket = new WebSocket(wsUrl);
        
        this.websocket.onopen = () => {
            console.log('WebSocket connected for real-time updates');
        };
        
        this.websocket.onmessage = (event) => {
            try {
                const data = JSON.parse(event.data);
                this.handleRealtimeEvent(data);
            } catch (error) {
                console.error('Failed to parse WebSocket message:', error);
            }
        };
        
        this.websocket.onclose = () => {
            console.log('WebSocket disconnected');
            // Reconnect after 5 seconds
            setTimeout(() => this.setupWebSocket(), 5000);
        };
        
        this.websocket.onerror = (error) => {
            console.error('WebSocket error:', error);
        };
    }

    handleRealtimeEvent(event) {
        // Handle different event types for real-time updates
        switch (event.topic) {
            case 'plugin.registered':
            case 'plugin.deregistered':
                this.loadPlugins();
                break;
            case 'health.updated':
                this.loadHealth();
                break;
            case 'service.status_changed':
                this.loadServices();
                break;
        }
    }

    formatUptime(seconds) {
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

    switchTab(tabName) {
        // Remove active class from all tabs and panels
        document.querySelectorAll('.tab-button').forEach(btn => btn.classList.remove('active'));
        document.querySelectorAll('.tab-panel').forEach(panel => panel.classList.remove('active'));

        // Add active class to selected tab and panel
        document.querySelector(`[data-tab="${tabName}"]`).classList.add('active');
        document.getElementById(`${tabName}-tab`).classList.add('active');

        // Load data for the selected tab
        switch(tabName) {
            case 'services': this.loadServices(); break;
            case 'users': this.loadUsers(); break;
            case 'groups': this.loadGroups(); break;
            case 'registry': break; // Registry is search-based
        }
    }

    async loadPlugins() {
        try {
            const result = await this.apiRequest('/api/plugins');
            const plugins = result.data || [];

            const container = document.getElementById('plugins-list');
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
        } catch (error) {
            document.getElementById('plugins-list').innerHTML = 
                `<div class="error">Failed to load plugins: ${error.message}</div>`;
        }
    }

    async loadServices() {
        if (!this.agentCapabilities.includes('systemd')) return;

        try {
            const result = await this.apiRequest('/api/admin/services');
            const services = result.data?.services || [];

            const container = document.getElementById('services-list');
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
                        <button onclick="pandemicConsole.controlService('${service.name}', 'start')">Start</button>
                        <button onclick="pandemicConsole.controlService('${service.name}', 'stop')">Stop</button>
                        <button onclick="pandemicConsole.controlService('${service.name}', 'restart')">Restart</button>
                        <button onclick="pandemicConsole.toggleServiceConfig('${service.name}')">Config</button>
                    </div>
                    <div id="config-${service.name}" class="service-config" style="display: none;">
                        <div class="config-actions">
                            <button onclick="pandemicConsole.showServiceConfig('${service.name}')">Show</button>
                            <button onclick="pandemicConsole.resetServiceConfig('${service.name}')">Reset</button>
                        </div>
                        <div id="config-details-${service.name}" class="config-details"></div>
                    </div>
                </div>
            `).join('');
        } catch (error) {
            document.getElementById('services-list').innerHTML = 
                `<div class="error">Failed to load services: ${error.message}</div>`;
        }
    }

    async controlService(serviceName, action) {
        try {
            await this.apiRequest(`/api/admin/services/${serviceName}/action`, {
                method: 'POST',
                body: JSON.stringify({ action })
            });

            // Reload services to show updated status
            setTimeout(() => this.loadServices(), 1000);
        } catch (error) {
            alert(`Failed to ${action} service: ${error.message}`);
        }
    }

    async loadUsers() {
        if (!this.agentCapabilities.includes('user_management')) return;

        try {
            const result = await this.apiRequest('/api/admin/users');
            const users = result.data?.users || [];

            const container = document.getElementById('users-list');
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
                        <button onclick="pandemicConsole.deleteUser('${user}')" class="danger">Delete</button>
                    </div>
                </div>
            `).join('');
        } catch (error) {
            document.getElementById('users-list').innerHTML = 
                `<div class="error">Failed to load users: ${error.message}</div>`;
        }
    }

    async loadGroups() {
        if (!this.agentCapabilities.includes('group_management')) return;

        try {
            const result = await this.apiRequest('/api/admin/groups');
            const groups = result.data?.groups || [];

            const container = document.getElementById('groups-list');
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
                        <button onclick="pandemicConsole.deleteGroup('${group}')" class="danger">Delete</button>
                    </div>
                </div>
            `).join('');
        } catch (error) {
            document.getElementById('groups-list').innerHTML =
                `<div class="error">Failed to load groups: ${error.message}</div>`;
        }
    }

    async deleteUser(username) {
        if (!confirm(`Delete user ${username}?`)) return;

        try {
            await this.apiRequest(`/api/admin/users/${username}`, { method: 'DELETE' });
            this.loadUsers();
        } catch (error) {
            alert(`Failed to delete user: ${error.message}`);
        }
    }

    async deleteGroup(groupname) {
        if (!confirm(`Delete group ${groupname}?`)) return;

        try {
            await this.apiRequest(`/api/admin/groups/${groupname}`, { method: 'DELETE' });
            this.loadGroups();
        } catch (error) {
            alert(`Failed to delete group: ${error.message}`);
        }
    }

    toggleServiceConfig(serviceName) {
        const configDiv = document.getElementById(`config-${serviceName}`);
        const isVisible = configDiv.style.display !== 'none';
        configDiv.style.display = isVisible ? 'none' : 'block';
    }

    async showServiceConfig(serviceName) {
        if (!this.agentCapabilities.includes('service_config')) return;

        try {
            const result = await this.apiRequest(`/api/admin/services/${serviceName}/config`);
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

    async resetServiceConfig(serviceName) {
        if (!confirm(`Reset configuration for ${serviceName}?`)) return;

        try {
            await this.apiRequest(`/api/admin/services/${serviceName}/config`, { method: 'DELETE' });
            const configDetails = document.getElementById(`config-details-${serviceName}`);
            configDetails.innerHTML = '<div class="success">Configuration reset successfully</div>';
        } catch (error) {
            alert(`Failed to reset config: ${error.message}`);
        }
    }

    async searchInfections() {
        const query = document.getElementById('registry-search').value.trim();
        if (!query) return;

        const container = document.getElementById('registry-results');
        container.innerHTML = '<div class="loading">Searching infections...</div>';

        try {
            const result = await this.apiRequest(`/api/admin/registry/search?q=${encodeURIComponent(query)}`);
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
                        <button onclick="pandemicConsole.viewInfectionManifest('${infection.name}')">View Details</button>
                        <button onclick="pandemicConsole.installInfection('${infection.name}')" class="primary">Install</button>
                    </div>
                    <div id="manifest-${infection.name}" class="infection-manifest" style="display: none;"></div>
                </div>
            `).join('');
        } catch (error) {
            container.innerHTML = `<div class="error">Search failed: ${error.message}</div>`;
        }
    }

    async viewInfectionManifest(infectionName) {
        const manifestDiv = document.getElementById(`manifest-${infectionName}`);
        const isVisible = manifestDiv.style.display !== 'none';
        
        if (isVisible) {
            manifestDiv.style.display = 'none';
            return;
        }

        manifestDiv.innerHTML = '<div class="loading">Loading manifest...</div>';
        manifestDiv.style.display = 'block';

        try {
            const result = await this.apiRequest(`/api/admin/registry/infections/${infectionName}`);
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

    async installInfection(infectionName) {
        if (!confirm(`Install infection '${infectionName}'?`)) return;

        try {
            const result = await this.apiRequest(`/api/admin/registry/infections/${infectionName}/install`, {
                method: 'POST',
                body: JSON.stringify({}),
            });

            if (result.status === 'Success') {
                alert(`Successfully installed ${infectionName}`);
                // Refresh plugins list to show the new infection
                this.loadPlugins();
            } else {
                alert(`Installation failed: ${result.message || 'Unknown error'}`);
            }
        } catch (error) {
            alert(`Installation failed: ${error.message}`);
        }
    }
}

// Initialize the console
window.pandemicConsole = new PandemicConsole();