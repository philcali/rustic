import './style.css'

class PandemicConsole {
  constructor() {
    this.apiUrl = localStorage.getItem('pandemic-api-url') || 'http://localhost:8080';
    this.apiKey = localStorage.getItem('pandemic-api-key') || '';
    this.connected = false;
    this.websocket = null;
    this.reconnectAttempts = 0;
    this.maxReconnectAttempts = 5;
    
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
      websocketStatus: document.getElementById('websocket-status'),
      activePlugins: document.getElementById('active-plugins'),
      memoryUsage: document.getElementById('memory-usage'),
      cpuUsage: document.getElementById('cpu-usage'),
      uptime: document.getElementById('uptime'),
      pluginsList: document.getElementById('plugins-list'),
      apiUrl: document.getElementById('api-url'),
      apiKey: document.getElementById('api-key'),
      connectBtn: document.getElementById('connect-btn'),
      disconnectBtn: document.getElementById('disconnect-btn')
    };
  }

  bindEvents() {
    this.elements.connectBtn.addEventListener('click', () => this.handleConnect());
    this.elements.disconnectBtn.addEventListener('click', () => this.disconnect());
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
      this.updateConnectionStatus('API: Connected', 'status-connected');
      this.elements.connectBtn.style.display = 'none';
      this.elements.disconnectBtn.style.display = 'inline-block';
      
      // Load initial data
      await this.loadDashboard();
      
      // Connect WebSocket for real-time updates
      this.connectWebSocket();
      
      // Start periodic updates (reduced frequency since we have WebSocket)
      this.startPeriodicUpdates();
      
    } catch (error) {
      this.connected = false;
      this.updateConnectionStatus('API: Disconnected', 'status-disconnected');
      this.showError(`Connection failed: ${error.message}`);
    }
  }

  updateConnectionStatus(text, className) {
    this.elements.connectionStatus.textContent = text;
    this.elements.connectionStatus.className = className;
  }

  updateWebSocketStatus(text, className) {
    this.elements.websocketStatus.textContent = text;
    this.elements.websocketStatus.className = className;
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

  connectWebSocket() {
    if (this.websocket) {
      this.websocket.close();
    }

    const wsUrl = this.apiUrl.replace('http://', 'ws://').replace('https://', 'wss://');
    const wsEndpoint = `${wsUrl}/api/events/stream?token=${encodeURIComponent(this.apiKey)}&topics=plugin.*,health.*`;
    
    try {
      this.websocket = new WebSocket(wsEndpoint);
      
      this.websocket.onopen = () => {
        console.log('WebSocket connected');
        this.reconnectAttempts = 0;
        this.updateWebSocketStatus('Events: Connected', 'status-connected');
      };
      
      this.websocket.onmessage = (event) => {
        try {
          const message = JSON.parse(event.data);
          this.handleWebSocketMessage(message);
        } catch (e) {
          console.error('Failed to parse WebSocket message:', e);
        }
      };
      
      this.websocket.onclose = () => {
        console.log('WebSocket disconnected');
        this.websocket = null;
        this.updateWebSocketStatus('Events: Disconnected', 'status-disconnected');
        
        // Attempt to reconnect
        if (this.connected && this.reconnectAttempts < this.maxReconnectAttempts) {
          this.reconnectAttempts++;
          console.log(`Attempting to reconnect WebSocket (${this.reconnectAttempts}/${this.maxReconnectAttempts})`);
          this.updateWebSocketStatus('Events: Reconnecting...', 'status-disconnected');
          setTimeout(() => this.connectWebSocket(), 2000 * this.reconnectAttempts);
        }
      };
      
      this.websocket.onerror = (error) => {
        console.error('WebSocket error:', error);
      };
      
    } catch (error) {
      console.error('Failed to create WebSocket connection:', error);
    }
  }

  handleWebSocketMessage(message) {
    switch (message.type) {
      case 'connected':
        console.log('WebSocket subscription confirmed for topics:', message.topics);
        break;
        
      case 'event':
        this.handleRealtimeEvent(message.data);
        break;
        
      case 'error':
        console.error('WebSocket error:', message.message);
        this.showError(`WebSocket error: ${message.message}`);
        break;
        
      default:
        console.log('Unknown WebSocket message type:', message.type);
    }
  }

  handleRealtimeEvent(event) {
    console.log('Received real-time event:', event);
    
    // Handle different event types
    if (event.topic.startsWith('plugin.')) {
      // Plugin-related events - refresh plugin list
      this.loadPlugins();
    }
    
    if (event.topic.startsWith('health.')) {
      // Health-related events - refresh health metrics
      this.loadHealth();
    }
    
    // Could add event log display here in the future
  }

  startPeriodicUpdates() {
    // Update every 30 seconds (reduced since WebSocket provides real-time updates)
    this.updateInterval = setInterval(() => {
      if (this.connected) {
        this.loadDashboard();
      }
    }, 30000);
  }

  showError(message) {
    // Simple error display - could be enhanced with a proper notification system
    console.error(message);
    alert(message);
  }

  disconnect() {
    this.connected = false;
    
    if (this.websocket) {
      this.websocket.close();
      this.websocket = null;
    }
    
    if (this.updateInterval) {
      clearInterval(this.updateInterval);
      this.updateInterval = null;
    }
    
    this.updateConnectionStatus('API: Disconnected', 'status-disconnected');
    this.updateWebSocketStatus('Events: Disconnected', 'status-disconnected');
    
    this.elements.connectBtn.style.display = 'inline-block';
    this.elements.disconnectBtn.style.display = 'none';
  }
}

// Initialize the console when DOM is loaded
document.addEventListener('DOMContentLoaded', () => {
  new PandemicConsole();
});