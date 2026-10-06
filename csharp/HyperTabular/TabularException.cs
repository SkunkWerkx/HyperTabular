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
	/// <summary>The container is a zip but neither an XLSX nor an ODS workbook.</summary>
	NotAWorkbook = 23,
	/// <summary>A cell names a shared string that is not in the table.</summary>
	SharedString = 24,
	/// <summary>More text than can be addressed: over 4 GiB in a batch or in the shared strings, or 2 GiB in a cell.</summary>
	TooLarge = 25
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

	/// <summary>The failure the core reported, read the way the Rust binding reads it.</summary>
	internal static TabularException From(in Native.RawFailure failure) =>
		new(failure.Code switch
		{
			1 => TabularFailure.UnclosedQuote,
			2 => TabularFailure.ColumnCount,
			3 => TabularFailure.RowTooLong,
			>= 16 and <= 25 => (TabularFailure)failure.Code,
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

	/// <summary>Delimited text: the byte offset of the record. A workbook: how far into the part's inflated bytes the read had got.</summary>
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
			TabularFailure.NotAWorkbook => "The zip is neither an XLSX nor an ODS workbook.",
			TabularFailure.SharedString =>
				$"Row {line} names shared string {found}; the table has {expected}.",
			TabularFailure.TooLarge => "The workbook holds more text than a batch can address.",
			_ => $"The input is structurally broken ({failure}).",
		};
}
