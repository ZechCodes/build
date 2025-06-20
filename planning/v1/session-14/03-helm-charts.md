# Session 14.3: Helm Charts & Configuration Management

## Objective
Implement comprehensive Helm charts with templating, values management, and multi-environment configuration to provide flexible, maintainable, and scalable deployment management for the Build platform across different environments.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for Helm deployment tracking and configuration monitoring
- **Session 11**: Integrates with metrics collection for Helm deployment performance
- **Session 12**: Manages rate limiting configuration through Helm values
- **Session 13**: Leverages high availability configuration through Helm templates
- **All Sessions**: Provides Helm-based configuration management for entire platform

## Core Implementation

### Helm Chart Structure
**Location**: `k8s/helm/build-platform/`

```
k8s/helm/build-platform/
├── Chart.yaml
├── values.yaml
├── values-staging.yaml
├── values-production.yaml
├── templates/
│   ├── _helpers.tpl
│   ├── api/
│   │   ├── deployment.yaml
│   │   ├── service.yaml
│   │   ├── hpa.yaml
│   │   └── servicemonitor.yaml
│   ├── frontend/
│   │   ├── deployment.yaml
│   │   ├── service.yaml
│   │   └── hpa.yaml
│   ├── vm-manager/
│   │   ├── deployment.yaml
│   │   ├── service.yaml
│   │   └── rbac.yaml
│   ├── ingress.yaml
│   ├── secrets.yaml
│   ├── configmap.yaml
│   └── networkpolicy.yaml
├── charts/
└── crds/
```

### Chart Metadata
**Location**: `k8s/helm/build-platform/Chart.yaml`

```yaml
# k8s/helm/build-platform/Chart.yaml
apiVersion: v2
name: build-platform
description: A Helm chart for the Build Platform - Cloud Development Environment
type: application
version: 1.0.0
appVersion: "1.0.0"
home: https://getbuild.ing
sources:
  - https://github.com/your-org/build-platform
maintainers:
  - name: Build Platform Team
    email: platform@getbuild.ing
keywords:
  - development
  - cloud
  - containers
  - firecracker
  - ide
annotations:
  category: Development
  licenses: MIT
dependencies:
  - name: postgresql
    version: "12.1.9"
    repository: https://charts.bitnami.com/bitnami
    condition: postgresql.enabled
  - name: redis
    version: "17.3.7"
    repository: https://charts.bitnami.com/bitnami
    condition: redis.enabled
  - name: minio
    version: "12.1.3"
    repository: https://charts.bitnami.com/bitnami
    condition: minio.enabled
```

### Default Values Configuration
**Location**: `k8s/helm/build-platform/values.yaml`

```yaml
# k8s/helm/build-platform/values.yaml
# Default values for build-platform

global:
  imageRegistry: ghcr.io
  imagePullSecrets:
    - name: ghcr-secret
  storageClass: "fast-ssd"
  
replicaCount: 3

image:
  api:
    repository: your-org/build-platform/api
    tag: "latest"
    pullPolicy: Always
  frontend:
    repository: your-org/build-platform/frontend
    tag: "latest"
    pullPolicy: Always
  vmManager:
    repository: your-org/build-platform/vm-manager
    tag: "latest"
    pullPolicy: Always

nameOverride: ""
fullnameOverride: ""

serviceAccount:
  create: true
  annotations: {}
  name: ""

podAnnotations:
  prometheus.io/scrape: "true"
  prometheus.io/port: "8000"

podSecurityContext:
  runAsNonRoot: true
  runAsUser: 1000
  runAsGroup: 1000
  fsGroup: 1000
  seccompProfile:
    type: RuntimeDefault

securityContext:
  allowPrivilegeEscalation: false
  readOnlyRootFilesystem: true
  capabilities:
    drop:
    - ALL

# API Configuration
api:
  replicaCount: 3
  image:
    repository: your-org/build-platform/api
    tag: "latest"
    pullPolicy: Always
  
  service:
    type: ClusterIP
    port: 80
    targetPort: 8000
    annotations: {}
  
  resources:
    limits:
      cpu: 1000m
      memory: 1Gi
      ephemeral-storage: 2Gi
    requests:
      cpu: 500m
      memory: 512Mi
      ephemeral-storage: 1Gi
  
  autoscaling:
    enabled: true
    minReplicas: 3
    maxReplicas: 10
    targetCPUUtilizationPercentage: 70
    targetMemoryUtilizationPercentage: 80
  
  nodeSelector: {}
  tolerations: []
  affinity: {}
  
  env:
    - name: ENVIRONMENT
      value: "production"
    - name: LOG_LEVEL
      value: "INFO"
    - name: DEBUG
      value: "false"
  
  healthcheck:
    enabled: true
    path: /health
    readinessPath: /health/ready
    startupPath: /health/startup
    livenessDelay: 30
    readinessDelay: 5
    startupDelay: 10

# Frontend Configuration
frontend:
  replicaCount: 2
  image:
    repository: your-org/build-platform/frontend
    tag: "latest"
    pullPolicy: Always
  
  service:
    type: ClusterIP
    port: 80
    targetPort: 80
    annotations: {}
  
  resources:
    limits:
      cpu: 500m
      memory: 512Mi
      ephemeral-storage: 1Gi
    requests:
      cpu: 250m
      memory: 256Mi
      ephemeral-storage: 512Mi
  
  autoscaling:
    enabled: true
    minReplicas: 2
    maxReplicas: 6
    targetCPUUtilizationPercentage: 70
  
  nodeSelector: {}
  tolerations: []
  affinity: {}

# VM Manager Configuration
vmManager:
  replicaCount: 2
  image:
    repository: your-org/build-platform/vm-manager
    tag: "latest"
    pullPolicy: Always
  
  service:
    type: ClusterIP
    port: 80
    targetPort: 8080
    annotations: {}
  
  resources:
    limits:
      cpu: 2000m
      memory: 2Gi
      ephemeral-storage: 5Gi
    requests:
      cpu: 1000m
      memory: 1Gi
      ephemeral-storage: 2Gi
  
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
  
  privileged: true  # Required for Firecracker
  capabilities:
    add:
      - NET_ADMIN
      - SYS_ADMIN

# Database Configuration
database:
  enabled: false  # Using external managed database
  external:
    host: postgres-cluster.internal
    port: 5432
    database: build_platform
    username: build_platform
    ssl: require
    connectionPoolSize: 20
    maxOverflow: 30

# Redis Configuration
redis:
  enabled: false  # Using external Redis cluster
  external:
    sentinels:
      - host: redis-sentinel-1.internal
        port: 26379
      - host: redis-sentinel-2.internal
        port: 26379
      - host: redis-sentinel-3.internal
        port: 26379
    masterName: mymaster
    password: ""
    ssl: true

# Storage Configuration
storage:
  minio:
    endpoint: minio.internal:9000
    bucket: build-platform
    ssl: true
    region: us-east-1

# Ingress Configuration
ingress:
  enabled: true
  className: nginx
  annotations:
    nginx.ingress.kubernetes.io/ssl-redirect: "true"
    nginx.ingress.kubernetes.io/force-ssl-redirect: "true"
    cert-manager.io/cluster-issuer: letsencrypt-prod
    nginx.ingress.kubernetes.io/rate-limit: "100"
    nginx.ingress.kubernetes.io/rate-limit-window: "1m"
    nginx.ingress.kubernetes.io/proxy-body-size: "100m"
    nginx.ingress.kubernetes.io/enable-cors: "true"
  
  hosts:
    - host: getbuild.ing
      paths:
        - path: /api
          pathType: Prefix
          service: build-platform-api
        - path: /
          pathType: Prefix
          service: build-platform-frontend
  
  tls:
    - secretName: build-platform-tls
      hosts:
        - getbuild.ing

# WebSocket Ingress Configuration
websocketIngress:
  enabled: true
  className: nginx
  annotations:
    nginx.ingress.kubernetes.io/ssl-redirect: "true"
    nginx.ingress.kubernetes.io/proxy-read-timeout: "3600"
    nginx.ingress.kubernetes.io/proxy-send-timeout: "3600"
    nginx.ingress.kubernetes.io/websocket-services: "build-platform-api"
    nginx.ingress.kubernetes.io/upstream-hash-by: "$binary_remote_addr"
  
  host: ws.getbuild.ing
  tls:
    secretName: build-platform-websocket-tls

# Monitoring Configuration
monitoring:
  enabled: true
  prometheus:
    enabled: true
    serviceMonitor:
      enabled: true
      interval: 30s
      scrapeTimeout: 10s
      additionalLabels: {}
  grafana:
    enabled: true
    dashboards:
      enabled: true
      annotations:
        grafana_folder: "Build Platform"
  alerts:
    enabled: true
    rules:
      - alert: HighCPUUsage
        expr: container_cpu_usage_seconds_total > 0.8
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "High CPU usage detected"

# Security Configuration
security:
  podSecurityPolicy:
    enabled: true
  networkPolicy:
    enabled: true
    ingress:
      enabled: true
      from:
        - namespaceSelector:
            matchLabels:
              name: ingress-nginx
    egress:
      enabled: true
      to:
        - namespaceSelector: {}
  rbac:
    create: true

# Application Configuration
config:
  environment: production
  logLevel: INFO
  debug: false
  cors:
    allowedOrigins:
      - https://getbuild.ing
    allowedMethods:
      - GET
      - POST
      - PUT
      - DELETE
      - PATCH
      - OPTIONS
    allowedHeaders:
      - Authorization
      - Content-Type
      - X-Requested-With
  rateLimit:
    enabled: true
    requests: 1000
    window: 60
  session:
    timeout: 3600
    maxConcurrent: 10
  vm:
    maxInstances: 100
    defaultCPU: 1
    defaultMemory: 512
    maxCPU: 4
    maxMemory: 4096

# Secrets Configuration
secrets:
  create: true
  annotations:
    external-secrets.io/backend: vault
    external-secrets.io/key: secret/build-platform
  stringData: {}

# ConfigMap Configuration
configMap:
  create: true
  data: {}

# Persistence Configuration
persistence:
  enabled: true
  storageClass: "fast-ssd"
  size: 10Gi
  accessModes:
    - ReadWriteOnce

# Node Configuration
nodeSelector: {}
tolerations: []
affinity: {}

# Pod Disruption Budget
podDisruptionBudget:
  enabled: true
  minAvailable: 2
  maxUnavailable: null
```

### Staging Environment Values
**Location**: `k8s/helm/build-platform/values-staging.yaml`

```yaml
# k8s/helm/build-platform/values-staging.yaml
# Staging environment specific values

replicaCount: 2

image:
  api:
    tag: "develop"
  frontend:
    tag: "develop"
  vmManager:
    tag: "develop"

api:
  replicaCount: 2
  resources:
    limits:
      cpu: 500m
      memory: 512Mi
    requests:
      cpu: 250m
      memory: 256Mi
  autoscaling:
    enabled: true
    minReplicas: 2
    maxReplicas: 4
  env:
    - name: ENVIRONMENT
      value: "staging"
    - name: LOG_LEVEL
      value: "DEBUG"
    - name: DEBUG
      value: "true"

frontend:
  replicaCount: 1
  resources:
    limits:
      cpu: 250m
      memory: 256Mi
    requests:
      cpu: 125m
      memory: 128Mi
  autoscaling:
    enabled: false

vmManager:
  replicaCount: 1
  resources:
    limits:
      cpu: 1000m
      memory: 1Gi
    requests:
      cpu: 500m
      memory: 512Mi

database:
  external:
    host: postgres-staging.internal
    database: build_platform_staging

redis:
  external:
    sentinels:
      - host: redis-staging-sentinel-1.internal
        port: 26379

storage:
  minio:
    endpoint: minio-staging.internal:9000
    bucket: build-platform-staging

ingress:
  hosts:
    - host: staging.getbuild.ing
      paths:
        - path: /api
          pathType: Prefix
          service: build-platform-api
        - path: /
          pathType: Prefix
          service: build-platform-frontend
  tls:
    - secretName: build-platform-staging-tls
      hosts:
        - staging.getbuild.ing

websocketIngress:
  host: ws-staging.getbuild.ing
  tls:
    secretName: build-platform-staging-websocket-tls

config:
  environment: staging
  logLevel: DEBUG
  debug: true
  cors:
    allowedOrigins:
      - https://staging.getbuild.ing
  vm:
    maxInstances: 20

monitoring:
  prometheus:
    serviceMonitor:
      additionalLabels:
        environment: staging

secrets:
  annotations:
    external-secrets.io/key: secret/build-platform-staging

podDisruptionBudget:
  enabled: false
```

### Production Environment Values
**Location**: `k8s/helm/build-platform/values-production.yaml`

```yaml
# k8s/helm/build-platform/values-production.yaml
# Production environment specific values

replicaCount: 5

image:
  api:
    tag: "1.0.0"  # Use specific version tags in production
  frontend:
    tag: "1.0.0"
  vmManager:
    tag: "1.0.0"

api:
  replicaCount: 5
  resources:
    limits:
      cpu: 2000m
      memory: 2Gi
      ephemeral-storage: 4Gi
    requests:
      cpu: 1000m
      memory: 1Gi
      ephemeral-storage: 2Gi
  autoscaling:
    enabled: true
    minReplicas: 5
    maxReplicas: 20
    targetCPUUtilizationPercentage: 60
    targetMemoryUtilizationPercentage: 70
  env:
    - name: ENVIRONMENT
      value: "production"
    - name: LOG_LEVEL
      value: "INFO"
    - name: DEBUG
      value: "false"

frontend:
  replicaCount: 3
  resources:
    limits:
      cpu: 1000m
      memory: 1Gi
    requests:
      cpu: 500m
      memory: 512Mi
  autoscaling:
    enabled: true
    minReplicas: 3
    maxReplicas: 10

vmManager:
  replicaCount: 3
  resources:
    limits:
      cpu: 4000m
      memory: 4Gi
      ephemeral-storage: 10Gi
    requests:
      cpu: 2000m
      memory: 2Gi
      ephemeral-storage: 5Gi

database:
  external:
    host: postgres-prod-cluster.internal
    database: build_platform_prod
    connectionPoolSize: 50
    maxOverflow: 100

redis:
  external:
    sentinels:
      - host: redis-prod-sentinel-1.internal
        port: 26379
      - host: redis-prod-sentinel-2.internal
        port: 26379
      - host: redis-prod-sentinel-3.internal
        port: 26379

storage:
  minio:
    endpoint: minio-prod.internal:9000
    bucket: build-platform-prod

ingress:
  annotations:
    nginx.ingress.kubernetes.io/rate-limit: "200"
    nginx.ingress.kubernetes.io/rate-limit-window: "1m"
  hosts:
    - host: getbuild.ing
      paths:
        - path: /api
          pathType: Prefix
          service: build-platform-api
        - path: /
          pathType: Prefix
          service: build-platform-frontend
  tls:
    - secretName: build-platform-prod-tls
      hosts:
        - getbuild.ing

websocketIngress:
  host: ws.getbuild.ing
  tls:
    secretName: build-platform-prod-websocket-tls

config:
  environment: production
  logLevel: INFO
  debug: false
  cors:
    allowedOrigins:
      - https://getbuild.ing
  rateLimit:
    requests: 2000
    window: 60
  vm:
    maxInstances: 1000

monitoring:
  prometheus:
    serviceMonitor:
      interval: 15s
      additionalLabels:
        environment: production
  alerts:
    enabled: true
    rules:
      - alert: HighErrorRate
        expr: rate(http_requests_total{status=~"5.."}[5m]) > 0.1
        for: 2m
        labels:
          severity: critical
        annotations:
          summary: "High error rate detected in production"

security:
  podSecurityPolicy:
    enabled: true
  networkPolicy:
    enabled: true
  rbac:
    create: true

secrets:
  annotations:
    external-secrets.io/key: secret/build-platform-prod

podDisruptionBudget:
  enabled: true
  minAvailable: 3

affinity:
  podAntiAffinity:
    requiredDuringSchedulingIgnoredDuringExecution:
    - labelSelector:
        matchExpressions:
        - key: app.kubernetes.io/name
          operator: In
          values:
          - build-platform
      topologyKey: kubernetes.io/hostname
```

### Helper Templates
**Location**: `k8s/helm/build-platform/templates/_helpers.tpl`

```yaml
{{/*
k8s/helm/build-platform/templates/_helpers.tpl
Common template helpers for build-platform chart
*/}}

{{/*
Expand the name of the chart.
*/}}
{{- define "build-platform.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
*/}}
{{- define "build-platform.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Create chart name and version as used by the chart label.
*/}}
{{- define "build-platform.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "build-platform.labels" -}}
helm.sh/chart: {{ include "build-platform.chart" . }}
{{ include "build-platform.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels
*/}}
{{- define "build-platform.selectorLabels" -}}
app.kubernetes.io/name: {{ include "build-platform.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Create the name of the service account to use for API
*/}}
{{- define "build-platform.api.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (printf "%s-api" (include "build-platform.fullname" .)) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
Create the name of the service account to use for Frontend
*/}}
{{- define "build-platform.frontend.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- printf "%s-frontend" (include "build-platform.fullname" .) }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
Create the name of the service account to use for VM Manager
*/}}
{{- define "build-platform.vmManager.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- printf "%s-vm-manager" (include "build-platform.fullname" .) }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
API image
*/}}
{{- define "build-platform.api.image" -}}
{{- $registry := .Values.global.imageRegistry | default "" }}
{{- $repository := .Values.api.image.repository | default .Values.image.api.repository }}
{{- $tag := .Values.api.image.tag | default .Values.image.api.tag | default .Chart.AppVersion }}
{{- if $registry }}
{{- printf "%s/%s:%s" $registry $repository $tag }}
{{- else }}
{{- printf "%s:%s" $repository $tag }}
{{- end }}
{{- end }}

{{/*
Frontend image
*/}}
{{- define "build-platform.frontend.image" -}}
{{- $registry := .Values.global.imageRegistry | default "" }}
{{- $repository := .Values.frontend.image.repository | default .Values.image.frontend.repository }}
{{- $tag := .Values.frontend.image.tag | default .Values.image.frontend.tag | default .Chart.AppVersion }}
{{- if $registry }}
{{- printf "%s/%s:%s" $registry $repository $tag }}
{{- else }}
{{- printf "%s:%s" $repository $tag }}
{{- end }}
{{- end }}

{{/*
VM Manager image
*/}}
{{- define "build-platform.vmManager.image" -}}
{{- $registry := .Values.global.imageRegistry | default "" }}
{{- $repository := .Values.vmManager.image.repository | default .Values.image.vmManager.repository }}
{{- $tag := .Values.vmManager.image.tag | default .Values.image.vmManager.tag | default .Chart.AppVersion }}
{{- if $registry }}
{{- printf "%s/%s:%s" $registry $repository $tag }}
{{- else }}
{{- printf "%s:%s" $repository $tag }}
{{- end }}
{{- end }}

{{/*
Database URL
*/}}
{{- define "build-platform.database.url" -}}
{{- if .Values.database.enabled }}
{{- printf "postgresql://%s:%s@%s:%d/%s" .Values.database.auth.username .Values.database.auth.password (include "postgresql.primary.fullname" .Subcharts.postgresql) .Values.database.primary.service.port .Values.database.auth.database }}
{{- else }}
{{- printf "postgresql://%s@%s:%d/%s?sslmode=%s" .Values.database.external.username .Values.database.external.host .Values.database.external.port .Values.database.external.database .Values.database.external.ssl }}
{{- end }}
{{- end }}

{{/*
Redis URL
*/}}
{{- define "build-platform.redis.url" -}}
{{- if .Values.redis.enabled }}
{{- printf "redis://%s:%d/0" (include "redis.fullname" .Subcharts.redis) .Values.redis.redisPort }}
{{- else }}
{{- printf "redis://sentinel:%s@%s/0" .Values.redis.external.password (join "," (range $i, $sentinel := .Values.redis.external.sentinels)){{ printf "%s:%d" $sentinel.host $sentinel.port }}{{ end }} }}
{{- end }}
{{- end }}

{{/*
Resource limits
*/}}
{{- define "build-platform.resources" -}}
{{- if . }}
resources:
  {{- if .limits }}
  limits:
    {{- range $key, $value := .limits }}
    {{ $key }}: {{ $value }}
    {{- end }}
  {{- end }}
  {{- if .requests }}
  requests:
    {{- range $key, $value := .requests }}
    {{ $key }}: {{ $value }}
    {{- end }}
  {{- end }}
{{- end }}
{{- end }}
```

### API Deployment Template
**Location**: `k8s/helm/build-platform/templates/api/deployment.yaml`

```yaml
# k8s/helm/build-platform/templates/api/deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: {{ include "build-platform.fullname" . }}-api
  namespace: {{ .Release.Namespace }}
  labels:
    {{- include "build-platform.labels" . | nindent 4 }}
    app.kubernetes.io/component: api
spec:
  replicas: {{ .Values.api.replicaCount | default .Values.replicaCount }}
  strategy:
    type: RollingUpdate
    rollingUpdate:
      maxSurge: 1
      maxUnavailable: 0
  selector:
    matchLabels:
      {{- include "build-platform.selectorLabels" . | nindent 6 }}
      app.kubernetes.io/component: api
  template:
    metadata:
      labels:
        {{- include "build-platform.selectorLabels" . | nindent 8 }}
        app.kubernetes.io/component: api
      annotations:
        {{- with .Values.podAnnotations }}
        {{- toYaml . | nindent 8 }}
        {{- end }}
        checksum/config: {{ include (print $.Template.BasePath "/configmap.yaml") . | sha256sum }}
        checksum/secret: {{ include (print $.Template.BasePath "/secrets.yaml") . | sha256sum }}
    spec:
      serviceAccountName: {{ include "build-platform.api.serviceAccountName" . }}
      securityContext:
        {{- toYaml .Values.podSecurityContext | nindent 8 }}
      
      {{- if .Values.global.imagePullSecrets }}
      imagePullSecrets:
        {{- range .Values.global.imagePullSecrets }}
        - name: {{ . }}
        {{- end }}
      {{- end }}

      initContainers:
      - name: migrate-database
        image: {{ include "build-platform.api.image" . }}
        imagePullPolicy: {{ .Values.api.image.pullPolicy | default .Values.image.api.pullPolicy }}
        command: ["python", "-m", "alembic", "upgrade", "head"]
        env:
        - name: DATABASE_URL
          valueFrom:
            secretKeyRef:
              name: {{ include "build-platform.fullname" . }}-secrets
              key: database-url
        securityContext:
          {{- toYaml .Values.securityContext | nindent 10 }}
        {{- include "build-platform.resources" .Values.api.initContainer.resources | nindent 8 }}
        volumeMounts:
        - name: tmp
          mountPath: /tmp

      containers:
      - name: api
        image: {{ include "build-platform.api.image" . }}
        imagePullPolicy: {{ .Values.api.image.pullPolicy | default .Values.image.api.pullPolicy }}
        ports:
        - name: http
          containerPort: 8000
          protocol: TCP
        {{- if .Values.monitoring.enabled }}
        - name: metrics
          containerPort: 9090
          protocol: TCP
        {{- end }}
        
        env:
        {{- range .Values.api.env }}
        - name: {{ .name }}
          value: {{ .value | quote }}
        {{- end }}
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
              name: {{ include "build-platform.fullname" . }}-secrets
              key: database-url
        - name: REDIS_URL
          valueFrom:
            secretKeyRef:
              name: {{ include "build-platform.fullname" . }}-secrets
              key: redis-url
        - name: JWT_SECRET
          valueFrom:
            secretKeyRef:
              name: {{ include "build-platform.fullname" . }}-secrets
              key: jwt-secret
        {{- if .Values.monitoring.enabled }}
        - name: LOGFIRE_TOKEN
          valueFrom:
            secretKeyRef:
              name: {{ include "build-platform.fullname" . }}-secrets
              key: logfire-token
        {{- end }}

        {{- if .Values.api.healthcheck.enabled }}
        livenessProbe:
          httpGet:
            path: {{ .Values.api.healthcheck.path }}
            port: http
          initialDelaySeconds: {{ .Values.api.healthcheck.livenessDelay }}
          periodSeconds: 30
          timeoutSeconds: 10
          failureThreshold: 3

        readinessProbe:
          httpGet:
            path: {{ .Values.api.healthcheck.readinessPath }}
            port: http
          initialDelaySeconds: {{ .Values.api.healthcheck.readinessDelay }}
          periodSeconds: 10
          timeoutSeconds: 5
          failureThreshold: 3

        startupProbe:
          httpGet:
            path: {{ .Values.api.healthcheck.startupPath }}
            port: http
          initialDelaySeconds: {{ .Values.api.healthcheck.startupDelay }}
          periodSeconds: 5
          timeoutSeconds: 3
          failureThreshold: 30
        {{- end }}

        {{- include "build-platform.resources" .Values.api.resources | nindent 8 }}

        securityContext:
          {{- toYaml .Values.securityContext | nindent 10 }}

        volumeMounts:
        - name: tmp
          mountPath: /tmp
        - name: cache
          mountPath: /app/.cache
        - name: logs
          mountPath: /app/logs
        - name: config
          mountPath: /app/config
          readOnly: true

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
      - name: config
        configMap:
          name: {{ include "build-platform.fullname" . }}-config

      {{- with .Values.api.nodeSelector | default .Values.nodeSelector }}
      nodeSelector:
        {{- toYaml . | nindent 8 }}
      {{- end }}
      
      {{- with .Values.api.affinity | default .Values.affinity }}
      affinity:
        {{- toYaml . | nindent 8 }}
      {{- end }}
      
      {{- with .Values.api.tolerations | default .Values.tolerations }}
      tolerations:
        {{- toYaml . | nindent 8 }}
      {{- end }}
```

## TDD Implementation Cycle

### Red Phase: Helm Chart Test Creation
```python
# k8s/helm/tests/test_helm_charts.py
import pytest
import yaml
import subprocess
import tempfile
import os
from typing import Dict, Any, List

class HelmChartTester:
    def __init__(self, chart_path: str = "k8s/helm/build-platform"):
        self.chart_path = chart_path
    
    def test_chart_validation(self):
        """Test that Helm chart is valid"""
        # This test should initially fail (Red phase)
        assert False, "Helm chart not implemented yet"
    
    def test_template_rendering(self):
        """Test that templates render correctly"""
        # This test should initially fail (Red phase)
        assert False, "Helm templates not implemented yet"
    
    def test_values_validation(self):
        """Test that values files are valid"""
        # This test should initially fail (Red phase)
        assert False, "Helm values validation not implemented yet"
    
    def test_multi_environment_config(self):
        """Test that multi-environment configuration works"""
        # This test should initially fail (Red phase)
        assert False, "Multi-environment configuration not implemented yet"
    
    def test_helm_install_simulation(self):
        """Test that Helm install works in dry-run mode"""
        # This test should initially fail (Red phase)
        assert False, "Helm installation not implemented yet"

# CLI for testing Helm charts
if __name__ == "__main__":
    tester = HelmChartTester()
    
    try:
        tester.test_chart_validation()
        tester.test_template_rendering()
        tester.test_values_validation()
        tester.test_multi_environment_config()
        tester.test_helm_install_simulation()
        print("All Helm chart tests passed!")
    except AssertionError as e:
        print(f"Helm chart test failed: {e}")
        exit(1)
```

### Green Phase: Helm Chart Implementation
```python
# Implement Helm chart features to make tests pass
# This involves creating templates, values files, and helper functions
```

### Refactor Phase: Helm Chart Optimization
```python
# Optimize Helm charts for performance and maintainability
# Add advanced templating and configuration management
# Improve values structure and documentation
```

## Security Checklist ✅

### Helm Chart Security
- [ ] Secure default values and configuration
- [ ] Template validation and sanitization
- [ ] Secret management and encryption
- [ ] RBAC configuration with least privilege
- [ ] Security context enforcement
- [ ] Network policy templates
- [ ] Pod security policy templates
- [ ] Image pull secret management
- [ ] Resource limit enforcement
- [ ] Security scanning integration

### Configuration Security
- [ ] Secure configuration management
- [ ] Environment-specific security settings
- [ ] Secret rotation and management
- [ ] Configuration validation and compliance
- [ ] Sensitive data protection
- [ ] Access control for configuration
- [ ] Configuration audit logging
- [ ] Secure defaults enforcement
- [ ] Configuration drift detection
- [ ] Emergency configuration procedures

### Template Security
- [ ] Template injection prevention
- [ ] Input validation and sanitization
- [ ] Secure templating practices
- [ ] Output validation and verification
- [ ] Template access controls
- [ ] Version control for templates
- [ ] Template security scanning
- [ ] Secure helper functions
- [ ] Template documentation security
- [ ] Template testing and validation

### Deployment Security
- [ ] Secure deployment procedures
- [ ] Environment isolation and protection
- [ ] Deployment validation and verification
- [ ] Rollback security procedures
- [ ] Change management security
- [ ] Deployment audit logging
- [ ] Access control for deployments
- [ ] Security testing integration
- [ ] Compliance validation
- [ ] Incident response procedures

## Performance Requirements

### Template Performance
- Template rendering time < 30 seconds
- Values validation time < 10 seconds
- Chart packaging time < 60 seconds
- Dependency resolution time < 30 seconds
- Configuration generation time < 15 seconds
- Multi-environment rendering efficiency > 90%

### Deployment Performance
- Helm install time < 5 minutes
- Helm upgrade time < 3 minutes
- Helm rollback time < 2 minutes
- Values override processing < 5 seconds
- Chart validation time < 10 seconds
- Dependency update time < 2 minutes

### Configuration Performance
- Configuration loading time < 1 second
- Environment switching time < 30 seconds
- Secret injection time < 10 seconds
- ConfigMap generation time < 5 seconds
- Template caching efficiency > 95%
- Configuration validation time < 5 seconds

## Commit Instructions

After implementing the Helm charts system:

```bash
git add k8s/helm/ k8s/helm/tests/
git commit -m "Add comprehensive Helm charts for multi-environment deployment

- Implement production-ready Helm chart with comprehensive templating
- Add environment-specific values for staging and production
- Include helper templates for configuration management
- Add API, frontend, and VM manager deployment templates
- Implement ingress and service configurations with templating
- Add secrets and ConfigMap management with external integration
- Include auto-scaling and resource management templates
- Add monitoring and security policy templates
- Implement multi-environment configuration management
- Add TDD cycle with Red-Green-Refactor for Helm charts
- Ensure >95% Helm chart test coverage

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete Helm chart test suite:

```bash
# Test Helm chart validation
helm lint k8s/helm/build-platform/

# Test template rendering
helm template build-platform k8s/helm/build-platform/ --values k8s/helm/build-platform/values.yaml
helm template build-platform k8s/helm/build-platform/ --values k8s/helm/build-platform/values-staging.yaml
helm template build-platform k8s/helm/build-platform/ --values k8s/helm/build-platform/values-production.yaml

# Test dry-run installation
helm install build-platform-test k8s/helm/build-platform/ --dry-run --debug

# Test chart packaging
helm package k8s/helm/build-platform/

# Test dependency management
helm dependency update k8s/helm/build-platform/
```

Validate Helm chart test coverage:
```bash
python k8s/helm/tests/test_helm_charts.py
pytest k8s/helm/tests/ --cov=helm --cov-report=html --cov-fail-under=95
```

## Integration Testing

Test Helm chart integration with platform components:
```bash
# Test integration with Session 1 (Logfire)
pytest k8s/helm/tests/integration/test_helm_logfire_integration.py -v

# Test integration with monitoring systems
pytest k8s/helm/tests/integration/test_helm_monitoring_integration.py -v

# Test complete platform Helm deployment
pytest k8s/helm/tests/integration/test_complete_helm_deployment.py -v
```