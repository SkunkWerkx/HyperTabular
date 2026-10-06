<?php

declare(strict_types=1);

namespace HyperTabular;

use FFI;
use FFI\CData;

/**
 * The buffers a workbook call works in, which it may ask to have grown: one set for a
 * workbook while it opens, one for each sheet. A call that finds one too small says which
 * and how large, having undone nothing — so the buffer is grown with what it held kept, and
 * the same call is made again.
 *
 * @internal FFI plumbing, not part of the public API — PHP has no package-private visibility.
 */
final class Scratch
{
    /**
     * For the tests: every buffer a workbook call works in starts with room for one element
     * and no shared-strings bound is asked for, so every call that can stop and resume does —
     * the grow-and-keep path exercised mid-part.
     */
    public static bool $stingy = false;

    /** @var array{int, int, int} for the tests: how many times the window, arena and cell table grew */
    public static array $grown = [0, 0, 0];

    public CData $window;
    public int $windowCap;
    public CData $arena;
    public int $arenaCap;
    public CData $cells;
    public int $cellsCap;
    private CData $row;
    private int $rowCap;
    private CData $buffers;
    public readonly CData $filled;

    /**
     * Allocates the four buffers.
     *
     * @param int $window bytes of window
     * @param int $arena bytes of arena
     * @param int $cells spans of cell table
     * @param int $row slots of row
     */
    public function __construct(int $window, int $arena, int $cells, int $row)
    {
        $ffi = Native::ffi();
        // FFI cannot allocate nothing; a zero capacity is said as zero all the same.
        $this->windowCap = $window;
        $this->window = $ffi->new('uint8_t[' . max(1, $window) . ']');
        $this->arenaCap = $arena;
        $this->arena = $ffi->new('uint8_t[' . max(1, $arena) . ']');
        $this->cellsCap = $cells;
        $this->cells = $ffi->new('ht_span[' . max(1, $cells) . ']');
        $this->rowCap = $row;
        $this->row = $ffi->new('ht_slot[' . max(1, $row) . ']');
        $this->buffers = $ffi->new('ht_buffers');
        $this->filled = $ffi->new('ht_filled');
    }

    /**
     * This scratch as the core takes it, with the workbook's tables when a sheet is being
     * read.
     *
     * @param Workbook|null $tables the workbook whose tables a sheet's calls are handed
     * @return CData a pointer to the `ht_buffers` block
     */
    public function buffers(?Workbook $tables = null): CData
    {
        $ffi = Native::ffi();
        $buffers = $this->buffers;
        $buffers->window = $ffi->cast('uint8_t *', FFI::addr($this->window));
        $buffers->window_cap = $this->windowCap;
        $buffers->arena = $ffi->cast('uint8_t *', FFI::addr($this->arena));
        $buffers->arena_cap = $this->arenaCap;
        $buffers->cells = $ffi->cast('ht_span *', FFI::addr($this->cells));
        $buffers->cells_cap = $this->cellsCap;
        $buffers->row = $ffi->cast('ht_slot *', FFI::addr($this->row));
        $buffers->row_cap = $this->rowCap;
        $tables?->tables($buffers);
        return FFI::addr($buffers);
    }

    /**
     * Makes the window at least `$needed` bytes, what it held kept.
     *
     * @param int $needed bytes
     * @return void
     */
    public function growWindow(int $needed): void
    {
        [$this->window, $this->windowCap] = self::grown($this->window, $this->windowCap, $needed, 'uint8_t', 1);
    }

    /**
     * Makes the arena at least `$needed` bytes, what it held kept.
     *
     * @param int $needed bytes
     * @return void
     */
    public function growArena(int $needed): void
    {
        [$this->arena, $this->arenaCap] = self::grown($this->arena, $this->arenaCap, $needed, 'uint8_t', 1);
    }

    /**
     * Makes the cell table at least `$needed` spans, what it held kept.
     *
     * @param int $needed spans
     * @return void
     */
    public function growCells(int $needed): void
    {
        [$this->cells, $this->cellsCap] = self::grown($this->cells, $this->cellsCap, $needed, 'ht_span', 8);
    }

    /**
     * Makes `$call` until it stops asking for room, growing the buffer it names each time.
     * Returns the code it ended on; what it reported is in {@see $filled}.
     *
     * @param callable(CData): int $call the call, given the buffers block
     * @param Workbook|null $tables the workbook whose tables the call is handed
     * @return int the code
     */
    public function drive(callable $call, ?Workbook $tables = null): int
    {
        while (true) {
            $code = $call($this->buffers($tables));
            $needed = $this->filled->needed;
            switch ($code) {
                case Native::ERR_WINDOW:
                    $this->growWindow($needed);
                    self::$grown[0]++;
                    break;
                case Native::ERR_ARENA:
                    $this->growArena($needed);
                    self::$grown[1]++;
                    break;
                case Native::ERR_CELLS:
                    $this->growCells($needed);
                    self::$grown[2]++;
                    break;
                default:
                    return $code;
            }
        }
    }

    /**
     * What a call's code means: done, or a structural failure. Anything else is this
     * binding's bug.
     *
     * @param int $code the call's code
     * @param CData $filled what it reported
     * @return void
     * @throws TabularException when the workbook is structurally broken
     */
    public static function settle(int $code, CData $filled): void
    {
        if ($code === Native::ERR_STRUCTURE) {
            throw TabularException::from($filled->failure);
        }
        if ($code !== Native::OK) {
            throw new \RuntimeException(
                'hypertabular: libhypertabular reported a contract violation — a binding bug, please report it'
            );
        }
    }

    /**
     * A larger array of `$type`, what the old one held kept.
     *
     * @param CData $old the array
     * @param int $capacity its elements
     * @param int $needed the elements it has to hold
     * @param string $type its element type
     * @param int $size bytes an element takes
     * @return array{CData, int} the array and its elements
     */
    private static function grown(CData $old, int $capacity, int $needed, string $type, int $size): array
    {
        $length = max($needed, $capacity + 1);
        $larger = Native::ffi()->new("{$type}[{$length}]");
        if ($capacity > 0) {
            FFI::memcpy($larger, $old, $capacity * $size);
        }
        return [$larger, $length];
    }
}
