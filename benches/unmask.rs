use criterion::*;

fn benchmark(c: &mut Criterion) {
  const STREAM_SIZE: usize = 64 << 20;

  let mut data: Vec<u8> = (0..STREAM_SIZE).map(|_| rand::random()).collect();
  let mut group = c.benchmark_group("unmask2");
  group.throughput(Throughput::Bytes(STREAM_SIZE as u64));
  group.bench_function("unmask 64 << 20", |b| {
    b.iter(|| {
      fastwebsockets::unmask(black_box(&mut data), [1, 2, 3, 4]);
    });
  });
  group.finish();
}

criterion_group!(benches, benchmark);
criterion_main!(benches);
