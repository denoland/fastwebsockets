import { $ } from "https://deno.land/x/dax@0.31.1/mod.ts";
import { chart } from "https://deno.land/x/fresh_charts/core.ts";
import { TextLineStream } from "https://deno.land/std/streams/text_line_stream.ts";

function wait(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function load_test(conn, port) {
  return $`../uWebSockets/benchmarks/load_test ${conn} 0.0.0.0 ${port} 0 0`
    .stdout("piped").spawn();
}

const targets = [
  // https://github.com/littledivy/fastwebsockets
  {
    conn: 100,
    port: 8080,
    name: "fastwebsockets",
    server: "target/release/examples/echo_server",
  },
  // // https://github.com/uNetworking/uWebSockets
  // { conn: 100, port: 9001, name: "uWebSockets", server: "../uWebSockets/EchoServer" },
  // https://github.com/snapview/tokio-tungstenite
  {
    conn: 100,
    port: 8080,
    name: "tokio-tungstenite",
    server: "../tokio-tungstenite/target/release/examples/echo-server",
  },
  // https://github.com/websockets-rs/rust-websocket
  {
    conn: 100,
    port: 9002,
    name: "rust-websocket",
    server: "../rust-websocket/target/release/examples/async-autobahn-server",
  },
];

let results = {};
for (const { conn, port, name, server, arg } of targets) {
  let logs = [];
  try {
    const proc = $`${server} ${arg || ""}`.spawn();
    console.log(`Waiting for ${name} to start...`);
    await wait(1000);

    const client = load_test(conn, port);
    const readable = client.stdout().pipeThrough(new TextDecoderStream())
      .pipeThrough(new TextLineStream());
    let count = 0;
    for await (const data of readable) {
      logs.push(data);
      count++;
      if (count === 2) {
        break;
      }
    }
    client.abort();
    proc.abort();
    await proc;
    await client;
  } catch (e) {
    console.log(e);
  }

  const lines = logs.filter((line) =>
    line.length > 0 && line.startsWith("Msg/sec")
  );
  const mps = lines.map((line) => parseInt(line.split(" ")[1].trim()), 10);
  const avg = mps.reduce((a, b) => a + b, 0) / mps.length;
  results[name] = avg;
}

results = Object.fromEntries(
  Object.entries(results).sort(([, a], [, b]) => b - a),
);

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
      },
    ],
  },
});

Deno.writeTextFileSync("./benches/chart.svg", svg);
