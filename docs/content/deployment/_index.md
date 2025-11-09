+++
title = "Deployment"
description = "Guide for deploying Hypha in production environments"
weight = 3
+++

# Deployment Guide

This guide covers deploying Hypha in production environments, from small-scale setups to large distributed deployments.

## Deployment Architectures

### Small Scale (Single Organization)

For small teams or organizations:

```
┌─────────────┐
│   Gateway   │ (1 instance)
└──────┬──────┘
       │
   ┌───┴────┐
   │        │
┌──▼───┐ ┌─▼────┐
│Worker│ │Worker│ (2-5 instances)
└──────┘ └──────┘
   │
┌──▼────────┐
│ Scheduler │ (1 instance)
└───────────┘
```

**Characteristics:**
- 1 gateway node
- 1 scheduler node
- 2-5 worker nodes
- Suitable for teams up to 50 users
- Cost-effective for development and small production

### Medium Scale (Multi-Team)

For medium organizations:

```
┌─────────────┐  ┌─────────────┐
│  Gateway 1  │  │  Gateway 2  │ (2-3 instances)
└──────┬──────┘  └──────┬──────┘
       │                │
       └────────┬───────┘
                │
   ┌────────────┼────────────┐
   │            │            │
┌──▼──────┐ ┌──▼──────┐ ┌──▼──────┐
│Scheduler│ │Scheduler│ │Scheduler│ (3+ instances)
└────┬────┘ └────┬────┘ └────┬────┘
     │           │           │
     └───────────┼───────────┘
                 │
     ┌───────────┼───────────┐
     │           │           │
  ┌──▼───┐   ┌──▼───┐   ┌──▼───┐
  │Worker│...│Worker│...│Worker│ (10-50 instances)
  └──────┘   └──────┘   └──────┘
```

**Characteristics:**
- 2-3 gateway nodes (high availability)
- 3+ scheduler nodes (load balancing)
- 10-50 worker nodes
- Suitable for 100-500 users
- Regional deployment possible

### Large Scale (Enterprise)

For large enterprises:

- Multiple gateway clusters across regions
- Scheduler federation
- Worker pools by capability
- 100+ worker nodes
- Global deployment
- Multi-datacenter support

## Infrastructure Requirements

### Gateway Nodes

**Minimum:**
- 2 CPU cores
- 2 GB RAM
- 10 GB storage
- Public IP address
- 100 Mbps network

**Recommended:**
- 4+ CPU cores
- 8 GB RAM
- 50 GB SSD storage
- Multiple public IPs
- 1 Gbps network

### Scheduler Nodes

**Minimum:**
- 2 CPU cores
- 4 GB RAM
- 20 GB storage
- Network connectivity to gateways

**Recommended:**
- 4+ CPU cores
- 16 GB RAM
- 100 GB SSD storage
- Low-latency network to workers

### Worker Nodes

**Minimum:**
- 4 CPU cores
- 16 GB RAM
- 100 GB storage

**Recommended (GPU workloads):**
- 8+ CPU cores
- 32+ GB RAM
- 500 GB SSD storage
- 1+ GPU (A100, H100, etc.)
- High-bandwidth network

## Deployment Methods

### Docker Compose

Simple deployment for testing:

```yaml
version: '3.8'

services:
  gateway:
    image: hypha/gateway:latest
    ports:
      - "4001:4001"
    volumes:
      - ./config/gateway.toml:/etc/hypha/config.toml
      - gateway-data:/var/lib/hypha
    restart: unless-stopped

  scheduler:
    image: hypha/scheduler:latest
    ports:
      - "8080:8080"
    volumes:
      - ./config/scheduler.toml:/etc/hypha/config.toml
    depends_on:
      - gateway
    restart: unless-stopped

  worker:
    image: hypha/worker:latest
    volumes:
      - ./config/worker.toml:/etc/hypha/config.toml
      - worker-data:/var/lib/hypha
    depends_on:
      - gateway
      - scheduler
    deploy:
      replicas: 3
    restart: unless-stopped

volumes:
  gateway-data:
  worker-data:
```

### Kubernetes

Production-grade deployment:

```yaml
# Gateway deployment
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: hypha-gateway
spec:
  serviceName: hypha-gateway
  replicas: 3
  selector:
    matchLabels:
      app: hypha-gateway
  template:
    metadata:
      labels:
        app: hypha-gateway
    spec:
      containers:
      - name: gateway
        image: hypha/gateway:latest
        ports:
        - containerPort: 4001
          name: p2p
        - containerPort: 9090
          name: metrics
        volumeMounts:
        - name: config
          mountPath: /etc/hypha
        - name: data
          mountPath: /var/lib/hypha
        resources:
          requests:
            cpu: "2"
            memory: "4Gi"
          limits:
            cpu: "4"
            memory: "8Gi"
      volumes:
      - name: config
        configMap:
          name: gateway-config
  volumeClaimTemplates:
  - metadata:
      name: data
    spec:
      accessModes: [ "ReadWriteOnce" ]
      resources:
        requests:
          storage: 50Gi
```

### Systemd

Native Linux service:

```ini
# /etc/systemd/system/hypha-gateway.service
[Unit]
Description=Hypha Gateway Node
After=network.target

[Service]
Type=simple
User=hypha
Group=hypha
ExecStart=/usr/local/bin/hypha-gateway --config /etc/hypha/gateway.toml
Restart=on-failure
RestartSec=5s

# Security hardening
PrivateTmp=true
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/hypha /var/log/hypha

[Install]
WantedBy=multi-user.target
```

## Configuration Management

### Configuration Files

Store configurations in version control:

```
config/
├── gateway.toml
├── scheduler.toml
├── worker.toml
└── production/
    ├── gateway.toml
    ├── scheduler.toml
    └── worker.toml
```

### Environment Variables

Override config with environment variables:

```bash
# Gateway configuration
export HYPHA_GATEWAY_RELAY_ENABLED=true
export HYPHA_GATEWAY_MAX_CONNECTIONS=500

# Scheduler configuration
export HYPHA_SCHEDULER_MAX_TASKS=100
export HYPHA_SCHEDULER_API_BIND_ADDRESS="0.0.0.0:8080"

# Worker configuration
export HYPHA_WORKER_MAX_CONCURRENT_TASKS=4
export HYPHA_WORKER_MODEL_DIR="/var/lib/hypha/models"
```

### Secrets Management

Use secret management for sensitive data:

```bash
# Using HashiCorp Vault
vault kv put secret/hypha/gateway \
  tls_cert=@/path/to/cert.pem \
  tls_key=@/path/to/key.pem

# Using Kubernetes secrets
kubectl create secret generic hypha-certs \
  --from-file=tls.crt=/path/to/cert.pem \
  --from-file=tls.key=/path/to/key.pem
```

## Networking

### Firewall Configuration

Required ports:

- **Gateway**: 4001 (P2P), 9090 (metrics)
- **Scheduler**: 8080 (API), 9091 (metrics)
- **Worker**: 9092 (metrics)

Example `iptables` rules:

```bash
# Gateway
iptables -A INPUT -p tcp --dport 4001 -j ACCEPT
iptables -A INPUT -p udp --dport 4001 -j ACCEPT

# Scheduler API
iptables -A INPUT -p tcp --dport 8080 -j ACCEPT

# Metrics (restrict to monitoring network)
iptables -A INPUT -p tcp --dport 9090:9092 -s 10.0.0.0/8 -j ACCEPT
```

### Load Balancing

Use load balancers for schedulers:

```nginx
# Nginx configuration
upstream hypha_schedulers {
    least_conn;
    server scheduler1.internal:8080;
    server scheduler2.internal:8080;
    server scheduler3.internal:8080;
}

server {
    listen 443 ssl;
    server_name api.hypha.example.com;

    location /api/ {
        proxy_pass http://hypha_schedulers;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
}
```

## Monitoring

### Prometheus

Scrape metrics from all components:

```yaml
# prometheus.yml
scrape_configs:
  - job_name: 'hypha-gateway'
    static_configs:
      - targets:
        - gateway1:9090
        - gateway2:9090

  - job_name: 'hypha-scheduler'
    static_configs:
      - targets:
        - scheduler1:9091
        - scheduler2:9091

  - job_name: 'hypha-worker'
    kubernetes_sd_configs:
      - role: pod
    relabel_configs:
      - source_labels: [__meta_kubernetes_pod_label_app]
        action: keep
        regex: hypha-worker
```

### Grafana Dashboards

Monitor key metrics:

- Gateway: connections, relay circuits, bandwidth
- Scheduler: task queue depth, completion rate, worker pool
- Worker: active tasks, resource utilization, model cache

### Logging

Centralized logging with structured logs:

```toml
[logging]
level = "info"
format = "json"
output = "stdout"

# Send to logging service
[[logging.outputs]]
type = "loki"
url = "http://loki:3100"
```

## Backup and Recovery

### State Backup

Important data to backup:

- Gateway: Peer records, relay state
- Scheduler: Task queue, worker registry
- Worker: Model cache, execution logs

### Disaster Recovery

Recovery procedures:

1. **Gateway failure**: Start new gateway, peers reconnect
2. **Scheduler failure**: Start new scheduler, tasks redistributed
3. **Worker failure**: Tasks reassigned to other workers
4. **Data loss**: Restore from backups, rebuild state

## Security Hardening

### TLS/SSL

Use valid certificates:

```bash
# Generate production certificates
certbot certonly --standalone \
  -d gateway.hypha.example.com \
  -d gateway2.hypha.example.com
```

### Access Control

Restrict access:

```toml
[security]
# Allowed scheduler peer IDs
allowed_schedulers = [
    "12D3KooWScheduler1...",
    "12D3KooWScheduler2..."
]

# Require task signatures
require_task_signatures = true

# Trusted certificate authorities
trusted_cas = ["/etc/hypha/ca.pem"]
```

### Network Isolation

Use network policies:

```yaml
# Kubernetes NetworkPolicy
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: hypha-worker-policy
spec:
  podSelector:
    matchLabels:
      app: hypha-worker
  ingress:
  - from:
    - podSelector:
        matchLabels:
          app: hypha-scheduler
```

## Scaling

### Horizontal Scaling

Scale components independently:

```bash
# Scale workers
kubectl scale statefulset hypha-worker --replicas=20

# Scale schedulers
kubectl scale deployment hypha-scheduler --replicas=5
```

### Auto-scaling

Use Kubernetes HPA:

```yaml
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: hypha-worker-hpa
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: StatefulSet
    name: hypha-worker
  minReplicas: 3
  maxReplicas: 50
  metrics:
  - type: Resource
    resource:
      name: cpu
      target:
        type: Utilization
        averageUtilization: 70
```

## Next Steps

- [Architecture Overview](/architecture/) - Understand the system design
- [Gateway Nodes](/architecture/gateway/) - Gateway deployment specifics
- [Monitoring](#monitoring) - Set up observability
