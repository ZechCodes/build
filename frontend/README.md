# Frontend Application

## Overview
React-based frontend application providing the user interface for the Build platform. Features a modern, responsive design with real-time terminal access, code editing capabilities, and comprehensive platform management.

## Structure
```
frontend/
├── public/                 # Static assets
├── src/
│   ├── components/         # Reusable UI components
│   ├── pages/              # Route-level components
│   ├── hooks/              # Custom React hooks
│   ├── store/              # State management (Zustand)
│   ├── services/           # API client services
│   ├── utils/              # Utility functions
│   ├── types/              # TypeScript type definitions
│   ├── styles/             # Global styles and themes
│   └── App.tsx             # Main application component
├── package.json            # Node.js dependencies
└── vite.config.ts          # Vite configuration
```

## Key Features
- Browser-based terminal with xterm.js
- Real-time WebSocket connections
- Monaco code editor integration
- VM management interface
- Session recovery and management
- Snapshot creation and restoration
- Git repository management
- Recording playback and export

## Technology Stack
- React 18+ with TypeScript
- Vite for build tooling
- Tailwind CSS for styling
- Radix UI for components
- TanStack Query for server state
- Zustand for client state
- xterm.js for terminal emulation
- Monaco Editor for code editing

## Development
Supports hot module replacement and includes comprehensive testing setup with Jest and React Testing Library.

## Security
Implements secure authentication flows, proper token management, and XSS protection measures.