import type { Env } from "./env";

export { Room } from "./room";

const handler: ExportedHandler<Env> = {
  fetch(request) {
    const url = new URL(request.url);

    if (request.method === "GET" && url.pathname === "/health") {
      return Response.json({ ok: true });
    }

    return new Response("not found", { status: 404 });
  },
};

export default handler;
