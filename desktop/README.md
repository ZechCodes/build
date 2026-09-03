# Build Desktop

Build Desktop is a sandboxed Electron shell for the hosted Build web client.
It connects to the user's separately installed bridge through Build's existing
end-to-end encrypted relay. The desktop app does not install, launch, or access
the bridge or the user's files.

## Develop

```bash
npm install
npm test
npm start
```

`npm start` opens `https://getbuild.ing/app/`. Development builds expose
Chromium developer tools; packaged builds do not.

## Package

Build an unsigned installer for the current platform:

```bash
npm run dist
```

Use `dist:mac`, `dist:win`, or `dist:linux` on the corresponding signing host.
Production releases must be signed and, on macOS, notarized before distribution.
