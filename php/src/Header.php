<?php

declare(strict_types=1);

namespace HyperTabular;

/**
 * A source's column names, as a header record or a sheet's header row wrote them — and the
 * lookup a plan built from names reads best with:
 *
 * ```php
 * $reader = DelimitedReader::fromString($csv, Dialect::csv());
 * $header = $reader->headerIndex();
 * $reader->bind([Column::text($header->ordinal('name')), Column::i32($header->ordinal('id'))]);
 * ```
 *
 * A name is matched exactly — byte for byte, case and spaces included — and a name the
 * header has twice is found at its first column. The header still reads as the list of
 * names it is: it counts, iterates and indexes like one, and {@see names()} is that list.
 */
final class Header implements \ArrayAccess, \Countable, \IteratorAggregate, \JsonSerializable
{
    /** @var array<string|int, int>|null each name's first column, built the first time a name is looked up */
    private ?array $first = null;

    /**
     * Made by a reader or a sheet from the names it read.
     *
     * @param list<string> $names the names, in column order
     * @internal
     */
    public function __construct(private readonly array $names)
    {
    }

    /**
     * The names, in the order the source wrote them: what the reader's `header()` returns.
     *
     * @return list<string> the names
     */
    public function names(): array
    {
        return $this->names;
    }

    /**
     * The first column with this name: what a plan's ordinal is looked up by.
     *
     * @param string $name the name, matched exactly
     * @return int the column's ordinal, from zero
     * @throws \OutOfBoundsException when the header has no column by that name — the
     *     message names it
     */
    public function ordinal(string $name): int
    {
        return $this->find($name) ?? throw new \OutOfBoundsException(
            "The header has no column named \"{$name}\"; it names " . \count($this->names) . ' columns'
        );
    }

    /**
     * {@see ordinal()}, with a missing name null rather than an exception.
     *
     * @param string $name the name, matched exactly
     * @return int|null the column's ordinal, from zero, or null when the header has none by that name
     */
    public function find(string $name): ?int
    {
        if ($this->first === null) {
            $first = [];
            foreach ($this->names as $ordinal => $each) {
                // A numeric name becomes an int key here and is looked up as one below, so
                // the match stays exact either way.
                $first[$each] ??= $ordinal;
            }
            $this->first = $first;
        }
        return $this->first[$name] ?? null;
    }

    /**
     * How many columns the header names: none for a source that had no record to read one from.
     *
     * @return int the count
     */
    public function count(): int
    {
        return \count($this->names);
    }

    /**
     * The names, in column order, keyed by ordinal.
     *
     * @return \ArrayIterator<int, string> the iterator
     */
    public function getIterator(): \ArrayIterator
    {
        return new \ArrayIterator($this->names);
    }

    /**
     * Whether the header has a column at this ordinal.
     *
     * @param mixed $offset the ordinal
     * @return bool whether there is a name there
     */
    public function offsetExists(mixed $offset): bool
    {
        return \is_int($offset) && isset($this->names[$offset]);
    }

    /**
     * The name of the column at this ordinal.
     *
     * @param mixed $offset the ordinal
     * @return string the name
     * @throws \OutOfRangeException when the header has no column there
     */
    public function offsetGet(mixed $offset): string
    {
        if (!\is_int($offset) || !isset($this->names[$offset])) {
            throw new \OutOfRangeException(
                'The header names ' . \count($this->names) . ' columns; there is no column '
                . var_export($offset, true)
            );
        }
        return $this->names[$offset];
    }

    /**
     * Refused: a header is what the source said.
     *
     * @param mixed $offset the ordinal
     * @param mixed $value the name
     * @return void
     * @throws \LogicException always
     */
    public function offsetSet(mixed $offset, mixed $value): void
    {
        throw new \LogicException('A header cannot be changed');
    }

    /**
     * Refused: a header is what the source said.
     *
     * @param mixed $offset the ordinal
     * @return void
     * @throws \LogicException always
     */
    public function offsetUnset(mixed $offset): void
    {
        throw new \LogicException('A header cannot be changed');
    }

    /**
     * The names, as a JSON array.
     *
     * @return list<string> the names
     */
    public function jsonSerialize(): array
    {
        return $this->names;
    }
}
