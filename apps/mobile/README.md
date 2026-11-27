# WOL UI Mobile

The Expo app provides a small mobile companion for signing in, browsing connected
sites and devices, and sending Wake-on-LAN requests.

## Start

From the repository root, run:

    bun run --filter @wolui/mobile start

Set the default server URL before starting Metro if you want to prefill it in
the sign-in form:

    EXPO_PUBLIC_API_URL=https://wolui.example.com bun run --filter @wolui/mobile start

The server URL can also be entered in the app. Public deployments must use
HTTPS. HTTP is accepted for local development only.

## Sign in

Use the server's configured access token once. The app exchanges it for a
session token, stores the session and CSRF tokens in the platform secure
keychain/keystore, and sends the session token as a bearer credential for API
requests. The original access token is cleared after sign-in and is not saved.

The server login response must include session_token and csrf_token, and
the API must accept bearer-authenticated sessions. See the server API contract
for the exact behavior.

## Scope

This first mobile slice supports read-only site/device browsing, Wake-on-LAN,
offline-site status, and an external handoff to the Tailscale SSH Console.
Add/edit device flows and site pairing remain in the web interface. SSH itself
runs in Tailscale's console and is governed by the tailnet policy.
