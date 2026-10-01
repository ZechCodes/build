# Device pairing security checklist

**Status:** verified (9/9 controls)

**Scope:** what `build-bridge pair` prints and the approve link it prints,
#319. The bridge registers as pending with only the hash of a fresh pairing
code and a signature binding its keys to it, and a signed-in person approves
the code in Build (`bridge/src/pairing.rs`, `skriftapp/buildapp/devices_controller.py`).
That flow is unchanged here and not rescored. This checklist covers the link
`<web>/app/#/pair/<code>`, the screen it opens, and the terminal output around it.

## Threat model

The pairing code is the secret that pairs a device: whoever approves it binds
the device to their account. A link carrying the code is exactly as sensitive
as the code on screen, and no more. It must leak no further than the screen
does: no server log, no Referer, nothing that outlives the page. Its approve
screen must still make a person compare and press.

Anyone can make such a link. An attacker runs `build-bridge pair` on their own
machine and sends a signed-in victim `/app/#/pair/<their code>`. The victim
sees a device name the attacker chose and a fingerprint they have nothing to
compare with. Approving it gives the attacker's machine access to the
victim's account. So a sheet a link opened says it came from a link, and says
plainly what approving does and when to do it (control 9).

## Controls

| # | Control | Status | Required evidence |
| --- | --- | --- | --- |
| 1 | The code travels only in the URL fragment, which browsers never send in a request or a Referer: never in the path or query string. | [x] | `the_approve_link_opens_the_pairing_screen_with_the_code_in_the_fragment` (Rust); `pairLink.test.js` reads the code only from `#/pair/<code>`. |
| 2 | The SPA takes the code out of the address before the router or anything else reads the hash, replacing the history entry with `#/account/devices`, both on load and when the link is followed in an open tab. | [x] | `pairLink.test.js`: "takes the code the page opened on out of the address", "hands on a link followed while the app is already open, before the router reads it"; `main.js` installs `watchPairLinks` before `initRouter`. |
| 3 | The link never approves. It fills the code in and looks it up, and approving is still the person's press after comparing. | [x] | `addDeviceDom.test.js` "a code handed in by the approve link is filled in and looked up, never approved"; `gateOnboardingDom.test.js` "opens Add a device with the link's code". |
| 4 | Only a well-formed code is taken from the fragment: letters, digits and `-`, at most 32 characters. It is set as an input's value, never as markup. | [x] | `pairLink.test.js` "reads nothing from any other fragment, or from a code that could carry markup" (markup, encoded spaces, nested paths, 40 characters). |
| 5 | The lookup and the approval send the code in a POST body, never a URL. Both need a signed-in session, and the bridge never sends the raw code, only its hash. | [x] | `api.js` `lookupDevice`/`approveDevice` post JSON bodies; `/api/devices/lookup` and `/approve` carry `build_auth_guard`; `build_register_request_never_includes_raw_code`. |
| 6 | The person compares the same fingerprint the bridge printed. The approve screen shows the form the terminal shows (the first 32 hex digits of SHA-256 of the identity key, 128 bits, in fours), with the full fingerprint beneath it. | [x] | `the_short_fingerprint_is_its_first_thirty_two_hex_digits_in_fours`, `the_pairing_prompt_is_one_link_a_code_and_a_short_fingerprint` (Rust); `addDeviceDom.test.js` "shows the fingerprint in the short form the bridge printed, with the full one beneath". |
| 7 | Sign-in keeps the fragment without becoming a redirect. The fragment goes back only onto a same-origin redirect that has none of its own, and the result is that parsed URL's absolute href, so it can choose which page of this site opens and nothing else. | [x] | `passkey-signin.test.mjs`: "a signed-in visitor goes on with the fragment they arrived with", "a redirect with its own fragment, or to another origin, is left as the server said" (absolute and protocol-relative), "a same-origin path that starts with // stays on this site", "the page goes on with the fragment once the passkey is accepted". |
| 8 | The terminal output names no secret beyond what it did before. The retire notice names the directory the old identity was moved to, `~`-shortened under the home directory and in full outside it, and, for an api other than the default, the api. It prints no identity contents and no file names. | [x] | `a_retired_approval_says_so_in_two_short_lines`, `a_retired_approval_outside_home_names_the_full_directory`, `a_retired_approval_names_an_api_that_is_not_the_default`, `the_retire_message_names_the_api_and_where_the_old_identity_is`. |
| 9 | A sheet a link opened says so and warns before anything can be approved: only approve if you just ran the installer or `build-bridge pair` on a machine you own, because approving gives that machine access to your account. The typed-code subtitle is not shown for a link. | [x] | `addDeviceDom.test.js` "a sheet a link opened says so and warns before anything can be approved" (pins the copy), "a sheet opened by hand keeps its own subtitle and carries no link warning"; `gateOnboardingDom.test.js` "opens Add a device with the link's code, marked as opened by a link". |

## Accepted

- The fingerprint a person compares is truncated to 128 bits (32 hex digits of
  the 256-bit SHA-256). Matching a substituted key's prefix while a code is
  pending would take a 2^128 second-preimage search. The full fingerprint is
  on the approve screen beneath it.
- The link warning is the defence against an attacker-crafted link. A person
  who ignores it and approves an attacker's code gives that machine access, as
  they would by typing the attacker's code.

- A signed-out visitor's sign-in page keeps `/auth/login?next=/app/#/pair/<code>`
  as its own history entry. That entry is on the same machine and in the same
  browser that showed the code, and the code stops pairing anything once it is
  approved.
