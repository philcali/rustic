# Epidemic Infections: Network Configuration Spreading

## Overview

Epidemic infections enable configuration and updates to "spread" across pandemic nodes in the network, with different infection levels controlling the propagation mechanism and intentionality.

## Infection Levels

### **Level 0: Isolated** 
- No network spreading
- Local infection only
- Default for most services

### **Level 1: Discoverable (mDNS)**
- Passive discovery via mDNS/Bonjour
- "Plug and play" - new nodes auto-discover
- Suitable for service discovery, local clusters

```bash
pandemic-cli epidemic set-level my-service 1
# Service becomes discoverable on local network
```

### **Level 2: Contagious (UDP Multicast)**
- Active spreading via UDP multicast
- Intentional configuration distribution
- Controlled by infection policies

```bash
pandemic-cli epidemic spread config-update --level 2 --target subnet:192.168.1.0/24
# Actively spreads to matching nodes
```

### **Level 3: Virulent (TCP Mesh)**
- Aggressive cross-network spreading
- Reliable delivery with retry logic
- For critical updates and security patches

```bash
pandemic-cli epidemic spread security-patch --level 3 --priority critical
# Spreads across network boundaries with guaranteed delivery
```

## Layered Architecture

### **Discovery Layer (Level 1)**
```
Node A ←→ mDNS ←→ Node B
  ↓                 ↓
"I exist"      "I see you"
```

### **Multicast Layer (Level 2)**  
```
Node A → UDP Multicast → [Node B, Node C, Node D]
         "Here's config X"
```

### **Mesh Layer (Level 3)**
```
Node A → TCP → Node B → TCP → Node C
  ↓              ↓              ↓
Relay         Relay         Apply
```

## Configuration Spreading

### **Epidemic Payload Structure**
```toml
[epidemic]
name = "edge-config-v2"
infection_level = 2
spread_policy = "multicast"
target_criteria = ["role:edge-device", "version:<2.0"]

[propagation]
max_hops = 3
ttl_seconds = 3600
verification = "signature_required"
rollback_on_failure = true

[payload]
type = "config_update"
data = { 
  api_endpoint = "https://api-v2.example.com",
  feature_flags = { new_ui = true, beta_api = false }
}
```

### **Spreading Mechanisms**

#### **Level 1: mDNS Discovery**
- Service announces: `_pandemic._tcp.local`
- Automatic peer discovery
- Service registry synchronization

#### **Level 2: UDP Multicast**
- Multicast group: `239.255.pandemic.1`
- Targeted spreading with criteria matching
- Efficient for subnet-wide updates

#### **Level 3: TCP Mesh**
- Persistent connections between nodes
- Guaranteed delivery with acknowledgments
- Cross-subnet and WAN propagation

## Use Cases by Level

### **Level 1 Examples**
```bash
# Service discovery
pandemic-cli epidemic discover --services
# → Finds: redis@192.168.1.10, mqtt@192.168.1.15

# Local cluster formation
pandemic-cli epidemic join-cluster edge-cluster
```

### **Level 2 Examples**
```bash
# Configuration rollout
pandemic-cli epidemic spread app-config --target role:web-server

# Feature flag updates
pandemic-cli epidemic spread feature-flags --canary 25%

# Service endpoint changes
pandemic-cli epidemic spread service-registry --immediate
```

### **Level 3 Examples**
```bash
# Security patches
pandemic-cli epidemic spread security-update --priority critical --verify-all

# System-wide policy changes
pandemic-cli epidemic spread compliance-policy --mandatory

# Emergency configuration
pandemic-cli epidemic spread emergency-config --override-all
```

## Implementation Strategy

### **Phase 1: Foundation**
- Infection level metadata in plugin registry
- Basic mDNS discovery (Level 1)
- CLI commands for level management

### **Phase 2: Multicast Spreading**
- UDP multicast implementation (Level 2)
- Target criteria matching
- Configuration payload distribution

### **Phase 3: Mesh Network**
- TCP mesh networking (Level 3)
- Reliable delivery guarantees
- Cross-network propagation

### **Phase 4: Advanced Features**
- Canary deployments
- Rollback mechanisms
- Conflict resolution
- Security and verification

## Security Considerations

- **Payload signing** - Cryptographic verification of epidemic payloads
- **Network isolation** - Respect network boundaries and firewall rules
- **Rate limiting** - Prevent epidemic storms and network flooding
- **Access control** - Only authorized nodes can initiate epidemics
- **Audit logging** - Track all epidemic activities for compliance

## Integration Points

- **pandemic-daemon** - Core epidemic coordination
- **pandemic-udp** - Level 2 multicast implementation
- **pandemic-cli** - Epidemic management interface
- **Event system** - Epidemic status and progress events
- **Configuration system** - Target for epidemic payloads