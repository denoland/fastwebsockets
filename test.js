const ws = new WebSocket('ws://localhost:8080');

const COUNT = 100_000;
let i = 0;

ws.onmessage = () => {
    i++;
    if (i < COUNT) ws.send('Next');
    else Deno.exit(0);
};

ws.onerror = (error) => {
    console.log(error);
};

ws.onopen = () => {
   console.log('Connected to server');
   ws.send('Start');
};
