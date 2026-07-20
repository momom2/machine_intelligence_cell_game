// The replay-collection Worker (owner infra, 2026-07-19): playtesters' builds POST
// replay files here; objects land in the `mi-replays` R2 bucket for the owner to pull.
//
//   POST /upload
//     X-Upload-Key: <shared key — a `secret_text` binding, baked into the game client;
//                    not true secrecy in a shipped binary, but it filters drive-by junk>
//     body: a `.mir` / `.mirx` text payload (must start with "mir 1" or "mir 2" —
//            v1 = the layer2-branch builds still in playtesters' hands, v2 = pure-L1)
//   → 200 {"ok":true,"key":"L07/1784295000000_ab12cd34.mirx"}
//
// Guards: shared key, 25 MB cap, payload sniff. Object keys are per-level with a server
// timestamp + random suffix, so pulls arrive pre-organized and names never collide.
// CORS is wide open for POST (the itch.io canvas calls cross-origin; the key is the gate).
//
// Deploy (see docs/CHANGELOG.md, replay-collection entry): uploaded via the Cloudflare
// API with bindings REPLAYS (r2_bucket → mi-replays) and UPLOAD_KEY (secret_text).

const CORS = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Methods": "POST, OPTIONS",
  "Access-Control-Allow-Headers": "content-type, x-upload-key",
};

export default {
  async fetch(request, env) {
    if (request.method === "OPTIONS") {
      return new Response(null, { status: 204, headers: CORS });
    }
    const url = new URL(request.url);
    if (request.method !== "POST" || url.pathname !== "/upload") {
      return new Response("not found", { status: 404, headers: CORS });
    }
    if (request.headers.get("x-upload-key") !== env.UPLOAD_KEY) {
      return new Response("forbidden", { status: 403, headers: CORS });
    }
    const MAX = 25 * 1024 * 1024;
    const text = await request.text();
    if (text.length === 0 || text.length > MAX || !/^mir [12]\b/.test(text)) {
      return new Response("bad payload", { status: 400, headers: CORS });
    }
    const lvl = (text.match(/^level (\d+)$/m) || [])[1];
    const dir = `L${(lvl || "0").padStart(2, "0")}`;
    const ext = /^f \d/m.test(text) ? "mirx" : "mir";
    const key = `${dir}/${Date.now()}_${crypto.randomUUID().slice(0, 8)}.${ext}`;
    await env.REPLAYS.put(key, text);
    return new Response(JSON.stringify({ ok: true, key }), {
      status: 200,
      headers: { "content-type": "application/json", ...CORS },
    });
  },
};
