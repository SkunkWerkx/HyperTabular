using BenchmarkDotNet.Running;
using HyperTabular.Benchmarks;

BenchmarkSwitcher.FromAssembly(typeof(WorkbookBenchmarks).Assembly).Run(args);
