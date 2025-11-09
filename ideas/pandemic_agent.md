# Pandemic Agent Architecture

## Overview

A dual-binary approach for privileged system management while maintaining pandemic's security model. The agent provides **additive** host control capabilities without compromising the core daemon's simplicity.

## Architecture

### Dual Binary Design

```
┌──────────────────┐    ┌──────────────────┐
│ pandemic-daemon  │    │ pandemic-agent   │
│ (unprivileged)   │    │ (root)           │
│ pandemic:pandemic│    │ root:root        │
│ user.sock        │    │ admin.sock       │
└──────────────────┘    └──────────────────┘
```

### Components

#### **1. Pandemic Agent Binary** (privileged)
- **Process**: Separate systemd service running as root
- **Socket**: `/var/run/pandemic/admin.sock` (root:root 600)
- **Protocol**: Reuses pandemic protocol for consistency
- **Scope**: Host control operations only

#### **2. Agent Integration Layer** (unprivileged)
- **REST API**: Admin-scoped routes in pandemic-rest
- **Console**: Admin UI components in pandemic-console  
- **Events**: Agent status/capability discovery via event system

### Communication Flow

```
Console (admin UI) → REST API (admin scope) → Agent Socket → Agent Binary → System
     ↓                      ↓                     ↓              ↓
  Web UI              Auth validation        UDS protocol    systemctl/host ops
```

## Layered Integration

### **Layer 1: Agent Binary**
```rust
// pandemic-agent: Minimal privileged binary
- systemd service management
- network configuration  
- package management
- user/group management
- firewall rules
```

### **Layer 2: REST API Integration**
```rust
// pandemic-rest: Admin scope routing
if auth.has_scope("admin") {
    let client = DaemonClient::connect("/var/run/pandemic/admin.sock").await?;
    client.send_request(&admin_request).await
}
```

### **Layer 3: Console Enhancement**
```javascript
// pandemic-console: Admin UI discovery
pandemic.subscribe("agent.capabilities", (caps) => {
    if (caps.includes("systemd")) enableServiceManagement();
    if (caps.includes("network")) enableNetworkConfig();
});
```

## Deployment Models

### **Basic Deployment** (existing)
```bash
pandemic-cli bootstrap install
# → Only pandemic.service (unprivileged)
```

### **Admin-Enabled Deployment** (new)
```bash
pandemic-cli bootstrap install --with-agent
# → pandemic.service + pandemic-agent.service
# → Enables admin scopes in REST API
# → Unlocks admin features in console
```

## Agent Protocol

### **Request Types**
```rust
enum AgentRequest {
    // Service management
    SystemdControl { action: String, service: String },
    ServiceInstall { name: String, config: ServiceConfig },
    
    // System configuration
    NetworkConfig { interface: String, config: NetworkConfig },
    FirewallRule { action: String, rule: FirewallRule },
    
    // Package management
    PackageInstall { packages: Vec<String> },
    SystemUpdate { security_only: bool },
    
    // User management
    UserCreate { username: String, config: UserConfig },
    GroupManage { action: String, group: String },
}
```

### **Capability Discovery**
```rust
// Agent publishes capabilities on startup
let capabilities = vec!["systemd", "network", "firewall", "packages"];
event_bus.publish("agent.capabilities", capabilities);
```

## Security Model

### **Privilege Separation**
- **Agent binary**: Minimal, focused, auditable privileged operations
- **Main daemon**: Remains completely unprivileged
- **REST API**: Scope-based routing (admin vs user)
- **Console**: Feature discovery based on available capabilities

### **Unix Socket Security**
```bash
# User socket (existing)
/var/run/pandemic/pandemic.sock     # pandemic:pandemic 660

# Admin socket (new)
/var/run/pandemic/admin.sock         # root:root 600
```

### **Authentication Flow**
```
User → REST API → Auth middleware → Scope check → Socket selection → Agent/Daemon
```

## Benefits

### **Additive Architecture**
- ✅ **Optional**: Admin features don't complicate basic deployments
- ✅ **Focused**: Agent only handles privileged operations
- ✅ **Layered**: Integrates cleanly with existing REST/Console/Events
- ✅ **Secure**: Clear privilege boundaries via Unix permissions

### **Unified Experience**
- ✅ **Single API**: Admin operations available via same REST endpoints
- ✅ **Integrated UI**: Console dynamically enables admin features
- ✅ **Event-driven**: Agent status/capabilities via existing event system
- ✅ **Consistent**: Reuses pandemic protocol and patterns

## Implementation Strategy

### **Phase 1: Agent Binary**
- Basic agent binary with systemd operations
- Unix socket with pandemic protocol
- Bootstrap integration for installation

### **Phase 2: REST Integration**
- Admin scope routing in pandemic-rest
- Agent socket client in pandemic-common
- Capability-based endpoint exposure

### **Phase 3: Console Enhancement**
- Admin UI components in pandemic-console
- Dynamic feature enablement
- Real-time agent status monitoring

### **Phase 4: Advanced Operations**
- Network configuration management
- Package and system update operations
- User and security management