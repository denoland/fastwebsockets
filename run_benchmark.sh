#!/bin/bash

echo "=== fastwebsockets echo server benchmark: tokio vs io_uring ==="
echo ""

# Check if io_uring is available
if [ ! -e "/proc/sys/kernel/osrelease" ] || ! grep -q "5\.1[1-9]\|5\.[2-9]\|[6-9]\." /proc/version; then
    echo "⚠️  io_uring may not be fully supported on this kernel"
    echo "   Recommended: Linux 5.11+"
    echo ""
fi

echo "📊 Running tokio baseline benchmark..."
echo "   (This may take a few minutes)"
echo ""

# Run tokio benchmark
cargo bench --bench echo_server_benchmark > tokio_results.txt 2>&1
if [ $? -eq 0 ]; then
    echo "✅ Tokio benchmark completed"
    echo ""
    echo "Top tokio results:"
    grep -E "(echo_server_small|echo_server_large|echo_server_concurrency)" tokio_results.txt | head -10
else
    echo "❌ Tokio benchmark failed"
    cat tokio_results.txt
fi

echo ""
echo "📊 Running io_uring benchmark..."
echo "   (This may take a few minutes)"
echo ""

# Run io_uring benchmark
cargo bench --bench echo_server_benchmark --features io-uring > uring_results.txt 2>&1
if [ $? -eq 0 ]; then
    echo "✅ io_uring benchmark completed"
    echo ""
    echo "Top io_uring results:"
    grep -E "(echo_server_small|echo_server_large|echo_server_concurrency)" uring_results.txt | head -10
else
    echo "❌ io_uring benchmark failed"
    echo ""
    echo "This might be due to:"
    echo "  - Kernel version < 5.11"
    echo "  - Missing io_uring support"
    echo "  - AsyncRead/AsyncWrite adapter limitations"
    echo ""
    echo "Error output:"
    cat uring_results.txt
fi

echo ""
echo "=== Benchmark Summary ==="
echo ""
echo "📁 Detailed results saved to:"
echo "   tokio_results.txt"
echo "   uring_results.txt"
echo ""
echo "🔍 To analyze results in detail:"
echo "   cargo bench --bench echo_server_benchmark"
echo "   cargo bench --bench echo_server_benchmark --features io-uring"
echo ""
echo "📈 For HTML reports:"
echo "   cargo install criterion"
echo "   Open target/criterion/*/report/index.html"