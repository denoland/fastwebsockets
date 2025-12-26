import { $ } from "https://deno.land/x/dax/mod.ts";

const pwd = new URL(".", import.meta.url).pathname;

const AUTOBAHN_TESTSUITE_DOCKER =
  "crossbario/autobahn-testsuite:latest@sha256:519915fb568b04c9383f70a1c405ae3ff44ab9e35835b085239c258b6fac3074";

const server = $`target/release/examples/echo_server`.spawn();
await $`docker run --name fuzzingserver	-v ${pwd}/fuzzingclient.json:/fuzzingclient.json:ro	-v ${pwd}/reports:/reports -p 9001:9001	--net=host --rm	${AUTOBAHN_TESTSUITE_DOCKER} wstest -m fuzzingclient -s fuzzingclient.json`.cwd(pwd);

const { fastwebsockets } = JSON.parse(
  Deno.readTextFileSync("./autobahn/reports/servers/index.json"),
);
const result = Object.values(fastwebsockets);

function failed(name) {
  return name != "OK" && name != "INFORMATIONAL" && name != "NON-STRICT";
}

const failedtests = result.filter((outcome) => failed(outcome.behavior));

console.log(
  `%c${result.length - failedtests.length} / ${result.length} tests OK`,
  `color: ${failedtests.length == 0 ? "green" : "red"}`,
);

Deno.exit(failedtests.length == 0 ? 0 : 1);
