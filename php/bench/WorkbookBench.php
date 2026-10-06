<?php

declare(strict_types=1);

namespace HyperTabular\Bench;

use HyperCast\Success;
use HyperTabular\Column;
use HyperTabular\SheetOptions;
use HyperTabular\Workbook;
use PhpBench\Attributes as Bench;

/**
 * The workbook reader over the two 300 000-row files of corpus/README.md — Excel's
 * excel-win-300k.xlsx and LibreOffice's libreoffice-300k.ods, 8 columns each — read whole
 * through the plan rust/benches/workbook_benchmarks.rs reads them through, so that this
 * binding's number sits beside the core's. `open` is the package opened and nothing read.
 * `values` is opened, then the first sheet read in batches of 4096, every column's values
 * made and its faults counted, and the text column's lengths summed: the checksum every
 * binding's benchmark arrives at, which says they all did the same work. `verdicts` counts
 * the cells that cast from every column's verdicts instead: a HyperCast Success or Fault a
 * cell, what a caller matching every cell pays.
 *
 * The files stay out of the tree (corpus/generate/out/, sha256 in the README's table);
 * HYPERTABULAR_BENCH_DIR names another directory holding them. A file that is not there is
 * skipped. Run: vendor/bin/phpbench run --report=aggregate
 */
#[Bench\BeforeMethods('setUp')]
#[Bench\Iterations(5)]
#[Bench\Revs(1)]
#[Bench\Warmup(1)]
#[Bench\OutputTimeUnit('milliseconds', precision: 1)]
#[Bench\ParamProviders('files')]
final class WorkbookBench
{
    private string $container = '';

    /** @var list<Column> */
    private array $plan = [];

    /**
     * The two files.
     *
     * @return array<string, array{file: string}>
     */
    public function files(): array
    {
        return [
            'xlsx' => ['file' => 'excel-win-300k.xlsx'],
            'ods' => ['file' => 'libreoffice-300k.ods'],
        ];
    }

    /**
     * @param array{file: string} $params
     */
    public function setUp(array $params): void
    {
        $directory = getenv('HYPERTABULAR_BENCH_DIR') ?: __DIR__ . '/../../corpus/generate/out';
        $path = $directory . '/' . $params['file'];
        if (!is_file($path)) {
            throw new \RuntimeException("{$params['file']}: not in {$directory}");
        }
        $this->container = (string) file_get_contents($path);
        $this->plan = [
            Column::i64(0),
            Column::f64(1),
            Column::text(2),
            Column::date(3),
            Column::time(4),
            Column::bool(5),
            Column::duration(6),
            Column::i64(7),
        ];
    }

    public function benchOpen(): void
    {
        \count(Workbook::fromString($this->container)->sheets());
    }

    public function benchValues(): void
    {
        $this->read(false);
    }

    public function benchVerdicts(): void
    {
        $this->read(true);
    }

    /**
     * Reads the first sheet whole, counting what cast from its values and faults or from its
     * verdicts.
     */
    private function read(bool $verdicts): int
    {
        $sheet = Workbook::fromString($this->container)->sheet(0, new SheetOptions(), $this->plan);
        $checksum = 0;
        while (($batch = $sheet->read()) !== null) {
            foreach (array_keys($this->plan) as $column) {
                if ($verdicts) {
                    foreach ($batch->verdicts($column) as $verdict) {
                        if ($verdict instanceof Success) {
                            $checksum++;
                        }
                    }
                } else {
                    $batch->values($column);
                    $checksum += $batch->rows() - \count($batch->faults($column));
                }
            }
            foreach ($batch->values(2) as $text) {
                if ($text !== null) {
                    $checksum += \strlen($text);
                }
            }
        }
        return $checksum;
    }
}
