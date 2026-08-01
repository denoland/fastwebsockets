### Plaintext, echo WebSocket server benchmark

![](./200-16384-chart.svg)

![](./500-16384-chart.svg)

![](./100-20-chart.svg)

Y-axis is number of messages sent per sec. (size per message = payload size)

```
Darwin Divys-MacBook-Pro-2.local 24.6.0 Darwin Kernel Version 24.6.0: Mon Jul 14 11:30:40 PDT 2025; root:xnu-11417.140.69~1/RELEASE_ARM64_T6041 arm64 arm Darwin

64GiB System memory
Apple M4 Max

fastwebsockets 0.11.0
rust-websocket 0.27.0
uWebSockets (main 65f0af8)
tokio-tungstenite 0.28.0
```
