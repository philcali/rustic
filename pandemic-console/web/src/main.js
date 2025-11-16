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
                        
                        <div class="services-container">
                            <h3>Pandemic Services</h3>
                            <div id="services-list" class="list-container">
                                <div class="loading">Loading services...</div>
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
}

// Initialize the console
window.pandemicConsole = new PandemicConsole();