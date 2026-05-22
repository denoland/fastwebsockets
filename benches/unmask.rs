use criterion::*;

fn benchmark(c: &mut Criterion) {
  let mut group = c.benchmark_group("unmask");
  for &size in &[64usize, 1024, 16 * 1024, 64 << 20] {
    let mut data: Vec<u8> = (0..size).map(|_| rand::random()).collect();
    group.throughput(Throughput::Bytes(size as u64));
    group.bench_function(format!("len={}", size), |b| {
      b.iter(|| {
        fastwebsockets::unmask(black_box(&mut data), [1, 2, 3, 4]);
      });
    });
  }
  group.finish();
}

criterion_group!(benches, benchmark);
criterion_main!(benches);
