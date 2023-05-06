import { $ } from "https://deno.land/x/dax/mod.ts";
import { chart } from "https://deno.land/x/fresh_charts/core.ts";
import { parse } from "https://deno.land/std@0.184.0/flags/mod.ts";

const { _: pids } = parse(Deno.args);

const freq = 200; // ms
const records = {};

// ps -p <PID> -o %cpu,%mem,rss,vsz
async function tick(pids) {
  for (const pid of pids) {
    const stdout = await $`ps -p ${parseInt(pid)} -o %cpu,%mem,rss,vsz`.text();
    const [cpu, mem, rss, vsz] = stdout.split("\n")[1].split(" ").filter(
      Boolean,
    );
    records[pid] ??= [];
    records[pid].push({ cpu, mem, rss, vsz });
  }
}

const interval = setInterval(() => tick(pids), freq);

Deno.addSignalListener("SIGINT", () => {
  clearInterval(interval);
  plot();
  Deno.exit();
});

function plot() {
  console.log(records);
  const labels = Object.values(records)[0].map((_, i) => `${i * freq / 1000}s`);
  // Line chart
  const svg = chart({
    type: "line",
    data: {
      labels,
      datasets: Object.keys(records).map((pid) => ({
        label: `PID ${pid}`,
        data: records[pid].map((r) => r.rss),
        borderColor: "rgb(75, 192, 192)",
        tension: 0.1,
        fill: true,
      })),
    },
  });

  Deno.writeTextFileSync(`./memcpu-chart.svg`, svg);
}
