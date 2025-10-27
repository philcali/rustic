# Pandemic Agent Architecture

## Overview

A hybrid approach for privileged system management while maintaining the pandemic ecosystem's least-privilege model.

## Architecture

### Components

1. **Agent Binary** (privileged)
   - Separate process with elevated privileges
   - Handles systemd service management
   - Manages process lifecycle and resource limits
   - Applies system-level configuration changes

2. **Agent Infection** (unprivileged)
   - Standard pandemic infection
   - Coordinates between daemon and agent binary
   - Validates and forwards requests
   - Provides pandemic-native interface

3. **Communication Flow**
   ```
   Cloud/External → Agent Infection → Daemon (IPC/events) → Agent Binary → System
   ```

### Design Principles

- **Privilege Separation**: Agent binary isolated with minimal surface area
- **Least Privilege**: Daemon and infections remain unprivileged
- **Unified Experience**: Agent appears as a normal infection to users
- **Clean Boundaries**: Each component has single responsibility

## Alternatives Considered

1. **External Agent Only**: Separate from pandemic ecosystem (rejected - poor UX)
2. **Limited Agent Infection**: Infection with restricted scope (rejected - too limited)
3. **CLI-Managed Agent**: External agent managed via CLI (viable alternative)
4. **Daemon Integration**: Roll process management into daemon (rejected - security risk)

## Benefits

- Maintains pandemic's security model
- Provides unified management experience
- Enables privileged operations when needed
- Scales to complex system management tasks
- Testable through agent binary mocking

## Implementation Notes

- Agent infection acts as validation/proxy layer
- Agent binary handles OS-specific operations
- Communication via Unix sockets or similar IPC
- Agent binary should be minimal and focused