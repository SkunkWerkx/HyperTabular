namespace HyperTabular;

/// <summary>Why an input could not be read as rows at all. The numbers are the core's failure codes.</summary>
public enum TabularFailure
{
	/// <summary>Sentinel CLR default — never produced.</summary>
	Unspecified = 0,
	/// <summary>The input ended inside a quoted cell.</summary>
	UnclosedQuote = 1,
	/// <summary>A record's cell count disagrees with the first record's.</summary>
	ColumnCount = 2,
	/// <summary>A single record is larger than <see cref="DelimitedReader.MaxRowBytes"/>.</summary>
	RowTooLong = 3,
	/// <summary>The workbook's container is not a zip file.</summary>
	NotAZip = 16,
	/// <summary>The zip's own structure is broken.</summary>
	Container = 17,
	/// <summary>A part of the workbook, or the whole document, is encrypted.</summary>
	Encrypted = 18,
	/// <summary>A part is compressed by a method that is neither stored nor deflate; <see cref="TabularException.Found"/> is the method.</summary>
	Method = 19,
	/// <summary>A part the workbook cannot be read without is missing.</summary>
	MissingPart = 20,
	/// <summary>A part's XML ends inside a construct.</summary>
	Xml = 21,
	/// <summary>A part's bytes are not a deflate stream, or stop before the stream does.</summary>
	Deflate = 22,
	/// <summary>The container is a zip but not an XLSX, ODS or Numbers workbook.</summary>
	NotAWorkbook = 23,
	/// <summary>A cell names a shared string that is not in the table.</summary>
	SharedString = 24,
	/// <summary>More text than can be addressed: over 4 GiB in a batch or in the shared strings, or 2 GiB in a cell.</summary>
	TooLarge = 25,
	/// <summary>
	/// A Numbers part is not what the IWA packaging says: <c>Found</c> is 1 for a broken chunk or Snappy block, 2 for a
	/// malformed message, 3 for an object a reference names that is not there (<c>Expected</c> is its identifier), 4 for
	/// cell storage of a version the core does not read (<c>Expected</c> is the version).
	/// </summary>
	Iwa = 26,
	/// <summary>
	/// Text that is not valid in its encoding: a unit a dialect's declared <see cref="TextEncoding"/> has no
	/// character for, or, under UTF-8, a header name that is not valid. A cell that is not valid UTF-8 is that
	/// cell's malformed verdict, not this.
	/// </summary>
	Encoding = 27
}

/// <summary>
/// What <see cref="Header.Ordinal(string)"/> says of a name two columns of the header share: it will
/// not choose between them. <see cref="Header.TryOrdinal(string, out int)"/> takes the first, and a plan
/// can name either by position. A <see cref="KeyNotFoundException"/>, as a name no column has is, so
/// that code catching one catches the other: neither name resolved to a column.
/// </summary>
public sealed class DuplicateColumnException : KeyNotFoundException
{
	internal DuplicateColumnException(string name, int first, int second)
		: base($"The header names two columns \"{name}\", at positions {first} and {second}: say which by position.")
	{
		Name = name;
		First = first;
		Second = second;
	}

	/// <summary>The name that was looked for.</summary>
	public string Name { get; }

	/// <summary>The zero-based column that has it first, counted as a plan's ordinals are.</summary>
	public int First { get; }

	/// <summary>The zero-based column that has it next. A later one is not named.</summary>
	public int Second { get; }
}

/// <summary>
/// A structural failure: the input is not rows of cells — a record of the wrong width,
/// input that ends inside a quoted cell, a workbook whose container or parts cannot be read.
/// Never a cell's verdict: a value that does not cast is a <c>Fault</c> in its column, and
/// the read goes on. A structure failure ends the input, after every intact row before it
/// has been delivered.
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

	/// <summary>A failure found by the binding rather than the core, which has nowhere in the input to point at.</summary>
	internal TabularException(TabularFailure failure, string message)
		: base(message) => Failure = failure;

	/// <summary>The failure the core reported, read the way the Rust binding reads it.</summary>
	internal static TabularException From(in Native.RawFailure failure) =>
		new(failure.Code switch
		{
			1 => TabularFailure.UnclosedQuote,
			2 => TabularFailure.ColumnCount,
			3 => TabularFailure.RowTooLong,
			>= 16 and <= 27 => (TabularFailure)failure.Code,
			// The one code left, and the reading of any the core has yet to be given.
			_ => TabularFailure.Container,
		}, (long)failure.Record, (int)failure.Line, (long)failure.Byte, (int)failure.Expected, (int)failure.Found);

	/// <summary>What is wrong.</summary>
	public TabularFailure Failure { get; }

	/// <summary>
	/// Delimited text: the zero-based index of the offending record, the header and skipped
	/// blank lines included. A workbook: the part being read (<c>1</c> <c>_rels/.rels</c>,
	/// <c>2</c> the workbook, <c>3</c> its relationships, <c>4</c> the styles, <c>5</c> the
	/// shared strings, <c>6</c> a worksheet, <c>7</c> <c>content.xml</c>, <c>8</c>
	/// <c>mimetype</c>, <c>9</c> the manifest), or <c>0</c> for the container itself.
	/// </summary>
	public long Record { get; }

	/// <summary>Delimited text: the one-based line the record starts on. A workbook: the sheet row being read, where there is one.</summary>
	public int Line { get; }

	/// <summary>Delimited text: the byte offset of the record, in the decoded text if an encoding was declared, except that an <see cref="TabularFailure.Encoding"/> failure under a declared encoding gives the unit's offset in the original input. A workbook: how far into the part's inflated bytes the read had got.</summary>
	public long Byte { get; }

	/// <summary>What was expected, for the kinds that say: the first record's cell count; the length of the shared-string table.</summary>
	public int Expected { get; }

	/// <summary>What was found, for the kinds that say: this record's cell count; the compression method; the shared-string index; which XML construct the part ends inside.</summary>
	public int Found { get; }

	static string Describe(TabularFailure failure, long record, int line, long @byte, int expected, int found) =>
		failure switch
		{
			TabularFailure.ColumnCount =>
				$"Record {record} (line {line}, byte {@byte}) has {found} cells; the first record had {expected}.",
			TabularFailure.UnclosedQuote =>
				$"The input ended inside a quoted cell in record {record} (line {line}, byte {@byte}).",
			TabularFailure.RowTooLong =>
				$"Record {record} (line {line}, byte {@byte}) exceeds the {DelimitedReader.MaxRowBytes}-byte row ceiling.",
			TabularFailure.NotAZip => "The workbook is not a zip file.",
			TabularFailure.Container => "The workbook's zip structure is broken.",
			TabularFailure.Encrypted => "The workbook is encrypted.",
			TabularFailure.Method => $"Part {record} of the workbook is compressed by method {found}, which is neither stored nor deflate.",
			TabularFailure.MissingPart => $"Part {record}, which the workbook cannot be read without, is missing.",
			TabularFailure.Xml => $"Part {record} of the workbook ends inside an XML construct (byte {@byte}).",
			TabularFailure.Deflate => $"Part {record} of the workbook is not a whole deflate stream (byte {@byte}).",
			TabularFailure.NotAWorkbook => "The zip is not an XLSX, ODS or Numbers workbook.",
			TabularFailure.SharedString =>
				$"Row {line} names shared string {found}; the table has {expected}.",
			TabularFailure.TooLarge => "The workbook holds more text than a batch can address.",
			TabularFailure.Iwa =>
				$"Part {record} of the Numbers document is not what the IWA packaging says (byte {@byte}, detail {found}).",
			TabularFailure.Encoding =>
				$"Record {record} (line {line}) holds text with no character in its encoding, at byte {@byte}.",
			_ => $"The input is structurally broken ({failure}).",
		};
}
