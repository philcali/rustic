import './style.css'

class PandemicConsole {
  constructor() {
    this.apiUrl = localStorage.getItem('pandemic-api-url') || 'http://localhost:8080';
    this.apiKey = localStorage.getItem('pandemic-api-key') || '';
    this.connected = false;
    
    this.initializeElements();
    this.bindEvents();
    this.loadSettings();
    
    if (this.apiKey) {
      this.connect();
    }
  }

  initializeElements() {
    this.elements = {
      connectionStatus: document.getElementById('connection-status'),
      activePlugins: document.getElementById('active-plugins'),
      memoryUsage: document.getElementById('memory-usage'),
      cpuUsage: document.getElementById('cpu-usage'),
      uptime: document.getElementById('uptime'),
      pluginsList: document.getElementById('plugins-list'),
      apiUrl: document.getElementById('api-url'),
      apiKey: document.getElementById('api-key'),
      connectBtn: document.getElementById('connect-btn')
    };
  }

  bindEvents() {
    this.elements.connectBtn.addEventListener('click', () => this.handleConnect());
    this.elements.apiUrl.addEventListener('change', () => this.saveSettings());
    this.elements.apiKey.addEventListener('change', () => this.saveSettings());
  }

  loadSettings() {
    this.elements.apiUrl.value = this.apiUrl;
    this.elements.apiKey.value = this.apiKey;
  }

  saveSettings() {
    this.apiUrl = this.elements.apiUrl.value;
    this.apiKey = this.elements.apiKey.value;
    
    localStorage.setItem('pandemic-api-url', this.apiUrl);
    localStorage.setItem('pandemic-api-key', this.apiKey);
  }

  async handleConnect() {
    this.saveSettings();
    
    if (!this.apiKey) {
      this.showError('Please enter an API key');
      return;
    }

    await this.connect();
  }

  async connect() {
    try {
      this.updateConnectionStatus('Connecting...', 'status-disconnected');
      
      // Test connection with health endpoint
      await this.fetchHealth();
      
      this.connected = true;
      this.updateConnectionStatus('Connected', 'status-connected');
      
      // Load initial data
      await this.loadDashboard();
      
      // Start periodic updates
      this.startPeriodicUpdates();
      
    } catch (error) {
      this.connected = false;
      this.updateConnectionStatus('Disconnected', 'status-disconnected');
      this.showError(`Connection failed: ${error.message}`);
    }
  }

  updateConnectionStatus(text, className) {
    this.elements.connectionStatus.textContent = text;
    this.elements.connectionStatus.className = className;
  }

  async loadDashboard() {
    await Promise.all([
      this.loadHealth(),
      this.loadPlugins()
    ]);
  }

  async loadHealth() {
    try {
      const health = await this.fetchHealth();
      this.updateHealthMetrics(health);
    } catch (error) {
      console.error('Failed to load health:', error);
    }
  }

  async loadPlugins() {
    try {
      const plugins = await this.fetchPlugins();
      this.updatePluginsList(plugins);
    } catch (error) {
      console.error('Failed to load plugins:', error);
      this.elements.pluginsList.innerHTML = '<div class="error">Failed to load plugins</div>';
    }
  }

  async fetchHealth() {
    const response = await this.apiRequest('/api/health');
    return response.data;
  }

  async fetchPlugins() {
    const response = await this.apiRequest('/api/plugins');
    return response.data;
  }

  async apiRequest(endpoint) {
    const response = await fetch(`${this.apiUrl}${endpoint}`, {
      headers: {
        'Authorization': `Bearer ${this.apiKey}`,
        'Content-Type': 'application/json'
      }
    });

    if (!response.ok) {
      throw new Error(`HTTP ${response.status}: ${response.statusText}`);
    }

    const data = await response.json();
    
    if (data.status !== 'success') {
      throw new Error(data.message || 'API request failed');
    }

    return data;
  }

  updateHealthMetrics(health) {
    this.elements.activePlugins.textContent = health.active_plugins;
    this.elements.memoryUsage.textContent = `${health.memory_used_mb}MB / ${health.memory_total_mb}MB`;
    this.elements.cpuUsage.textContent = `${health.cpu_usage_percent.toFixed(1)}%`;
    this.elements.uptime.textContent = this.formatUptime(health.uptime_seconds);
  }

  updatePluginsList(plugins) {
    if (!plugins || plugins.length === 0) {
      this.elements.pluginsList.innerHTML = '<div class="loading">No plugins registered</div>';
      return;
    }

    const pluginsHtml = plugins.map(plugin => `
      <div class="plugin-item">
        <div>
          <div class="plugin-name">${plugin.name}</div>
          <div class="plugin-version">v${plugin.version}</div>
          ${plugin.description ? `<div class="plugin-description">${plugin.description}</div>` : ''}
        </div>
        <div class="plugin-status status-active">Active</div>
      </div>
    `).join('');

    this.elements.pluginsList.innerHTML = pluginsHtml;
  }

  formatUptime(seconds) {
    const hours = Math.floor(seconds / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    const secs = seconds % 60;
    
    if (hours > 0) {
      return `${hours}h ${minutes}m`;
    } else if (minutes > 0) {
      return `${minutes}m ${secs}s`;
    } else {
      return `${secs}s`;
    }
  }

  startPeriodicUpdates() {
    // Update every 5 seconds
    this.updateInterval = setInterval(() => {
      if (this.connected) {
        this.loadDashboard();
      }
    }, 5000);
  }

  showError(message) {
    // Simple error display - could be enhanced with a proper notification system
    console.error(message);
    alert(message);
  }
}

// Initialize the console when DOM is loaded
document.addEventListener('DOMContentLoaded', () => {
  new PandemicConsole();
});