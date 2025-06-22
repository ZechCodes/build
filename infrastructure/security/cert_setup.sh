#!/bin/bash
# Certificate Setup Script for Production TLS
# Generates self-signed certificates for development/staging
# and provides instructions for production certificates

set -euo pipefail

CERT_DIR="$(dirname "$0")/certs"
CA_KEY="$CERT_DIR/ca.key"
CA_CERT="$CERT_DIR/ca.crt"
REDIS_KEY="$CERT_DIR/redis.key"
REDIS_CERT="$CERT_DIR/redis.crt"
CLIENT_KEY="$CERT_DIR/client.key"
CLIENT_CERT="$CERT_DIR/client.crt"
NGINX_KEY="$CERT_DIR/nginx.key"
NGINX_CERT="$CERT_DIR/nginx.crt"

echo "🔐 Setting up TLS certificates for Build Platform"

# Create certificate directory
mkdir -p "$CERT_DIR"
cd "$CERT_DIR"

# Generate CA private key
echo "📝 Generating Certificate Authority (CA) private key..."
openssl genrsa -out ca.key 4096

# Generate CA certificate
echo "📜 Generating CA certificate..."
openssl req -new -x509 -days 365 -key ca.key -out ca.crt -subj "/C=US/ST=CA/L=SF/O=BuildPlatform/CN=Build Platform CA"

# Generate Redis server private key
echo "🔑 Generating Redis server private key..."
openssl genrsa -out redis.key 2048

# Generate Redis server certificate signing request
echo "📋 Generating Redis server certificate request..."
openssl req -new -key redis.key -out redis.csr -subj "/C=US/ST=CA/L=SF/O=BuildPlatform/CN=redis"

# Sign Redis server certificate
echo "✅ Signing Redis server certificate..."
openssl x509 -req -days 365 -in redis.csr -CA ca.crt -CAkey ca.key -CAcreateserial -out redis.crt

# Generate client private key
echo "🔑 Generating client private key..."
openssl genrsa -out client.key 2048

# Generate client certificate signing request
echo "📋 Generating client certificate request..."
openssl req -new -key client.key -out client.csr -subj "/C=US/ST=CA/L=SF/O=BuildPlatform/CN=client"

# Sign client certificate
echo "✅ Signing client certificate..."
openssl x509 -req -days 365 -in client.csr -CA ca.crt -CAkey ca.key -CAcreateserial -out client.crt

# Generate Nginx private key
echo "🔑 Generating Nginx private key..."
openssl genrsa -out nginx.key 2048

# Generate Nginx certificate signing request with SAN
echo "📋 Generating Nginx certificate request..."
cat > nginx.conf <<EOF
[req]
distinguished_name = req_distinguished_name
req_extensions = v3_req
prompt = no

[req_distinguished_name]
C = US
ST = CA
L = SF
O = BuildPlatform
CN = buildplatform.dev

[v3_req]
keyUsage = keyEncipherment, dataEncipherment
extendedKeyUsage = serverAuth
subjectAltName = @alt_names

[alt_names]
DNS.1 = buildplatform.dev
DNS.2 = app.buildplatform.dev
DNS.3 = admin.buildplatform.dev
DNS.4 = api.buildplatform.dev
DNS.5 = ws.buildplatform.dev
DNS.6 = localhost
IP.1 = 127.0.0.1
EOF

openssl req -new -key nginx.key -out nginx.csr -config nginx.conf

# Sign Nginx certificate
echo "✅ Signing Nginx certificate..."
openssl x509 -req -days 365 -in nginx.csr -CA ca.crt -CAkey ca.key -CAcreateserial -out nginx.crt -extensions v3_req -extfile nginx.conf

# Set appropriate permissions
chmod 600 *.key
chmod 644 *.crt

# Clean up temporary files
rm -f *.csr *.srl nginx.conf

echo "🎉 Certificate generation complete!"
echo ""
echo "📁 Generated certificates:"
echo "   CA Certificate: $CA_CERT"
echo "   Redis Certificate: $REDIS_CERT"
echo "   Client Certificate: $CLIENT_CERT"
echo "   Nginx Certificate: $NGINX_CERT"
echo ""
echo "🔒 Private keys (secure these!):"
echo "   CA Key: $CA_KEY"
echo "   Redis Key: $REDIS_KEY"
echo "   Client Key: $CLIENT_KEY"
echo "   Nginx Key: $NGINX_KEY"
echo ""
echo "⚠️  IMPORTANT NOTES:"
echo "   - These are self-signed certificates for development/staging"
echo "   - For production, use certificates from a trusted CA (Let's Encrypt, etc.)"
echo "   - Never commit private keys to version control"
echo "   - Rotate certificates regularly (recommended: every 90 days)"
echo ""
echo "🚀 Next steps:"
echo "   1. Copy certificates to your deployment environment"
echo "   2. Update environment variables in .env.production"
echo "   3. Start services with: podman-compose -f production_deployment.yml up"
echo ""

# Verify certificates
echo "🔍 Verifying certificates..."
echo "Redis certificate:"
openssl x509 -in redis.crt -noout -subject -issuer -dates

echo ""
echo "Client certificate:"
openssl x509 -in client.crt -noout -subject -issuer -dates

echo ""
echo "Nginx certificate:"
openssl x509 -in nginx.crt -noout -subject -issuer -dates

echo ""
echo "✅ Certificate verification complete!"
echo ""
echo "🔧 Testing Redis TLS connection:"
echo "   redis-cli --tls --cert client.crt --key client.key --cacert ca.crt -h localhost -p 6379 ping"
echo ""
echo "🌐 Testing HTTPS connection:"
echo "   curl -k https://localhost/health"