export const WAITLIST_ENDPOINT = "/api/waitlist";

export async function submitWaitlistEmail(email) {
  const response = await fetch(WAITLIST_ENDPOINT, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ email }),
  });
  return response.ok;
}
