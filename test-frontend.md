# Frontend Terminal Testing Guide

## Local Development Server Status

✅ **Development server is running successfully!**

- Server URL: http://localhost:3002/
- Status: Ready and serving content
- Build system: Vite with TypeScript support

## Testing the Terminal Functionality

### 1. Basic Application Access
Visit: http://localhost:3002/

**Expected:** You should see the home page of the Build platform.

### 2. Dashboard with Terminal
Visit: http://localhost:3002/dashboard

**Expected:** 
- Dashboard page with VM statistics cards
- Recent VMs section on the left
- Terminal component on the right side titled "Terminal - Development VM 1"
- Terminal should show xterm.js interface with toolbar

### 3. Terminal Features to Test

#### Visual Features:
- [ ] Terminal component renders with dark theme by default
- [ ] Toolbar is visible with controls (theme, search, copy, paste, clear, fit)
- [ ] Terminal has proper responsive sizing (400px height)
- [ ] Hover states work on toolbar buttons

#### Interactive Features:
- [ ] Theme switcher in toolbar (should have 5 themes available)
- [ ] Search functionality (click search icon, enter text)
- [ ] Right-click context menu in terminal area
- [ ] Terminal cursor should be visible and blinking

#### Connection Features:
- [ ] WebSocket connection attempts (will fail - expected)
- [ ] Connection status indicators
- [ ] Error messages for failed connections (expected)

### 4. Performance Testing

#### Browser DevTools:
1. Open DevTools (F12)
2. Go to Console tab
3. Look for performance monitoring messages
4. Check for any JavaScript errors

#### Expected Console Output:
- Security measures initialization message
- Performance monitoring startup messages
- WebSocket connection attempt (will fail without backend)
- No critical JavaScript errors

### 5. Security Features

#### Clipboard Operations:
- Try copy/paste operations (may be limited without proper terminal data)
- Should see sanitization warnings in console if any

#### XSS Protection:
- All user inputs should be sanitized
- No script execution vulnerabilities

### 6. Known Limitations (Expected)

❌ **Backend Connection**: WebSocket connections will fail (no backend running)
❌ **Session Management**: Cannot create/restore sessions (API not available)
❌ **Terminal I/O**: No actual terminal input/output (no PTY backend)

✅ **What Should Work**:
- UI components render correctly
- Theme switching
- Search interface
- Context menus
- Performance monitoring
- Security features
- Responsive design

## Quick Verification Checklist

1. **Navigation**: Can you navigate to different pages?
2. **Terminal UI**: Does the terminal component render properly?
3. **Themes**: Can you switch between the 5 terminal themes?
4. **Search**: Does the search interface appear when clicked?
5. **Context Menu**: Right-click in terminal shows context menu?
6. **Console**: No critical errors in browser console?
7. **Responsive**: Does the layout work on different screen sizes?

## Next Steps for Full Testing

To test the complete functionality, you would need:

1. **Backend API**: Session 7's snapshot manager API running
2. **WebSocket Server**: Terminal WebSocket endpoint implementation
3. **VM Integration**: Actual VM instances to connect to
4. **Authentication**: JWT tokens for secure connections

## Troubleshooting

If you encounter issues:

1. **Dev Server Won't Start**: Run `npm install` then `npm run dev`
2. **TypeScript Errors**: Most should be resolved, check console
3. **Missing Components**: Ensure all dependencies are installed
4. **Performance Issues**: Performance monitoring is enabled by default

## Success Criteria

✅ **Frontend is ready for local testing if:**
- Development server starts without errors
- Dashboard page renders with terminal component
- No critical JavaScript console errors
- UI interactions work (clicking, hovering, navigation)
- Terminal renders with proper styling and themes

The frontend terminal interface is **functionally ready** for local testing of the UI components and interactions, pending backend integration for full terminal functionality.