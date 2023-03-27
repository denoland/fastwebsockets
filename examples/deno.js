const { core } = Deno;
const { ops } = core;

function onmessage(data) {
   return data;
}

core.opAsync("op_serve", onmessage);