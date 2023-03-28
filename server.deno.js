import { serve } from "https://deno.land/std/http/server.ts";

serve(r => {
    const { socket, response } = Deno.upgradeWebSocket(r, { idleTimeout: 0 });
    socket.onmessage = (e) => {   
        socket.send(e.data);
    }

    socket.onopen = () => {
        console.log('Connected to client');
    }

    socket.onerror = (e) => {
        console.log(e);
    }

    return response;
}, { port: 8080 });
