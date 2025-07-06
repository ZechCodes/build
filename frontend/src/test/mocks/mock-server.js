/**
 * Mock API Server for E2E Testing
 * Provides realistic backend responses for comprehensive testing
 */

const express = require('express');
const WebSocket = require('ws');
const cors = require('cors');

const app = express();
const PORT = 8000;
const WS_PORT = 8001;

// Middleware
app.use(cors());
app.use(express.json());

// Mock data store
const mockData = {
  users: new Map(),
  sessions: new Map(),
  vms: new Map(),
  snapshots: new Map()
};

// Initialize mock data
mockData.users.set('e2e-user-123', {
  id: 'e2e-user-123',
  username: 'e2e-test-user',
  email: 'e2e@test.com',
  permissions: ['terminal:access', 'vm:manage']
});

mockData.vms.set('e2e-test-vm-456', {
  id: 'e2e-test-vm-456',
  user_id: 'e2e-user-123',
  state: 'running',
  created_at: new Date().toISOString(),
  config: {
    vcpus: 2,
    memory_mb: 1024,
    disk_gb: 10
  }
});

// Authentication endpoints
app.post('/api/v1/auth/login', (req, res) => {
  const { username, password } = req.body;
  
  if (username === 'e2e-test-user' && password === 'test-password') {
    res.json({
      access_token: 'mock-e2e-token',
      token_type: 'bearer',
      user: mockData.users.get('e2e-user-123')
    });
  } else {
    res.status(401).json({ error: 'Invalid credentials' });
  }
});

// Terminal session endpoints
app.post('/api/v1/sessions', (req, res) => {
  const sessionId = 'e2e-test-session-789';
  const session = {
    id: sessionId,
    vm_id: req.body.vm_id || 'e2e-test-vm-456',
    user_id: 'e2e-user-123',
    state: 'active',
    created_at: new Date().toISOString(),
    terminal_size: req.body.terminal_size || { cols: 80, rows: 24 }
  };
  
  mockData.sessions.set(sessionId, session);
  res.json(session);
});

app.get('/api/v1/sessions/:sessionId', (req, res) => {
  const session = mockData.sessions.get(req.params.sessionId);
  if (session) {
    res.json(session);
  } else {
    res.status(404).json({ error: 'Session not found' });
  }
});

app.post('/api/v1/sessions/:sessionId/attach', (req, res) => {
  const session = mockData.sessions.get(req.params.sessionId);
  if (session) {
    res.json({
      success: true,
      session_id: req.params.sessionId,
      buffer_data: 'Welcome to the E2E test terminal!\\r\\n$ '
    });
  } else {
    res.status(404).json({ error: 'Session not found' });
  }
});

app.delete('/api/v1/sessions/:sessionId', (req, res) => {
  mockData.sessions.delete(req.params.sessionId);
  res.json({ success: true });
});

// VM management endpoints
app.get('/api/v1/vms/:vmId', (req, res) => {
  const vm = mockData.vms.get(req.params.vmId);
  if (vm) {
    res.json(vm);
  } else {
    res.status(404).json({ error: 'VM not found' });
  }
});

// Health check
app.get('/api/v1/health', (req, res) => {
  res.json({
    status: 'healthy',
    timestamp: new Date().toISOString(),
    services: {
      database: 'healthy',
      redis: 'healthy',
      websocket: 'healthy'
    }
  });
});

// Error simulation endpoints
app.get('/api/v1/error/500', (req, res) => {
  res.status(500).json({ error: 'Internal server error' });
});

app.get('/api/v1/error/401', (req, res) => {
  res.status(401).json({ error: 'Unauthorized' });
});

// Start HTTP server
app.listen(PORT, () => {
  console.log(`Mock API server running on http://localhost:${PORT}`);
});

// WebSocket server for terminal communication
const wss = new WebSocket.Server({ port: WS_PORT });

console.log(`Mock WebSocket server running on ws://localhost:${WS_PORT}`);

wss.on('connection', (ws, req) => {
  console.log('WebSocket connection established');
  
  // Send welcome message
  setTimeout(() => {
    ws.send(JSON.stringify({
      type: 'connection_established',
      message: 'Connected to mock terminal server'
    }));
  }, 100);
  
  ws.on('message', (data) => {
    try {
      const message = JSON.parse(data);
      console.log('Received WebSocket message:', message);
      
      switch (message.type) {
        case 'session_create':
          ws.send(JSON.stringify({
            type: 'session_created',
            data: {
              session_id: 'e2e-test-session-789',
              vm_id: message.vm_id
            }
          }));
          break;
          
        case 'session_join':
          ws.send(JSON.stringify({
            type: 'session_joined',
            session_id: message.session_id,
            buffer_data: 'Welcome to the E2E test terminal!\\r\\n$ '
          }));
          break;
          
        case 'terminal_data':
          // Echo the input back
          ws.send(JSON.stringify({
            type: 'terminal_data',
            session_id: message.session_id,
            data: message.data
          }));
          
          // Simulate command responses
          if (message.data.includes('ls')) {
            setTimeout(() => {
              ws.send(JSON.stringify({
                type: 'terminal_data',
                session_id: message.session_id,
                data: '\\r\\nfile1.txt  file2.txt  directory1/\\r\\n$ '
              }));
            }, 100);
          } else if (message.data.includes('pwd')) {
            setTimeout(() => {
              ws.send(JSON.stringify({
                type: 'terminal_data',
                session_id: message.session_id,
                data: '\\r\\n/home/user\\r\\n$ '
              }));
            }, 100);
          }
          break;
          
        case 'terminal_resize':
          ws.send(JSON.stringify({
            type: 'terminal_resized',
            session_id: message.session_id,
            cols: message.data.cols,
            rows: message.data.rows
          }));
          break;
          
        case 'heartbeat':
          ws.send(JSON.stringify({
            type: 'heartbeat_response',
            timestamp: Date.now()
          }));
          break;
          
        default:
          console.log('Unknown message type:', message.type);
      }
    } catch (error) {
      console.error('Error processing WebSocket message:', error);
    }
  });
  
  ws.on('close', () => {
    console.log('WebSocket connection closed');
  });
  
  ws.on('error', (error) => {
    console.error('WebSocket error:', error);
  });
});

// Graceful shutdown
process.on('SIGINT', () => {
  console.log('\\nShutting down mock servers...');
  wss.close();
  process.exit(0);
});