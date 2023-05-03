import { $ } from "https://deno.land/x/dax@0.31.1/mod.ts";
import { chart } from "https://deno.land/x/fresh_charts/core.ts";

function wait(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function load_test(conn, port, out) {
  return $`target/release/examples/load_test ${conn} ${port} 1 ${out}`;
}

const targets = [
  // https://github.com/littledivy/fastwebsockets
  { conn: 100, port: 8080, name: "fastwebsockets", server: "target/release/examples/echo_server" },
  // https://github.com/uNetworking/uWebSockets
  { conn: 100, port: 9001, name: "uWebSockets", server: "../uWebSockets/EchoServer" },
  // https://github.com/snapview/tokio-tungstenite
  { conn: 100, port: 8080, name: "tokio-tungstenite", server: "../tokio-tungstenite/target/release/examples/echo-server" },
  // https://github.com/websockets-rs/rust-websocket
  { conn: 100, port: 9002, name: "rust-websocket", server: "../rust-websocket/target/release/examples/async-autobahn-server" },
];

let results = {};
for (const { conn, port, name, server, arg } of targets) {
  try {
    const proc = $`${server} ${arg || ""}`.spawn();
    console.log(`Waiting for ${name} to start...`);
    await wait(1000);
    await load_test(conn, port, `./${name}.txt`);
    proc.abort();
    await proc;
  } catch (e) {
    console.log(e);
  }

  const result = Deno.readTextFileSync(`./${name}.txt`);
  const lines = result.split("\n").filter((line) => line.length > 0);
  const mps = lines.map((line) => parseInt(line.trim(), 10));
  const avg = mps.reduce((a, b) => a + b, 0) / mps.length;
  markdowntable += `| ${name} | ${conn} | ${avg} |\n`;
  results[name] = avg;
}

results = Object.fromEntries(Object.entries(results).sort(([, a], [, b]) => b - a));

const svg = chart({
  type: "bar",
  data: {
    labels: Object.keys(results),
    datasets: [
      {
        label: "Messages per second",
        data: Object.values(results),
        backgroundColor: [
          "rgba(54, 162, 235, 255)",
        ],
      }
    ]
  },
});

Deno.writeTextFileSync("./benches/chart.svg", svg);
