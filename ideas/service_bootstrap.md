# Service Bootstrap Templates

## Overview

A template-based system for instantly deploying common infrastructure services as pandemic infections with zero configuration.

## Core Concept

```bash
# One-command service deployment
pandemic-cli bootstrap service redis
pandemic-cli bootstrap service mqtt --port 1883
pandemic-cli bootstrap service postgres --data-dir /var/lib/postgres
```

## Template Structure

### Template Directory Layout
```
/usr/share/pandemic/templates/
├── redis.toml
├── mqtt.toml  
├── postgres.toml
├── nginx.toml
├── elasticsearch.toml
└── custom/
    └── user-templates.toml
```

### Template Format
```toml
[infection]
name = "redis"
version = "7.0"
description = "Redis in-memory data store"

[runtime]
command = ["redis-server", "--port", "{{port}}", "--dir", "{{data_dir}}"]
health_check = ["redis-cli", "ping"]
health_interval = 15

[template]
variables = ["port", "data_dir", "max_memory"]
defaults = { port = "6379", data_dir = "/var/lib/redis", max_memory = "256mb" }

[systemd]
user = "redis"
group = "redis" 
directories = ["/var/lib/redis", "/var/log/redis"]
```

## Implementation Plan

### Phase 1: Core Templates
- Redis (key-value store)
- MQTT (message broker) 
- PostgreSQL (database)
- Nginx (web server)

### Phase 2: Advanced Features
- Template variables and substitution
- Service dependencies and stacks
- Environment-specific configurations
- Custom template support

### Phase 3: Ecosystem
- Template marketplace/registry
- Community templates
- Stack definitions (multi-service deployments)

## Benefits

- **Zero-config deployment** - Common services work out of the box
- **Pandemic integration** - Automatic health monitoring and event publishing
- **Unified management** - All services managed through pandemic-cli
- **Customizable** - Override defaults with command-line arguments
- **Extensible** - Users can create custom templates

## Usage Examples

```bash
# Basic deployment
pandemic-cli bootstrap service redis
pandemic-cli bootstrap service mqtt

# With customization
pandemic-cli bootstrap service redis --port 6380 --max-memory 1gb
pandemic-cli bootstrap service postgres --db myapp --user appuser

# Stack deployment (future)
pandemic-cli bootstrap stack web-app  # redis + postgres + nginx + app

# Template management (future)
pandemic-cli template install community/elasticsearch
pandemic-cli template list --category database
```

## Integration Points

- **pandemic-proxy** - Wraps template-generated services
- **pandemic-cli** - Template management and deployment
- **systemd** - Service lifecycle and process management
- **Event system** - Health monitoring and alerting

## Future Enhancements

- Template validation and testing
- Service discovery integration
- Configuration management (secrets, environment variables)
- Multi-node deployment support
- Integration with container registries