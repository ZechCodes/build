# Build Desktop

Build Desktop is a sandboxed Electron shell for the hosted Build web client.
It connects to the user's separately installed bridge through Build's existing
end-to-end encrypted relay. The desktop app does not install, launch, or access
the bridge or the user's files.

Sign-in opens the existing passkey flow in the system browser through Skrift's
OAuth 2.0 Authorization Code flow with PKCE. Access and refresh tokens remain in
the Electron main process and are never exposed to the hosted renderer.

## Develop

```bash
npm install
npm test
npm start
```

`npm start` opens `https://getbuild.ing/app/`. Development builds expose
Chromium developer tools; packaged builds do not.

## Package

Build an installer for the current platform:

```bash
npm run dist
```

Use `dist:mac`, `dist:win`, or `dist:linux` on the corresponding signing host.
When no macOS signing identity is available, local builds receive an ad-hoc
signature so macOS can run the hardened binary. A configured `CSC_LINK`,
`CSC_NAME`, or keychain identity takes precedence for production signing.
Production macOS releases must also be notarized before distribution.
