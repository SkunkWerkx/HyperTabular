<?php

declare(strict_types=1);

namespace HyperTabular;

/**
 * How the text is delimited, declared by the caller. Nothing is sniffed: the separator is
 * stated, quoting is stated, the header is stated — the same stance HyperCast's NumFormat
 * takes for numeric notation.
 */
final readonly class Dialect
{
    /**
     * Declares a dialect. Named arguments read well here:
     * `new Dialect(';', hasHeader: false)`.
     *
     * @param string $separator the single-byte separator: tab, or any printable ASCII
     *     character except `"`
     * @param bool $quoting whether `"` quotes cells (RFC 4180, `""` for a literal quote);
     *     off, a quote is an ordinary byte
     * @param bool $hasHeader whether the first record is a header, exposed through
     *     {@see DelimitedReader::header()} and never delivered as a row
     * @param bool $skipBlankLines whether a completely empty line is skipped rather than
     *     read as a one-cell row
     * @throws \InvalidArgumentException when the separator is not exactly one byte
     */
    public function __construct(
        public string $separator,
        public bool $quoting = true,
        public bool $hasHeader = true,
        public bool $skipBlankLines = true,
    ) {
        // One byte is all this layer can check without repeating the core's rule; which
        // bytes the scanner honours is the core's to say, when a reader is built.
        if (\strlen($separator) !== 1) {
            throw new \InvalidArgumentException(
                'The separator must be a single ASCII byte; got ' . \strlen($separator) . ' bytes'
            );
        }
    }

    /**
     * Comma-separated, quoted, with a header, blank lines skipped.
     *
     * @return self the shared CSV dialect
     */
    public static function csv(): self
    {
        static $csv = null;
        return $csv ??= new self(',');
    }

    /**
     * Tab-separated, otherwise as {@see csv()}.
     *
     * @return self the shared TSV dialect
     */
    public static function tsv(): self
    {
        static $tsv = null;
        return $tsv ??= new self("\t");
    }

    /**
     * Pipe-separated, otherwise as {@see csv()}.
     *
     * @return self the shared pipe-separated dialect
     */
    public static function psv(): self
    {
        static $psv = null;
        return $psv ??= new self('|');
    }
}
