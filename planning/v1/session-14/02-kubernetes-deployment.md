# Session 14.2: Kubernetes Deployment & Container Orchestration

## Objective
Implement production-ready Kubernetes deployment manifests and Helm charts with auto-scaling, health checks, security policies, and resource management to provide robust container orchestration for the entire Build platform stack.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for Kubernetes cluster monitoring and pod observability
- **Session 11**: Integrates with Prometheus for container metrics collection
- **Session 12**: Implements rate limiting at ingress and service mesh level
- **Session 13**: Leverages high availability infrastructure for zero-downtime deployments
- **All Sessions**: Provides Kubernetes orchestration for complete platform deployment

## Core Implementation

### Kubernetes Namespace Configuration
**Location**: `k8s/manifests/namespace.yaml`

```yaml
# k8s/manifests/namespace.yaml
apiVersion: v1
kind: Namespace
metadata:
  name: build-platform
  labels:
    name: build-platform
    app.kubernetes.io/name: build-platform
    app.kubernetes.io/version: "1.0"
    app.kubernetes.io/managed-by: helm
---
apiVersion: v1
kind: Namespace
metadata:
  name: build-platform-staging
  labels:
    name: build-platform-staging
    app.kubernetes.io/name: build-platform
    app.kubernetes.io/version: "1.0"
    app.kubernetes.io/managed-by: helm
    environment: staging
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: build-platform-network-policy
  namespace: build-platform
spec:
  podSelector: {}
  policyTypes:
  - Ingress
  - Egress
  ingress:
  - from:
    - namespaceSelector:
        matchLabels:
          name: build-platform
    - namespaceSelector:
        matchLabels:
          name: ingress-nginx
  egress:
  - to: []
    ports:
    - protocol: TCP
      port: 53
    - protocol: UDP
      port: 53
  - to:
    - namespaceSelector:
        matchLabels:
          name: build-platform
```

### API Server Deployment
**Location**: `k8s/manifests/api-deployment.yaml`

```yaml
# k8s/manifests/api-deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: build-platform-api
  namespace: build-platform
  labels:
    app.kubernetes.io/name: build-platform-api
    app.kubernetes.io/component: api
    app.kubernetes.io/part-of: build-platform
    app.kubernetes.io/version: "1.0"
spec:
  replicas: 3
  strategy:
    type: RollingUpdate
    rollingUpdate:
      maxSurge: 1
      maxUnavailable: 0
  selector:
    matchLabels:
      app.kubernetes.io/name: build-platform-api
  template:
    metadata:
      labels:
        app.kubernetes.io/name: build-platform-api
        app.kubernetes.io/component: api
        app.kubernetes.io/version: "1.0"
      annotations:
        prometheus.io/scrape: "true"
        prometheus.io/port: "8000"
        prometheus.io/path: "/metrics"
    spec:
      serviceAccountName: build-platform-api
      securityContext:
        runAsNonRoot: true
        runAsUser: 1000
        runAsGroup: 1000
        fsGroup: 1000
        seccompProfile:
          type: RuntimeDefault
      
      initContainers:
      - name: migrate-database
        image: ghcr.io/your-org/build-platform/api:latest
        command: ["python", "-m", "alembic", "upgrade", "head"]
        env:
        - name: DATABASE_URL
          valueFrom:
            secretKeyRef:
              name: build-platform-secrets
              key: database-url
        securityContext:
          allowPrivilegeEscalation: false
          readOnlyRootFilesystem: true
          capabilities:
            drop:
            - ALL
        volumeMounts:
        - name: tmp
          mountPath: /tmp
        resources:
          limits:
            cpu: 200m
            memory: 256Mi
          requests:
            cpu: 100m
            memory: 128Mi

      containers:
      - name: api
        image: ghcr.io/your-org/build-platform/api:latest
        imagePullPolicy: Always
        ports:
        - name: http
          containerPort: 8000
          protocol: TCP
        - name: metrics
          containerPort: 9090
          protocol: TCP
        
        env:
        - name: ENVIRONMENT
          value: "production"
        - name: POD_NAME
          valueFrom:
            fieldRef:
              fieldPath: metadata.name
        - name: POD_NAMESPACE
          valueFrom:
            fieldRef:
              fieldPath: metadata.namespace
        - name: DATABASE_URL
          valueFrom:
            secretKeyRef:
              name: build-platform-secrets
              key: database-url
        - name: REDIS_URL
          valueFrom:
            secretKeyRef:
              name: build-platform-secrets
              key: redis-url
        - name: JWT_SECRET
          valueFrom:
            secretKeyRef:
              name: build-platform-secrets
              key: jwt-secret
        - name: LOGFIRE_TOKEN
          valueFrom:
            secretKeyRef:
              name: build-platform-secrets
              key: logfire-token

        livenessProbe:
          httpGet:
            path: /health
            port: http
          initialDelaySeconds: 30
          periodSeconds: 30
          timeoutSeconds: 10
          failureThreshold: 3
          successThreshold: 1
        
        readinessProbe:
          httpGet:
            path: /health/ready
            port: http
          initialDelaySeconds: 5
          periodSeconds: 10
          timeoutSeconds: 5
          failureThreshold: 3
          successThreshold: 1

        startupProbe:
          httpGet:
            path: /health/startup
            port: http
          initialDelaySeconds: 10
          periodSeconds: 5
          timeoutSeconds: 3
          failureThreshold: 30
          successThreshold: 1

        resources:
          limits:
            cpu: 1000m
            memory: 1Gi
            ephemeral-storage: 2Gi
          requests:
            cpu: 500m
            memory: 512Mi
            ephemeral-storage: 1Gi

        securityContext:
          allowPrivilegeEscalation: false
          readOnlyRootFilesystem: true
          capabilities:
            drop:
            - ALL

        volumeMounts:
        - name: tmp
          mountPath: /tmp
        - name: cache
          mountPath: /app/.cache
        - name: logs
          mountPath: /app/logs

      volumes:
      - name: tmp
        emptyDir:
          sizeLimit: 1Gi
      - name: cache
        emptyDir:
          sizeLimit: 1Gi
      - name: logs
        emptyDir:
          sizeLimit: 2Gi

      affinity:
        podAntiAffinity:
          preferredDuringSchedulingIgnoredDuringExecution:
          - weight: 100
            podAffinityTerm:
              labelSelector:
                matchExpressions:
                - key: app.kubernetes.io/name
                  operator: In
                  values:
                  - build-platform-api
              topologyKey: kubernetes.io/hostname
        nodeAffinity:
          requiredDuringSchedulingIgnoredDuringExecution:
            nodeSelectorTerms:
            - matchExpressions:
              - key: kubernetes.io/arch
                operator: In
                values:
                - amd64

      tolerations:
      - key: node.kubernetes.io/memory-pressure
        operator: Exists
        effect: NoSchedule
      - key: node.kubernetes.io/disk-pressure
        operator: Exists
        effect: NoSchedule

---
apiVersion: v1
kind: Service
metadata:
  name: build-platform-api
  namespace: build-platform
  labels:
    app.kubernetes.io/name: build-platform-api
    app.kubernetes.io/component: api
  annotations:
    prometheus.io/scrape: "true"
    prometheus.io/port: "9090"
spec:
  type: ClusterIP
  ports:
  - port: 80
    targetPort: http
    protocol: TCP
    name: http
  - port: 9090
    targetPort: metrics
    protocol: TCP
    name: metrics
  selector:
    app.kubernetes.io/name: build-platform-api

---
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: build-platform-api-hpa
  namespace: build-platform
  labels:
    app.kubernetes.io/name: build-platform-api
    app.kubernetes.io/component: api
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: build-platform-api
  minReplicas: 3
  maxReplicas: 10
  metrics:
  - type: Resource
    resource:
      name: cpu
      target:
        type: Utilization
        averageUtilization: 70
  - type: Resource
    resource:
      name: memory
      target:
        type: Utilization
        averageUtilization: 80
  behavior:
    scaleDown:
      stabilizationWindowSeconds: 300
      policies:
      - type: Percent
        value: 10
        periodSeconds: 60
    scaleUp:
      stabilizationWindowSeconds: 60
      policies:
      - type: Percent
        value: 50
        periodSeconds: 60
```

### Frontend Deployment
**Location**: `k8s/manifests/frontend-deployment.yaml`

```yaml
# k8s/manifests/frontend-deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: build-platform-frontend
  namespace: build-platform
  labels:
    app.kubernetes.io/name: build-platform-frontend
    app.kubernetes.io/component: frontend
    app.kubernetes.io/part-of: build-platform
    app.kubernetes.io/version: "1.0"
spec:
  replicas: 2
  strategy:
    type: RollingUpdate
    rollingUpdate:
      maxSurge: 1
      maxUnavailable: 0
  selector:
    matchLabels:
      app.kubernetes.io/name: build-platform-frontend
  template:
    metadata:
      labels:
        app.kubernetes.io/name: build-platform-frontend
        app.kubernetes.io/component: frontend
        app.kubernetes.io/version: "1.0"
    spec:
      serviceAccountName: build-platform-frontend
      securityContext:
        runAsNonRoot: true
        runAsUser: 101  # nginx user
        runAsGroup: 101
        fsGroup: 101
        seccompProfile:
          type: RuntimeDefault

      containers:
      - name: frontend
        image: ghcr.io/your-org/build-platform/frontend:latest
        imagePullPolicy: Always
        ports:
        - name: http
          containerPort: 80
          protocol: TCP

        livenessProbe:
          httpGet:
            path: /health
            port: http
          initialDelaySeconds: 30
          periodSeconds: 30
          timeoutSeconds: 5
          failureThreshold: 3

        readinessProbe:
          httpGet:
            path: /health
            port: http
          initialDelaySeconds: 5
          periodSeconds: 10
          timeoutSeconds: 3
          failureThreshold: 3

        resources:
          limits:
            cpu: 500m
            memory: 512Mi
            ephemeral-storage: 1Gi
          requests:
            cpu: 250m
            memory: 256Mi
            ephemeral-storage: 512Mi

        securityContext:
          allowPrivilegeEscalation: false
          readOnlyRootFilesystem: true
          capabilities:
            drop:
            - ALL

        volumeMounts:
        - name: nginx-cache
          mountPath: /var/cache/nginx
        - name: nginx-pid
          mountPath: /var/run

      volumes:
      - name: nginx-cache
        emptyDir:
          sizeLimit: 1Gi
      - name: nginx-pid
        emptyDir:
          sizeLimit: 100Mi

      affinity:
        podAntiAffinity:
          preferredDuringSchedulingIgnoredDuringExecution:
          - weight: 100
            podAffinityTerm:
              labelSelector:
                matchExpressions:
                - key: app.kubernetes.io/name
                  operator: In
                  values:
                  - build-platform-frontend
              topologyKey: kubernetes.io/hostname

---
apiVersion: v1
kind: Service
metadata:
  name: build-platform-frontend
  namespace: build-platform
  labels:
    app.kubernetes.io/name: build-platform-frontend
    app.kubernetes.io/component: frontend
spec:
  type: ClusterIP
  ports:
  - port: 80
    targetPort: http
    protocol: TCP
    name: http
  selector:
    app.kubernetes.io/name: build-platform-frontend

---
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: build-platform-frontend-hpa
  namespace: build-platform
  labels:
    app.kubernetes.io/name: build-platform-frontend
    app.kubernetes.io/component: frontend
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: build-platform-frontend
  minReplicas: 2
  maxReplicas: 6
  metrics:
  - type: Resource
    resource:
      name: cpu
      target:
        type: Utilization
        averageUtilization: 70
```

### VM Manager Deployment
**Location**: `k8s/manifests/vm-manager-deployment.yaml`

```yaml
# k8s/manifests/vm-manager-deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: build-platform-vm-manager
  namespace: build-platform
  labels:
    app.kubernetes.io/name: build-platform-vm-manager
    app.kubernetes.io/component: vm-manager
    app.kubernetes.io/part-of: build-platform
    app.kubernetes.io/version: "1.0"
spec:
  replicas: 2
  strategy:
    type: RollingUpdate
    rollingUpdate:
      maxSurge: 1
      maxUnavailable: 0
  selector:
    matchLabels:
      app.kubernetes.io/name: build-platform-vm-manager
  template:
    metadata:
      labels:
        app.kubernetes.io/name: build-platform-vm-manager
        app.kubernetes.io/component: vm-manager
        app.kubernetes.io/version: "1.0"
      annotations:
        prometheus.io/scrape: "true"
        prometheus.io/port: "9091"
        prometheus.io/path: "/metrics"
    spec:
      serviceAccountName: build-platform-vm-manager
      securityContext:
        runAsNonRoot: true
        runAsUser: 1000
        runAsGroup: 1000
        fsGroup: 1000
        seccompProfile:
          type: RuntimeDefault

      containers:
      - name: vm-manager
        image: ghcr.io/your-org/build-platform/vm-manager:latest
        imagePullPolicy: Always
        ports:
        - name: http
          containerPort: 8080
          protocol: TCP
        - name: metrics
          containerPort: 9091
          protocol: TCP

        env:
        - name: ENVIRONMENT
          value: "production"
        - name: POD_NAME
          valueFrom:
            fieldRef:
              fieldPath: metadata.name
        - name: DATABASE_URL
          valueFrom:
            secretKeyRef:
              name: build-platform-secrets
              key: database-url
        - name: FIRECRACKER_SOCKET_PATH
          value: "/firecracker/sockets"
        - name: FIRECRACKER_KERNEL_PATH
          value: "/firecracker/kernels"
        - name: FIRECRACKER_ROOTFS_PATH
          value: "/firecracker/rootfs"

        livenessProbe:
          httpGet:
            path: /health
            port: http
          initialDelaySeconds: 30
          periodSeconds: 30
          timeoutSeconds: 10
          failureThreshold: 3

        readinessProbe:
          httpGet:
            path: /health/ready
            port: http
          initialDelaySeconds: 10
          periodSeconds: 10
          timeoutSeconds: 5
          failureThreshold: 3

        resources:
          limits:
            cpu: 2000m
            memory: 2Gi
            ephemeral-storage: 5Gi
          requests:
            cpu: 1000m
            memory: 1Gi
            ephemeral-storage: 2Gi

        securityContext:
          allowPrivilegeEscalation: true  # Required for Firecracker
          capabilities:
            add:
            - NET_ADMIN
            - SYS_ADMIN
            drop:
            - ALL

        volumeMounts:
        - name: firecracker-sockets
          mountPath: /firecracker/sockets
        - name: firecracker-kernels
          mountPath: /firecracker/kernels
          readOnly: true
        - name: firecracker-rootfs
          mountPath: /firecracker/rootfs
        - name: dev-kvm
          mountPath: /dev/kvm

      volumes:
      - name: firecracker-sockets
        emptyDir:
          sizeLimit: 1Gi
      - name: firecracker-kernels
        hostPath:
          path: /opt/firecracker/kernels
          type: Directory
      - name: firecracker-rootfs
        emptyDir:
          sizeLimit: 10Gi
      - name: dev-kvm
        hostPath:
          path: /dev/kvm
          type: CharDevice

      nodeSelector:
        workload-type: firecracker
        
      tolerations:
      - key: firecracker
        operator: Equal
        value: "true"
        effect: NoSchedule

      affinity:
        nodeAffinity:
          requiredDuringSchedulingIgnoredDuringExecution:
            nodeSelectorTerms:
            - matchExpressions:
              - key: workload-type
                operator: In
                values:
                - firecracker

---
apiVersion: v1
kind: Service
metadata:
  name: build-platform-vm-manager
  namespace: build-platform
  labels:
    app.kubernetes.io/name: build-platform-vm-manager
    app.kubernetes.io/component: vm-manager
  annotations:
    prometheus.io/scrape: "true"
    prometheus.io/port: "9091"
spec:
  type: ClusterIP
  ports:
  - port: 80
    targetPort: http
    protocol: TCP
    name: http
  - port: 9091
    targetPort: metrics
    protocol: TCP
    name: metrics
  selector:
    app.kubernetes.io/name: build-platform-vm-manager
```

### Ingress Configuration
**Location**: `k8s/manifests/ingress.yaml`

```yaml
# k8s/manifests/ingress.yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: build-platform-ingress
  namespace: build-platform
  labels:
    app.kubernetes.io/name: build-platform
    app.kubernetes.io/component: ingress
  annotations:
    kubernetes.io/ingress.class: nginx
    nginx.ingress.kubernetes.io/ssl-redirect: "true"
    nginx.ingress.kubernetes.io/force-ssl-redirect: "true"
    cert-manager.io/cluster-issuer: letsencrypt-prod
    nginx.ingress.kubernetes.io/rate-limit: "100"
    nginx.ingress.kubernetes.io/rate-limit-window: "1m"
    nginx.ingress.kubernetes.io/proxy-body-size: "100m"
    nginx.ingress.kubernetes.io/proxy-connect-timeout: "60"
    nginx.ingress.kubernetes.io/proxy-send-timeout: "60"
    nginx.ingress.kubernetes.io/proxy-read-timeout: "60"
    nginx.ingress.kubernetes.io/enable-cors: "true"
    nginx.ingress.kubernetes.io/cors-allow-origin: "https://getbuild.ing"
    nginx.ingress.kubernetes.io/cors-allow-methods: "GET, POST, PUT, DELETE, PATCH, OPTIONS"
    nginx.ingress.kubernetes.io/cors-allow-headers: "DNT,X-CustomHeader,Keep-Alive,User-Agent,X-Requested-With,If-Modified-Since,Cache-Control,Content-Type,Authorization"
spec:
  tls:
  - hosts:
    - getbuild.ing
    - api.getbuild.ing
    secretName: build-platform-tls
  rules:
  - host: getbuild.ing
    http:
      paths:
      - path: /api
        pathType: Prefix
        backend:
          service:
            name: build-platform-api
            port:
              number: 80
      - path: /
        pathType: Prefix
        backend:
          service:
            name: build-platform-frontend
            port:
              number: 80
  - host: api.getbuild.ing
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: build-platform-api
            port:
              number: 80

---
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: build-platform-websocket-ingress
  namespace: build-platform
  labels:
    app.kubernetes.io/name: build-platform
    app.kubernetes.io/component: websocket-ingress
  annotations:
    kubernetes.io/ingress.class: nginx
    nginx.ingress.kubernetes.io/ssl-redirect: "true"
    nginx.ingress.kubernetes.io/proxy-read-timeout: "3600"
    nginx.ingress.kubernetes.io/proxy-send-timeout: "3600"
    nginx.ingress.kubernetes.io/websocket-services: "build-platform-api"
    nginx.ingress.kubernetes.io/upstream-hash-by: "$binary_remote_addr"
    cert-manager.io/cluster-issuer: letsencrypt-prod
spec:
  tls:
  - hosts:
    - ws.getbuild.ing
    secretName: build-platform-websocket-tls
  rules:
  - host: ws.getbuild.ing
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: build-platform-api
            port:
              number: 80
```

### Service Account and RBAC
**Location**: `k8s/manifests/rbac.yaml`

```yaml
# k8s/manifests/rbac.yaml
apiVersion: v1
kind: ServiceAccount
metadata:
  name: build-platform-api
  namespace: build-platform
  labels:
    app.kubernetes.io/name: build-platform-api
    app.kubernetes.io/component: api
automountServiceAccountToken: true

---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: build-platform-frontend
  namespace: build-platform
  labels:
    app.kubernetes.io/name: build-platform-frontend
    app.kubernetes.io/component: frontend
automountServiceAccountToken: false

---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: build-platform-vm-manager
  namespace: build-platform
  labels:
    app.kubernetes.io/name: build-platform-vm-manager
    app.kubernetes.io/component: vm-manager
automountServiceAccountToken: true

---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: build-platform-api-role
  namespace: build-platform
rules:
- apiGroups: [""]
  resources: ["pods", "services", "configmaps"]
  verbs: ["get", "list", "watch"]
- apiGroups: ["apps"]
  resources: ["deployments"]
  verbs: ["get", "list", "watch"]

---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: build-platform-api-binding
  namespace: build-platform
subjects:
- kind: ServiceAccount
  name: build-platform-api
  namespace: build-platform
roleRef:
  kind: Role
  name: build-platform-api-role
  apiGroup: rbac.authorization.k8s.io

---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: build-platform-vm-manager-role
  namespace: build-platform
rules:
- apiGroups: [""]
  resources: ["pods", "services", "persistentvolumeclaims"]
  verbs: ["get", "list", "watch", "create", "update", "patch", "delete"]
- apiGroups: ["apps"]
  resources: ["deployments"]
  verbs: ["get", "list", "watch", "create", "update", "patch", "delete"]

---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: build-platform-vm-manager-binding
  namespace: build-platform
subjects:
- kind: ServiceAccount
  name: build-platform-vm-manager
  namespace: build-platform
roleRef:
  kind: Role
  name: build-platform-vm-manager-role
  apiGroup: rbac.authorization.k8s.io
```

## TDD Implementation Cycle

### Red Phase: Kubernetes Deployment Test Creation
```python
# k8s/tests/test_kubernetes_deployment.py
import pytest
import kubernetes
from kubernetes import client, config
import yaml
import requests
import time

class KubernetesDeploymentTester:
    def __init__(self, namespace: str = "build-platform"):
        self.namespace = namespace
        config.load_incluster_config()  # or load_kube_config() for local testing
        self.v1 = client.CoreV1Api()
        self.apps_v1 = client.AppsV1Api()
        self.autoscaling_v2 = client.AutoscalingV2Api()
    
    def test_namespace_exists(self):
        """Test that the namespace exists"""
        # This test should initially fail (Red phase)
        assert False, "Kubernetes namespace not created yet"
    
    def test_deployments_ready(self):
        """Test that all deployments are ready"""
        # This test should initially fail (Red phase)
        assert False, "Kubernetes deployments not implemented yet"
    
    def test_services_accessible(self):
        """Test that services are accessible"""
        # This test should initially fail (Red phase)
        assert False, "Kubernetes services not implemented yet"
    
    def test_ingress_configuration(self):
        """Test that ingress is properly configured"""
        # This test should initially fail (Red phase)
        assert False, "Kubernetes ingress not implemented yet"
    
    def test_autoscaling_works(self):
        """Test that horizontal pod autoscaling works"""
        # This test should initially fail (Red phase)
        assert False, "Kubernetes autoscaling not implemented yet"

# CLI for testing Kubernetes deployment
if __name__ == "__main__":
    tester = KubernetesDeploymentTester()
    
    try:
        tester.test_namespace_exists()
        tester.test_deployments_ready()
        tester.test_services_accessible()
        tester.test_ingress_configuration()
        tester.test_autoscaling_works()
        print("All Kubernetes tests passed!")
    except AssertionError as e:
        print(f"Kubernetes test failed: {e}")
        exit(1)
```

### Green Phase: Kubernetes Deployment Implementation
```python
# Implement Kubernetes deployment features to make tests pass
# This involves creating manifests, services, and ingress configurations
```

### Refactor Phase: Kubernetes Deployment Optimization
```python
# Optimize Kubernetes deployment for performance and reliability
# Add advanced deployment strategies and enhanced monitoring
# Improve resource allocation and security configurations
```

## Security Checklist ✅

### Kubernetes Security
- [ ] Pod Security Standards enforcement
- [ ] Network policies for traffic isolation
- [ ] RBAC with least privilege principles
- [ ] Service mesh security (mTLS)
- [ ] Admission controllers for policy enforcement
- [ ] Secrets encryption at rest and in transit
- [ ] Audit logging for cluster activities
- [ ] Node security and hardening
- [ ] Workload identity and authentication
- [ ] Security scanning of Kubernetes manifests

### Container Security
- [ ] Container image vulnerability scanning
- [ ] Non-root container execution
- [ ] Read-only root filesystems
- [ ] Resource limits and security contexts
- [ ] Container runtime security policies
- [ ] Image signing and verification
- [ ] Registry access controls and scanning
- [ ] Container network security
- [ ] Runtime threat detection
- [ ] Secure container configuration

### Deployment Security
- [ ] Secure deployment pipelines with approval gates
- [ ] Environment isolation and access controls
- [ ] Production deployment restrictions
- [ ] Rollback procedures and emergency access
- [ ] Change management and approval processes
- [ ] Deployment verification and health checks
- [ ] Monitoring and alerting for deployments
- [ ] Incident response procedures
- [ ] Compliance validation and reporting
- [ ] Security testing in deployment pipeline

### Network Security
- [ ] Network policies restricting pod-to-pod communication
- [ ] Ingress controller security configuration
- [ ] TLS termination and certificate management
- [ ] Service mesh security policies
- [ ] DNS security and filtering
- [ ] Load balancer security configuration
- [ ] API gateway security controls
- [ ] Network segmentation and isolation
- [ ] Firewall rules and access controls
- [ ] VPN and private network access

## Performance Requirements

### Deployment Performance
- Pod startup time < 30 seconds
- Service discovery latency < 10ms
- Rolling update completion < 5 minutes
- Ingress response time < 1ms overhead
- Auto-scaling response < 2 minutes
- Health check response < 100ms

### Resource Performance
- CPU utilization efficiency > 80%
- Memory utilization efficiency > 85%
- Network throughput > 1 Gbps
- Storage I/O performance > 1000 IOPS
- Container registry pull speed > 100 MB/s
- Image build cache hit rate > 80%

### Availability Performance
- Service availability > 99.9%
- Pod failure recovery < 30 seconds
- Node failure recovery < 2 minutes
- Deployment success rate > 99%
- Zero-downtime deployment capability
- Graceful shutdown completion < 30 seconds

## Commit Instructions

After implementing the Kubernetes deployment system:

```bash
git add k8s/manifests/ k8s/tests/
git commit -m "Add production-ready Kubernetes deployment manifests

- Implement comprehensive Kubernetes deployments for all platform components
- Add API server deployment with auto-scaling and health checks
- Include frontend deployment with nginx optimization
- Add VM manager deployment with Firecracker-specific configurations
- Implement ingress configuration with SSL termination and rate limiting
- Add service accounts and RBAC with least privilege access
- Include network policies for security isolation
- Add horizontal pod autoscalers for dynamic scaling
- Implement comprehensive security contexts and policies
- Add TDD cycle with Red-Green-Refactor for Kubernetes deployment
- Ensure >95% Kubernetes deployment test coverage

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete Kubernetes deployment test suite:

```bash
# Test Kubernetes manifest validation
kubectl apply --dry-run=client -f k8s/manifests/

# Test deployment readiness
kubectl rollout status deployment/build-platform-api -n build-platform
kubectl rollout status deployment/build-platform-frontend -n build-platform
kubectl rollout status deployment/build-platform-vm-manager -n build-platform

# Test service accessibility
kubectl get services -n build-platform
kubectl port-forward service/build-platform-api 8080:80 -n build-platform &
curl http://localhost:8080/health

# Test ingress configuration
kubectl get ingress -n build-platform
curl -k https://getbuild.ing/health
```

Validate Kubernetes deployment test coverage:
```bash
python k8s/tests/test_kubernetes_deployment.py
pytest k8s/tests/ --cov=kubernetes --cov-report=html --cov-fail-under=95
```

## Integration Testing

Test Kubernetes deployment integration with platform components:
```bash
# Test integration with Session 1 (Logfire)
pytest k8s/tests/integration/test_kubernetes_logfire_integration.py -v

# Test integration with monitoring systems
pytest k8s/tests/integration/test_kubernetes_monitoring_integration.py -v

# Test complete platform deployment
pytest k8s/tests/integration/test_complete_platform_deployment.py -v
```