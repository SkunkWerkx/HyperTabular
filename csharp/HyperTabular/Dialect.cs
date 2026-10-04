namespace HyperTabular;

/// <summary>
/// How the text is delimited, declared by the caller. Nothing is sniffed: the separator is
/// stated, quoting is stated, the header is stated — the same stance HyperCast's
/// <c>NumFormat</c> takes for numeric notation.
/// </summary>
/// <param name="Separator">The single-byte separator: tab, or any printable ASCII character except <c>"</c>.</param>
/// <param name="Quoting">Whether <c>"</c> quotes cells (RFC 4180, <c>""</c> for a literal quote). Off, a quote is an ordinary byte.</param>
/// <param name="HasHeader">Whether the first record is a header, exposed through <see cref="DelimitedReader.Header"/> and never delivered as a row.</param>
/// <param name="SkipBlankLines">Whether a completely empty line is skipped rather than read as a one-cell row.</param>
public readonly record struct Dialect(char Separator, bool Quoting = true, bool HasHeader = true, bool SkipBlankLines = true)
{
	/// <summary>Comma-separated, quoted, with a header, blank lines skipped.</summary>
	public static readonly Dialect Csv = new(',');

	/// <summary>Tab-separated, otherwise as <see cref="Csv"/>.</summary>
	public static readonly Dialect Tsv = new('\t');

	/// <summary>Pipe-separated, otherwise as <see cref="Csv"/>.</summary>
	public static readonly Dialect Psv = new('|');
}
