# Self-hosting goodvoice

> Stub — written for real in plan.md task 6.1.

The short version: free-tier Cloudflare account → create a Realtime (Calls) app →
`wrangler secret put CALLS_APP_ID` + `wrangler secret put CALLS_APP_SECRET` →
`wrangler deploy` from `server/` → paste your Worker URL into the client settings.
