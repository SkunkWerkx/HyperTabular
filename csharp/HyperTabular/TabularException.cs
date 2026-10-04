namespace HyperTabular;

/// <summary>Why an input could not be read as rows at all.</summary>
public enum TabularFailure
{
	/// <summary>Sentinel CLR default — never produced.</summary>
	Unspecified = 0,
	/// <summary>The input ended inside a quoted cell.</summary>
	UnclosedQuote = 1,
	/// <summary>A record's cell count disagrees with the first record's.</summary>
	ColumnCount = 2,
	/// <summary>A single record is larger than <see cref="DelimitedReader.MaxRowBytes"/>.</summary>
	RowTooLong = 3
}

/// <summary>
/// A structural failure: the input is not rows of cells — a record of the wrong width,
/// input that ends inside a quoted cell. Never a cell's verdict: a value that does not
/// cast is a <c>Fault</c> in its column, and the read goes on. A structure failure ends
/// the input, after every intact row before it has been delivered.
/// </summary>
public sealed class TabularException : Exception
{
	internal TabularException(TabularFailure failure, long record, int line, long @byte, int expected, int found)
		: base(Describe(failure, record, line, @byte, expected, found))
	{
		Failure = failure;
		Record = record;
		Line = line;
		Byte = @byte;
		Expected = expected;
		Found = found;
	}

	/// <summary>What is wrong.</summary>
	public TabularFailure Failure { get; }

	/// <summary>Zero-based index of the offending record — the header and skipped blank lines included.</summary>
	public long Record { get; }

	/// <summary>One-based line the offending record starts on.</summary>
	public int Line { get; }

	/// <summary>Absolute byte offset of the offending record's start.</summary>
	public long Byte { get; }

	/// <summary>Cells in the first record, for <see cref="TabularFailure.ColumnCount"/>.</summary>
	public int Expected { get; }

	/// <summary>Cells in this record, for <see cref="TabularFailure.ColumnCount"/>.</summary>
	public int Found { get; }

	static string Describe(TabularFailure failure, long record, int line, long @byte, int expected, int found) =>
		failure switch
		{
			TabularFailure.ColumnCount =>
				$"Record {record} (line {line}, byte {@byte}) has {found} cells; the first record had {expected}.",
			TabularFailure.UnclosedQuote =>
				$"The input ended inside a quoted cell in record {record} (line {line}, byte {@byte}).",
			_ => $"Record {record} (line {line}, byte {@byte}) exceeds the {DelimitedReader.MaxRowBytes}-byte row ceiling.",
		};
}
