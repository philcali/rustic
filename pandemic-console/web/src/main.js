import './style.css'
import { apiRequest } from './api.js'
import { setupWebSocket } from './websocket.js'
import { loadHealth } from './health.js'
import { loadPlugins } from './plugins.js'
import { loadServices, toggleServiceConfig, showServiceConfig, resetServiceConfig, controlService } from './services.js'
import { loadUsers, deleteUser } from './users.js'
import { loadGroups, deleteGroup } from './groups.js'
import { searchInfections, viewInfectionManifest, installInfection } from './registry.js'
import { setupTabs } from './tabs.js'

class PandemicConsole {
    constructor() {
        this.apiBase = localStorage.getItem('pandemic-api-url') || `${window.location.protocol}//${window.location.hostname}:8080`;
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

    async checkAgentCapabilities() {
        try {
            const result = await apiRequest(this.apiBase, this.apiKey, '/api/admin/capabilities');
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
        await loadHealth(this.apiBase, this.apiKey);
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
        await loadPlugins(this.apiBase, this.apiKey);
    }

    async loadServices() {
        await loadServices(this.apiBase, this.apiKey, this.agentCapabilities);
    }

    async controlService(serviceName, action) {
        await controlService(serviceName, action, this.apiBase, this.apiKey);
    }

    async loadUsers() {
        await loadUsers(this.apiBase, this.apiKey, this.agentCapabilities);
    }

    async loadGroups() {
        await loadGroups(this.apiBase, this.apiKey, this.agentCapabilities);
    }

    async deleteUser(username) {
        await deleteUser(username, this.apiBase, this.apiKey, () => this.loadUsers());
    }

    async deleteGroup(groupname) {
        await deleteGroup(groupname, this.apiBase, this.apiKey, () => this.loadGroups());
    }

    toggleServiceConfig(serviceName) {
        toggleServiceConfig(serviceName);
    }

    async showServiceConfig(serviceName) {
        if (!this.agentCapabilities.includes('service_config')) return;
        await showServiceConfig(serviceName, this.agentCapabilities, this.apiBase, this.apiKey);
    }

    async resetServiceConfig(serviceName) {
        if (!confirm(`Reset configuration for ${serviceName}?`)) return;
        await resetServiceConfig(serviceName, this.apiBase, this.apiKey);
    }

    async searchInfections() {
        await searchInfections(this.apiBase, this.apiKey, document.getElementById('registry-results'));
    }

    async viewInfectionManifest(infectionName) {
        await viewInfectionManifest(infectionName, this.apiBase, this.apiKey);
    }

    async installInfection(infectionName) {
        await installInfection(infectionName, this.apiBase, this.apiKey, () => this.loadPlugins());
    }
}

// Initialize the console
window.pandemicConsole = new PandemicConsole();