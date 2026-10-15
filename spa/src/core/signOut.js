// Leave the document for the server's CSRF-protected logout confirmation.
// Electron routes this same destination through its native Account sign-out.
export const signOutHtml = () => '<a class="btn" href="/auth/logout">Sign out</a>';
