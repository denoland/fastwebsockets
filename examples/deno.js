const { core } = Deno;
const { ops } = core;

// data is copied into JS.
function onmessage(data) {
  return data;
}

core.opAsync("op_serve", onmessage);
